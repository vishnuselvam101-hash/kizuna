//! Retry-safe background jobs (STABILITY_2026-11-06).
//!
//! Delivery is at-least-once: a job row is claimed with a lease (`FOR UPDATE SKIP LOCKED`), its handler runs
//! outside the claim transaction, and completion is recorded only by the lease holder. A worker that dies leaves
//! the lease to expire, and the job is claimed again. Every handler is therefore idempotent:
//! - `review_batch.run`: each step commits atomically (`review_batches::step`); a repeated step finds the work done;
//! - `verification.run`: detection is a pure function of the stored claims (advisory-locked per tenant);
//! - `gc.tenant`: deletes of expired rows;
//! - `alerts.deliver`: each executed `notify_admins` action is marked `alerted_at` after sending (a crash between
//!   send and mark re-sends once).
//!
//! `attempts` counts claims, so a job that kills its worker is counted too. A failure backs off exponentially
//! (2^attempts s, at most 5 min); after `max_attempts` the job is quarantined and an incident (`job_quarantined`,
//! high) with a pre-authorised `notify_admins` action is opened in its tenant. Quarantined jobs stay until an
//! incident manager re-queues them (`POST /jobs/{id}/requeue`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admission::{self, RouteClass, RouteCtx};
use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiQuery, ApiResult};

pub const REVIEW_BATCH: &str = "review_batch.run";
pub const VERIFICATION: &str = "verification.run";
pub const GC: &str = "gc.tenant";
pub const ALERTS: &str = "alerts.deliver";
pub const DEFAULT_MAX_ATTEMPTS: i32 = 5;
pub const LEASE_SECS: i64 = 300;
/// Finished jobs are kept this long for inspection, then removed by `gc.tenant`.
pub const DONE_RETENTION_SECS: i64 = 24 * 3600;
/// Review-batch steps per job run; more work re-enqueues the job (other tenants get a turn).
const REVIEW_STEPS_PER_RUN: usize = 200;

/// Adds a job to the tenant's queue in the caller's transaction (commit with the work that caused it: outbox).
/// Idempotent: while a job with the same (kind, dedupe key) is queued, its id is returned instead.
pub async fn enqueue(
    tx: &mut Tx,
    tenant: Uuid,
    kind: &str,
    dedupe: &str,
    payload: Value,
    created_by: Option<Uuid>,
) -> ApiResult<i64> {
    // The queued twin can be claimed between the insert and the lookup (stability soak 2026-11-06: was a 500);
    // then the insert succeeds on the next round.
    for _ in 0..3 {
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO jobs (tenant_id, kind, dedupe_key, payload, max_attempts, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(tenant)
        .bind(kind)
        .bind(dedupe)
        .bind(&payload)
        .bind(DEFAULT_MAX_ATTEMPTS)
        .bind(created_by)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(id) = id {
            return Ok(id);
        }
        let queued: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE tenant_id = $1 AND kind = $2 AND dedupe_key = $3 AND state = 'queued'",
        )
        .bind(tenant)
        .bind(kind)
        .bind(dedupe)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(id) = queued {
            return Ok(id);
        }
    }
    Err(ApiError::Conflict("job_enqueue_contended"))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub created_by: Option<Uuid>,
}

/// Claims the tenant's next due job (or one whose lease expired) for `worker` until `now + lease`.
pub async fn claim(
    pool: &PgPool,
    tenant: Uuid,
    worker: &str,
    now: OffsetDateTime,
    lease_secs: i64,
) -> ApiResult<Option<Job>> {
    let mut tx = db::tenant_tx(pool, tenant).await?;
    let job: Option<Job> = sqlx::query_as(
        "UPDATE jobs SET state = 'running', attempts = attempts + 1, lease_owner = $1, \
                lease_expires_at = $2 + make_interval(secs => $3), updated_at = $2 \
         WHERE id = (SELECT id FROM jobs \
                      WHERE (state = 'queued' AND run_after <= $2) OR (state = 'running' AND lease_expires_at < $2) \
                      ORDER BY run_after, id LIMIT 1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, kind, payload, attempts, max_attempts, created_by",
    )
    .bind(worker)
    .bind(now)
    .bind(lease_secs as f64)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(job)
}

