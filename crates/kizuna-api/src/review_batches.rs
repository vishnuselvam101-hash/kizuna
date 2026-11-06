//! Review batches: invalidations too large to apply inline. The evidence change commits; its impact is recorded as
//! a batch that a background job materialises once and then applies in bounded chunks. Each chunk applies its
//! impacts and marks them applied in one transaction, so the job is resumable and idempotent across restarts.
//! While a batch is open its subject is "impact analysis in progress" and no new decision may rely on it.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use dependency_engine::{Impact, NeedsReviewBatch};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::decisions::{Cause, apply_impacts, impacts_of, subject_of};
use crate::error::{ApiError, ApiResult};

/// Records a review batch for `cause` (in the caller's transaction, so it commits with the change).
pub(crate) async fn enqueue(
    tx: &mut Tx,
    tenant: Uuid,
    actor: Uuid,
    cause: &Cause,
    bound: &NeedsReviewBatch,
) -> ApiResult<Uuid> {
    let (stype, keys) = subject_of(tx, cause).await?;
    let (cause_claim, cause_decision) = match cause {
        Cause::Claim { change } => (
            match change {
                dependency_engine::Change::Superseded { old_claim: c, .. }
                | dependency_engine::Change::Withdrawn { old_claim: c, .. }
                | dependency_engine::Change::NewEvidence { new_claim: c, .. } => Uuid::parse_str(c).ok(),
            },
            None,
        ),
        Cause::Revoked { decision } => (None, Some(*decision)),
    };
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO review_batches (id, tenant_id, cause, cause_claim, cause_decision, subject_type, subject_keys, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(id)
    .bind(tenant)
    .bind(json!(cause))
    .bind(cause_claim)
    .bind(cause_decision)
    .bind(&stype)
    .bind(&keys)
    .bind(actor)
    .execute(&mut **tx)
    .await?;
    audit::append(
        tx,
        tenant,
        Record::user(
            actor,
            "invalidation.review_batch_created",
            "review_batch",
            id,
            json!({ "limit": bound.limit, "reached": bound.reached, "subject_type": stype, "subject_keys": keys }),
        ),
    )
    .await?;
    tracing::info!(batch = %id, limit = bound.limit, "invalidation exceeds the inline bound: review batch queued");
    Ok(id)
}

/// Whether `subject` (any key form: canonical or confirmed alias) has an open review batch.
pub(crate) async fn subject_in_progress(tx: &mut Tx, stype: &str, key: &str) -> ApiResult<bool> {
    let canonical = crate::verification::canonical_key_of(tx, stype, key).await?;
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM review_batches WHERE status <> 'done' AND subject_type = $1 \
         AND ($2 = ANY(subject_keys) OR $3 = ANY(subject_keys)))",
    )
    .bind(stype)
    .bind(key)
    .bind(&canonical)
    .fetch_one(&mut **tx)
    .await?)
}

/// An impact as stored in `review_batch_items`.
#[derive(Serialize, Deserialize)]
struct StoredImpact {
    decision: String,
    material: bool,
    cause_kind: String,
    reason: String,
    /// Absent on items stored before message codes: the recorded English text is shown as is.
    #[serde(default)]
    reason_msg: Option<kizuna_messages::Msg>,
    path: Vec<String>,
    semantic_change: Value,
}

impl StoredImpact {
    fn into_impact(self) -> ApiResult<Impact> {
        let cause_kind = match self.cause_kind.as_str() {
            "superseded" => "superseded",
            "withdrawn" => "withdrawn",
            "new_evidence" => "new_evidence",
            "upstream_decision" => "upstream_decision",
            other => return Err(ApiError::internal(format!("unknown cause kind {other}"))),
        };
        Ok(Impact {
            decision: self.decision,
            material: self.material,
            cause_kind,
            reason_msg: self
                .reason_msg
                .unwrap_or_else(|| kizuna_messages::Msg::new("text.verbatim").with("text", self.reason.as_str())),
            reason: self.reason,
            path: self.path,
            semantic_change: self.semantic_change,
        })
    }
}

/// What one step did.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    /// The batch's impacts were computed and stored (`total` items).
    Materialised {
        batch: Uuid,
        total: usize,
    },
    /// One chunk was applied; `done` when it was the last.
    Applied {
        batch: Uuid,
        items: usize,
        done: bool,
    },
}

#[derive(sqlx::FromRow)]
struct OpenBatch {
    id: Uuid,
    cause: Value,
    cause_claim: Option<Uuid>,
    cause_decision: Option<Uuid>,
    total: Option<i32>,
    created_by: Uuid,
}

