//! Writes and verifies the per-tenant hash-chained audit log (hashing in `audit-engine`).

use audit_engine::{ChainVerifier, Event, StoredEvent, VerifyReport, chain_hash};
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};
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
    pub fn user(actor: Uuid, action: &'a str, target_type: &'a str, target_id: impl ToString, details: Value) -> Self {
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

pub(crate) fn micros(t: OffsetDateTime) -> i64 {
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
    // Serialise appends per tenant with a transaction lock (the runtime role cannot UPDATE audit_heads, so no row
    // lock; the head advances in a database trigger that only accepts the next link — deep review KZ-I1).
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 41))")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    let (last_seq, last_hash): (i64, Vec<u8>) =
        sqlx::query_as("SELECT last_seq, last_hash FROM audit_heads WHERE tenant_id = $1")
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
    Ok(ev.seq)
}

pub(crate) type Row = (
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

pub(crate) const COLS: &str =
    "seq, occurred_at, actor_user_id, actor_kind, action, target_type, target_id, details, prev_hash, hash";

#[derive(Debug, Serialize)]
pub struct ChainStatus {
    pub intact: bool,
    pub events_checked: usize,
    pub problem: Option<String>,
    pub head_seq: i64,
    pub head_hash: String,
}

/// Recomputes the whole chain in batches and compares its end with the stored head (detects tail truncation).
/// Call inside a snapshot transaction (`db::tenant_snapshot_tx`) so concurrent appends cannot cause false alarms.
pub async fn verify_tenant(tx: &mut Tx, tenant: Uuid) -> Result<ChainStatus, ApiError> {
    verify_from(tx, tenant, None).await
}

/// Verifies the chain from a trusted anchor `(seq, hash)` (or from genesis) to the stored head.
pub async fn verify_from(tx: &mut Tx, tenant: Uuid, anchor: Option<(i64, [u8; 32])>) -> Result<ChainStatus, ApiError> {
    db::set_tenant(tx, tenant).await?;
    let head: Option<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT last_seq, last_hash FROM audit_heads WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_optional(&mut **tx)
            .await?;
    let (mut v, mut last) = match anchor {
        Some((seq, h)) => (ChainVerifier::resume(tenant, seq, h), seq),
        None => (ChainVerifier::new(tenant), 0i64),
    };
    let mut total = usize::try_from(last).unwrap_or(0);
    loop {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM audit_events WHERE tenant_id = $1 AND seq > $2 ORDER BY seq LIMIT 1000"
        ))
        .bind(tenant)
        .bind(last)
        .fetch_all(&mut **tx)
        .await?;
        let Some(r) = rows.last() else { break };
        last = r.0;
        total += rows.len();
        if !rows.into_iter().all(|r| v.feed(&to_stored(tenant, r))) {
            break;
        }
    }
    let rep: VerifyReport = v.report();
    let (head_seq, head_hash) = head.unwrap_or((0, audit_engine::GENESIS.to_vec()));
    let mut problem = rep.first_break.as_ref().map(|b| format!("{b:?}"));
    if problem.is_none() && (head_seq != total as i64 || head_hash != rep.head_hash.to_vec()) {
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

/// Full-chain work is O(chain): per user 6/min and per tenant 20/min (review M5-M7 M2).
pub fn heavy_limit(st: &AppState, p: &Principal) -> ApiResult<()> {
    if !st.limiter.check(&format!("heavy:user:{}", p.user_id), 6)
        || !st.limiter.check(&format!("heavy:tenant:{}", p.tenant_id), 20)
    {
        return Err(ApiError::TooManyRequests);
    }
    Ok(())
}

pub async fn verify(State(st): State<AppState>, p: Principal) -> ApiResult<Json<ChainStatus>> {
    p.require(&st, Permission::AuditVerify).await?;
    heavy_limit(&st, &p)?;
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
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

#[derive(Serialize, Deserialize, Clone)]
pub struct Checkpoint {
    pub tenant_id: Uuid,
    pub seq: i64,
    pub head_hash: String,
    pub signed_at_us: i64,
    pub key_id: String,
    pub public_key: String,
    pub signature: String,
    /// KZCP2 chain link: SHA-256 (hex) of the previous checkpoint's signature; absent on legacy KZCP1 checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_signature_sha256: Option<String>,
    /// KZCP3: RFC 6962 Merkle root (hex) over the chain hashes of events `1..=seq`; absent on older checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merkle_root: Option<String>,
}

/// The chain link a checkpoint must carry: SHA-256 of the previous signature, or zeros for the first one.
fn expected_prev(last_sig: Option<&[u8]>) -> [u8; 32] {
    last_sig.map(crate::crypto::sha256).unwrap_or([0u8; 32])
}

fn hex32(h: Option<&str>) -> Option<[u8; 32]> {
    h.and_then(|h| hex::decode(h).ok())
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
}

pub(crate) fn message_of(c: &Checkpoint, head: &[u8]) -> Vec<u8> {
    match (
        hex32(c.prev_signature_sha256.as_deref()),
        hex32(c.merkle_root.as_deref()),
    ) {
        (Some(prev), Some(root)) => {
            audit_engine::checkpoint::message_v3(c.tenant_id, c.seq, head, c.signed_at_us, &prev, &root)
        }
        (Some(prev), None) => audit_engine::checkpoint::message_v2(c.tenant_id, c.seq, head, c.signed_at_us, &prev),
        _ => audit_engine::checkpoint::message(c.tenant_id, c.seq, head, c.signed_at_us),
    }
}

/// Chain hashes of events `1..=upto` in order (leaves of the KZCP3 Merkle tree); errors when a sequence number is
/// missing. Call inside the same snapshot transaction as the chain verification.
pub(crate) async fn chain_hashes(tx: &mut Tx, tenant: Uuid, upto: i64) -> ApiResult<Vec<[u8; 32]>> {
    let mut out = Vec::with_capacity(usize::try_from(upto).unwrap_or(0));
    let mut last = 0i64;
    while last < upto {
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, hash FROM audit_events WHERE tenant_id = $1 AND seq > $2 AND seq <= $3 ORDER BY seq LIMIT 50000",
        )
        .bind(tenant)
        .bind(last)
        .bind(upto)
        .fetch_all(&mut **tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        for (seq, h) in rows {
            if seq != last + 1 {
                return Err(ApiError::Conflict("audit_chain_not_intact"));
            }
            out.push(<[u8; 32]>::try_from(h.as_slice()).map_err(|_| ApiError::Conflict("audit_chain_not_intact"))?);
            last = seq;
        }
    }
    if last != upto {
        return Err(ApiError::Conflict("audit_chain_not_intact"));
    }
    Ok(out)
}

pub(crate) fn signing_key(st: &AppState) -> ApiResult<[u8; 32]> {
    st.cfg
        .audit_signing_key
        .as_ref()
        .map(|k| *k.expose())
        .ok_or(ApiError::Conflict("signing_key_not_configured"))
}

pub(crate) fn key_id(public: &[u8; 32]) -> String {
    hex::encode(&crate::crypto::sha256(public)[..8])
}

/// Verification keys by key id: the current signing key and retired ones (review M5-M7 L4).
pub(crate) fn verification_keys(st: &AppState) -> Vec<(String, [u8; 32], bool)> {
    let mut out = Vec::new();
    if let Some(k) = &st.cfg.audit_signing_key {
        let p = audit_engine::checkpoint::public_key(k.expose());
        out.push((key_id(&p), p, true));
    }
    for k in &st.cfg.audit_signing_keys_previous {
        let p = audit_engine::checkpoint::public_key(k.expose());
        out.push((key_id(&p), p, false));
    }
    out
}

pub(crate) type CpRow = (
    i64,
    Vec<u8>,
    OffsetDateTime,
    String,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);
pub(crate) const CP_COLS: &str = "seq, head_hash, signed_at, key_id, signature, prev_sig_sha256, merkle_root";

/// Signs the chain head of a consistent snapshot (only if the chain verifies). Verification starts at the latest
/// checkpoint whose signature and stored hash still check out (incremental), and no lock is held while verifying,
/// so audited writes of the tenant never wait for it (review M5-M7 M2). Export the returned JSON outside the DB.
pub async fn create_checkpoint(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Checkpoint>> {
    p.require(&st, Permission::AuditVerify).await?;
    heavy_limit(&st, &p)?;
    Ok(Json(sign_checkpoint(&st, &p).await?))
}

/// Verifies the chain (incrementally from the newest valid checkpoint) and signs a KZCP3 checkpoint of its head.
/// Callers check permissions and rate limits.
pub(crate) async fn sign_checkpoint(st: &AppState, p: &Principal) -> ApiResult<Checkpoint> {
    let seed = signing_key(st)?;
    let keys = verification_keys(st);
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    // Every checkpoint under a known key must still match the chain; otherwise refuse and alarm instead of
    // signing a (possibly re-chained) head (deep review KZ-C1). The newest valid one is the anchor.
    let rows: Vec<CpRow> = sqlx::query_as(&format!(
        "SELECT {CP_COLS} FROM audit_checkpoints ORDER BY seq, signed_at"
    ))
    .fetch_all(&mut *tx)
    .await?;
    let mut anchor = None;
    let mut last_sig: Option<Vec<u8>> = None;
    for (seq, head, at, kid, sig, prev, root) in rows {
        // Chain link: a KZCP2 checkpoint must point at the signature of the one before it.
        let link_ok = prev
            .as_ref()
            .is_none_or(|p| p.as_slice() == expected_prev(last_sig.as_deref()));
        last_sig = Some(sig.clone());
        let Some((_, public, _)) = keys.iter().find(|k| k.0 == kid) else {
            continue;
        };
        let c = Checkpoint {
            tenant_id: p.tenant_id,
            seq,
            head_hash: hex::encode(&head),
            signed_at_us: micros(at),
            key_id: kid,
            public_key: hex::encode(public),
            signature: hex::encode(&sig),
            prev_signature_sha256: prev.map(hex::encode),
            merkle_root: root.map(hex::encode),
        };
        let r = check_one(&mut tx, &keys, &c).await?;
        if r["signature_valid"] != true || r["chain_matches"] != true || !link_ok {
            tx.commit().await?;
            crate::security_events::record(
                &st.pool,
                Some(p.tenant_id),
                crate::security_events::Kind::AuditChainBroken,
                Some(p.user_id),
                None,
                json!({ "via": "checkpoint_create", "seq": seq }),
            )
            .await;
            return Err(ApiError::Conflict("audit_chain_not_intact"));
        }
        if let Ok(h) = <[u8; 32]>::try_from(head.as_slice()) {
            anchor = Some((seq, h));
        }
    }
    let status = verify_from(&mut tx, p.tenant_id, anchor).await?;
    if !status.intact {
        tx.commit().await?;
        return Err(ApiError::Conflict("audit_chain_not_intact"));
    }
    // Same snapshot: the Merkle leaves are exactly the verified chain up to the head.
    let leaves = chain_hashes(&mut tx, p.tenant_id, status.head_seq).await?;
    tx.commit().await?;
    let tree = tokio::task::spawn_blocking(move || audit_engine::merkle::Tree::from_leaves(leaves))
        .await
        .map_err(ApiError::internal)?;
    let root = tree.root();
    let head = hex::decode(&status.head_hash).map_err(ApiError::internal)?;
    let at = micros(now_us());
    let prev = expected_prev(last_sig.as_deref());
    let msg = audit_engine::checkpoint::message_v3(p.tenant_id, status.head_seq, &head, at, &prev, &root);
    let sig = audit_engine::checkpoint::sign(&seed, &msg);
    let public = audit_engine::checkpoint::public_key(&seed);
    let kid = key_id(&public);
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("INSERT INTO audit_checkpoints (id, tenant_id, seq, head_hash, signed_at, key_id, signature, created_by, prev_sig_sha256, merkle_root) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(status.head_seq)
        .bind(&head)
        .bind(OffsetDateTime::from_unix_timestamp_nanos(at as i128 * 1000).map_err(ApiError::internal)?)
        .bind(&kid)
        .bind(sig.to_vec())
        .bind(p.user_id)
        .bind(prev.to_vec())
        .bind(root.to_vec())
        .execute(&mut *tx)
        .await?;
    append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "audit.checkpoint",
            "audit_log",
            status.head_seq,
            json!({ "key_id": kid, "incremental_from": anchor.map(|a| a.0) }),
        ),
    )
    .await?;
    tx.commit().await?;
    let cp = Checkpoint {
        tenant_id: p.tenant_id,
        seq: status.head_seq,
        head_hash: status.head_hash,
        signed_at_us: at,
        key_id: kid,
        public_key: hex::encode(public),
        signature: hex::encode(sig),
        prev_signature_sha256: Some(hex::encode(prev)),
        merkle_root: Some(hex::encode(root)),
    };
    // Export off-host automatically (KZ-C3); delivery failures never fail the checkpoint.
    if st.alerts.enabled() {
        let alerts = st.alerts.clone();
        let a = crate::alerts::checkpoint_alert(alerts.label(), &json!(cp));
        let _ = tokio::task::spawn_blocking(move || alerts.send(&a)).await;
    }
    Ok(cp)
}

/// Checks a checkpoint's signature against the key named by its key id (current or retired) and that the stored
/// chain still has that hash at that position. `key_status`: `current`, `retired` or `unknown_key` (cannot be
/// checked: not evidence of tampering).
async fn check_one(tx: &mut Tx, keys: &[(String, [u8; 32], bool)], c: &Checkpoint) -> ApiResult<Value> {
    let head = hex::decode(&c.head_hash).unwrap_or_default();
    let sig = hex::decode(&c.signature).unwrap_or_default();
    let key = keys.iter().find(|k| k.0 == c.key_id);
    let key_status = match key {
        Some((_, _, true)) => "current",
        Some(_) => "retired",
        None => "unknown_key",
    };
    let sig_ok = key.is_some_and(|(_, public, _)| {
        c.public_key == hex::encode(public) && audit_engine::checkpoint::verify(public, &message_of(c, &head), &sig)
    });
    let stored: Option<Vec<u8>> = sqlx::query_scalar("SELECT hash FROM audit_events WHERE tenant_id = $1 AND seq = $2")
        .bind(c.tenant_id)
        .bind(c.seq)
        .fetch_optional(&mut **tx)
        .await?;
    let chain_ok = stored.as_deref() == Some(head.as_slice());
    Ok(
        json!({ "seq": c.seq, "key_id": c.key_id, "key_status": key_status, "signature_valid": sig_ok, "chain_matches": chain_ok }),
    )
}

pub async fn verify_checkpoints(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::AuditVerify).await?;
    heavy_limit(&st, &p)?;
    signing_key(&st)?;
    let keys = verification_keys(&st);
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let rows: Vec<CpRow> = sqlx::query_as(&format!(
        "SELECT {CP_COLS} FROM audit_checkpoints ORDER BY seq, signed_at"
    ))
    .fetch_all(&mut *tx)
    .await?;
    let mut results = Vec::new();
    let mut last_sig: Option<Vec<u8>> = None;
    let mut links_ok = true;
    for (seq, head, at, kid, sig, prev, root) in rows {
        if let Some(p) = &prev
            && p.as_slice() != expected_prev(last_sig.as_deref())
        {
            links_ok = false;
        }
        last_sig = Some(sig.clone());
        let public = keys
            .iter()
            .find(|k| k.0 == kid)
            .map(|k| hex::encode(k.1))
            .unwrap_or_default();
        let c = Checkpoint {
            tenant_id: p.tenant_id,
            seq,
            head_hash: hex::encode(head),
            signed_at_us: micros(at),
            key_id: kid,
            public_key: public,
            signature: hex::encode(&sig),
            prev_signature_sha256: prev.map(hex::encode),
            merkle_root: root.map(hex::encode),
        };
        results.push(check_one(&mut tx, &keys, &c).await?);
    }
    let chain = verify_tenant(&mut tx, p.tenant_id).await?;
    tx.commit().await?;
    // Checkpoints under a key that is no longer configured cannot be checked; they are reported, not alarmed.
    let unknown_keys = results.iter().filter(|r| r["key_status"] == "unknown_key").count();
    let all_ok = chain.intact
        && links_ok
        && results
            .iter()
            .filter(|r| r["key_status"] != "unknown_key")
            .all(|r| r["signature_valid"] == true && r["chain_matches"] == true);
    if !all_ok {
        crate::security_events::record(
            &st.pool,
            Some(p.tenant_id),
            crate::security_events::Kind::AuditChainBroken,
            Some(p.user_id),
            None,
            json!({ "via": "checkpoint_verification" }),
        )
        .await;
    }
    Ok(Json(
        json!({ "intact": all_ok, "chain": chain, "checkpoints": results, "checkpoint_links_intact": links_ok,
               "unverifiable_unknown_key": unknown_keys }),
    ))
}

/// Verifies an exported checkpoint (kept outside the database) against the current chain.
pub async fn verify_external(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(c): ApiJson<Checkpoint>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::AuditVerify).await?;
    heavy_limit(&st, &p)?;
    if c.tenant_id != p.tenant_id {
        return Err(ApiError::Forbidden("cross_tenant"));
    }
    signing_key(&st)?;
    let keys = verification_keys(&st);
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let r = check_one(&mut tx, &keys, &c).await?;
    let chain = verify_tenant(&mut tx, p.tenant_id).await?;
    tx.commit().await?;
    if r["signature_valid"] != true {
        // A forged or garbled checkpoint is a bad request, not evidence of tampering (final review L1).
        return Err(ApiError::BadRequest("invalid_checkpoint_signature"));
    }
    let ok = r["signature_valid"] == true && r["chain_matches"] == true && chain.intact;
    if !ok {
        crate::security_events::record(
            &st.pool,
            Some(p.tenant_id),
            crate::security_events::Kind::AuditChainBroken,
            Some(p.user_id),
            None,
            json!({ "via": "external_checkpoint", "seq": c.seq }),
        )
        .await;
    }
    Ok(Json(json!({ "intact": ok, "checkpoint": r, "chain": chain })))
}
