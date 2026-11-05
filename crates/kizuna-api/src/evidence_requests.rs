//! Minimal supplier re-requests. When evidence changes flag decisions `potentially_affected`, compute per supplier the
//! fewest supplier documents that must be resent: only documents carrying claims that (a) the flagged decisions
//! depend on and (b) the material change makes stale (`mfg-resupply@1`). The rest of the supplier's documents stay
//! valid and are not requested. Drafts are bilingual text for a person to approve and send; KIZUNA never sends.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use dependency_engine::cover::minimal_cover;
use domain_manufacturing::resupply::{self, Draft, Lang, RESUPPLY_VERSION, Reason, RequestDoc, RequestItem};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};
use crate::simulations::Overlay;

#[derive(Debug, Clone, Serialize)]
pub struct SupplierPlan {
    pub supplier: String,
    pub documents: Vec<RequestDoc>,
    /// Supplier documents the flagged decisions rely on (the "resend everything" baseline).
    pub baseline_count: usize,
    pub exact: bool,
    pub uncovered_items: Vec<String>,
    pub decision_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub engine: &'static str,
    pub decisions_considered: usize,
    pub suppliers: Vec<SupplierPlan>,
    /// Flagged decisions whose material changes need no supplier evidence (internal changes such as approvals).
    pub no_supplier_action: Vec<Value>,
}

type DepRow = (
    String,
    String,
    Option<String>,
    Value,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A [`DepRow`] with the decision it belongs to.
type DepRowOf = (
    Uuid,
    String,
    String,
    Option<String>,
    Value,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
);

struct Dep {
    part: String,
    predicate: String,
    text: Option<String>,
    scope: BTreeMap<String, String>,
    doc: Option<Uuid>,
    kind: Option<String>,
    name: Option<String>,
    sha: Option<String>,
}

fn str_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn item_id(i: &RequestItem) -> String {
    serde_json::to_string(i).unwrap_or_default()
}

/// Computes the plan for the given flagged decisions (all `potentially_affected` decisions when `only` is empty).
pub async fn compute(tx: &mut Tx, only: &[Uuid]) -> ApiResult<Plan> {
    let flagged: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM decisions WHERE status = 'potentially_affected' AND (cardinality($1::uuid[]) = 0 OR id = ANY($1)) \
         ORDER BY id LIMIT 500",
    )
    .bind(only)
    .fetch_all(&mut **tx)
    .await?;
    compute_with(tx, &flagged, &BTreeMap::new(), &Overlay::default()).await
}

/// Most flagged decisions one plan considers.
pub const PLAN_DECISIONS: usize = 500;

