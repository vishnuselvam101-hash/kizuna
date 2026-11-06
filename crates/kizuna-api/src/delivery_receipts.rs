//! Sealed delivery receipts (innovation I7). When a supplier delivers a document through the portal and it is stored,
//! the capture transaction also appends a `delivery_receipt.issued` audit event (file SHA-256 and size, file name,
//! supplier, uploading supplier user, request, capture event) and a `delivery_receipts` row linking the upload to it.
//!
//! `GET /portal/uploads/{id}/receipt` builds the receipt from that event: **pending** (the event only) until a signed
//! KZCP3 checkpoint covers it, then **final** (with the RFC 6962 inclusion proof and the checkpoint). Download it again
//! after the next checkpoint to get the final receipt. The supplier checks it offline with
//! `kizuna-verify-pack receipt <receipt.json> --file <document> --trusted-key <key id>`. Format and verifier:
//! `kizuna_verify_pack::receipt`. A file refused before storage (rejected type) gets no receipt.
//!
//! Not done here: commitments to extracted values (`delivery_receipt.claims_committed`) — the format and verifier
//! support them, the server does not issue them yet.

use audit_engine::checkpoint;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use kizuna_verify_pack::H32;
use kizuna_verify_pack::receipt::{
    ISSUED, RECEIPT_FORMAT, RECEIPT_VERSION, Receipt, ReceiptDetails, TARGET_TYPE, UploaderRef, build_receipt,
    verify_receipt,
};
use serde_json::json;
use uuid::Uuid;

use crate::app::AppState;
use crate::artifacts::Uploader;
use crate::audit::{self, Record};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};
use crate::evidence_pack::{anchored, entry, latest_covering};
use crate::portal::PortalPrincipal;
use crate::security_events::{self as sec, Kind};

/// Receipt downloads per supplier and minute.
pub const RECEIPTS_PER_MIN: u32 = 20;

