//! Artifact ingestion: untrusted uploads are size-limited, typed by content, statically scanned, hashed and stored
//! content-addressed. Originals are immutable; every capture is recorded. Integrity is re-verified on every read.
//! A matching hash proves the bytes are unchanged since capture, not that the document is truthful or authentic.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use evidence_core::scan::{self, Limits, Verdict};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::antivirus::Outcome;
use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::crypto;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};
use crate::security_events::{self as sec, Kind};
use crate::storage::ObjectKey;

pub const KINDS: [&str; 6] = [
    "spec_document",
    "drawing",
    "purchase_order",
    "inspection_report",
    "spreadsheet",
    "other",
];

/// Display-only file name: last path component, no control or quote characters, at most 255 chars.
pub fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '`'))
        .take(255)
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "upload".into() } else { clean }
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ArtifactOut {
    pub id: Uuid,
    pub sha256: String,
    pub size_bytes: i64,
    pub media_type: String,
    pub kind: String,
    pub status: String,
    pub scan_result: Value,
    pub display_name: String,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// `internal` or `supplier_portal` (uploaded by a supplier user; reviewed like any other document).
    pub origin: String,
}

pub const ART_COLS: &str = "id, encode(sha256, 'hex') AS sha256, size_bytes, media_type, kind, status, scan_result, \
                            display_name, created_by, created_at, origin";

#[derive(Serialize)]
pub struct UploadResp {
    pub artifact: ArtifactOut,
    pub duplicate: bool,
}

pub(crate) async fn read_upload(mut mp: Multipart, max: usize) -> ApiResult<(Vec<u8>, String, Option<String>, String)> {
    let (mut data, mut name, mut declared, mut kind) = (None, String::new(), None, "other".to_string());
    while let Some(mut field) = mp
        .next_field()
        .await
        .map_err(|_| ApiError::BadRequest("invalid_multipart"))?
    {
        match field.name() {
            Some("file") if data.is_none() => {
                name = sanitize_filename(field.file_name().unwrap_or(""));
                declared = field.content_type().map(|s| s.chars().take(100).collect());
                let mut buf = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(|_| ApiError::TooLarge)? {
                    if buf.len() + chunk.len() > max {
                        return Err(ApiError::TooLarge);
                    }
                    buf.extend_from_slice(&chunk);
                }
                data = Some(buf);
            }
            Some("kind") => {
                let k = field
                    .text()
                    .await
                    .map_err(|_| ApiError::BadRequest("invalid_multipart"))?;
                if !KINDS.contains(&k.as_str()) {
                    return Err(ApiError::BadRequest("invalid_kind"));
                }
                kind = k;
            }
            _ => return Err(ApiError::BadRequest("unexpected_field")),
        }
    }
    let data = data
        .filter(|d| !d.is_empty())
        .ok_or(ApiError::BadRequest("missing_file"))?;
    Ok((data, name, declared, kind))
}

/// Effective per-upload byte limit: the tenant's Import setting (`max_upload_mb`, when set and positive) can only
/// lower the operator ceiling `KIZUNA_MAX_UPLOAD_BYTES`, never raise it.
pub fn upload_limit(server_max: usize, tenant_mb: Option<i64>) -> usize {
    match tenant_mb {
        Some(mb) if mb > 0 => usize::try_from(mb)
            .ok()
            .and_then(|m| m.checked_mul(1024 * 1024))
            .map_or(server_max, |b| b.min(server_max)),
        _ => server_max,
    }
}

pub async fn upload(State(st): State<AppState>, p: Principal, mp: Multipart) -> ApiResult<Json<UploadResp>> {
    p.require(&st, Permission::ArtifactUpload).await?;
    if !st.limiter.check(
        &format!("upload:user:{}", p.user_id),
        st.cfg.rate.uploads_per_user_per_min,
    ) {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::RateLimited,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "endpoint": "upload" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    let limit = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let blocked = crate::incidents::uploads_blocked(&mut tx, p.user_id).await?;
        let tenant_mb = tenant_upload_mb(&mut tx).await?;
        tx.commit().await?;
        if blocked {
            return Err(ApiError::Forbidden("uploads_restricted"));
        }
        upload_limit(st.cfg.max_upload_bytes, tenant_mb)
    };
    let (data, name, declared, kind) = read_upload(mp, limit).await?;
    let up = Uploader {
        tenant: p.tenant_id,
        actor: p.user_id,
        session: Session::User(p.session_id),
        ip: &p.ip,
    };
    let r = ingest(&st, &up, data, &name, declared, &kind, None).await?;
    Ok(Json(UploadResp {
        artifact: r.artifact,
        duplicate: r.duplicate,
    }))
}