/// The plan for `flagged` (sorted ids, at most [`PLAN_DECISIONS`]) with simulated material changes appended after
/// the recorded ones (`extra`: decision → (cause kind, semantic change)) and a simulated evidence change `ov`.
pub(crate) async fn compute_with(
    tx: &mut Tx,
    flagged: &[Uuid],
    extra: &BTreeMap<Uuid, Vec<(String, Value)>>,
    ov: &Overlay,
) -> ApiResult<Plan> {
    // Per supplier: item → candidate docs; item → reasons; docs → metadata; baseline docs; decisions.
    #[derive(Default)]
    struct Acc {
        needs: BTreeMap<String, BTreeSet<String>>,
        items: BTreeMap<String, RequestItem>,
        reasons: BTreeMap<String, BTreeSet<Reason>>,
        baseline: BTreeSet<String>,
        decisions: BTreeSet<Uuid>,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut no_action = Vec::new();
    // Recorded material changes and evidence rows of all flagged decisions, two queries in total.
    let mut recorded: BTreeMap<Uuid, Vec<(String, Value)>> = BTreeMap::new();
    let ev: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT decision_id, cause_kind, semantic_change FROM invalidation_events WHERE decision_id = ANY($1) AND material \
         ORDER BY decision_id, recorded_at",
    )
    .bind(flagged)
    .fetch_all(&mut **tx)
    .await?;
    for (d, k, v) in ev {
        recorded.entry(d).or_default().push((k, v));
    }
    let mut dep_rows: BTreeMap<Uuid, Vec<DepRow>> = BTreeMap::new();
    let all: Vec<DepRowOf> = sqlx::query_as(
            "SELECT dd.decision_id, c.subject_key, c.predicate, c.value_text, c.scope, c.artifact_id, a.kind, a.display_name, \
                    encode(a.sha256, 'hex') \
             FROM decision_dependencies dd JOIN claims c ON c.id = dd.depends_on_claim LEFT JOIN artifacts a ON a.id = c.artifact_id \
             WHERE dd.decision_id = ANY($1) ORDER BY dd.decision_id, c.id",
        )
        .bind(flagged)
        .fetch_all(&mut **tx)
        .await?;
    for r in all {
        dep_rows
            .entry(r.0)
            .or_default()
            .push((r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8));
    }
    for d in flagged {
        let mut changes = recorded.remove(d).unwrap_or_default();
        changes.extend(extra.get(d).into_iter().flatten().cloned());
        let rows = dep_rows.remove(d).unwrap_or_default();
        let deps: Vec<Dep> = rows
            .into_iter()
            .map(|r| Dep {
                part: r.0,
                predicate: r.1,
                text: r.2,
                scope: str_map(&r.3),
                doc: r.4,
                kind: r.5,
                name: r.6,
                sha: r.7,
            })
            .collect();
        // Supplier of the decision's PO line, for certificates that do not name their supplier.
        let po_supplier = deps
            .iter()
            .find(|x| x.predicate == "po.supplier")
            .and_then(|x| x.text.clone())
            .or_else(|| deps.iter().find_map(|x| x.scope.get("supplier").cloned()));
        let supplier_of = |x: &Dep| {
            x.scope
                .get("supplier")
                .cloned()
                .or(po_supplier.clone())
                .unwrap_or_else(|| "UNKNOWN SUPPLIER".into())
        };
        let mut any = false;
        for (kind, sem) in &changes {
            let Some(pred) = sem.get("predicate").and_then(Value::as_str) else {
                continue;
            };
            let reason = Reason {
                predicate: pred.to_string(),
                from: sem.get("from").and_then(Value::as_str).map(str::to_string),
                to: sem.get("to").and_then(Value::as_str).map(str::to_string),
                cause: kind.clone(),
            };
            for x in deps
                .iter()
                .filter(|x| x.kind.as_deref().is_some_and(resupply::supplier_side))
            {
                if !resupply::needs_resupply(pred, &x.predicate) {
                    continue;
                }
                let Some(doc) = x.doc else { continue };
                any = true;
                let item = RequestItem {
                    part: x.part.clone(),
                    predicate: x.predicate.clone(),
                    purchase_order: x.scope.get("purchase_order").cloned(),
                    po_line: x.scope.get("po_line").cloned(),
                    lot: x.scope.get("lot").cloned(),
                    heat: x.scope.get("heat").cloned(),
                };
                let a = acc.entry(supplier_of(x)).or_default();
                let id = item_id(&item);
                a.needs.entry(id.clone()).or_default().insert(doc.to_string());
                a.items.insert(id.clone(), item);
                a.reasons.entry(id).or_default().insert(reason.clone());
                a.decisions.insert(*d);
            }
        }
        // Baseline: every supplier document this decision relies on.
        for x in deps
            .iter()
            .filter(|x| x.kind.as_deref().is_some_and(resupply::supplier_side))
        {
            if let Some(doc) = x.doc {
                meta.insert(
                    doc.to_string(),
                    (x.name.clone().unwrap_or_default(), x.sha.clone().unwrap_or_default()),
                );
                if any {
                    acc.entry(supplier_of(x)).or_default().baseline.insert(doc.to_string());
                }
            }
        }
        if !any {
            let preds: BTreeSet<&str> = changes
                .iter()
                .filter_map(|c| c.1.get("predicate").and_then(Value::as_str))
                .collect();
            no_action.push(json!({ "decision_id": d, "changed_predicates": preds,
                "why": "the material changes do not concern any supplier statement this decision relies on" }));
        }
    }
    // Other current supplier documents carrying the same item can also answer it: add them as candidates.
    for a in acc.values_mut() {
        for (id, item) in &a.items {
            let mut alt: Vec<(Uuid, String, String)> = sqlx::query_as(
                "SELECT DISTINCT c.artifact_id, a.display_name, encode(a.sha256, 'hex') FROM claims c JOIN artifacts a ON a.id = c.artifact_id \
                 WHERE c.subject_type = 'part' AND c.subject_key = $1 AND c.predicate = $2 AND c.superseded_at IS NULL \
                 AND c.review_status <> 'rejected' AND a.kind = 'inspection_report' \
                 AND coalesce(c.scope->>'lot', '') = coalesce($3, '') AND coalesce(c.scope->>'heat', '') = coalesce($4, '') \
                 AND coalesce(c.scope->>'purchase_order', '') = coalesce($5, '') AND NOT (c.id = ANY($6)) LIMIT 20",
            )
            .bind(&item.part)
            .bind(&item.predicate)
            .bind(&item.lot)
            .bind(&item.heat)
            .bind(&item.purchase_order)
            .bind(ov.removed.iter().copied().collect::<Vec<Uuid>>())
            .fetch_all(&mut **tx)
            .await?;
            if let Some(s) = &ov.added
                && let Some(art) = &s.artifact
                && art.kind == "inspection_report"
                && s.claim.subject_type == "part"
                && s.claim.subject_key == item.part
                && s.claim.predicate == item.predicate
                && [
                    ("lot", &item.lot),
                    ("heat", &item.heat),
                    ("purchase_order", &item.purchase_order),
                ]
                .iter()
                .all(|(k, v)| s.claim.scope.get(*k).and_then(Value::as_str).unwrap_or("") == v.as_deref().unwrap_or(""))
                && alt.len() < 20
                && !alt.iter().any(|x| x.0 == art.id)
            {
                alt.push((art.id, art.display_name.clone(), art.sha256.clone()));
            }
            for (doc, name, sha) in alt {
                meta.entry(doc.to_string()).or_insert((name, sha));
                if let Some(c) = a.needs.get_mut(id) {
                    c.insert(doc.to_string());
                }
            }
        }
    }
    let mut suppliers = Vec::new();
    for (supplier, a) in acc {
        let cover = minimal_cover(&a.needs);
        let mut assigned: BTreeSet<&String> = BTreeSet::new();
        let mut documents = Vec::new();
        for doc in &cover.docs {
            let mut items = Vec::new();
            let mut reasons: BTreeSet<Reason> = BTreeSet::new();
            for (id, cands) in &a.needs {
                if cands.contains(doc) && assigned.insert(id) {
                    items.push(a.items[id].clone());
                    reasons.extend(a.reasons.get(id).into_iter().flatten().cloned());
                }
            }
            let (name, sha) = meta.get(doc).cloned().unwrap_or_default();
            documents.push(RequestDoc {
                doc: doc.clone(),
                name,
                sha256: sha,
                items,
                reasons: reasons.into_iter().collect(),
            });
        }
        let baseline_count = a.baseline.len().max(documents.len());
        suppliers.push(SupplierPlan {
            supplier,
            documents,
            baseline_count,
            exact: cover.exact,
            uncovered_items: cover.uncovered,
            decision_ids: a.decisions.into_iter().collect(),
        });
    }
    Ok(Plan {
        engine: RESUPPLY_VERSION,
        decisions_considered: flagged.len(),
        suppliers,
        no_supplier_action: no_action,
    })
}