/// Receipts a supplier portal delivery inside the capture transaction (`tx`, tenant scope). Returns the receipt id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn issue(
    tx: &mut Tx,
    up: &Uploader<'_>,
    portal_upload: Uuid,
    artifact: Uuid,
    sha256: &[u8; 32],
    size: usize,
    filename: &str,
    capture_seq: i64,
) -> ApiResult<Uuid> {
    let (supplier, request): (Uuid, Uuid) =
        sqlx::query_as("SELECT supplier_id, request_id FROM portal_uploads WHERE id = $1")
            .bind(portal_upload)
            .fetch_one(&mut **tx)
            .await?;
    let id = Uuid::now_v7();
    let details = ReceiptDetails {
        receipt_id: id,
        supplier_id: supplier,
        artifact_id: artifact,
        sha256: H32(*sha256),
        commitments: Vec::new(),
        channel: Some("supplier_portal".into()),
        uploader: Some(UploaderRef {
            kind: "supplier_user".into(),
            id: up.actor,
        }),
        size: Some(size as u64),
        filename: Some(filename.into()),
        capture_event_seq: Some(capture_seq),
        request_id: Some(request),
        delivery_receipt_id: None,
    };
    let seq = audit::append(
        tx,
        up.tenant,
        Record {
            actor_user_id: Some(up.actor),
            actor_kind: "supplier_user",
            action: ISSUED,
            target_type: TARGET_TYPE,
            target_id: Some(id.to_string()),
            details: serde_json::to_value(&details).map_err(ApiError::internal)?,
        },
    )
    .await?;
    sqlx::query(
        "INSERT INTO delivery_receipts (id, tenant_id, supplier_id, portal_upload_id, artifact_id, event_seq) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(id)
    .bind(up.tenant)
    .bind(supplier)
    .bind(portal_upload)
    .bind(artifact)
    .bind(seq)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// GET /portal/uploads/{id}/receipt — the canonical receipt JSON of one of the caller's supplier's uploads.
pub async fn download(
    State(st): State<AppState>,
    pp: PortalPrincipal,
    Path(upload): Path<Uuid>,
) -> ApiResult<Response> {
    const PATH: &str = "/portal/uploads/{id}/receipt";
    // A final receipt rebuilds the Merkle tree over the tenant's chain: bounded per supplier, like other heavy reads.
    if !st
        .limiter
        .check(&format!("portal-receipt:supplier:{}", pp.supplier_id), RECEIPTS_PER_MIN)
    {
        return Err(ApiError::TooManyRequests);
    }
    // Scope: the receipt row is visible only under the caller's own supplier scope (RLS; the predicate repeats it).
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    let row: Option<(Uuid, i64)> =
        sqlx::query_as("SELECT id, event_seq FROM delivery_receipts WHERE portal_upload_id = $1 AND supplier_id = $2")
            .bind(upload)
            .bind(pp.supplier_id)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    let Some((receipt_id, seq)) = row else {
        crate::portal::deny(
            &st,
            pp.tenant_id,
            Some(pp.user_id),
            &pp.ip,
            "receipt_out_of_scope",
            PATH,
        )
        .await;
        return Err(ApiError::NotFound);
    };

    // The receipt event itself (audit rows are outside the portal scope; the row above established the scope).
    let mut tx = db::tenant_snapshot_tx(&st.pool, pp.tenant_id).await?;
    let ev: audit::Row = sqlx::query_as(&format!("SELECT {} FROM audit_events WHERE seq = $1", audit::COLS))
        .bind(seq)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let receipt_target = receipt_id.to_string();
    if ev.4 != ISSUED || ev.5 != TARGET_TYPE || ev.6.as_deref() != Some(receipt_target.as_str()) {
        return Err(chain_broken(&st, &pp, "receipt_event_mismatch").await);
    }

    // Final when a valid checkpoint under the current key covers the event; pending otherwise (never signs here: a
    // supplier cannot trigger checkpoint creation).
    let public = audit::signing_key(&st).ok().map(|s| checkpoint::public_key(&s));
    let cp = match &public {
        Some(public) => latest_covering(&st, pp.tenant_id, seq, public).await?,
        None => None,
    };
    let (event, cp_entry) = match (cp, public) {
        (Some(cp), Some(public)) => {
            let (cp_entry, tree) = anchored(
                &st,
                pp.tenant_id,
                None,
                "delivery_receipt",
                &cp,
                &public,
                std::slice::from_ref(&ev),
            )
            .await?;
            (entry(&ev, Some(&tree))?, Some(cp_entry))
        }
        _ => (entry(&ev, None)?, None),
    };
    let receipt = Receipt {
        format: RECEIPT_FORMAT.into(),
        version: RECEIPT_VERSION,
        tenant_id: pp.tenant_id,
        receipt_id,
        event,
        checkpoint: cp_entry,
        openings: Vec::new(),
    };
    let bytes = build_receipt(&receipt).map_err(ApiError::internal)?;
    // Never hand out a receipt the offline verifier would refuse.
    let report = verify_receipt(&bytes, None, &Default::default());
    if report.status == "failed" {
        let codes: Vec<&str> = report.report.failures().map(|c| c.code).collect();
        tracing::error!(?codes, %receipt_id, "built delivery receipt does not verify");
        return Err(chain_broken(&st, &pp, "receipt_self_check_failed").await);
    }
    let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
    audit::append(
        &mut tx,
        pp.tenant_id,
        Record {
            actor_user_id: Some(pp.user_id),
            actor_kind: "supplier_user",
            action: "portal.receipt_downloaded",
            target_type: TARGET_TYPE,
            target_id: Some(receipt_target),
            details: json!({ "status": report.status, "checkpoint_seq": receipt.checkpoint.as_ref().map(|c| c.seq),
                             "receipt_sha256": hex::encode(crate::crypto::sha256(&bytes)) }),
        },
    )
    .await?;
    tx.commit().await?;
    let mut res = Response::new(Body::from(bytes));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"kizuna-delivery-receipt-{receipt_id}.json\""
        ))
        .map_err(ApiError::internal)?,
    );
    Ok(res)
}

async fn chain_broken(st: &AppState, pp: &PortalPrincipal, why: &str) -> ApiError {
    sec::record(
        &st.pool,
        Some(pp.tenant_id),
        Kind::AuditChainBroken,
        None,
        None,
        json!({ "via": "delivery_receipt", "problem": why }),
    )
    .await;
    ApiError::Conflict("audit_chain_not_intact")
}
