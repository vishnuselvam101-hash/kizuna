//! Tenant-curated grade equivalences and grade limits (`mfg-grade@1`, migration 0033).
//!
//! KIZUNA ships no standards content. A tenant proposes entries (typed in or imported from its own CSV), a second
//! person approves them (four-eyes, also enforced by the database), and only approved versions reach the three-way
//! match. Every entry is labelled "unverified — check against the standard" unless the proposer recorded that it
//! was checked against the standard document (named in `reference`) and the approver confirmed that statement.
//! Approving an edit or a retirement of an equivalence supersedes the previous version and flags every decision
//! that rested on it (selective invalidation).

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use domain_manufacturing::grades::{
    self, ApprovedSubstitution, Equivalence, EquivalenceKind, GradeCtx, GradeLimit, GradeSide, Proposal,
};
use policy_engine::Permission;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};

type EqRow = (
    Uuid,
    Uuid,
    i32,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    bool,
);

type LimitRow = (
    Uuid,
    Uuid,
    i32,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
);

/// Stored content of an equivalence row (for a retirement).
type EqContent = (
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);
/// Stored content of a limit row (for a retirement).
type LimitContent = (
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Everything the material check needs from this tenant: approved equivalences, approved limits, and valid
/// substitution decisions (kind `material_substitution`, outcome approved or conditional).
pub(crate) async fn load_ctx(tx: &mut Tx) -> ApiResult<GradeCtx> {
    let eqs: Vec<EqRow> = sqlx::query_as(
        "SELECT id, entry_id, version, a_standard, a_grade, b_standard, b_grade, equivalence, conditions, source_checked \
         FROM grade_equivalences WHERE status = 'approved' ORDER BY id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let equivalences = eqs
        .into_iter()
        .filter_map(
            |(id, entry, version, a_std, a_grade, b_std, b_grade, kind, conditions, checked)| {
                Some(Equivalence {
                    id: id.to_string(),
                    entry: entry.to_string(),
                    version,
                    a: GradeSide {
                        standard: a_std,
                        grade: a_grade,
                    },
                    b: GradeSide {
                        standard: b_std,
                        grade: b_grade,
                    },
                    kind: EquivalenceKind::parse(&kind)?,
                    conditions,
                    source_checked: checked,
                })
            },
        )
        .collect();
    let lims: Vec<LimitRow> = sqlx::query_as(
        "SELECT id, entry_id, version, standard, grade, property, min_value::text, max_value::text, unit, source_checked \
         FROM grade_limits WHERE status = 'approved' ORDER BY id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let limits = lims
        .into_iter()
        .map(
            |(id, entry, version, standard, grade, property, min, max, unit, checked)| GradeLimit {
                id: id.to_string(),
                entry: entry.to_string(),
                version,
                grade: GradeSide { standard, grade },
                property,
                min: min.and_then(|v| v.parse::<Decimal>().ok()),
                max: max.and_then(|v| v.parse::<Decimal>().ok()),
                unit,
                source_checked: checked,
            },
        )
        .collect();
    // A substitution decision depends on the certificate claim (basis), the reference claim (condition) and one
    // approved equivalence version.
    let subs: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT d.id, c.depends_on_claim, r.depends_on_claim, e.depends_on_equivalence FROM decisions d \
         JOIN decision_dependencies c ON c.decision_id = d.id AND c.role = 'basis' AND c.depends_on_claim IS NOT NULL \
         JOIN decision_dependencies r ON r.decision_id = d.id AND r.role = 'condition' AND r.depends_on_claim IS NOT NULL \
         JOIN decision_dependencies e ON e.decision_id = d.id AND e.depends_on_equivalence IS NOT NULL \
         WHERE d.kind = 'material_substitution' AND d.status = 'valid' AND d.outcome IN ('approved', 'conditional') \
         ORDER BY d.id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let substitutions = subs
        .into_iter()
        .map(|(d, c, r, e)| ApprovedSubstitution {
            decision: d.to_string(),
            certificate_claim: c.to_string(),
            reference_claim: r.to_string(),
            equivalence: e.to_string(),
        })
        .collect();
    Ok(GradeCtx {
        equivalences,
        substitutions,
        limits,
    })
}

#[derive(Clone, Copy)]
enum Table {
    Equivalences,
    Limits,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Equivalences => "grade_equivalences",
            Table::Limits => "grade_limits",
        }
    }
    fn target(self) -> &'static str {
        match self {
            Table::Equivalences => "grade_equivalence",
            Table::Limits => "grade_limit",
        }
    }
}