#[derive(Deserialize, Default)]
pub struct PlanReq {
    #[serde(default)]
    pub decision_ids: Vec<Uuid>,
    pub supplier: Option<String>,
}

#[derive(Deserialize)]
pub struct PreviewQuery {
    pub decision_id: Option<Uuid>,
}

/// GET /evidence-requests/preview — the plan without recording anything.
pub async fn preview(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<PreviewQuery>,
) -> ApiResult<Json<Plan>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let only: Vec<Uuid> = q.decision_id.into_iter().collect();
    let plan = compute(&mut tx, &only).await?;
    tx.commit().await?;
    Ok(Json(plan))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct RequestOut {
    pub id: Uuid,
    pub reference: String,
    pub supplier: String,
    pub engine: String,
    pub decision_ids: Vec<Uuid>,
    pub documents: Value,
    pub requested_count: i32,
    pub baseline_count: i32,
    pub exact: bool,
    pub body_ja: String,
    pub body_en: String,
    pub content_sha256: String,
    pub status: String,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub reviewed_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<OffsetDateTime>,
    pub review_comment: Option<String>,
    /// The supplier record this request is addressed to (supplier portal); set only by an explicit admin attach.
    pub supplier_id: Option<Uuid>,
    /// Answer-by date, fixed at approval; shown to the supplier in the portal.
    #[serde(with = "iso_date::option")]
    pub due_on: Option<time::Date>,
}

