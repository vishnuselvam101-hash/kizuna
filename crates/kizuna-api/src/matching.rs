//! Quality three-way match API: PO line ↔ drawing ↔ supplier certificates (`mfg-match@1`, domain pack), recorded
//! match runs, and the acceptance decision of a PO line whose typed dependencies are exactly the cited claims, so
//! selective invalidation (M4) reaches the acceptance when any of them changes.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::State;
use conflict_engine::Fact;
use dependency_engine::Materiality;
use domain_manufacturing::three_way::{self, MATCH_RULE_VERSION, MatchSet, PoLine, norm_line};
use entity_resolution::safe_normalize;
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::claims::{self, CLAIM_COLS, ClaimOut};
use crate::db::{self, Tx};
use crate::decisions::{self, CreateReq, Prepared};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};
use crate::simulations::Overlay;
use crate::verification::load_facts_with;

/// Current part facts and the document each came from.
async fn load(tx: &mut Tx) -> ApiResult<(Vec<Fact>, BTreeMap<String, String>)> {
    load_with(tx, &Overlay::default()).await
}

/// [`load`] with a simulated change applied (what-if).
pub(crate) async fn load_with(tx: &mut Tx, ov: &Overlay) -> ApiResult<(Vec<Fact>, BTreeMap<String, String>)> {
    let facts = load_facts_with(tx, "part", None, ov).await?;
    let docs: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, artifact_id FROM claims WHERE subject_type = 'part' AND artifact_id IS NOT NULL AND superseded_at IS NULL",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut docs: BTreeMap<String, String> = docs
        .into_iter()
        .filter(|(c, _)| ov.keeps(c))
        .map(|(c, a)| (c.to_string(), a.to_string()))
        .collect();
    if let Some(s) = &ov.added
        && s.claim.subject_type == "part"
        && let Some(a) = s.claim.artifact_id
    {
        docs.insert(s.claim.id.to_string(), a.to_string());
    }
    Ok((facts, docs))
}

/// Findings of every PO line (`mfg-match@1`) over `facts`.
pub(crate) fn all_findings(facts: &[Fact], docs: &BTreeMap<String, String>) -> Vec<(PoLine, MatchSet)> {
    let parts = by_part(facts);
    three_way::po_lines(facts)
        .into_iter()
        .map(|l| {
            let m = three_way::evaluate(&l, parts.get(l.part.as_str()).map_or(&[][..], |v| v), docs);
            (l, m)
        })
        .collect()
}

fn by_part(facts: &[Fact]) -> BTreeMap<&str, Vec<Fact>> {
    let mut m: BTreeMap<&str, Vec<Fact>> = BTreeMap::new();
    for f in facts {
        m.entry(f.subject.as_str()).or_default().push(f.clone());
    }
    m
}

#[derive(Serialize)]
pub struct LineSummary {
    pub purchase_order: String,
    pub po_line: String,
    pub part: String,
    pub supplier: Option<String>,
    pub ordered_revision: Option<String>,
    pub certificates: usize,
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
    pub kinds: Vec<&'static str>,
    pub acceptance: Option<Value>,
}

async fn latest_acceptance(tx: &mut Tx) -> ApiResult<BTreeMap<String, Value>> {
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (subject_key) subject_key, json_build_object('decision_id', id, 'outcome', outcome, 'status', status, 'decided_at', decided_at) \
         FROM decisions WHERE kind = 'po_line_acceptance' ORDER BY subject_key, decided_at DESC",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().collect())
}

pub fn line_key(l: &PoLine) -> String {
    format!("{}/{}", l.purchase_order, l.line)
}