const EQ_JSON: &str = "json_build_object('id', id, 'entry_id', entry_id, 'version', version, 'change', change, \
     'a_standard', a_standard, 'a_grade', a_grade, 'b_standard', b_standard, 'b_grade', b_grade, \
     'equivalence', equivalence, 'conditions', conditions, 'reference', reference, 'source_checked', source_checked, \
     'status', status, 'proposed_by', proposed_by, 'proposed_at', proposed_at, 'proposal_note', proposal_note, \
     'import_batch', import_batch, 'supersedes', supersedes, 'decided_by', decided_by, 'decided_at', decided_at, \
     'decision_comment', decision_comment)";

const LIMIT_JSON: &str = "json_build_object('id', id, 'entry_id', entry_id, 'version', version, 'change', change, \
     'standard', standard, 'grade', grade, 'property', property, 'min', min_value::text, 'max', max_value::text, \
     'unit', unit, 'reference', reference, 'source_checked', source_checked, 'status', status, \
     'proposed_by', proposed_by, 'proposed_at', proposed_at, 'proposal_note', proposal_note, 'supersedes', supersedes, \
     'decided_by', decided_by, 'decided_at', decided_at, 'decision_comment', decision_comment)";

/// Adds the verification label and the not-authoritative note to a row.
fn labelled(mut v: Value) -> Value {
    let checked = v["source_checked"].as_bool().unwrap_or(false);
    v["verification"] = json!(grades::verification(checked));
    v["note"] = json!(if checked {
        grades::CHECKED_NOTE
    } else {
        grades::UNVERIFIED_NOTE
    });
    v
}

async fn rows(tx: &mut Tx, t: Table, id: Option<Uuid>) -> ApiResult<Vec<Value>> {
    let cols = match t {
        Table::Equivalences => EQ_JSON,
        Table::Limits => LIMIT_JSON,
    };
    let out: Vec<Value> = sqlx::query_scalar(&format!(
        "SELECT {cols} FROM {} WHERE $1::uuid IS NULL OR id = $1 ORDER BY entry_id, version DESC LIMIT 5000",
        t.name()
    ))
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(out.into_iter().map(labelled).collect())
}

async fn list(st: &AppState, p: &Principal, t: Table) -> ApiResult<Json<Value>> {
    p.require(st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = rows(&mut tx, t, None).await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "rule_version": grades::GRADE_RULE_VERSION, "note": grades::UNVERIFIED_NOTE, "entries": out }),
    ))
}

/// GET /grades/equivalences — every version of every entry, each with its verification label.
pub async fn list_equivalences(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    list(&st, &p, Table::Equivalences).await
}

/// GET /grades/limits — every version of every grade limit, each with its verification label.
pub async fn list_limits(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    list(&st, &p, Table::Limits).await
}

fn opt(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}

fn note_of(n: &Option<String>) -> ApiResult<Option<String>> {
    let n = n.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if n.is_some_and(|s| s.chars().count() > 2000 || s.chars().any(char::is_control)) {
        return Err(ApiError::BadRequest("invalid_note"));
    }
    Ok(n.map(str::to_string))
}

/// For an edit or a retirement: the entry's approved version (locked) and the next version number.
async fn current(tx: &mut Tx, t: Table, entry: Uuid) -> ApiResult<(Uuid, i32)> {
    let cur: Option<(Uuid, i32)> = sqlx::query_as(&format!(
        "SELECT id, version FROM {} WHERE entry_id = $1 AND status = 'approved' FOR UPDATE",
        t.name()
    ))
    .bind(entry)
    .fetch_optional(&mut **tx)
    .await?;
    let (id, _) = cur.ok_or(ApiError::Conflict("entry_not_effective"))?;
    let next: i32 = sqlx::query_scalar(&format!(
        "SELECT max(version) + 1 FROM {} WHERE entry_id = $1",
        t.name()
    ))
    .bind(entry)
    .fetch_one(&mut **tx)
    .await?;
    Ok((id, next))
}