time::serde::format_description!(pub iso_date, Date, "[year]-[month]-[day]");

/// Default answer period for an approved request without an explicit due date.
pub const DEFAULT_DUE_DAYS: i64 = 14;

const REQ_COLS: &str = "id, reference, supplier, engine, decision_ids, documents, requested_count, baseline_count, exact, body_ja, \
     body_en, encode(content_sha256, 'hex') AS content_sha256, status, created_by, created_at, reviewed_by, reviewed_at, review_comment, supplier_id, due_on";

async fn fetch(tx: &mut Tx, id: Uuid) -> ApiResult<RequestOut> {
    sqlx::query_as::<_, RequestOut>(&format!("SELECT {REQ_COLS} FROM evidence_requests WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

/// POST /evidence-requests — records one draft per supplier that has documents to resend.
pub async fn create(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PlanReq>,
) -> ApiResult<Json<Vec<RequestOut>>> {
    p.require(&st, Permission::DecisionWrite).await?;
    if req.decision_ids.len() > 500 {
        return Err(ApiError::BadRequest("too_many_decisions"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let plan = compute(&mut tx, &req.decision_ids).await?;
    let date = db::tenant_today(&mut tx, p.tenant_id, st.clock.now())
        .await?
        .to_string();
    let want = req.supplier.as_deref().map(entity_resolution::safe_normalize);
    let mut out = Vec::new();
    for s in plan.suppliers.iter().filter(|s| !s.documents.is_empty()) {
        if want
            .as_ref()
            .is_some_and(|w| entity_resolution::safe_normalize(&s.supplier) != *w)
        {
            continue;
        }
        let id = Uuid::now_v7();
        let reference = format!("RR-{}", &id.simple().to_string()[..12].to_uppercase());
        let draft = Draft {
            reference: &reference,
            supplier: &s.supplier,
            date: &date,
            documents: &s.documents,
            not_requested: s.baseline_count - s.documents.len(),
        };
        let (ja, en) = (resupply::render(&draft, Lang::Ja), resupply::render(&draft, Lang::En));
        let docs = serde_json::to_value(&s.documents).map_err(ApiError::internal)?;
        let content = json!({ "reference": reference, "supplier": s.supplier, "documents": docs, "body_ja": ja, "body_en": en,
                               "decision_ids": s.decision_ids, "engine": RESUPPLY_VERSION });
        let sha = crate::crypto::sha256(
            audit_engine::canonical_json(&content)
                .map_err(ApiError::internal)?
                .as_bytes(),
        );
        sqlx::query(
            "INSERT INTO evidence_requests (id, tenant_id, reference, supplier, engine, decision_ids, documents, requested_count, \
             baseline_count, exact, body_ja, body_en, content_sha256, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(id)
        .bind(p.tenant_id)
        .bind(&reference)
        .bind(s.supplier.chars().take(200).collect::<String>())
        .bind(RESUPPLY_VERSION)
        .bind(&s.decision_ids)
        .bind(&docs)
        .bind(i32::try_from(s.documents.len()).unwrap_or(i32::MAX))
        .bind(i32::try_from(s.baseline_count).unwrap_or(i32::MAX))
        .bind(s.exact)
        .bind(&ja)
        .bind(&en)
        .bind(sha.to_vec())
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
        audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "evidence_request.drafted", "evidence_request", id,
            json!({ "reference": reference, "supplier": s.supplier, "requested": s.documents.len(), "baseline": s.baseline_count,
                    "exact": s.exact, "decisions": s.decision_ids, "content_sha256": hex::encode(sha) }))).await?;
        out.push(fetch(&mut tx, id).await?);
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<RequestOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, RequestOut>(&format!(
        "SELECT {REQ_COLS} FROM evidence_requests WHERE ($1::text IS NULL OR status = $1) ORDER BY created_at DESC LIMIT 200"
    ))
    .bind(q.status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<RequestOut>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let r = fetch(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub struct ReviewReq {
    pub comment: Option<String>,
    /// Approval only: the date the supplier is asked to answer by (default: today + 14 days in the tenant zone).
    #[serde(default, with = "iso_date::option")]
    pub due_on: Option<time::Date>,
}

async fn review(
    st: AppState,
    p: Principal,
    id: Uuid,
    comment: Option<String>,
    due_on: Option<time::Date>,
    to: &'static str,
) -> ApiResult<Json<RequestOut>> {
    // Approving or cancelling what goes to a supplier is a review decision (privileged: MFA-verified session).
    p.require(&st, Permission::ReviewDecide).await?;
    if comment.as_deref().is_some_and(|c| c.chars().count() > 2000) {
        return Err(ApiError::BadRequest("comment_too_long"));
    }
    if to == "cancelled" && comment.as_deref().is_none_or(|c| c.trim().is_empty()) {
        return Err(ApiError::BadRequest("comment_required"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (status, creator): (String, Uuid) =
        sqlx::query_as("SELECT status, created_by FROM evidence_requests WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if status != "draft" {
        return Err(ApiError::Conflict("request_not_draft"));
    }
    // Four-eyes: the person who drafted a supplier request does not approve it.
    if to == "approved" && creator == p.user_id {
        return Err(ApiError::Forbidden("own_request"));
    }
    let due = if to == "approved" {
        let today = db::tenant_today(&mut tx, p.tenant_id, st.clock.now()).await?;
        let due = due_on.unwrap_or(today + time::Duration::days(DEFAULT_DUE_DAYS));
        if due < today || due > today + time::Duration::days(366) {
            return Err(ApiError::BadRequest("invalid_due_date"));
        }
        Some(due)
    } else {
        None
    };
    sqlx::query(
        "UPDATE evidence_requests SET status = $2, reviewed_by = $3, reviewed_at = now(), review_comment = $4, due_on = $5 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(to)
    .bind(p.user_id)
    .bind(comment.as_deref().map(str::trim))
    .bind(due)
    .execute(&mut *tx)
    .await?;
    let action = if to == "approved" {
        "evidence_request.approved"
    } else {
        "evidence_request.cancelled"
    };
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            action,
            "evidence_request",
            id,
            json!({ "comment": comment, "due_on": due }),
        ),
    )
    .await?;
    let r = fetch(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(r))
}

pub async fn approve(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<ReviewReq>,
) -> ApiResult<Json<RequestOut>> {
    review(st, p, id, r.comment, r.due_on, "approved").await
}

pub async fn cancel(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<ReviewReq>,
) -> ApiResult<Json<RequestOut>> {
    review(st, p, id, r.comment, None, "cancelled").await
}

#[derive(Deserialize)]
pub struct DocQuery {
    pub lang: Option<String>,
}

/// GET /evidence-requests/{id}/document?lang=ja|en — the letter as UTF-8 text (download). Drafts are marked as such.
pub async fn document(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiQuery(q): ApiQuery<DocQuery>,
) -> ApiResult<Response> {
    p.require(&st, Permission::DecisionRead).await?;
    let lang = match q.lang.as_deref().unwrap_or("ja") {
        "ja" => Lang::Ja,
        "en" => Lang::En,
        _ => return Err(ApiError::BadRequest("invalid_lang")),
    };
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let r = fetch(&mut tx, id).await?;
    audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "evidence_request.exported", "evidence_request", id,
        json!({ "lang": q.lang.as_deref().unwrap_or("ja"), "status": r.status, "content_sha256": r.content_sha256 }))).await?;
    tx.commit().await?;
    let mut body = match lang {
        Lang::Ja => r.body_ja,
        Lang::En => r.body_en,
    };
    if r.status != "approved" {
        let mark = match lang {
            Lang::Ja => "【下書き・未承認／送付不可】\n\n",
            Lang::En => "[DRAFT - NOT APPROVED - DO NOT SEND]\n\n",
        };
        body.insert_str(0, mark);
    }
    let name = format!(
        "attachment; filename=\"{}-{}.txt\"",
        r.reference,
        if lang == Lang::Ja { "ja" } else { "en" }
    );
    let mut resp = body.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&name).map_err(ApiError::internal)?,
    );
    Ok(resp)
}
