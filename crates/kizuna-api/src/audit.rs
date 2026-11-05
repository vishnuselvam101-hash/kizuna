//! Writes and verifies the per-tenant hash-chained audit log (hashing in `audit-engine`).

use audit_engine::{Event, StoredEvent, VerifyReport, chain_hash, verify_chain};
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiQuery, ApiResult};
use policy_engine::Permission;

pub struct Record<'a> {
    pub actor_user_id: Option<Uuid>,
    pub actor_kind: &'a str,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<String>,
    pub details: Value,
}

impl<'a> Record<'a> {
    pub fn user(
        actor: Uuid,
        action: &'a str,
        target_type: &'a str,
        target_id: impl ToString,
        details: Value,
    ) -> Self {
        Record {
            actor_user_id: Some(actor),
            actor_kind: "user",
            action,
            target_type,
            target_id: Some(target_id.to_string()),
            details,
        }
    }
}

pub fn now_us() -> OffsetDateTime {
    let ns = OffsetDateTime::now_utc().unix_timestamp_nanos();
    OffsetDateTime::from_unix_timestamp_nanos(ns - ns % 1000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn micros(t: OffsetDateTime) -> i64 {
    (t.unix_timestamp_nanos() / 1000) as i64
}

/// Appends an event in the caller's transaction (it commits or rolls back with the change it describes).
pub async fn append(tx: &mut Tx, tenant: Uuid, r: Record<'_>) -> Result<i64, ApiError> {
    db::set_tenant(tx, tenant).await?;
    sqlx::query("INSERT INTO audit_heads (tenant_id, last_seq, last_hash) VALUES ($1, 0, $2) ON CONFLICT DO NOTHING")
        .bind(tenant)
        .bind(audit_engine::GENESIS.to_vec())
        .execute(&mut **tx)
        .await?;
    let (last_seq, last_hash): (i64, Vec<u8>) = sqlx::query_as(
        "SELECT last_seq, last_hash FROM audit_heads WHERE tenant_id = $1 FOR UPDATE",
    )
    .bind(tenant)
    .fetch_one(&mut **tx)
    .await?;
    let at = now_us();
    let ev = Event {
        tenant_id: tenant,
        seq: last_seq + 1,
        occurred_at_us: micros(at),
        actor_user_id: r.actor_user_id,
        actor_kind: r.actor_kind.into(),
        action: r.action.into(),
        target_type: r.target_type.into(),
        target_id: r.target_id,
        details: r.details,
    };
    let hash = chain_hash(&ev, &last_hash).map_err(ApiError::internal)?;
    sqlx::query(
        "INSERT INTO audit_events (tenant_id, seq, occurred_at, actor_user_id, actor_kind, action, target_type, \
         target_id, details, prev_hash, hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(tenant)
    .bind(ev.seq)
    .bind(at)
    .bind(ev.actor_user_id)
    .bind(&ev.actor_kind)
    .bind(&ev.action)
    .bind(&ev.target_type)
    .bind(&ev.target_id)
    .bind(&ev.details)
    .bind(&last_hash)
    .bind(hash.to_vec())
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE audit_heads SET last_seq = $2, last_hash = $3 WHERE tenant_id = $1")
        .bind(tenant)
        .bind(ev.seq)
        .bind(hash.to_vec())
        .execute(&mut **tx)
        .await?;
    Ok(ev.seq)
}

type Row = (
    i64,
    OffsetDateTime,
    Option<Uuid>,
    String,
    String,
    String,
    Option<String>,
    Value,
    Vec<u8>,
    Vec<u8>,
);

fn to_stored(tenant: Uuid, r: Row) -> StoredEvent {
    StoredEvent {
        event: Event {
            tenant_id: tenant,
            seq: r.0,
            occurred_at_us: micros(r.1),
            actor_user_id: r.2,
            actor_kind: r.3,
            action: r.4,
            target_type: r.5,
            target_id: r.6,
            details: r.7,
        },
        prev_hash: r.8,
        hash: r.9,
    }
}

const COLS: &str = "seq, occurred_at, actor_user_id, actor_kind, action, target_type, target_id, details, prev_hash, hash";

#[derive(Debug, Serialize)]
pub struct ChainStatus {
    pub intact: bool,
    pub events_checked: usize,
    pub problem: Option<String>,
    pub head_seq: i64,
    pub head_hash: String,
}

/// Recomputes the whole chain and compares its end with the stored head (detects tail truncation).
pub async fn verify_tenant(tx: &mut Tx, tenant: Uuid) -> Result<ChainStatus, ApiError> {
    db::set_tenant(tx, tenant).await?;
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events WHERE tenant_id = $1 ORDER BY seq"
    ))
    .bind(tenant)
    .fetch_all(&mut **tx)
    .await?;
    let events: Vec<StoredEvent> = rows.into_iter().map(|r| to_stored(tenant, r)).collect();
    let rep: VerifyReport = verify_chain(tenant, &events);
    let head: Option<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT last_seq, last_hash FROM audit_heads WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_optional(&mut **tx)
            .await?;
    let (head_seq, head_hash) = head.unwrap_or((0, audit_engine::GENESIS.to_vec()));
    let mut problem = rep.first_break.as_ref().map(|b| format!("{b:?}"));
    if problem.is_none() && (head_seq != events.len() as i64 || head_hash != rep.head_hash.to_vec())
    {
        problem = Some("HeadMismatch".into());
    }
    Ok(ChainStatus {
        intact: problem.is_none(),
        events_checked: rep.events_checked,
        problem,
        head_seq,
        head_hash: hex::encode(head_hash),
    })
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub after_seq: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct AuditEventOut {
    pub seq: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub actor_user_id: Option<Uuid>,
    pub actor_kind: String,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub details: Value,
    pub hash: String,
}

pub async fn list_events(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<AuditEventOut>>> {
    p.require(&st, Permission::AuditRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events WHERE seq > $1 ORDER BY seq LIMIT $2"
    ))
    .bind(q.after_seq.unwrap_or(0))
    .bind(q.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AuditEventOut {
                seq: r.0,
                occurred_at: r.1,
                actor_user_id: r.2,
                actor_kind: r.3,
                action: r.4,
                target_type: r.5,
                target_id: r.6,
                details: r.7,
                hash: hex::encode(r.9),
            })
            .collect(),
    ))
}

pub async fn verify(State(st): State<AppState>, p: Principal) -> ApiResult<Json<ChainStatus>> {
    p.require(&st, Permission::AuditVerify).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let status = verify_tenant(&mut tx, p.tenant_id).await?;
    if !status.intact {
        crate::security_events::record(
            &st.pool,
            Some(p.tenant_id),
            crate::security_events::Kind::AuditChainBroken,
            Some(p.user_id),
            None,
            serde_json::json!({ "problem": status.problem }),
        )
        .await;
    }
    tx.commit().await?;
    Ok(Json(status))
}