async fn refuse_open_proposal(tx: &mut Tx, t: Table, entry: Uuid) -> ApiResult<()> {
    let open: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE entry_id = $1 AND status = 'proposed')",
        t.name()
    ))
    .bind(entry)
    .fetch_one(&mut **tx)
    .await?;
    if open {
        Err(ApiError::Conflict("proposal_open"))
    } else {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct EquivalenceReq {
    /// create | edit | retire
    pub change: String,
    pub entry_id: Option<Uuid>,
    pub a_standard: Option<String>,
    pub a_grade: Option<String>,
    pub b_standard: Option<String>,
    pub b_grade: Option<String>,
    pub equivalence: Option<String>,
    pub conditions: Option<String>,
    pub reference: Option<String>,
    #[serde(default)]
    pub source_checked: bool,
    pub note: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn insert_equivalence(
    tx: &mut Tx,
    p: &Principal,
    entry: Uuid,
    version: i32,
    change: &str,
    pr: &Proposal,
    source_checked: bool,
    note: Option<&str>,
    batch: Option<Uuid>,
    supersedes: Option<Uuid>,
) -> ApiResult<Uuid> {
    if source_checked && pr.reference.is_none() {
        return Err(ApiError::BadRequest("reference_required_when_checked"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO grade_equivalences (id, tenant_id, entry_id, version, change, a_standard, a_grade, b_standard, b_grade, \
         pair_key, equivalence, conditions, reference, source_checked, proposed_by, proposal_note, import_batch, supersedes) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(entry)
    .bind(version)
    .bind(change)
    .bind(&pr.a.standard)
    .bind(&pr.a.grade)
    .bind(&pr.b.standard)
    .bind(&pr.b.grade)
    .bind(grades::pair_key(&pr.a, &pr.b))
    .bind(pr.kind.as_str())
    .bind(&pr.conditions)
    .bind(&pr.reference)
    .bind(source_checked)
    .bind(p.user_id)
    .bind(note)
    .bind(batch)
    .bind(supersedes)
    .execute(&mut **tx)
    .await?;
    audit::append(
        tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "grade_equivalence.propose",
            "grade_equivalence",
            id,
            json!({ "entry_id": entry, "version": version, "change": change, "pair_key": grades::pair_key(&pr.a, &pr.b),
                    "equivalence": pr.kind.as_str(), "source_checked": source_checked, "import_batch": batch }),
        ),
    )
    .await?;
    Ok(id)
}

async fn pair_effective(tx: &mut Tx, key: &str, except_entry: Option<Uuid>) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM grade_equivalences WHERE pair_key = $1 AND status = 'approved' \
         AND ($2::uuid IS NULL OR entry_id <> $2))",
    )
    .bind(key)
    .bind(except_entry)
    .fetch_one(&mut **tx)
    .await?)
}