fn summarize(m: &MatchSet, acceptance: Option<Value>) -> LineSummary {
    let count = |s: three_way::Severity| m.findings.iter().filter(|f| f.severity == s).count();
    let mut kinds: Vec<&'static str> = m.findings.iter().map(|f| f.kind).collect();
    kinds.dedup();
    LineSummary {
        purchase_order: m.line.purchase_order.clone(),
        po_line: m.line.line.clone(),
        part: m.line.part.clone(),
        supplier: m.supplier.clone(),
        ordered_revision: m.ordered_revision.clone(),
        certificates: m.certificates.len(),
        critical: count(three_way::Severity::Critical),
        major: count(three_way::Severity::Major),
        minor: count(three_way::Severity::Minor),
        kinds,
        acceptance,
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub q: Option<String>,
}

/// GET /match/lines — every PO line with its match summary (newest acceptance decision included).
pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (facts, docs) = load(&mut tx).await?;
    let acc = latest_acceptance(&mut tx).await?;
    tx.commit().await?;
    let parts = by_part(&facts);
    let needle = q.q.as_deref().map(safe_normalize).filter(|s| !s.is_empty());
    let mut out = Vec::new();
    for l in three_way::po_lines(&facts) {
        if let Some(n) = &needle
            && !line_key(&l).contains(n.as_str())
            && !l.part.contains(n.as_str())
        {
            continue;
        }
        let m = three_way::evaluate(&l, parts.get(l.part.as_str()).map_or(&[][..], |v| v), &docs);
        out.push(summarize(&m, acc.get(&line_key(&l)).cloned()));
        if out.len() >= 1000 {
            break;
        }
    }
    Ok(Json(json!({ "rule_version": MATCH_RULE_VERSION, "lines": out })))
}

#[derive(Deserialize)]
pub struct LineQuery {
    pub po: String,
    pub line: String,
    pub part: Option<String>,
}

fn resolve(facts: &[Fact], q: &LineQuery) -> ApiResult<PoLine> {
    if q.po.trim().is_empty() || q.po.len() > 200 || q.line.trim().is_empty() || q.line.len() > 40 {
        return Err(ApiError::BadRequest("invalid_po_line"));
    }
    let (po, line) = (safe_normalize(&q.po), norm_line(&q.line));
    let part = q.part.as_deref().map(safe_normalize);
    let hits: Vec<PoLine> = three_way::po_lines(facts)
        .into_iter()
        .filter(|l| l.purchase_order == po && l.line == line && part.as_ref().is_none_or(|p| &l.part == p))
        .collect();
    match hits.len() {
        0 => Err(ApiError::NotFound),
        1 => Ok(hits.into_iter().next().ok_or(ApiError::NotFound)?),
        _ => Err(ApiError::Conflict("ambiguous_po_line")),
    }
}

/// Each cited claim with its exact source location and document.
async fn citations(tx: &mut Tx, m: &MatchSet) -> ApiResult<Value> {
    let ids: Vec<Uuid> = m
        .po_claims
        .iter()
        .chain(&m.drawing_claims)
        .chain(&m.certificate_claims)
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let rows: Vec<ClaimOut> = sqlx::query_as(&format!("SELECT {CLAIM_COLS} FROM claims WHERE id = ANY($1)"))
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
    let arts: Vec<(Uuid, String, String, String)> =
        sqlx::query_as("SELECT id, kind, display_name, encode(sha256, 'hex') FROM artifacts WHERE id = ANY($1)")
            .bind(rows.iter().filter_map(|c| c.artifact_id).collect::<Vec<_>>())
            .fetch_all(&mut **tx)
            .await?;
    let claims: serde_json::Map<String, Value> = rows
        .iter()
        .map(|c| {
            (c.id.to_string(), json!({
                "predicate": c.predicate, "value_text": c.value_text, "value_num": c.value_num, "value_unit": c.value_unit,
                "revision": c.revision, "scope": c.scope, "artifact_id": c.artifact_id, "source_location": c.source_location,
                "extraction_method": c.extraction_method, "confidence_bps": c.confidence_bps, "review_status": c.review_status,
            }))
        })
        .collect();
    let documents: serde_json::Map<String, Value> = arts
        .into_iter()
        .map(|(id, kind, name, sha)| {
            (
                id.to_string(),
                json!({ "kind": kind, "display_name": name, "sha256": sha }),
            )
        })
        .collect();
    Ok(json!({ "claims": claims, "documents": documents }))
}