/// The tenant's Import setting `max_upload_mb`, if any.
pub(crate) async fn tenant_upload_mb(tx: &mut Tx) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT CASE WHEN jsonb_typeof(settings->'max_upload_mb') = 'number' \
           THEN floor((settings->>'max_upload_mb')::numeric)::bigint END \
         FROM import_settings ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?
    .flatten())
}

/// Whose session carries an upload. A supplier session is a different principal (supplier portal, ADR 0025).
pub(crate) enum Session {
    User(Uuid),
    Supplier(Uuid),
}

pub(crate) struct Uploader<'a> {
    pub tenant: Uuid,
    /// User id, or supplier-user id for a portal upload (`artifacts.created_by` has no user foreign key).
    pub actor: Uuid,
    pub session: Session,
    pub ip: &'a str,
}

pub(crate) struct Ingested {
    pub artifact: ArtifactOut,
    pub duplicate: bool,
}

/// The one ingestion pipeline for every upload: static scan, antivirus (fail closed), content-addressed storage,
/// capture provenance and audit. `portal_upload` links a supplier portal upload row, updated in the same transaction.
pub(crate) async fn ingest(
    st: &AppState,
    up: &Uploader<'_>,
    data: Vec<u8>,
    name: &str,
    declared: Option<String>,
    kind: &str,
    portal_upload: Option<Uuid>,
) -> ApiResult<Ingested> {
    let (origin, actor_kind, user) = match up.session {
        Session::User(_) => ("internal", "user", Some(up.actor)),
        Session::Supplier(_) => ("supplier_portal", "supplier_user", None),
    };
    let mut report = scan::scan(&data, &Limits::default());
    let sha = crypto::sha256(&data);
    // Antivirus after the static scan (ADR 0017): any non-clean engine outcome quarantines (fail closed).
    let mut scanner_down_only = false;
    let av = if report.verdict == Verdict::Reject {
        None
    } else {
        let (scanner, bytes) = (Arc::clone(&st.av), data.clone());
        let outcome = tokio::task::spawn_blocking(move || scanner.scan(&bytes))
            .await
            .map_err(ApiError::internal)?;
        if let Some(f) = outcome.finding() {
            // Quarantined only because the scanner is down (chaos C3): an outage, recorded as such below.
            scanner_down_only = report.verdict == Verdict::Clean && matches!(outcome, Outcome::Unavailable(_));
            report.findings.push(f);
            report.verdict = Verdict::Quarantine;
        }
        Some(outcome.to_json(st.av.engine()))
    };
    if report.verdict == Verdict::Reject {
        sec::record(
            &st.pool,
            Some(up.tenant),
            Kind::UploadRejected,
            user,
            Some(up.ip),
            json!({ "sha256": hex::encode(sha), "size": data.len(), "findings": report.findings, "origin": origin,
                    "actor": up.actor }),
        )
        .await;
        if let Some(pu) = portal_upload {
            let mut tx = db::tenant_tx(&st.pool, up.tenant).await?;
            set_portal_status(&mut tx, pu, "not_accepted", Some("file_type_not_accepted"), None).await?;
            tx.commit().await?;
        }
        return Err(ApiError::UnsupportedMedia);
    }
    let media = report.file_type.unwrap_or("application/octet-stream");
    let status = if report.verdict == Verdict::Clean {
        "accepted"
    } else {
        "quarantined"
    };
    let mut scan_json = serde_json::to_value(&report).map_err(ApiError::internal)?;
    if let (Some(av), Some(o)) = (av, scan_json.as_object_mut()) {
        o.insert("antivirus".into(), av);
    }

    let mut tx = db::tenant_tx(&st.pool, up.tenant).await?;
    // The body may have streamed for a long time: a session revoked meanwhile must not finish the upload.
    // FOR SHARE also holds off a concurrent revocation until this transaction ends.
    let live: Option<i32> = match up.session {
        Session::User(sid) => {
            sqlx::query_scalar(
                "SELECT 1 FROM sessions WHERE id = $1 AND revoked_at IS NULL AND expires_at > $2 FOR SHARE",
            )
            .bind(sid)
            .bind(st.clock.now())
            .fetch_optional(&mut *tx)
            .await?
        }
        Session::Supplier(sid) => {
            sqlx::query_scalar(
                "SELECT 1 FROM supplier_sessions s JOIN supplier_users u ON u.tenant_id = s.tenant_id AND u.id = s.supplier_user_id \
                 WHERE s.id = $1 AND s.revoked_at IS NULL AND s.expires_at > $2 AND u.status = 'active' FOR SHARE OF s, u",
            )
            .bind(sid)
            .bind(st.clock.now())
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    if live.is_none() {
        return Err(ApiError::Unauthorized("invalid_token"));
    }
    // Serialise uploads of identical content in this tenant.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 11))")
        .bind(format!("{}:{}", up.tenant, hex::encode(sha)))
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM artifacts WHERE sha256 = $1")
        .bind(sha.to_vec())
        .fetch_optional(&mut *tx)
        .await?;
    // Always (re)store: content-addressed, so a re-upload of the original repairs a damaged stored copy.
    let key = ObjectKey {
        tenant: up.tenant,
        sha256: sha,
    };
    // Stage now, promote after the commit (orphan objects, PERF_VERIFY_2026-11-05): until then the object is only a
    // staging copy, dropped when this request fails before committing and resolved by the staging sweep if the
    // commit outcome is unknown (connection lost during COMMIT).
    let store = Arc::clone(&st.store);
    let bytes = data.clone();
    let staged = tokio::task::spawn_blocking(move || store.stage(&key, &bytes))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::storage)?;
    let mut staged = StagedGuard(Arc::clone(&st.store), Some(staged));
    let (id, duplicate) = match existing {
        Some(id) => (id, true),
        None => {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, scan_result, \
                 storage_key, display_name, created_by, origin) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            )
            .bind(id)
            .bind(up.tenant)
            .bind(sha.to_vec())
            .bind(data.len() as i64)
            .bind(media)
            .bind(kind)
            .bind(status)
            .bind(&scan_json)
            .bind(key.as_string())
            .bind(name)
            .bind(up.actor)
            .bind(origin)
            .execute(&mut *tx)
            .await?;
            (id, false)
        }
    };
    sqlx::query(
        "INSERT INTO artifact_captures (id, tenant_id, artifact_id, uploaded_by, original_filename, declared_type, client_ip, \
         origin) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::now_v7())
    .bind(up.tenant)
    .bind(id)
    .bind(up.actor)
    .bind(name)
    .bind(declared)
    .bind(up.ip)
    .bind(origin)
    .execute(&mut *tx)
    .await?;
    let artifact = fetch(&mut tx, id).await?;
    if let Some(pu) = portal_upload {
        // The supplier sees the outcome of the scan of the bytes it sent (a duplicate keeps the stored verdict).
        let shown = if artifact.status == "accepted" {
            "received_for_review"
        } else {
            "quarantined"
        };
        set_portal_status(&mut tx, pu, shown, None, Some(id)).await?;
    }
    audit::append(
        &mut tx,
        up.tenant,
        Record {
            actor_user_id: Some(up.actor),
            actor_kind,
            action: if duplicate {
                "artifact.recaptured"
            } else {
                "artifact.uploaded"
            },
            target_type: "artifact",
            target_id: Some(id.to_string()),
            details: json!({ "sha256": hex::encode(sha), "size": data.len(), "verdict": report.verdict, "requested_kind": kind,
                             "origin": origin, "portal_upload": portal_upload }),
        },
    )
    .await?;
    // From here the commit outcome may be unknown: never discard; the sweep promotes or deletes by the row.
    let staged = staged.1.take();
    tx.commit().await?;
    if let Some(staged) = staged {
        let store = Arc::clone(&st.store);
        // Committed: a failed promotion is not the client's failure. `get` serves the staged copy and the staging
        // sweep promotes it.
        match tokio::task::spawn_blocking(move || store.promote(&staged)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(error = %e, "promotion after commit failed; left for the staging sweep"),
            Err(e) => tracing::error!(error = %e, "promotion task failed; left for the staging sweep"),
        }
    }
    if !duplicate && status == "quarantined" {
        sec::record(
            &st.pool,
            Some(up.tenant),
            if scanner_down_only {
                Kind::ScannerUnavailable
            } else {
                Kind::ArtifactQuarantined
            },
            user,
            Some(up.ip),
            json!({ "artifact_id": id, "findings": report.findings, "origin": origin }),
        )
        .await;
    }
    Ok(Ingested { artifact, duplicate })
}