/// POST /grades/equivalences — proposes a new entry, an edit, or a retirement (a second person approves it).
pub async fn propose_equivalence(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<EquivalenceReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let note = note_of(&req.note)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    serialize(&mut tx, &p).await?;
    let fields = |r: &EquivalenceReq| {
        grades::validate(
            opt(&r.a_standard),
            opt(&r.a_grade),
            opt(&r.b_standard),
            opt(&r.b_grade),
            opt(&r.equivalence),
            opt(&r.conditions),
            opt(&r.reference),
        )
        .map_err(ApiError::BadRequest)
    };
    let id = match req.change.as_str() {
        "create" => {
            let pr = fields(&req)?;
            if pair_effective(&mut tx, &grades::pair_key(&pr.a, &pr.b), None).await? {
                return Err(ApiError::Conflict("pair_exists"));
            }
            insert_equivalence(
                &mut tx,
                &p,
                Uuid::now_v7(),
                1,
                "create",
                &pr,
                req.source_checked,
                note.as_deref(),
                None,
                None,
            )
            .await?
        }
        "edit" | "retire" => {
            let entry = req.entry_id.ok_or(ApiError::BadRequest("entry_id_required"))?;
            refuse_open_proposal(&mut tx, Table::Equivalences, entry).await?;
            let (cur, next) = current(&mut tx, Table::Equivalences, entry).await?;
            let pr = if req.change == "edit" {
                let pr = fields(&req)?;
                if pair_effective(&mut tx, &grades::pair_key(&pr.a, &pr.b), Some(entry)).await? {
                    return Err(ApiError::Conflict("pair_exists"));
                }
                pr
            } else {
                // A retirement repeats the current content (the row records what stops being effective).
                let (a_s, a_g, b_s, b_g, k, c, r): EqContent = sqlx::query_as(
                    "SELECT a_standard, a_grade, b_standard, b_grade, equivalence, conditions, reference \
                     FROM grade_equivalences WHERE id = $1",
                )
                .bind(cur)
                .fetch_one(&mut *tx)
                .await?;
                grades::validate(opt(&a_s), &a_g, opt(&b_s), &b_g, &k, opt(&c), opt(&r))
                    .map_err(ApiError::BadRequest)?
            };
            let checked = req.change == "edit" && req.source_checked;
            insert_equivalence(
                &mut tx,
                &p,
                entry,
                next,
                &req.change,
                &pr,
                checked,
                note.as_deref(),
                None,
                Some(cur),
            )
            .await?
        }
        _ => return Err(ApiError::BadRequest("invalid_change")),
    };
    let out = rows(&mut tx, Table::Equivalences, Some(id)).await?;
    tx.commit().await?;
    Ok(Json(out.into_iter().next().unwrap_or(Value::Null)))
}