/// GET /match/line?po=&line=[&part=] — the full match set with citations and the line's recorded runs.
pub async fn get(State(st): State<AppState>, p: Principal, ApiQuery(q): ApiQuery<LineQuery>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (facts, docs) = load(&mut tx).await?;
    let l = resolve(&facts, &q)?;
    let m = three_way::evaluate(&l, &facts, &docs);
    let cites = citations(&mut tx, &m).await?;
    let runs: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', r.id, 'created_at', r.created_at, 'blocking', r.blocking, \
                'result_sha256', encode(r.result_sha256, 'hex'), 'decision_id', r.decision_id, 'decision_status', d.status, 'outcome', d.outcome) \
         FROM match_runs r LEFT JOIN decisions d ON d.id = r.decision_id \
         WHERE r.purchase_order = $1 AND r.po_line = $2 AND r.part_key = $3 ORDER BY r.created_at DESC LIMIT 20",
    )
    .bind(&l.purchase_order)
    .bind(&l.line)
    .bind(&l.part)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "match": m, "blocking": m.blocking(), "citations": cites, "runs": runs }),
    ))
}

/// Inserts the run and its findings; returns (run id, result sha256 hex).
async fn record_run(tx: &mut Tx, p: &Principal, id: Uuid, m: &MatchSet, decision: Option<Uuid>) -> ApiResult<String> {
    let result = serde_json::to_value(m).map_err(ApiError::internal)?;
    let canonical = audit_engine::canonical_json(&result).map_err(ApiError::internal)?;
    let sha = crate::crypto::sha256(canonical.as_bytes());
    sqlx::query(
        "INSERT INTO match_runs (id, tenant_id, purchase_order, po_line, part_key, rule_version, result, result_sha256, blocking, decision_id, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(&m.line.purchase_order)
    .bind(&m.line.line)
    .bind(&m.line.part)
    .bind(MATCH_RULE_VERSION)
    .bind(&result)
    .bind(sha.to_vec())
    .bind(i32::try_from(m.blocking()).unwrap_or(i32::MAX))
    .bind(decision)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    let uuids = |v: &[String]| v.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect::<Vec<_>>();
    for f in &m.findings {
        sqlx::query(
            "INSERT INTO match_findings (id, tenant_id, run_id, kind, severity, check_name, po_claims, drawing_claims, certificate_claims, certificate_doc, explanation) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(f.kind)
        .bind(f.severity.as_str())
        .bind(&f.check)
        .bind(uuids(&f.po_claims))
        .bind(uuids(&f.drawing_claims))
        .bind(uuids(&f.certificate_claims))
        .bind(f.certificate_doc.as_deref().and_then(|d| Uuid::parse_str(d).ok()))
        .bind(&f.explanation)
        .execute(&mut **tx)
        .await?;
    }
    Ok(hex::encode(sha))
}

/// POST /match/line/runs — records the current evaluation (append-only) for the audit trail.
pub async fn record(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(q): ApiJson<LineQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (facts, docs) = load(&mut tx).await?;
    let l = resolve(&facts, &q)?;
    let m = three_way::evaluate(&l, &facts, &docs);
    let id = Uuid::now_v7();
    let sha = record_run(&mut tx, &p, id, &m, None).await?;
    audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "match.recorded", "match_run", id,
        json!({ "po_line": line_key(&l), "part": l.part, "rule_version": MATCH_RULE_VERSION, "blocking": m.blocking(), "result_sha256": sha }))).await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "run_id": id, "result_sha256": sha, "blocking": m.blocking(), "findings": m.findings.len() }),
    ))
}

#[derive(Deserialize)]
pub struct AcceptReq {
    pub po: String,
    pub line: String,
    pub part: Option<String>,
    pub outcome: String,
    pub rationale: String,
}

