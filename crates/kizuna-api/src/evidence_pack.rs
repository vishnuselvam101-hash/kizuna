//! Offline-verifiable evidence pack of a decision (`POST /decisions/{id}/evidence-pack`). The format, the builder and
//! the standalone verifier live in the `kizuna-verify-pack` crate; this module only gathers the data.
//!
//! The pack carries the decision record, its derivation (claims → source locations, hashed into the audit chain at
//! decision time), the verification report, the source documents (or only their hashes), and RFC 6962 inclusion
//! proofs of the decision and capture events into a signed KZCP3 checkpoint.

use std::collections::BTreeSet;

use audit_engine::{checkpoint, merkle};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use kizuna_verify_pack::{self as kvp, ArtifactEntry, CheckpointEntry, EventEntry, H32, H64, Subject};
use policy_engine::{Decision, Permission, Resource};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::security_events::{self as sec, Kind};

/// Upper bound of document bytes in one pack; larger exports must use hash-only for some documents.
pub const MAX_PACK_DOCUMENT_BYTES: i64 = 256 << 20;
const CAPTURE_ACTIONS: [&str; 2] = ["artifact.uploaded", "artifact.recaptured"];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExportReq {
    /// Artifacts to export as hash only (redaction): the file is left out, its hash and proofs stay.
    #[serde(default)]
    pub hash_only: Vec<Uuid>,
}

fn h32(v: &[u8]) -> ApiResult<H32> {
    <[u8; 32]>::try_from(v)
        .map(H32)
        .map_err(|_| ApiError::Conflict("audit_chain_not_intact"))
}

async fn capture_event(tx: &mut Tx, artifact: Uuid, sha: &str) -> ApiResult<Option<audit::Row>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {} FROM audit_events WHERE target_type = 'artifact' AND target_id = $1 AND action = ANY($2) \
         AND details->>'sha256' = $3 ORDER BY seq LIMIT 1",
        audit::COLS
    ))
    .bind(artifact.to_string())
    .bind(CAPTURE_ACTIONS.map(String::from).to_vec())
    .bind(sha)
    .fetch_optional(&mut **tx)
    .await?)
}

