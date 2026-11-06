//! Regulation-list versions as dependency nodes (research I6, ADR 0028, `mfg-reglist@1`, migration 0031).
//!
//! A tenant imports versioned substance lists (KIZUNA ships no list content), supplier compliance declarations that
//! name the list version they answered, and material composition (FMD-style) data. Importing a new list version
//! assesses every current declaration on that list once: when a covered material could contain a newly listed
//! substance — or nobody can say what the material contains — the declaration's dependent decisions are flagged
//! `potentially_affected` through the existing selective invalidation. Nothing is reversed automatically; people
//! re-review. Every declaration considered is recorded with its verdict, so "why was this not flagged" is answerable.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, State};
use domain_manufacturing::reglist::{self, DeclRow, FmdRow, ListEntry, REGLIST_VERSION, Verdict};
use evidence_core::claim::ClaimInput;
use kizuna_messages::Msg;
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::artifacts;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::claims::{self, NewClaim};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};

/// Rows one declaration or composition import may create (each becomes a claim with its own impact analysis).
pub const MAX_IMPORT_ROWS: usize = 5_000;

/// Cause kind of the invalidation events a list change writes.
pub const CAUSE_KIND: &str = "regulation_list_changed";

const EXTRACTION_METHOD: &str = "import:mfg-reglist@1";

/// Reads a released, intact upload of the expected kind.
async fn csv_of(st: &AppState, p: &Principal, id: Uuid, kind: &str) -> ApiResult<Vec<u8>> {
    let (intact, bytes, art) = artifacts::verify_integrity(st, p, id).await?;
    if art.kind != kind {
        return Err(ApiError::BadRequest("reglist_wrong_document_kind"));
    }
    if art.status != "accepted" {
        return Err(ApiError::Conflict("artifact_not_released"));
    }
    if !intact {
        return Err(ApiError::Conflict("integrity_check_failed"));
    }
    Ok(bytes)
}

fn import_err(e: reglist::ImportError) -> ApiError {
    ApiError::BadRequest(e.code())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct VersionOut {
    pub id: Uuid,
    pub label: String,
    pub seq: i32,
    pub artifact_id: Uuid,
    pub previous_id: Option<Uuid>,
    pub entry_count: i32,
    pub engine: String,
    pub summary: Value,
    pub imported_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub imported_at: OffsetDateTime,
}

const VERSION_COLS: &str =
    "id, label, seq, artifact_id, previous_id, entry_count, engine, summary, imported_by, imported_at";

#[derive(Debug, Serialize)]
pub struct ListOut {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub versions: Vec<VersionOut>,
}

/// GET /reg-lists — the tenant's lists with their versions (newest first).
pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<ListOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let lists: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, code, name FROM reg_lists ORDER BY code LIMIT 200")
            .fetch_all(&mut *tx)
            .await?;
    let mut out = Vec::with_capacity(lists.len());
    for (id, code, name) in lists {
        let versions = sqlx::query_as(&format!(
            "SELECT {VERSION_COLS} FROM reg_list_versions WHERE list_id = $1 ORDER BY seq DESC LIMIT 100"
        ))
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        out.push(ListOut {
            id,
            code,
            name,
            versions,
        });
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct CreateListReq {
    pub code: String,
    pub name: String,
}

/// POST /reg-lists — registers a list (e.g. code `svhc`); its versions are imported separately.
pub async fn create_list(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CreateListReq>,
) -> ApiResult<Json<ListOut>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let code = req.code.trim().to_lowercase();
    if !reglist::valid_list_code(&code) {
        return Err(ApiError::BadRequest("reglist_invalid_code"));
    }
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("reglist_invalid_name"));
    }
    let id = Uuid::now_v7();
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let inserted = sqlx::query(
        "INSERT INTO reg_lists (id, tenant_id, code, name, created_by) VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (tenant_id, code) DO NOTHING",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(&code)
    .bind(name)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(ApiError::Conflict("reglist_exists"));
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "reg_list.created", "reg_list", id, json!({ "code": code })),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ListOut {
        id,
        code,
        name: name.to_owned(),
        versions: vec![],
    }))
}

#[derive(Debug, Deserialize)]
pub struct ImportVersionReq {
    pub label: String,
    pub artifact_id: Uuid,
}

/// One current declaration on a list, with the version it answered.
#[derive(sqlx::FromRow)]
struct DeclOn {
    claim_id: Uuid,
    version_id: Uuid,
    label: String,
    supplier: String,
    part: String,
    materials: Vec<String>,
}