/// Marks the job done if `worker` still holds its lease. `false`: the lease was lost (another worker took over).
pub async fn complete(pool: &PgPool, tenant: Uuid, id: i64, worker: &str, now: OffsetDateTime) -> ApiResult<bool> {
    let mut tx = db::tenant_tx(pool, tenant).await?;
    let n = sqlx::query(
        "UPDATE jobs SET state = 'done', finished_at = $3, updated_at = $3, lease_owner = NULL, lease_expires_at = NULL, \
                last_error = NULL \
         WHERE id = $1 AND state = 'running' AND lease_owner = $2",
    )
    .bind(id)
    .bind(worker)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(n == 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Retry,
    Quarantined,
    LeaseLost,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Done => "ok",
            Outcome::Retry => "retry",
            Outcome::Quarantined => "quarantined",
            Outcome::LeaseLost => "lease_lost",
        }
    }
}

fn clip(e: &str) -> String {
    e.chars().filter(|c| !c.is_control()).take(480).collect()
}

/// Records a failed attempt: back off and re-queue, or quarantine after `max_attempts`.
pub async fn fail(
    pool: &PgPool,
    tenant: Uuid,
    job: &Job,
    worker: &str,
    err: &str,
    now: OffsetDateTime,
) -> ApiResult<Outcome> {
    let err = clip(err);
    let mut tx = db::tenant_tx(pool, tenant).await?;
    let held: Option<i64> =
        sqlx::query_scalar("SELECT id FROM jobs WHERE id = $1 AND state = 'running' AND lease_owner = $2 FOR UPDATE")
            .bind(job.id)
            .bind(worker)
            .fetch_optional(&mut *tx)
            .await?;
    if held.is_none() {
        tx.commit().await?;
        return Ok(Outcome::LeaseLost);
    }
    if job.attempts >= job.max_attempts {
        quarantine(&mut tx, tenant, job, &err, now).await?;
        tx.commit().await?;
        return Ok(Outcome::Quarantined);
    }
    let backoff = 2f64.powi(job.attempts.clamp(0, 16)).min(300.0);
    sqlx::query(
        "UPDATE jobs SET state = 'queued', run_after = $2 + make_interval(secs => $3), lease_owner = NULL, \
                lease_expires_at = NULL, last_error = $4, updated_at = $2 WHERE id = $1",
    )
    .bind(job.id)
    .bind(now)
    .bind(backoff)
    .bind(&err)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Outcome::Retry)
}