/// One proposal or approval at a time per tenant, so pair and version checks are not raced.
async fn serialize(tx: &mut Tx, p: &Principal) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 32))")
        .bind(p.tenant_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ImportReq {
    pub csv: String,
    pub note: Option<String>,
}

/// POST /grades/equivalences/import — the tenant's own CSV; every row becomes a proposal (all or nothing).
pub async fn import_equivalences(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<ImportReq>,
) -> ApiResult<Response> {
    p.require(&st, Permission::ClaimWrite).await?;
    if req.csv.len() > 1_000_000 {
        return Err(ApiError::TooLarge);
    }
    let note = note_of(&req.note)?;
    let (ok, errs) = grades::parse_csv(&req.csv).map_err(ApiError::BadRequest)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    serialize(&mut tx, &p).await?;
    // Pairs that already have an approved entry are changed through an edit, not re-imported.
    let mut existing = Vec::new();
    for pr in &ok {
        let key = grades::pair_key(&pr.a, &pr.b);
        if pair_effective(&mut tx, &key, None).await? {
            existing.push(key);
        }
    }
    if !errs.is_empty() || !existing.is_empty() || ok.is_empty() {
        let code = if ok.is_empty() && errs.is_empty() {
            "no_rows"
        } else {
            "invalid_rows"
        };
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": code, "rows": errs, "existing_pairs": existing })),
        )
            .into_response());
    }
    let batch = Uuid::now_v7();
    let mut ids = Vec::new();
    for pr in &ok {
        // Imported rows are never marked as checked against the standard: that is a per-entry statement.
        ids.push(
            insert_equivalence(
                &mut tx,
                &p,
                Uuid::now_v7(),
                1,
                "create",
                pr,
                false,
                note.as_deref(),
                Some(batch),
                None,
            )
            .await?,
        );
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "grade_equivalence.import",
            "grade_equivalence_import",
            batch,
            json!({ "rows": ids.len() }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(
        Json(json!({ "import_batch": batch, "proposed": ids, "status": "proposed", "note": grades::UNVERIFIED_NOTE }))
            .into_response(),
    )
}

#[derive(Debug, Deserialize)]
pub struct LimitReq {
    pub change: String,
    pub entry_id: Option<Uuid>,
    pub standard: Option<String>,
    pub grade: Option<String>,
    pub property: Option<String>,
    pub min: Option<String>,
    pub max: Option<String>,
    pub unit: Option<String>,
    pub reference: Option<String>,
    #[serde(default)]
    pub source_checked: bool,
    pub note: Option<String>,
}

/// POST /grades/limits — proposes a property limit of a grade (create / edit / retire), four-eyes like equivalences.
pub async fn propose_limit(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<LimitReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let note = note_of(&req.note)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    serialize(&mut tx, &p).await?;
    let (entry, version, supersedes, pr, checked) = match req.change.as_str() {
        "create" | "edit" => {
            let pr = grades::validate_limit(
                opt(&req.standard),
                opt(&req.grade),
                opt(&req.property),
                opt(&req.min),
                opt(&req.max),
                opt(&req.unit),
                opt(&req.reference),
            )
            .map_err(ApiError::BadRequest)?;
            let (entry, version, sup) = if req.change == "create" {
                (Uuid::now_v7(), 1, None)
            } else {
                let entry = req.entry_id.ok_or(ApiError::BadRequest("entry_id_required"))?;
                refuse_open_proposal(&mut tx, Table::Limits, entry).await?;
                let (cur, next) = current(&mut tx, Table::Limits, entry).await?;
                (entry, next, Some(cur))
            };
            let (key, except) = (grades::limit_key(&pr.grade, &pr.property), sup.map(|_| entry));
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM grade_limits WHERE limit_key = $1 AND status = 'approved' \
                 AND ($2::uuid IS NULL OR entry_id <> $2))",
            )
            .bind(&key)
            .bind(except)
            .fetch_one(&mut *tx)
            .await?;
            if exists {
                return Err(ApiError::Conflict("limit_exists"));
            }
            (entry, version, sup, pr, req.source_checked)
        }
        "retire" => {
            let entry = req.entry_id.ok_or(ApiError::BadRequest("entry_id_required"))?;
            refuse_open_proposal(&mut tx, Table::Limits, entry).await?;
            let (cur, next) = current(&mut tx, Table::Limits, entry).await?;
            let (s, g, pr, mn, mx, u, r): LimitContent = sqlx::query_as(
                "SELECT standard, grade, property, min_value::text, max_value::text, unit, reference FROM grade_limits WHERE id = $1",
            )
            .bind(cur)
            .fetch_one(&mut *tx)
            .await?;
            let pr = grades::validate_limit(opt(&s), &g, &pr, opt(&mn), opt(&mx), opt(&u), opt(&r))
                .map_err(ApiError::BadRequest)?;
            (entry, next, Some(cur), pr, false)
        }
        _ => return Err(ApiError::BadRequest("invalid_change")),
    };
    if checked && pr.reference.is_none() {
        return Err(ApiError::BadRequest("reference_required_when_checked"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO grade_limits (id, tenant_id, entry_id, version, change, standard, grade, property, limit_key, \
         min_value, max_value, unit, reference, source_checked, proposed_by, proposal_note, supersedes) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::numeric,$11::numeric,$12,$13,$14,$15,$16,$17)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(entry)
    .bind(version)
    .bind(&req.change)
    .bind(&pr.grade.standard)
    .bind(&pr.grade.grade)
    .bind(&pr.property)
    .bind(grades::limit_key(&pr.grade, &pr.property))
    .bind(pr.min.map(|d| d.to_string()))
    .bind(pr.max.map(|d| d.to_string()))
    .bind(&pr.unit)
    .bind(&pr.reference)
    .bind(checked)
    .bind(p.user_id)
    .bind(&note)
    .bind(supersedes)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "grade_limit.propose", "grade_limit", id,
            json!({ "entry_id": entry, "version": version, "change": req.change, "property": pr.property, "source_checked": checked })),
    )
    .await?;
    let out = rows(&mut tx, Table::Limits, Some(id)).await?;
    tx.commit().await?;
    Ok(Json(out.into_iter().next().unwrap_or(Value::Null)))
}

#[derive(Debug, Deserialize)]
pub struct DecideReq {
    pub comment: Option<String>,
}