/// The newest KZCP3 checkpoint of `tenant` under the current key, if it covers `upto` and its signature verifies.
pub(crate) async fn latest_covering(
    st: &AppState,
    tenant: Uuid,
    upto: i64,
    public: &[u8; 32],
) -> ApiResult<Option<audit::Checkpoint>> {
    let kid = audit::key_id(public);
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let row: Option<audit::CpRow> = sqlx::query_as(&format!(
        "SELECT {} FROM audit_checkpoints WHERE merkle_root IS NOT NULL ORDER BY seq DESC, signed_at DESC LIMIT 1",
        audit::CP_COLS
    ))
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    if let Some((seq, head, at, key, sig, prev, root)) = row
        && seq >= upto
        && key == kid
    {
        let c = audit::Checkpoint {
            tenant_id: tenant,
            seq,
            head_hash: hex::encode(&head),
            signed_at_us: audit::micros(at),
            key_id: key,
            public_key: hex::encode(public),
            signature: hex::encode(&sig),
            prev_signature_sha256: prev.map(hex::encode),
            merkle_root: root.map(hex::encode),
        };
        if checkpoint::verify(public, &audit::message_of(&c, &head), &sig) {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// The newest covering checkpoint (see [`latest_covering`]); otherwise a new one when the caller may create
/// checkpoints (`audit_verify`), else 409 `checkpoint_required`.
async fn covering_checkpoint(
    st: &AppState,
    p: &Principal,
    upto: i64,
    public: &[u8; 32],
) -> ApiResult<audit::Checkpoint> {
    if let Some(c) = latest_covering(st, p.tenant_id, upto, public).await? {
        return Ok(c);
    }
    let may_sign = matches!(
        st.policy.authorize(
            &p.subject(),
            Permission::AuditVerify,
            &Resource {
                tenant_id: p.tenant_id,
                kind: "tenant",
                owner_user_id: None
            }
        ),
        Decision::Allow
    );
    if !may_sign {
        return Err(ApiError::Conflict("checkpoint_required"));
    }
    audit::sign_checkpoint(st, p).await
}

/// An audit row as a pack/receipt event entry, with its inclusion proof in `tree` (none when `tree` is `None`).
pub(crate) fn entry(r: &audit::Row, tree: Option<&merkle::Tree>) -> ApiResult<EventEntry> {
    let proof = match tree {
        None => Vec::new(),
        Some(tree) => u64::try_from(r.0 - 1)
            .ok()
            .and_then(|i| tree.prove(i))
            .ok_or(ApiError::Conflict("audit_chain_not_intact"))?,
    };
    Ok(EventEntry {
        seq: r.0,
        occurred_at_us: audit::micros(r.1),
        actor_user_id: r.2,
        actor_kind: r.3.clone(),
        action: r.4.clone(),
        target_type: r.5.clone(),
        target_id: r.6.clone(),
        details: r.7.clone(),
        prev_hash: h32(&r.8)?,
        hash: h32(&r.9)?,
        proof: proof.into_iter().map(H32).collect(),
    })
}

/// The Merkle tree over the stored chain up to `cp`, checked against the signed root and head and against every row of
/// `events` (a mismatch is recorded as `audit.chain_broken`), and `cp` as a pack/receipt checkpoint entry.
pub(crate) async fn anchored(
    st: &AppState,
    tenant: Uuid,
    user: Option<Uuid>,
    via: &str,
    cp: &audit::Checkpoint,
    public: &[u8; 32],
    events: &[audit::Row],
) -> ApiResult<(CheckpointEntry, merkle::Tree)> {
    let mut tx = db::tenant_snapshot_tx(&st.pool, tenant).await?;
    let leaves = audit::chain_hashes(&mut tx, tenant, cp.seq).await?;
    tx.commit().await?;
    let tree = tokio::task::spawn_blocking(move || merkle::Tree::from_leaves(leaves))
        .await
        .map_err(ApiError::internal)?;
    let root = hex::decode(cp.merkle_root.as_deref().unwrap_or_default()).unwrap_or_default();
    let head = hex::decode(&cp.head_hash).unwrap_or_default();
    let head_proof = tree.prove(tree.len().saturating_sub(1));
    let leaf_ok = |seq: i64, h: &[u8]| {
        u64::try_from(seq - 1)
            .ok()
            .and_then(|i| tree.prove(i).map(|p| (i, p)))
            .is_some_and(|(i, p)| {
                h.try_into()
                    .is_ok_and(|h: [u8; 32]| merkle::verify(&tree.root(), tree.len(), i, &h, &p).is_ok())
            })
    };
    if tree.root().as_slice() != root.as_slice()
        || head_proof.is_none()
        || !leaf_ok(cp.seq, &head)
        || !events.iter().all(|e| leaf_ok(e.0, &e.9))
    {
        sec::record(
            &st.pool,
            Some(tenant),
            Kind::AuditChainBroken,
            user,
            None,
            json!({ "via": via, "checkpoint_seq": cp.seq }),
        )
        .await;
        return Err(ApiError::Conflict("audit_chain_not_intact"));
    }
    let cp_entry = CheckpointEntry {
        message_version: "KZCP3".into(),
        seq: cp.seq,
        head_hash: h32(&head)?,
        signed_at_us: cp.signed_at_us,
        prev_signature_sha256: h32(
            &hex::decode(cp.prev_signature_sha256.as_deref().unwrap_or_default()).unwrap_or_default()
        )?,
        merkle_root: h32(&root)?,
        key_id: cp.key_id.clone(),
        public_key: H32(*public),
        signature: H64(
            <[u8; 64]>::try_from(hex::decode(&cp.signature).unwrap_or_default().as_slice())
                .map_err(ApiError::internal)?,
        ),
        head_proof: head_proof.unwrap_or_default().into_iter().map(H32).collect(),
    };
    Ok((cp_entry, tree))
}

fn pretty(v: &Value) -> ApiResult<Vec<u8>> {
    serde_json::to_vec_pretty(v).map_err(ApiError::internal)
}

pub async fn export(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    rl: crate::lang::ReqLang,
    ApiJson(req): ApiJson<ExportReq>,
) -> ApiResult<Response> {
    p.require(&st, Permission::DecisionRead).await?;
    p.require(&st, Permission::ArtifactRead).await?;
    audit::heavy_limit(&st, &p)?;
    if req.hash_only.len() > 1000 {
        return Err(ApiError::BadRequest("too_many_ids"));
    }
    let seed = audit::signing_key(&st)?;
    let public = checkpoint::public_key(&seed);

    // One consistent snapshot of the decision, its derivation, events and artifacts.
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let trace = crate::decisions::build_trace(&mut tx, id).await?;
    let (derivation, stored): (Value, Vec<u8>) =
        sqlx::query_as("SELECT derivation, derivation_sha256 FROM decisions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let derivation_bytes = audit_engine::canonical_json(&derivation)
        .map_err(ApiError::internal)?
        .into_bytes();
    if crate::crypto::sha256(&derivation_bytes).as_slice() != stored.as_slice() {
        return Err(ApiError::Conflict("derivation_not_reproducible"));
    }
    let stored_hex = hex::encode(&stored);
    // The decision's own history (recorded, invalidated, reconfirmed, revoked); earlier exports are not evidence.
    let decision_events: Vec<audit::Row> = sqlx::query_as(&format!(
        "SELECT {} FROM audit_events WHERE target_type = 'decision' AND target_id = $1 \
         AND action <> 'decision.evidence_pack_exported' ORDER BY seq LIMIT 1000",
        audit::COLS
    ))
    .bind(id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let recorded_seq = decision_events
        .iter()
        .find(|r| r.4 == "decision.recorded" && r.7["derivation_sha256"].as_str() == Some(stored_hex.as_str()))
        .map(|r| r.0)
        .ok_or(ApiError::Conflict("decision_not_audited"))?;
    let mut cited = BTreeSet::new();
    for e in derivation["evidence"].as_array().into_iter().flatten() {
        if let Some(a) = e["artifact_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            cited.insert(a);
        }
    }
    if let Some(u) = req.hash_only.iter().find(|a| !cited.contains(*a)) {
        tracing::debug!(artifact = %u, "hash_only names an artifact the decision does not cite");
        return Err(ApiError::BadRequest("unknown_artifact"));
    }
    let mut artifacts = Vec::new();
    let mut events = decision_events;
    for aid in &cited {
        let a = crate::artifacts::fetch(&mut tx, *aid).await?;
        let cap = capture_event(&mut tx, *aid, &a.sha256)
            .await?
            .ok_or(ApiError::Conflict("artifact_capture_not_audited"))?;
        artifacts.push((a, cap.0));
        if !events.iter().any(|e| e.0 == cap.0) {
            events.push(cap);
        }
    }
    let document_bytes: i64 = artifacts
        .iter()
        .filter(|(a, _)| a.status == "accepted" && !req.hash_only.contains(&a.id))
        .map(|(a, _)| a.size_bytes)
        .sum();
    tx.commit().await?;
    if document_bytes > MAX_PACK_DOCUMENT_BYTES {
        return Err(ApiError::Conflict("pack_too_large_use_hash_only"));
    }
    events.sort_by_key(|e| e.0);
    let upto = events.last().map(|e| e.0).unwrap_or(recorded_seq);

    // Signed checkpoint covering every event, and its Merkle tree rebuilt from the stored chain.
    let cp = covering_checkpoint(&st, &p, upto, &public).await?;
    let (cp_entry, tree) = anchored(
        &st,
        p.tenant_id,
        Some(p.user_id),
        "evidence_pack",
        &cp,
        &public,
        &events,
    )
    .await?;
    let event_entries = events
        .iter()
        .map(|e| entry(e, Some(&tree)))
        .collect::<ApiResult<Vec<_>>>()?;

    // Document bytes, re-checked against the captured hash on the way out (never ship a corrupted file).
    let mut pack_artifacts = Vec::new();
    for (a, cap_seq) in artifacts {
        let reason = if req.hash_only.contains(&a.id) {
            Some("requested")
        } else if a.status != "accepted" {
            Some("not_released")
        } else {
            None
        };
        let bytes = match reason {
            Some(_) => None,
            None => {
                let (ok, bytes, _) = crate::artifacts::verify_integrity(&st, &p, a.id).await?;
                if !ok {
                    return Err(ApiError::Conflict("integrity_check_failed"));
                }
                Some(bytes)
            }
        };
        pack_artifacts.push((
            ArtifactEntry {
                id: a.id,
                name: a.display_name.clone(),
                media_type: a.media_type.clone(),
                sha256: h32(&hex::decode(&a.sha256).unwrap_or_default())?,
                size: u64::try_from(a.size_bytes).unwrap_or(0),
                included: bytes.is_some(),
                excluded_reason: reason.map(String::from),
                path: None,
                capture_event_seq: cap_seq,
            },
            bytes,
        ));
    }
    let report = crate::decisions::report_body(&st, &p, id, rl.0.unwrap_or(kizuna_messages::Lang::En)).await?;
    let hash_only: Vec<Uuid> = pack_artifacts
        .iter()
        .filter(|(a, b)| b.is_none() && a.excluded_reason.as_deref() == Some("requested"))
        .map(|(a, _)| a.id)
        .collect();
    let input = kvp::BuildInput {
        generated_at_us: audit::micros(audit::now_us()),
        tenant_id: p.tenant_id,
        subject: Subject {
            kind: "decision".into(),
            id,
            derivation_sha256: h32(&stored)?,
            recorded_event_seq: recorded_seq,
        },
        artifacts: pack_artifacts,
        events: event_entries,
        checkpoint: cp_entry,
        documents: vec![
            (kvp::DERIVATION.into(), derivation_bytes),
            ("decision/record.json".into(), pretty(&trace["decision"])?),
            ("decision/trace.json".into(), pretty(&trace)?),
            ("decision/report.json".into(), pretty(&report)?),
        ],
    };
    let zip = tokio::task::spawn_blocking(move || kvp::build(input, &seed))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let pack_sha = hex::encode(crate::crypto::sha256(&zip));
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "decision.evidence_pack_exported",
            "decision",
            id,
            json!({ "pack_sha256": pack_sha, "checkpoint_seq": cp.seq, "key_id": cp.key_id, "hash_only": hash_only, "size": zip.len() }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::ExportReport,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "decision_id": id, "format": "evidence_pack", "pack_sha256": pack_sha }),
    )
    .await;
    let mut res = Response::new(Body::from(zip));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"kizuna-evidence-pack-{id}.zip\""))
            .map_err(ApiError::internal)?,
    );
    Ok(res)
}