async fn quarantine(tx: &mut Tx, tenant: Uuid, job: &Job, err: &str, now: OffsetDateTime) -> ApiResult<()> {
    sqlx::query(
        "UPDATE jobs SET state = 'quarantined', finished_at = $2, updated_at = $2, lease_owner = NULL, \
                lease_expires_at = NULL, last_error = $3 WHERE id = $1",
    )
    .bind(job.id)
    .bind(now)
    .bind(err)
    .execute(&mut **tx)
    .await?;
    let subject = format!("job:{}", job.kind);
    let summary = format!(
        "background job {} #{} failed {} times and was quarantined",
        job.kind, job.id, job.attempts
    );
    let (incident, inserted): (Uuid, bool) = sqlx::query_as(
        "INSERT INTO incidents (id, tenant_id, rule, severity, subject, summary) \
         VALUES ($1, $2, 'job_quarantined', 'high', $3, $4) \
         ON CONFLICT (tenant_id, rule, subject) WHERE status IN ('open', 'contained') \
         DO UPDATE SET last_seen = now(), summary = EXCLUDED.summary \
         RETURNING id, (xmax = 0)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(&subject)
    .bind(&summary)
    .fetch_one(&mut **tx)
    .await?;
    if inserted {
        audit::append(
            tx,
            tenant,
            Record {
                actor_user_id: None,
                actor_kind: "system",
                action: "incident.opened",
                target_type: "incident",
                target_id: Some(incident.to_string()),
                details: json!({ "rule": "job_quarantined", "severity": "high", "subject": subject }),
            },
        )
        .await?;
        sqlx::query(
            "INSERT INTO response_actions (id, tenant_id, incident_id, kind, target, policy, status, detail) \
             VALUES ($1, $2, $3, 'notify_admins', 'operators', 'pre_authorised', 'executed', 'job quarantined')",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(incident)
        .execute(&mut **tx)
        .await?;
    }
    audit::append(
        tx,
        tenant,
        Record {
            actor_user_id: None,
            actor_kind: "system",
            action: "job.quarantined",
            target_type: "job",
            target_id: Some(job.id.to_string()),
            details: json!({ "kind": job.kind, "attempts": job.attempts, "incident": incident }),
        },
    )
    .await?;
    tracing::error!(job = job.id, kind = %job.kind, %tenant, attempts = job.attempts, "job quarantined");
    Ok(())
}

/// What job handlers need from the server.
#[derive(Clone)]
pub struct JobCtx {
    pub pool: PgPool,
    pub alerts: Arc<crate::alerts::Alerter>,
    pub metrics: Arc<crate::metrics::Metrics>,
    pub review_chunk: usize,
    pub statement_timeout_ms: u64,
    pub lease_secs: i64,
}

async fn execute(ctx: &JobCtx, tenant: Uuid, job: &Job) -> Result<(), String> {
    let e = |e: ApiError| e.to_string();
    match job.kind.as_str() {
        REVIEW_BATCH => {
            for _ in 0..REVIEW_STEPS_PER_RUN {
                let mut tx = db::tenant_tx(&ctx.pool, tenant).await.map_err(|x| x.to_string())?;
                let s = crate::review_batches::step(&mut tx, tenant, ctx.review_chunk)
                    .await
                    .map_err(e)?;
                tx.commit().await.map_err(|x| x.to_string())?;
                if s == crate::review_batches::Step::Idle {
                    return Ok(());
                }
            }
            let mut tx = db::tenant_tx(&ctx.pool, tenant).await.map_err(|x| x.to_string())?;
            enqueue(&mut tx, tenant, REVIEW_BATCH, "", json!({}), None)
                .await
                .map_err(e)?;
            tx.commit().await.map_err(|x| x.to_string())?;
            Ok(())
        }
        VERIFICATION => {
            let mut tx = db::tenant_tx(&ctx.pool, tenant).await.map_err(|x| x.to_string())?;
            let r = crate::verification::detect(
                &mut tx,
                tenant,
                OffsetDateTime::now_utc(),
                crate::verification::RunMode::Auto,
            )
            .await
            .map_err(e)?;
            audit::append(
                &mut tx,
                tenant,
                Record {
                    actor_user_id: job.created_by,
                    actor_kind: "system",
                    action: "verification.run",
                    target_type: "tenant",
                    target_id: Some(tenant.to_string()),
                    details: json!({ "job": job.id, "rule_version": r.rule_version, "mode": r.mode,
                        "new": r.findings_new, "total": r.findings_total, "obsolete": r.marked_obsolete,
                        "subjects": r.subjects_evaluated, "watermark": r.watermark }),
                },
            )
            .await
            .map_err(e)?;
            tx.commit().await.map_err(|x| x.to_string())?;
            Ok(())
        }
        GC => {
            let now = OffsetDateTime::now_utc();
            let mut tx = db::tenant_tx(&ctx.pool, tenant).await.map_err(|x| x.to_string())?;
            let keys = crate::idempotency::purge_expired(&mut tx, now)
                .await
                .map_err(|x| x.to_string())?;
            let jobs = sqlx::query("DELETE FROM jobs WHERE state = 'done' AND finished_at < $1")
                .bind(now - time::Duration::seconds(DONE_RETENTION_SECS))
                .execute(&mut *tx)
                .await
                .map_err(|x| x.to_string())?
                .rows_affected();
            tx.commit().await.map_err(|x| x.to_string())?;
            if keys + jobs > 0 {
                tracing::info!(%tenant, idempotency_keys = keys, jobs, "gc removed expired rows");
            }
            Ok(())
        }
        ALERTS => deliver_alerts(ctx, tenant).await.map_err(e),
        _ => Err("unknown_job_kind".into()),
    }
}

async fn deliver_alerts(ctx: &JobCtx, tenant: Uuid) -> ApiResult<()> {
    if !ctx.alerts.enabled() {
        return Ok(());
    }
    let mut tx = db::tenant_tx(&ctx.pool, tenant).await?;
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await?;
    let rows: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT a.id, i.id, i.rule, i.severity FROM response_actions a \
         JOIN incidents i ON i.tenant_id = a.tenant_id AND i.id = a.incident_id \
         WHERE a.kind = 'notify_admins' AND a.status = 'executed' AND a.alerted_at IS NULL ORDER BY a.created_at LIMIT 100",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let label = ctx.alerts.label().to_string();
    for (action, incident, rule, sev) in rows {
        let a = crate::alerts::incident_alert(&label, &slug, incident, &rule, &sev);
        let al = Arc::clone(&ctx.alerts);
        tokio::task::spawn_blocking(move || al.send(&a))
            .await
            .map_err(ApiError::internal)?;
        let mut tx = db::tenant_tx(&ctx.pool, tenant).await?;
        sqlx::query("UPDATE response_actions SET alerted_at = now() WHERE id = $1 AND alerted_at IS NULL")
            .bind(action)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Claims and runs at most one job of `tenant`. Returns the job id and outcome, or `None` when nothing was due.
pub async fn run_one(ctx: &JobCtx, tenant: Uuid, worker: &str) -> ApiResult<Option<(i64, Outcome)>> {
    let Some(job) = claim(&ctx.pool, tenant, worker, OffsetDateTime::now_utc(), ctx.lease_secs).await? else {
        return Ok(None);
    };
    let t0 = Instant::now();
    // A job claimed more often than allowed never completed (its workers died): quarantine without running it.
    let outcome = if job.attempts > job.max_attempts {
        fail(
            &ctx.pool,
            tenant,
            &job,
            worker,
            "worker lost repeatedly",
            OffsetDateTime::now_utc(),
        )
        .await?
    } else {
        let (c, j) = (ctx.clone(), job.clone());
        let rctx = RouteCtx {
            class: RouteClass::Job,
            statement_timeout_ms: ctx.statement_timeout_ms,
        };
        // Own task: a panicking handler fails the attempt instead of killing the worker. Bounded below the lease.
        let h = tokio::spawn(admission::scope(rctx, async move { execute(&c, tenant, &j).await }));
        let budget = Duration::from_secs((ctx.lease_secs.max(20) - 10) as u64);
        let r = match tokio::time::timeout(budget, h).await {
            Ok(Ok(r)) => r,
            Ok(Err(join)) => Err(if join.is_panic() {
                "handler panicked".into()
            } else {
                "handler cancelled".into()
            }),
            Err(_) => Err("handler exceeded its lease".into()),
        };
        match r {
            Ok(()) => {
                if complete(&ctx.pool, tenant, job.id, worker, OffsetDateTime::now_utc()).await? {
                    Outcome::Done
                } else {
                    Outcome::LeaseLost
                }
            }
            Err(e) => {
                tracing::warn!(job = job.id, kind = %job.kind, attempt = job.attempts, error = %e, "job attempt failed");
                fail(&ctx.pool, tenant, &job, worker, &e, OffsetDateTime::now_utc()).await?
            }
        }
    };
    ctx.metrics.observe_job(&job.kind, outcome.label(), t0.elapsed());
    Ok(Some((job.id, outcome)))
}

/// Tenants with a claimable job (owner-defined function: identifiers only).
pub async fn ready_tenants(pool: &PgPool) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT kz_jobs_ready_tenants()")
        .fetch_all(pool)
        .await
}

/// Not-done jobs by state, all tenants.
pub async fn depth(pool: &PgPool) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as("SELECT state, n FROM kz_jobs_depth()")
        .fetch_all(pool)
        .await
}

/// Runs every due job until none is left. Returns the number of attempts made (tests, admin tooling).
pub async fn drain(ctx: &JobCtx, worker: &str) -> ApiResult<usize> {
    let mut n = 0;
    loop {
        let tenants = ready_tenants(&ctx.pool).await?;
        let mut any = false;
        for t in tenants {
            if run_one(ctx, t, worker).await?.is_some() {
                any = true;
                n += 1;
            }
        }
        if !any {
            return Ok(n);
        }
    }
}

/// Server: `n` workers polling every `poll` when idle.
pub fn spawn_workers(ctx: JobCtx, n: usize, poll: Duration) {
    for i in 0..n {
        let ctx = ctx.clone();
        let worker = format!("{}:{}:{}", std::process::id(), i, Uuid::new_v4().simple());
        tokio::spawn(async move {
            loop {
                let busy = match ready_tenants(&ctx.pool).await {
                    Ok(ts) => {
                        let mut busy = false;
                        for t in ts {
                            match run_one(&ctx, t, &worker).await {
                                Ok(Some(_)) => busy = true,
                                Ok(None) => {}
                                Err(e) => tracing::error!(error = %e, "job worker error"),
                            }
                        }
                        busy
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "job poll failed");
                        false
                    }
                };
                if !busy {
                    tokio::time::sleep(poll).await;
                }
            }
        });
    }
}

/// One scheduler pass over every active tenant: enqueue (deduplicated) the review-batch safety net when batches are
/// open, GC when `gc_due`, and alert delivery when undelivered notifications exist and alerts are on.
pub async fn schedule_tick(pool: &PgPool, gc_due: bool, alerts_enabled: bool) -> ApiResult<usize> {
    let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE status = 'active'")
        .fetch_all(pool)
        .await?;
    let mut n = 0;
    for t in tenants {
        let mut tx = db::tenant_tx(pool, t).await?;
        n += sqlx::query(
            "INSERT INTO jobs (tenant_id, kind) SELECT $1, $2 \
             WHERE EXISTS (SELECT 1 FROM review_batches WHERE status <> 'done') ON CONFLICT DO NOTHING",
        )
        .bind(t)
        .bind(REVIEW_BATCH)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if gc_due {
            n += sqlx::query("INSERT INTO jobs (tenant_id, kind) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(t)
                .bind(GC)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        if alerts_enabled {
            n += sqlx::query(
                "INSERT INTO jobs (tenant_id, kind) SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM response_actions \
                 WHERE kind = 'notify_admins' AND status = 'executed' AND alerted_at IS NULL) ON CONFLICT DO NOTHING",
            )
            .bind(t)
            .bind(ALERTS)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
    }
    Ok(n as usize)
}

/// Server: scheduler pass every `every`; GC once an hour.
pub fn spawn_scheduler(pool: PgPool, alerts_enabled: bool, every: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        let mut last_gc: Option<Instant> = None;
        loop {
            tick.tick().await;
            let gc_due = last_gc.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600));
            match schedule_tick(&pool, gc_due, alerts_enabled).await {
                Ok(_) if gc_due => last_gc = Some(Instant::now()),
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "job scheduler pass failed"),
            }
        }
    });
}