/// Approves or rejects a proposal. Approval of an edit or a retirement supersedes the previous approved version
/// in the same transaction; for equivalences the decisions resting on that version are flagged.
async fn decide(
    st: &AppState,
    p: &Principal,
    t: Table,
    id: Uuid,
    approve: bool,
    req: &DecideReq,
) -> ApiResult<Json<Value>> {
    p.require(st, Permission::ReviewDecide).await?;
    let comment = note_of(&req.comment)?;
    if !approve && comment.is_none() {
        return Err(ApiError::BadRequest("comment_required"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    serialize(&mut tx, p).await?;
    let row: Option<(String, String, Uuid, Option<Uuid>, Uuid)> = sqlx::query_as(&format!(
        "SELECT status, change, proposed_by, supersedes, entry_id FROM {} WHERE id = $1 FOR UPDATE",
        t.name()
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let (status, change, proposer, supersedes, entry) = row.ok_or(ApiError::NotFound)?;
    if status != "proposed" {
        return Err(ApiError::Conflict("not_proposed"));
    }
    if proposer == p.user_id {
        return Err(ApiError::Forbidden("four_eyes_required"));
    }
    let mut flagged = 0;
    let new_status = if !approve {
        "rejected"
    } else {
        if let Some(old) = supersedes {
            let n = sqlx::query(&format!(
                "UPDATE {} SET status = 'superseded' WHERE id = $1 AND status = 'approved'",
                t.name()
            ))
            .bind(old)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if n != 1 {
                return Err(ApiError::Conflict("stale_proposal"));
            }
        }
        if change == "retire" { "retired" } else { "approved" }
    };
    if new_status == "approved" {
        // One effective entry per pair / per grade property (also a unique index).
        let clash: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {0} o, {0} n WHERE n.id = $1 AND o.status = 'approved' AND o.entry_id <> n.entry_id \
             AND o.{1} = n.{1})",
            t.name(),
            match t {
                Table::Equivalences => "pair_key",
                Table::Limits => "limit_key",
            }
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if clash {
            return Err(ApiError::Conflict("pair_exists"));
        }
    }
    sqlx::query(&format!(
        "UPDATE {} SET status = $2, decided_by = $3, decided_at = now(), decision_comment = $4 WHERE id = $1",
        t.name()
    ))
    .bind(id)
    .bind(new_status)
    .bind(p.user_id)
    .bind(&comment)
    .execute(&mut *tx)
    .await?;
    if approve && let (Table::Equivalences, Some(old)) = (t, supersedes) {
        flagged = crate::decisions::on_equivalence_change(&mut tx, p.tenant_id, p.user_id, old).await?;
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            if approve {
                "grade_curation.approve"
            } else {
                "grade_curation.reject"
            },
            t.target(),
            id,
            json!({ "entry_id": entry, "change": change, "status": new_status, "supersedes": supersedes,
                    "decisions_flagged": flagged }),
        ),
    )
    .await?;
    let out = rows(&mut tx, t, Some(id)).await?;
    tx.commit().await?;
    let mut v = out.into_iter().next().unwrap_or(Value::Null);
    v["decisions_flagged"] = json!(flagged);
    Ok(Json(v))
}

pub async fn approve_equivalence(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<DecideReq>,
) -> ApiResult<Json<Value>> {
    decide(&st, &p, Table::Equivalences, id, true, &req).await
}

pub async fn reject_equivalence(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<DecideReq>,
) -> ApiResult<Json<Value>> {
    decide(&st, &p, Table::Equivalences, id, false, &req).await
}

pub async fn approve_limit(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<DecideReq>,
) -> ApiResult<Json<Value>> {
    decide(&st, &p, Table::Limits, id, true, &req).await
}

pub async fn reject_limit(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<DecideReq>,
) -> ApiResult<Json<Value>> {
    decide(&st, &p, Table::Limits, id, false, &req).await
}

/// Equivalence versions a match result rests on (for decision dependencies), sorted and unique.
pub(crate) fn basis_ids(m: &domain_manufacturing::three_way::MatchSet) -> Vec<Uuid> {
    m.material_basis
        .iter()
        .filter_map(|b| Uuid::parse_str(&b.equivalence).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
