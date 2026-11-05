//! Tenant-scoped label synonyms learnt from accepted reviews (migration 0025, `mfg-map@5`).
//!
//! A worker claim mapped by a fallback guess carries `label` / `mapped_as` (and `subject_label` / `subject_as` when
//! its subject was guessed) in its source location. When a reviewer approves such a claim, the guess becomes the
//! tenant's synonym and the next extraction for that tenant maps the label directly (basis `tenant_synonym`).
//! Rows are RLS-scoped to one tenant, audited, and revoked rather than deleted.

use axum::Json;
use axum::extract::{Path, State};
use domain_manufacturing::LabelSynonyms;
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};

/// The tenant's active synonyms (the transaction is tenant-scoped, so only that tenant's rows are visible).
pub(crate) async fn load(tx: &mut Tx) -> ApiResult<LabelSynonyms> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT label, target FROM label_synonyms WHERE revoked_at IS NULL ORDER BY created_at, id")
            .fetch_all(&mut **tx)
            .await?;
    let mut s = LabelSynonyms::new();
    for (l, t) in rows {
        s.insert(&l, &t);
    }
    Ok(s)
}

/// The (label, target) pairs an accepted worker claim teaches: its own fallback label mapping and its guessed
/// subject. Pairs the mapper would refuse (unknown target, a rule label) are dropped.
pub fn lessons(extraction_method: &str, loc: &Value) -> Vec<(String, String)> {
    if !extraction_method.starts_with("worker:") {
        return vec![]; // only the mapper writes these keys; user-written claims teach nothing
    }
    let s = |k: &str| loc.get(k).and_then(Value::as_str);
    let mut out = vec![];
    if s("label_basis") == Some("fallback")
        && let (Some(l), Some(t)) = (s("label"), s("mapped_as"))
    {
        out.push((l, t));
    }
    if s("subject_basis") == Some("fallback")
        && let (Some(l), Some(t)) = (s("subject_label"), s("subject_as"))
    {
        out.push((l, t));
    }
    out.into_iter()
        .filter(|(l, t)| LabelSynonyms::new().insert(l, t))
        .filter_map(|(l, t)| Some((domain_manufacturing::infer::synonym_key(l)?, t.to_string())))
        .collect()
}

/// Called in the review transaction when a claim becomes `accepted`.
pub(crate) async fn learn(
    tx: &mut Tx,
    p: &Principal,
    claim: Uuid,
    extraction_method: &str,
    loc: &Value,
) -> ApiResult<()> {
    for (label, target) in lessons(extraction_method, loc) {
        let id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO label_synonyms (id, tenant_id, label, target, source_claim, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (tenant_id, label) WHERE revoked_at IS NULL DO NOTHING",
        )
        .bind(id)
        .bind(p.tenant_id)
        .bind(&label)
        .bind(&target)
        .bind(claim)
        .bind(p.user_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if inserted == 1 {
            audit::append(
                tx,
                p.tenant_id,
                Record::user(
                    p.user_id,
                    "label_synonym.learn",
                    "label_synonym",
                    id,
                    json!({ "label": label, "target": target, "source_claim": claim }),
                ),
            )
            .await?;
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SynonymOut {
    pub id: Uuid,
    pub label: String,
    pub target: String,
    pub source_claim: Uuid,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    pub revoked_by: Option<Uuid>,
    pub revoke_reason: Option<String>,
}

const COLS: &str = "id, label, target, source_claim, created_by, created_at, revoked_at, revoked_by, revoke_reason";

/// GET /label-synonyms — the tenant's learnt synonyms, revoked ones included.
pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<SynonymOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = sqlx::query_as(&format!(
        "SELECT {COLS} FROM label_synonyms ORDER BY created_at DESC, id LIMIT 500"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct RevokeReq {
    pub reason: Option<String>,
}

/// POST /label-synonyms/{id}/revoke — stops using a synonym (reversal of a learnt mapping). Claims already
/// extracted keep their provenance; the next extraction no longer uses the synonym.
pub async fn revoke(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<RevokeReq>,
) -> ApiResult<Json<SynonymOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let reason = req.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if reason.is_some_and(|r| r.chars().count() > 500) {
        return Err(ApiError::BadRequest("reason_too_long"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let revoked: Option<Option<OffsetDateTime>> =
        sqlx::query_scalar("SELECT revoked_at FROM label_synonyms WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match revoked {
        None => return Err(ApiError::NotFound),
        Some(Some(_)) => return Err(ApiError::Conflict("synonym_already_revoked")),
        Some(None) => {}
    }
    sqlx::query("UPDATE label_synonyms SET revoked_at = now(), revoked_by = $2, revoke_reason = $3 WHERE id = $1")
        .bind(id)
        .bind(p.user_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "label_synonym.revoke",
            "label_synonym",
            id,
            json!({ "reason": reason }),
        ),
    )
    .await?;
    let out = sqlx::query_as(&format!("SELECT {COLS} FROM label_synonyms WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_worker_fallback_claims_teach_valid_pairs() {
        let loc = json!({"label_basis": "fallback", "label": "製品名", "mapped_as": "part name",
                         "subject_basis": "fallback", "subject_label": "製品番号", "subject_as": "part number"});
        assert_eq!(
            lessons("worker:pdf-text@2+mfg-map@5", &loc),
            vec![
                ("製品名".to_string(), "part name".to_string()),
                ("製品番号".to_string(), "part number".to_string())
            ]
        );
        assert!(
            lessons("user", &loc).is_empty(),
            "a hand-written claim cannot plant a synonym"
        );
        assert!(lessons("ai:model", &loc).is_empty());
        let rule = json!({"label_basis": "rule"});
        assert!(lessons("worker:x", &rule).is_empty());
        let bad = json!({"label_basis": "fallback", "label": "品番", "mapped_as": "material"});
        assert!(lessons("worker:x", &bad).is_empty(), "rule labels are never remapped");
        let bad = json!({"label_basis": "fallback", "label": "備考", "mapped_as": "approved by; drop"});
        assert!(lessons("worker:x", &bad).is_empty(), "targets come from the fixed list");
    }
}