/// Sets a portal upload's supplier-visible status. Later artifact status changes (release, re-quarantine, rejection)
/// follow automatically through the `artifacts_portal_follow` trigger (migration 0027).
pub(crate) async fn set_portal_status(
    tx: &mut Tx,
    id: Uuid,
    status: &str,
    reason: Option<&str>,
    artifact: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE portal_uploads SET status = $2, reason_code = $3, artifact_id = coalesce($4, artifact_id), updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .bind(artifact)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn fetch(tx: &mut Tx, id: Uuid) -> ApiResult<ArtifactOut> {
    sqlx::query_as::<_, ArtifactOut>(&format!("SELECT {ART_COLS} FROM artifacts WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<ArtifactOut>>> {
    p.require(&st, Permission::ArtifactRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, ArtifactOut>(&format!(
        "SELECT {ART_COLS} FROM artifacts ORDER BY created_at DESC LIMIT 500"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct CaptureOut {
    pub uploaded_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub uploaded_at: OffsetDateTime,
    pub original_filename: String,
    pub declared_type: Option<String>,
}

#[derive(Serialize)]
pub struct ArtifactDetail {
    pub artifact: ArtifactOut,
    pub captures: Vec<CaptureOut>,
    pub claim_count: i64,
    pub last_integrity_check: Option<Value>,
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<ArtifactDetail>> {
    p.require(&st, Permission::ArtifactRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let artifact = fetch(&mut tx, id).await?;
    let captures = sqlx::query_as::<_, CaptureOut>(
        "SELECT uploaded_by, uploaded_at, original_filename, declared_type FROM artifact_captures WHERE artifact_id = $1 ORDER BY uploaded_at",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let claim_count: i64 = sqlx::query_scalar("SELECT count(*) FROM claims WHERE artifact_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let last_integrity_check: Option<Value> = sqlx::query_scalar(
        "SELECT json_build_object('ok', ok, 'checked_at', checked_at) FROM integrity_checks WHERE artifact_id = $1 ORDER BY checked_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(ArtifactDetail {
        artifact,
        captures,
        claim_count,
        last_integrity_check,
    }))
}

/// Reads stored bytes and checks them against the recorded digest. Records the check; alerts on mismatch.
pub async fn verify_integrity(st: &AppState, p: &Principal, id: Uuid) -> ApiResult<(bool, Vec<u8>, ArtifactOut)> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let art = fetch(&mut tx, id).await?;
    let sha: [u8; 32] = hex::decode(&art.sha256)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| ApiError::internal("bad sha"))?;
    let key = ObjectKey {
        tenant: p.tenant_id,
        sha256: sha,
    };
    let store = Arc::clone(&st.store);
    let read = tokio::task::spawn_blocking(move || store.get(&key))
        .await
        .map_err(ApiError::internal)?;
    let (ok, observed, detail, bytes) = match read {
        Ok(b) => {
            let h = crypto::sha256(&b);
            (h == sha, Some(h.to_vec()), None, b)
        }
        Err(e) => (false, None, Some(format!("unreadable: {}", e.kind())), Vec::new()),
    };
    sqlx::query("INSERT INTO integrity_checks (id, tenant_id, artifact_id, ok, observed_sha256, detail) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(ok)
        .bind(&observed)
        .bind(&detail)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if !ok {
        sec::record(&st.pool, Some(p.tenant_id), Kind::ArtifactIntegrityFailure, Some(p.user_id), Some(&p.ip),
            json!({ "artifact_id": id, "expected": art.sha256, "observed": observed.map(hex::encode), "detail": detail })).await;
    }
    Ok((ok, bytes, art))
}

pub async fn verify(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ArtifactRead).await?;
    // I4: each verification reads the whole object.
    if !st.limiter.check(&format!("verify:user:{}", p.user_id), 60) {
        return Err(ApiError::TooManyRequests);
    }
    let (ok, _, art) = verify_integrity(&st, &p, id).await?;
    Ok(Json(json!({
        "artifact_id": id,
        "intact": ok,
        "expected_sha256": art.sha256,
        "meaning": "intact = stored bytes are unchanged since capture; says nothing about whether the content is true or authentic",
    })))
}

pub async fn content(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Response> {
    p.require(&st, Permission::ArtifactRead).await?;
    let (ok, bytes, art) = verify_integrity(&st, &p, id).await?;
    if art.status != "accepted" {
        return Err(ApiError::Conflict("artifact_not_released"));
    }
    if !ok {
        return Err(ApiError::Conflict("integrity_check_failed"));
    }
    // Downloads are audited (review finding L7).
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "artifact.downloaded",
            "artifact",
            id,
            json!({ "sha256": art.sha256, "size": bytes.len() }),
        ),
    )
    .await?;
    tx.commit().await?;
    let disp = format!(
        "attachment; filename=\"{}\"",
        art.display_name.replace(|c: char| !c.is_ascii() || c == '\\', "_")
    );
    let mut res = Response::new(Body::from(bytes));
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&art.media_type).map_err(ApiError::internal)?,
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disp).unwrap_or(HeaderValue::from_static("attachment")),
    );
    Ok(res)
}

#[derive(Deserialize)]
pub struct ReleaseReq {
    pub reason: Option<String>,
}

/// Releases a quarantined artifact after human review (privileged, MFA, audited, reason required). The body is
/// parsed only after the permission check so an unauthorised caller always gets 403.
pub async fn release(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> ApiResult<Json<ArtifactOut>> {
    p.require(&st, Permission::IncidentManage).await?;
    let req: ReleaseReq = serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest("invalid_json"))?;
    let reason = req.reason.as_deref().map(str::trim).unwrap_or("").to_owned();
    if reason.is_empty() {
        return Err(ApiError::BadRequest("reason_required"));
    }
    if reason.chars().count() > 2000 {
        return Err(ApiError::BadRequest("reason_too_long"));
    }
    // Four-eyes: nobody releases a document they uploaded (review M5-M7 L8).
    {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let own: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = $1 AND created_by = $2) \
             OR EXISTS (SELECT 1 FROM artifact_captures WHERE artifact_id = $1 AND uploaded_by = $2)",
        )
        .bind(id)
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        p.require_on(
            &st,
            Permission::IncidentManage,
            &policy_engine::Resource {
                tenant_id: p.tenant_id,
                kind: "artifact",
                owner_user_id: own.then_some(p.user_id),
            },
        )
        .await?;
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(String, Value)> =
        sqlx::query_as("SELECT status, scan_result FROM artifacts WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (status, scan) = row.ok_or(ApiError::NotFound)?;
    if status != "quarantined" {
        return Err(ApiError::Conflict("not_quarantined"));
    }
    // Quarantined by an integrity incident: the stored bytes must verify again before release.
    if scan.get("quarantined_by_incident").is_some() {
        tx.commit().await?;
        let (intact, _, _) = verify_integrity(&st, &p, id).await?;
        if !intact {
            return Err(ApiError::Conflict("integrity_check_failed"));
        }
        tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let still: Option<String> = sqlx::query_scalar("SELECT status FROM artifacts WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if still.as_deref() != Some("quarantined") {
            return Err(ApiError::Conflict("not_quarantined"));
        }
    }
    sqlx::query(
        "UPDATE artifacts SET status = 'accepted', scan_result = scan_result || jsonb_build_object('released_by', $2::text, 'released_at', now()) WHERE id = $1",
    )
    .bind(id)
    .bind(p.user_id.to_string())
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "artifact.released",
            "artifact",
            id,
            json!({ "reason": reason }),
        ),
    )
    .await?;
    let out = fetch(&mut tx, id).await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::QuarantineReleased,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "artifact_id": id, "reason": reason }),
    )
    .await;
    Ok(Json(out))
}