/// One bounded unit of work on the oldest open batch of the transaction's tenant. The caller commits; a crash or
/// rollback before the commit leaves the batch exactly as it was.
pub async fn step(tx: &mut Tx, tenant: Uuid, chunk: usize) -> ApiResult<Step> {
    let Some(b) = sqlx::query_as::<_, OpenBatch>(
        "SELECT id, cause, cause_claim, cause_decision, total, created_by FROM review_batches \
         WHERE status <> 'done' ORDER BY created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(Step::Idle);
    };
    if b.total.is_none() {
        let cause: Cause = serde_json::from_value(b.cause).map_err(ApiError::internal)?;
        let impacts = match impacts_of(tx, &cause, usize::MAX).await? {
            Ok(c) => c.impacts,
            Err(e) => return Err(ApiError::internal(e)),
        };
        for (n, part) in impacts.chunks(1000).enumerate() {
            let seqs: Vec<i32> = (0..part.len()).map(|i| (n * 1000 + i) as i32).collect();
            let decisions: Vec<Uuid> = part
                .iter()
                .map(|i| Uuid::parse_str(&i.decision))
                .collect::<Result<_, _>>()
                .map_err(ApiError::internal)?;
            let rows: Vec<Value> = part.iter().map(|i| json!(i)).collect();
            sqlx::query(
                "INSERT INTO review_batch_items (tenant_id, batch_id, seq, decision_id, impact) \
                 SELECT $1, $2, s, d, i FROM UNNEST($3::int[], $4::uuid[], $5::jsonb[]) AS u(s, d, i)",
            )
            .bind(tenant)
            .bind(b.id)
            .bind(&seqs)
            .bind(&decisions)
            .bind(&rows)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query("UPDATE review_batches SET total = $2, status = 'running', updated_at = now() WHERE id = $1")
            .bind(b.id)
            .bind(impacts.len() as i32)
            .execute(&mut **tx)
            .await?;
        return Ok(Step::Materialised {
            batch: b.id,
            total: impacts.len(),
        });
    }
    let rows: Vec<(i32, Value)> = sqlx::query_as(
        "SELECT seq, impact FROM review_batch_items WHERE batch_id = $1 AND NOT applied ORDER BY seq LIMIT $2",
    )
    .bind(b.id)
    .bind(chunk.max(1) as i64)
    .fetch_all(&mut **tx)
    .await?;
    let seqs: Vec<i32> = rows.iter().map(|r| r.0).collect();
    let impacts = rows
        .into_iter()
        .map(|(_, v)| {
            serde_json::from_value::<StoredImpact>(v)
                .map_err(ApiError::internal)?
                .into_impact()
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let n = impacts.len();
    let flagged = apply_impacts(tx, tenant, b.created_by, impacts, b.cause_claim, b.cause_decision, None).await?;
    sqlx::query("UPDATE review_batch_items SET applied = true WHERE batch_id = $1 AND seq = ANY($2)")
        .bind(b.id)
        .bind(&seqs)
        .execute(&mut **tx)
        .await?;
    let (processed, total): (i32, Option<i32>) = sqlx::query_as(
        "UPDATE review_batches SET processed = processed + $2, flagged = flagged + $3, updated_at = now() \
         WHERE id = $1 RETURNING processed, total",
    )
    .bind(b.id)
    .bind(n as i32)
    .bind(flagged as i32)
    .fetch_one(&mut **tx)
    .await?;
    let done = Some(processed) >= total;
    if done {
        sqlx::query("UPDATE review_batches SET status = 'done', finished_at = now() WHERE id = $1")
            .bind(b.id)
            .execute(&mut **tx)
            .await?;
        audit::append(
            tx,
            tenant,
            Record {
                actor_user_id: Some(b.created_by),
                actor_kind: "system",
                action: "invalidation.review_batch_completed",
                target_type: "review_batch",
                target_id: Some(b.id.to_string()),
                details: json!({ "processed": processed }),
            },
        )
        .await?;
    }
    Ok(Step::Applied {
        batch: b.id,
        items: n,
        done,
    })
}

/// Runs steps for every active tenant until no batch has work left. Returns the number of steps taken.
pub async fn run_pending(pool: &PgPool, chunk: usize) -> ApiResult<usize> {
    let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE status = 'active'")
        .fetch_all(pool)
        .await?;
    let mut steps = 0;
    for t in tenants {
        loop {
            let mut tx = db::tenant_tx(pool, t).await?;
            let s = step(&mut tx, t, chunk).await?;
            tx.commit().await?;
            if s == Step::Idle {
                break;
            }
            steps += 1;
        }
    }
    Ok(steps)
}

/// Server background task: process review batches every `every`.
pub fn spawn_worker(pool: PgPool, chunk: usize, every: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            if let Err(e) = run_pending(&pool, chunk).await {
                tracing::error!(error = ?e, "review batch step failed");
            }
        }
    });
}

#[derive(Serialize, sqlx::FromRow)]
pub struct BatchOut {
    pub id: Uuid,
    pub status: String,
    pub subject_type: String,
    pub subject_keys: Vec<String>,
    pub cause_claim: Option<Uuid>,
    pub cause_decision: Option<Uuid>,
    pub total: Option<i32>,
    pub processed: i32,
    pub flagged: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

const COLS: &str = "id, status, subject_type, subject_keys, cause_claim, cause_decision, total, processed, flagged, created_at, finished_at";

/// GET /review-batches: the tenant's batches, newest first (progress = processed / total).
pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<BatchOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = sqlx::query_as(&format!(
        "SELECT {COLS} FROM review_batches ORDER BY created_at DESC LIMIT 200"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// GET /review-batches/{id}
pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<BatchOut>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = sqlx::query_as(&format!("SELECT {COLS} FROM review_batches WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}