/// The verdict of one declaration and what it seeds.
struct Assessed {
    decl: DeclOn,
    verdict: Verdict,
}

/// Substance keys of a stored version.
async fn keys_of(tx: &mut Tx, version: Uuid) -> ApiResult<BTreeSet<String>> {
    let keys: Vec<String> = sqlx::query_scalar("SELECT substance_key FROM reg_list_entries WHERE version_id = $1")
        .bind(version)
        .fetch_all(&mut **tx)
        .await?;
    Ok(keys.into_iter().collect())
}

/// Composition per part from current, non-rejected `reg.composition` claims: material → declared substances.
async fn compositions(
    tx: &mut Tx,
    parts: &[String],
) -> ApiResult<BTreeMap<String, BTreeMap<String, BTreeSet<String>>>> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT subject_key, value_text, scope->>'substance' FROM claims WHERE subject_type = 'part' \
         AND predicate = 'reg.composition' AND superseded_at IS NULL AND review_status <> 'rejected' \
         AND value_text IS NOT NULL AND subject_key = ANY($1)",
    )
    .bind(parts)
    .fetch_all(&mut **tx)
    .await?;
    let mut out: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (part, material, substance) in rows {
        let m = out.entry(part).or_default().entry(material).or_default();
        m.extend(substance);
    }
    Ok(out)
}

/// The tenant's current curated profiles: material → substances it could contain.
async fn profiles(tx: &mut Tx) -> ApiResult<BTreeMap<String, BTreeSet<String>>> {
    let rows: Vec<(String, Vec<String>)> =
        sqlx::query_as("SELECT material_key, substances FROM material_profiles WHERE superseded_at IS NULL")
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows.into_iter().map(|(m, s)| (m, s.into_iter().collect())).collect())
}

fn substance_names(keys: &[String], names: &BTreeMap<&str, &str>) -> String {
    const SHOWN: usize = 5;
    let mut out: Vec<String> = keys
        .iter()
        .take(SHOWN)
        .map(|k| names.get(k.as_str()).map_or_else(|| k.clone(), |n| (*n).to_string()))
        .collect();
    if keys.len() > SHOWN {
        out.push(format!("+{}", keys.len() - SHOWN));
    }
    out.join(", ")
}

fn verdict_msg(v: &Verdict, names: &BTreeMap<&str, &str>) -> Msg {
    match v {
        Verdict::MayContain { substances, .. } => {
            Msg::new("reglist.may_contain").with("substances", substance_names(substances, names))
        }
        Verdict::CannotRuleOut { substances, .. } => {
            Msg::new("reglist.cannot_rule_out").with("substances", substance_names(substances, names))
        }
        Verdict::NotAffected => Msg::new("reglist.not_affected"),
    }
}