/// Discards a staged upload when the request ends before its transaction commits (the rollback is certain then).
struct StagedGuard(Arc<dyn crate::storage::ObjectStore>, Option<crate::storage::Staged>);

impl Drop for StagedGuard {
    fn drop(&mut self) {
        if let Some(s) = self.1.take() {
            self.0.discard(s);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tenant_upload_limit_only_lowers_the_server_ceiling() {
        use super::upload_limit as l;
        const MB: usize = 1024 * 1024;
        assert_eq!(l(25 * MB, None), 25 * MB);
        assert_eq!(l(25 * MB, Some(5)), 5 * MB);
        assert_eq!(l(25 * MB, Some(500)), 25 * MB);
        assert_eq!(l(25 * MB, Some(0)), 25 * MB);
        assert_eq!(l(25 * MB, Some(-3)), 25 * MB);
        assert_eq!(l(25 * MB, Some(i64::MAX)), 25 * MB);
    }

    #[test]
    fn filenames_are_display_only() {
        use super::sanitize_filename as s;
        assert_eq!(s("../../etc/passwd"), "passwd");
        assert_eq!(s("C:\\x\\spec.pdf"), "spec.pdf");
        assert_eq!(s("a\"b\n.pdf"), "ab.pdf");
        assert_eq!(s(".."), "upload");
        assert_eq!(s(""), "upload");
    }
}
