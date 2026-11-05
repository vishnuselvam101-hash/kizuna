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
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

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
}

pub const ART_COLS: &str = "id, encode(sha256, 'hex') AS sha256, size_bytes, media_type, kind, status, scan_result, \
                            display_name, created_by, created_at";

#[derive(Serialize)]
pub struct UploadResp {
    pub artifact: ArtifactOut,
    pub duplicate: bool,
}

async fn read_upload(mut mp: Multipart, max: usize) -> ApiResult<(Vec<u8>, String, Option<String>, String)> {
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
    let (data, name, declared, kind) = read_upload(mp, st.cfg.max_upload_bytes).await?;
    let report = scan::scan(&data, &Limits::default());
    let sha = crypto::sha256(&data);
    if report.verdict == Verdict::Reject {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::UploadRejected,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "sha256": hex::encode(sha), "size": data.len(), "findings": report.findings }),
        )
        .await;
        return Err(ApiError::UnsupportedMedia);
    }
    let media = report.file_type.unwrap_or("application/octet-stream");
    let status = if report.verdict == Verdict::Clean {
        "accepted"
    } else {
        "quarantined"
    };
    let scan_json = serde_json::to_value(&report).map_err(ApiError::internal)?;

    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    // Serialise uploads of identical content in this tenant.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 11))")
        .bind(format!("{}:{}", p.tenant_id, hex::encode(sha)))
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM artifacts WHERE sha256 = $1")
        .bind(sha.to_vec())
        .fetch_optional(&mut *tx)
        .await?;
    let (id, duplicate) = match existing {
        Some(id) => (id, true),
        None => {
            let key = ObjectKey {
                tenant: p.tenant_id,
                sha256: sha,
            };
            let store = Arc::clone(&st.store);
            let bytes = data.clone();
            tokio::task::spawn_blocking(move || store.put(&key, &bytes))
                .await
                .map_err(ApiError::internal)?
                .map_err(ApiError::internal)?;
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, scan_result, \
                 storage_key, display_name, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(id)
            .bind(p.tenant_id)
            .bind(sha.to_vec())
            .bind(data.len() as i64)
            .bind(media)
            .bind(&kind)
            .bind(status)
            .bind(&scan_json)
            .bind(key.as_string())
            .bind(&name)
            .bind(p.user_id)
            .execute(&mut *tx)
            .await?;
            (id, false)
        }
    };
    sqlx::query(
        "INSERT INTO artifact_captures (id, tenant_id, artifact_id, uploaded_by, original_filename, declared_type, client_ip) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(Uuid::now_v7())
    .bind(p.tenant_id)
    .bind(id)
    .bind(p.user_id)
    .bind(&name)
    .bind(declared)
    .bind(&p.ip)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            if duplicate {
                "artifact.recaptured"
            } else {
                "artifact.uploaded"
            },
            "artifact",
            id,
            json!({ "sha256": hex::encode(sha), "size": data.len(), "verdict": report.verdict, "kind": kind }),
        ),
    )
    .await?;
    let artifact = fetch(&mut tx, id).await?;
    tx.commit().await?;
    if !duplicate && status == "quarantined" {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::ArtifactQuarantined,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "artifact_id": id, "findings": report.findings }),
        )
        .await;
    }
    Ok(Json(UploadResp { artifact, duplicate }))
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

/// Releases a quarantined artifact after human review (privileged, MFA, audited).
pub async fn release(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<ArtifactOut>> {
    p.require(&st, Permission::IncidentManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM artifacts WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if status.ok_or(ApiError::NotFound)? != "quarantined" {
        return Err(ApiError::Conflict("not_quarantined"));
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
        Record::user(p.user_id, "artifact.released", "artifact", id, json!({})),
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
        json!({ "artifact_id": id }),
    )
    .await;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
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