/// POST /reg-lists/{id}/versions — imports a new version of a list from a released `regulation_list` CSV and
/// assesses every current declaration that answered an earlier version. Flags decisions; never reverses them.
pub async fn import_version(
    State(st): State<AppState>,
    p: Principal,
    Path(list_id): Path<Uuid>,
    ApiJson(req): ApiJson<ImportVersionReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let label = req.label.trim().to_owned();
    if !reglist::valid_version_label(&label) {
        return Err(ApiError::BadRequest("reglist_invalid_version"));
    }
    let bytes = csv_of(&st, &p, req.artifact_id, "regulation_list").await?;
    let entries = reglist::parse_list(&bytes).map_err(import_err)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let code: String = sqlx::query_scalar("SELECT code FROM reg_lists WHERE id = $1")
        .bind(list_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    // One import per list at a time: versions are numbered and each is assessed against a stable set of declarations.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 61))")
        .bind(list_id.to_string())
        .execute(&mut *tx)
        .await?;
    let prev: Option<(Uuid, i32)> =
        sqlx::query_as("SELECT id, seq FROM reg_list_versions WHERE list_id = $1 ORDER BY seq DESC LIMIT 1")
            .bind(list_id)
            .fetch_optional(&mut *tx)
            .await?;
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reg_list_versions WHERE list_id = $1 AND label = $2)")
            .bind(list_id)
            .bind(&label)
            .fetch_one(&mut *tx)
            .await?;
    if taken {
        return Err(ApiError::Conflict("reglist_version_exists"));
    }
    let seq = prev.map_or(1, |p| p.1 + 1);
    let version_id = Uuid::now_v7();
    let new_keys: BTreeSet<String> = entries.iter().map(|e| e.key.clone()).collect();
    let names: BTreeMap<&str, &str> = entries.iter().map(|e| (e.key.as_str(), e.name.as_str())).collect();

    // Every current declaration on this list that answered an earlier version.
    let decls: Vec<DeclOn> = sqlx::query_as(
        "SELECT d.claim_id, d.version_id, v.label, d.supplier, d.part, d.materials FROM reg_declarations d \
         JOIN reg_list_versions v ON v.id = d.version_id JOIN claims c ON c.id = d.claim_id \
         WHERE d.list_id = $1 AND c.superseded_at IS NULL AND c.review_status <> 'rejected' \
         ORDER BY d.part, d.supplier, d.claim_id",
    )
    .bind(list_id)
    .fetch_all(&mut *tx)
    .await?;
    let parts: Vec<String> = decls
        .iter()
        .map(|d| d.part.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let fmd = compositions(&mut tx, &parts).await?;
    let prof = profiles(&mut tx).await?;
    let mut answered: BTreeMap<Uuid, BTreeSet<String>> = BTreeMap::new();
    let mut assessed = Vec::with_capacity(decls.len());
    for d in decls {
        if let std::collections::btree_map::Entry::Vacant(e) = answered.entry(d.version_id) {
            e.insert(keys_of(&mut tx, d.version_id).await?);
        }
        let added: BTreeSet<String> = new_keys.difference(&answered[&d.version_id]).cloned().collect();
        let empty = BTreeMap::new();
        let verdict = reglist::assess(&added, fmd.get(&d.part).unwrap_or(&empty), &d.materials, &prof);
        assessed.push(Assessed { decl: d, verdict });
    }

    // Flag the decisions that relied on a flagged declaration (selective invalidation; humans re-review).
    let seeds: BTreeMap<Uuid, (&'static str, Msg, Value)> = assessed
        .iter()
        .filter(|a| a.verdict.flagged())
        .map(|a| {
            let why = Msg::new("invalidation.reglist")
                .with("list", code.clone())
                .with("from", a.decl.label.clone())
                .with("to", label.clone())
                .with_msg("verdict", verdict_msg(&a.verdict, &names));
            let sem = json!({
                "predicate": format!("reg.list.{code}"), "list": code, "from": a.decl.label, "to": label,
                "list_version": version_id, "assessment": a.verdict, "engine": REGLIST_VERSION,
            });
            (a.decl.claim_id, (CAUSE_KIND, why, sem))
        })
        .collect();
    let flagged_per = if seeds.is_empty() {
        BTreeMap::new()
    } else {
        crate::decisions::invalidate_seeded(&mut tx, p.tenant_id, p.user_id, &seeds).await?
    };

    let count = |code: &str| assessed.iter().filter(|a| a.verdict.code() == code).count();
    let decisions_flagged: usize = flagged_per.values().sum();
    let summary = json!({
        "declarations_considered": assessed.len(),
        "may_contain": count("may_contain"),
        "cannot_rule_out": count("cannot_rule_out"),
        "not_affected": count("not_affected"),
        "decisions_flagged": decisions_flagged,
    });
    sqlx::query(
        "INSERT INTO reg_list_versions (id, tenant_id, list_id, label, seq, artifact_id, previous_id, entry_count, engine, \
         summary, imported_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(version_id)
    .bind(p.tenant_id)
    .bind(list_id)
    .bind(&label)
    .bind(seq)
    .bind(req.artifact_id)
    .bind(prev.map(|p| p.0))
    .bind(i32::try_from(entries.len()).map_err(ApiError::internal)?)
    .bind(REGLIST_VERSION)
    .bind(&summary)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    insert_entries(&mut tx, p.tenant_id, version_id, &entries).await?;
    for a in &assessed {
        sqlx::query(
            "INSERT INTO reg_assessments (id, tenant_id, version_id, declaration_claim, answered_version, part, supplier, \
             verdict, detail, decisions_flagged) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(version_id)
        .bind(a.decl.claim_id)
        .bind(a.decl.version_id)
        .bind(&a.decl.part)
        .bind(&a.decl.supplier)
        .bind(a.verdict.code())
        .bind(json!({ "assessment": a.verdict, "msg": verdict_msg(&a.verdict, &names).to_value() }))
        .bind(i32::try_from(flagged_per.get(&a.decl.claim_id).copied().unwrap_or(0)).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await?;
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "reg_list.version_imported",
            "reg_list_version",
            version_id,
            json!({ "list": code, "label": label, "seq": seq, "artifact_id": req.artifact_id, "entries": entries.len(),
                    "summary": summary, "engine": REGLIST_VERSION }),
        ),
    )
    .await?;
    let version: VersionOut = sqlx::query_as(&format!("SELECT {VERSION_COLS} FROM reg_list_versions WHERE id = $1"))
        .bind(version_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "list": { "id": list_id, "code": code }, "version": version }),
    ))
}

async fn insert_entries(tx: &mut Tx, tenant: Uuid, version: Uuid, entries: &[ListEntry]) -> ApiResult<()> {
    for chunk in entries.chunks(5_000) {
        let col = |f: fn(&ListEntry) -> Option<String>| chunk.iter().map(f).collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO reg_list_entries (tenant_id, version_id, substance_key, name, cas, ec, date_added) \
             SELECT $1, $2, k, n, c, e, d FROM UNNEST($3::text[], $4::text[], $5::text[], $6::text[], $7::text[]) AS t(k, n, c, e, d)",
        )
        .bind(tenant)
        .bind(version)
        .bind(col(|e| Some(e.key.clone())))
        .bind(col(|e| Some(e.name.clone())))
        .bind(col(|e| e.cas.clone()))
        .bind(col(|e| e.ec.clone()))
        .bind(col(|e| e.date_added.clone()))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EntryOut {
    pub substance_key: String,
    pub name: String,
    pub cas: Option<String>,
    pub ec: Option<String>,
    pub date_added: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AssessmentOut {
    pub declaration_claim: Uuid,
    pub answered_version: Uuid,
    pub part: String,
    pub supplier: String,
    pub verdict: String,
    pub detail: Value,
    pub decisions_flagged: i32,
}

/// GET /reg-lists/{id}/versions/{version} — what changed against the previous version and the assessment of every
/// declaration considered (flagged or not).
pub async fn get_version(
    State(st): State<AppState>,
    p: Principal,
    Path((list_id, version_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let v: VersionOut = sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM reg_list_versions WHERE id = $1 AND list_id = $2"
    ))
    .bind(version_id)
    .bind(list_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    const DIFF: &str = "SELECT substance_key, name, cas, ec, date_added FROM reg_list_entries WHERE version_id = $1 \
         AND substance_key NOT IN (SELECT substance_key FROM reg_list_entries WHERE version_id = $2) \
         ORDER BY substance_key LIMIT 1000";
    let added: Vec<EntryOut> = sqlx::query_as(DIFF)
        .bind(v.id)
        .bind(v.previous_id)
        .fetch_all(&mut *tx)
        .await?;
    let removed: Vec<EntryOut> = match v.previous_id {
        Some(prev) => sqlx::query_as(DIFF).bind(prev).bind(v.id).fetch_all(&mut *tx).await?,
        None => vec![],
    };
    let assessments: Vec<AssessmentOut> = sqlx::query_as(
        "SELECT declaration_claim, answered_version, part, supplier, verdict, detail, decisions_flagged \
         FROM reg_assessments WHERE version_id = $1 ORDER BY verdict, part, supplier, declaration_claim LIMIT 5000",
    )
    .bind(v.id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "version": v, "added": added, "removed": removed, "assessments": assessments,
        "limitations": Msg::new("limitations.reglist").to_value(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct ImportDocReq {
    pub artifact_id: Uuid,
}

async fn refuse_reimport(tx: &mut Tx, artifact: Uuid, predicate: &str) -> ApiResult<()> {
    let done: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM claims WHERE artifact_id = $1 AND predicate = $2)")
            .bind(artifact)
            .bind(predicate)
            .fetch_one(&mut **tx)
            .await?;
    if done {
        return Err(ApiError::Conflict("reglist_already_imported"));
    }
    Ok(())
}

/// Inserts one imported row as a claim, audited, with the usual new-evidence impact analysis.
async fn import_claim(tx: &mut Tx, p: &Principal, artifact: Uuid, input: ClaimInput, line: usize) -> ApiResult<Uuid> {
    let mut input = input;
    input.source_location = Some(json!({ "row": line.min(999_999) }));
    input.confidence_bps = Some(10_000);
    let v = claims::validate_typed(&input)?;
    let id = claims::insert_claim(
        tx,
        p.tenant_id,
        NewClaim {
            claim: &v,
            artifact_id: Some(artifact),
            valid_from: None,
            valid_to: None,
            extraction_method: EXTRACTION_METHOD,
            provenance: json!({ "imported_by": p.user_id, "engine": REGLIST_VERSION }),
            recorded_by: Some(p.user_id),
            ai_suggested: false,
        },
    )
    .await?;
    let out = claims::fetch(tx, id).await?;
    crate::decisions::on_claim_change(tx, p.tenant_id, p.user_id, crate::decisions::new_evidence(&out)).await?;
    Ok(id)
}

fn claim_input(part: &str, predicate: &str, value: &str, scope: Value) -> ClaimInput {
    ClaimInput {
        subject_type: "part".into(),
        subject_key: part.into(),
        predicate: predicate.into(),
        value_text: Some(value.into()),
        value_num: None,
        value_unit: None,
        scope: Some(scope),
        revision: None,
        source_location: None,
        confidence_bps: None,
    }
}

/// POST /reg-lists/declarations — imports a released `compliance_declaration` CSV: one `reg.declaration` claim per
/// row, linked to the list version it answered (which must already be imported).
pub async fn import_declarations(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<ImportDocReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let bytes = csv_of(&st, &p, req.artifact_id, "compliance_declaration").await?;
    let rows: Vec<DeclRow> = reglist::parse_declarations(&bytes).map_err(import_err)?;
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(ApiError::BadRequest("csv_too_many_rows"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    refuse_reimport(&mut tx, req.artifact_id, "reg.declaration").await?;
    let known: Vec<(String, String, Uuid, Uuid)> = sqlx::query_as(
        "SELECT l.code, v.label, v.id, l.id FROM reg_list_versions v JOIN reg_lists l ON l.id = v.list_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let known: BTreeMap<(String, String), (Uuid, Uuid)> =
        known.into_iter().map(|(c, l, v, li)| ((c, l), (v, li))).collect();
    let mut ids = Vec::with_capacity(rows.len());
    for r in &rows {
        let &(version, list) = known
            .get(&(r.list.clone(), r.version.clone()))
            .ok_or(ApiError::BadRequest("decl_unknown_list_version"))?;
        let mut scope = json!({ "supplier": r.supplier, "list": r.list, "list_version": r.version });
        if !r.materials.is_empty() {
            scope["materials"] = json!(r.materials.join(";"));
        }
        let id = import_claim(
            &mut tx,
            &p,
            req.artifact_id,
            claim_input(&r.part, "reg.declaration", r.status.as_str(), scope),
            r.line,
        )
        .await?;
        sqlx::query(
            "INSERT INTO reg_declarations (claim_id, tenant_id, version_id, list_id, supplier, part, status, materials) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(id)
        .bind(p.tenant_id)
        .bind(version)
        .bind(list)
        .bind(&r.supplier)
        .bind(&r.part)
        .bind(r.status.as_str())
        .bind(&r.materials)
        .execute(&mut *tx)
        .await?;
        ids.push(id);
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "reg_list.declarations_imported",
            "artifact",
            req.artifact_id,
            json!({ "claims": ids.len(), "engine": REGLIST_VERSION }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "claims_created": ids.len(), "claim_ids": ids })))
}

/// POST /reg-lists/compositions — imports a released `material_declaration` (FMD-style) CSV: one `reg.composition`
/// claim per row (value = homogeneous material; scope = declared substance and supplier when given).
pub async fn import_compositions(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<ImportDocReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let bytes = csv_of(&st, &p, req.artifact_id, "material_declaration").await?;
    let rows: Vec<FmdRow> = reglist::parse_fmd(&bytes).map_err(import_err)?;
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(ApiError::BadRequest("csv_too_many_rows"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    refuse_reimport(&mut tx, req.artifact_id, "reg.composition").await?;
    let mut n = 0usize;
    for r in &rows {
        let mut scope = json!({});
        if let Some(s) = &r.substance {
            scope["substance"] = json!(s);
        }
        if let Some(s) = &r.supplier {
            scope["supplier"] = json!(s);
        }
        import_claim(
            &mut tx,
            &p,
            req.artifact_id,
            claim_input(&r.part, "reg.composition", &r.material, scope),
            r.line,
        )
        .await?;
        n += 1;
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "reg_list.compositions_imported",
            "artifact",
            req.artifact_id,
            json!({ "claims": n, "engine": REGLIST_VERSION }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "claims_created": n })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProfileOut {
    pub id: Uuid,
    pub material_key: String,
    pub substances: Vec<String>,
    pub note: Option<String>,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub superseded_at: Option<OffsetDateTime>,
}

const PROFILE_COLS: &str = "id, material_key, substances, note, created_by, created_at, superseded_at";

/// GET /material-profiles — the tenant's curated material profiles, superseded ones included.
pub async fn list_profiles(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<ProfileOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM material_profiles ORDER BY material_key, created_at DESC LIMIT 2000"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ProfileReq {
    pub material: String,
    /// Substance keys (`cas:…`, `ec:…`, `name:…`) or bare CAS / EC numbers. Empty = curated as containing none of
    /// interest (a material without a profile is never assumed clean).
    pub substances: Vec<String>,
    pub note: Option<String>,
}

/// A substance reference as typed: a bare CAS/EC number or a key with a valid CAS/EC number or a name.
pub fn substance_ref(s: &str) -> Option<String> {
    let s = s.trim();
    if reglist::valid_cas(s) {
        return Some(format!("cas:{s}"));
    }
    if reglist::valid_ec(s) {
        return Some(format!("ec:{s}"));
    }
    match s.split_once(':') {
        Some(("cas", c)) if reglist::valid_cas(c) => Some(s.to_string()),
        Some(("ec", e)) if reglist::valid_ec(e) => Some(s.to_string()),
        Some(("name", n)) => reglist::substance_key(n, None, None),
        _ => None,
    }
}

/// PUT /material-profiles — records the tenant's curated view of which listed substances a material could contain,
/// superseding the current profile. Privileged: a profile can stop declarations from being flagged.
pub async fn put_profile(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<ProfileReq>,
) -> ApiResult<Json<ProfileOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let material = reglist::material_key(&req.material);
    if material.is_empty() || material.chars().count() > 200 {
        return Err(ApiError::BadRequest("reglist_invalid_material"));
    }
    if req.substances.len() > 500 {
        return Err(ApiError::BadRequest("reglist_too_many_substances"));
    }
    let subs: BTreeSet<String> = req
        .substances
        .iter()
        .map(|s| substance_ref(s).ok_or(ApiError::BadRequest("reglist_invalid_substance")))
        .collect::<ApiResult<_>>()?;
    if subs.iter().any(|s| s.chars().count() > 520) {
        return Err(ApiError::BadRequest("reglist_invalid_substance"));
    }
    let note = req.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.chars().count() > 1000) {
        return Err(ApiError::BadRequest("reglist_note_too_long"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 62))")
        .bind(&material)
        .execute(&mut *tx)
        .await?;
    let id = Uuid::now_v7();
    let old: Option<Uuid> = sqlx::query_scalar(
        "UPDATE material_profiles SET superseded_at = now(), superseded_by = $2 \
         WHERE material_key = $1 AND superseded_at IS NULL RETURNING id",
    )
    .bind(&material)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let subs: Vec<String> = subs.into_iter().collect();
    sqlx::query(
        "INSERT INTO material_profiles (id, tenant_id, material_key, substances, note, created_by) VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(&material)
    .bind(&subs)
    .bind(note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "material_profile.recorded",
            "material_profile",
            id,
            json!({ "material": material, "substances": subs, "supersedes": old }),
        ),
    )
    .await?;
    let out = sqlx::query_as(&format!("SELECT {PROFILE_COLS} FROM material_profiles WHERE id = $1"))
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
    fn substance_refs() {
        assert_eq!(substance_ref("50-00-0").as_deref(), Some("cas:50-00-0"));
        assert_eq!(substance_ref("200-001-8").as_deref(), Some("ec:200-001-8"));
        assert_eq!(substance_ref("cas:50-00-0").as_deref(), Some("cas:50-00-0"));
        assert_eq!(
            substance_ref("name:group: imaginary salts").as_deref(),
            Some("name:GROUP: IMAGINARY SALTS")
        );
        assert_eq!(substance_ref("50-00-1"), None, "bad check digit");
        assert_eq!(substance_ref("cas:1"), None);
        assert_eq!(substance_ref("lead"), None, "a bare name needs the name: prefix");
    }

    #[test]
    fn names_are_shortened() {
        let keys: Vec<String> = (0..7).map(|i| format!("cas:{i}")).collect();
        let names: BTreeMap<&str, &str> = [("cas:0", "Kizunium oxide")].into();
        assert_eq!(
            substance_names(&keys, &names),
            "Kizunium oxide, cas:1, cas:2, cas:3, cas:4, +2"
        );
    }
}