// ---------------------------------------------------------------- HTTP

#[derive(Serialize, sqlx::FromRow)]
pub struct JobOut {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

const COLS: &str = "id, kind, state, attempts, max_attempts, last_error, created_at, finished_at";

/// POST /verification/jobs: queue a background verification run (202). One queued run per tenant.
pub async fn enqueue_verification(State(st): State<AppState>, p: Principal) -> ApiResult<(StatusCode, Json<Value>)> {
    p.require(&st, Permission::ClaimWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let id = enqueue(&mut tx, p.tenant_id, VERIFICATION, "", json!({}), Some(p.user_id)).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "job_id": id, "kind": VERIFICATION })),
    ))
}

/// GET /jobs/{id}
pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<i64>) -> ApiResult<Json<JobOut>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let j: Option<JobOut> = sqlx::query_as(&format!("SELECT {COLS} FROM jobs WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    j.map(Json).ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub state: Option<String>,
}

/// GET /jobs?state=queued|running|done|quarantined (latest 200).
pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<JobOut>>> {
    p.require(&st, Permission::IncidentManage).await?;
    if let Some(s) = &q.state
        && !["queued", "running", "done", "quarantined"].contains(&s.as_str())
    {
        return Err(ApiError::BadRequest("invalid_state"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows: Vec<JobOut> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM jobs WHERE ($1::text IS NULL OR state = $1) ORDER BY id DESC LIMIT 200"
    ))
    .bind(&q.state)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// POST /jobs/{id}/requeue: a quarantined job gets a fresh set of attempts (after the cause was fixed).
pub async fn requeue(State(st): State<AppState>, p: Principal, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::IncidentManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let kind: Option<String> = sqlx::query_scalar(
        "UPDATE jobs SET state = 'queued', attempts = 0, run_after = now(), finished_at = NULL, updated_at = now() \
         WHERE id = $1 AND state = 'quarantined' RETURNING kind",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| match &e {
        // Another job of the same kind is already queued.
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => ApiError::Conflict("job_already_queued"),
        _ => ApiError::from(e),
    })?;
    let Some(kind) = kind else {
        return Err(ApiError::Conflict("job_not_quarantined"));
    };
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "job.requeued", "job", id, json!({ "kind": kind })),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "state": "queued" })))
}