/// POST /match/line/accept — the acceptance decision of a PO line. Its dependencies are the cited claims, typed by
/// side: PO claims are `condition`s, drawing and certificate claims are the `basis`. `approved` is refused while
/// critical or major findings are open; `conditional` and `rejected` record the reviewer's judgement over them.
pub async fn accept(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<AcceptReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    if !matches!(req.outcome.as_str(), "approved" | "conditional" | "rejected") {
        return Err(ApiError::BadRequest("invalid_outcome"));
    }
    if req.rationale.trim().is_empty() || req.rationale.chars().count() > 4000 {
        return Err(ApiError::BadRequest("rationale_required"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    // Serialise acceptances per tenant so two reviewers cannot accept on different evidence snapshots at once.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 15))")
        .bind(p.tenant_id.to_string())
        .execute(&mut *tx)
        .await?;
    let (facts, docs) = load(&mut tx).await?;
    let q = LineQuery {
        po: req.po.clone(),
        line: req.line.clone(),
        part: req.part.clone(),
    };
    let l = resolve(&facts, &q)?;
    let m = three_way::evaluate(&l, &facts, &docs);
    if req.outcome == "approved" && m.blocking() > 0 {
        return Err(ApiError::Conflict("match_has_blocking_findings"));
    }
    let mut evidence = Vec::new();
    for (ids, role) in [
        (&m.po_claims, "condition"),
        (&m.drawing_claims, "basis"),
        (&m.certificate_claims, "basis"),
    ] {
        for s in ids {
            let id = Uuid::parse_str(s).map_err(ApiError::internal)?;
            sqlx::query("SELECT 1 FROM claims WHERE id = $1 FOR SHARE")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let c = claims::fetch(&mut tx, id).await?;
            if c.superseded_at.is_some() || c.review_status == "rejected" {
                return Err(ApiError::Conflict("claim_changed_retry"));
            }
            let artifact_sha256 = decisions::artifact_sha(&mut tx, &c).await?;
            evidence.push(Prepared {
                claim: c,
                role: role.into(),
                materiality: Materiality::AnyChange,
                artifact_sha256,
            });
        }
    }
    if evidence.is_empty() {
        return Err(ApiError::BadRequest("evidence_required"));
    }
    if evidence.len() > 200 {
        return Err(ApiError::BadRequest("too_many_dependencies"));
    }
    let run_id = Uuid::now_v7();
    let result = serde_json::to_value(&m).map_err(ApiError::internal)?;
    let result_sha = hex::encode(crate::crypto::sha256(
        audit_engine::canonical_json(&result)
            .map_err(ApiError::internal)?
            .as_bytes(),
    ));
    let header = CreateReq {
        kind: "po_line_acceptance".into(),
        title: format!("Accept {} line {} ({})", l.purchase_order, l.line, l.part)
            .chars()
            .take(200)
            .collect(),
        subject_type: "po_line".into(),
        subject_key: line_key(&l),
        outcome: req.outcome.clone(),
        rationale: req.rationale.trim().to_string(),
        effective_from: None,
        evidence: vec![],
        depends_on: vec![],
    };
    let notes = vec![
        format!("match_run:{run_id}"),
        format!("match_rule:{MATCH_RULE_VERSION}"),
        format!("match_result_sha256:{result_sha}"),
        format!(
            "match_findings:{}",
            m.findings.iter().map(|f| f.kind).collect::<Vec<_>>().join(",")
        ),
    ];
    let did = decisions::insert_decision(&mut tx, &p, &header, None, evidence, vec![], None, notes).await?;
    let sha = record_run(&mut tx, &p, run_id, &m, Some(did)).await?;
    audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "match.accepted", "decision", did,
        json!({ "po_line": line_key(&l), "part": l.part, "outcome": req.outcome, "match_run": run_id, "result_sha256": sha,
                "findings": m.findings.len(), "blocking": m.blocking() }))).await?;
    let out = decisions::fetch(&mut tx, did).await?;
    tx.commit().await?;
    Ok(Json(json!({ "decision": out, "run_id": run_id, "result_sha256": sha })))
}
