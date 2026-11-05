//! Corrective-action cases (8D / 是正処置), ADR 0026. A case opens from a finding (conflict, three-way-match
//! finding, integrity signal, revision drift, SPC out-of-control) and carries that finding's evidence. D1–D8 are
//! structured steps; completing one needs its evidence (`domain_manufacturing::capa::missing`); closure needs a second
//! person; every transition is audited. Supplier 8D responses are evidence of origin "supplier", never approvals.
//! Later evidence that contradicts the effectiveness check (D6) reopens the case (hook in selective invalidation).

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use dependency_engine::Change;
use domain_manufacturing::capa::{
    self, ActionFact, CAPA_VERSION, Case, CaseStatus, Citation, Cited, Content, Discipline, Event, Facts, Origin,
    SpcCheck,
};
use policy_engine::Permission;
use quality_stats::spc::{RuleSet, check_rules, individuals_moving_range};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Date, OffsetDateTime, macros::format_description};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};

/// Fewest points for an SPC chart, and fewest new points before "back in control" can be judged.
pub const MIN_SPC_POINTS: usize = 8;
const MAX_SPC_POINTS: i64 = 1000;

fn micros(t: OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos() / 1000).unwrap_or(i64::MAX)
}

fn parse_date(s: &str) -> ApiResult<Date> {
    Date::parse(s, format_description!("[year]-[month]-[day]")).map_err(|_| ApiError::BadRequest("invalid_date"))
}

fn text_ok(s: &str, max: usize, code: &'static str) -> ApiResult<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > max {
        return Err(ApiError::BadRequest(code));
    }
    Ok(s.to_string())
}

/// Appends to the case history and to the tamper-evident audit chain in one go.
async fn log(
    tx: &mut Tx,
    tenant: Uuid,
    case: Uuid,
    actor: Option<Uuid>,
    kind: &'static str,
    d: Option<Discipline>,
    detail: Value,
) -> ApiResult<()> {
    let actor_kind = if actor.is_some() { "user" } else { "system" };
    sqlx::query(
        "INSERT INTO capa_events (id, tenant_id, case_id, actor_id, actor_kind, kind, discipline, detail) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(case)
    .bind(actor)
    .bind(actor_kind)
    .bind(kind)
    .bind(d.map(|x| i16::from(x.number())))
    .bind(&detail)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE capa_cases SET updated_at = now() WHERE id = $1")
        .bind(case)
        .execute(&mut **tx)
        .await?;
    let mut details = detail;
    if let (Some(obj), Some(d)) = (details.as_object_mut(), d) {
        obj.insert("discipline".into(), json!(d.number()));
    }
    audit::append(
        tx,
        tenant,
        Record {
            actor_user_id: actor,
            actor_kind,
            action: kind,
            target_type: "capa_case",
            target_id: Some(case.to_string()),
            details,
        },
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Loading and persisting the state machine

#[derive(sqlx::FromRow)]
struct CaseRow {
    status: String,
    reopened: i32,
    closure_requested_by: Option<Uuid>,
    created_at: OffsetDateTime,
    source_kind: String,
    source: Value,
    subject_type: Option<String>,
    subject_key: Option<String>,
    supplier_key: Option<String>,
    reference: String,
    title: String,
}

async fn load(tx: &mut Tx, id: Uuid, lock: bool) -> ApiResult<(CaseRow, Case<Uuid>)> {
    let row: CaseRow = sqlx::query_as(&format!(
        "SELECT status, reopened, closure_requested_by, created_at, source_kind, source, subject_type, subject_key, \
         supplier_key, reference, title FROM capa_cases WHERE id = $1{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let done: Vec<i16> = sqlx::query_scalar("SELECT discipline FROM capa_steps WHERE case_id = $1 AND status = 'done'")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    let mut c = Case::new();
    c.status = CaseStatus::parse(&row.status).ok_or_else(|| ApiError::internal("bad case status"))?;
    c.reopened = u32::try_from(row.reopened).unwrap_or(0);
    c.closure_requested_by = row.closure_requested_by;
    for d in done {
        if let Some(i) = usize::try_from(d).ok().filter(|i| (1..=8).contains(i)) {
            c.done[i - 1] = true;
        }
    }
    Ok((row, c))
}

/// Writes the difference between two machine states (status columns and step flags).
async fn persist(tx: &mut Tx, id: Uuid, before: &Case<Uuid>, after: &Case<Uuid>, actor: Option<Uuid>) -> ApiResult<()> {
    for d in Discipline::ALL {
        let (b, a) = (before.is_done(d), after.is_done(d));
        if a != b {
            sqlx::query(
                "UPDATE capa_steps SET status = $3, completed_by = $4, completed_at = CASE WHEN $5 THEN now() END, \
                 updated_at = now() WHERE case_id = $1 AND discipline = $2",
            )
            .bind(id)
            .bind(i16::from(d.number()))
            .bind(if a { "done" } else { "open" })
            .bind(if a { actor } else { None })
            .bind(a)
            .execute(&mut **tx)
            .await?;
        }
    }
    if before.status != after.status || before.reopened != after.reopened {
        sqlx::query(
            "UPDATE capa_cases SET status = $2, reopened = $3, closure_requested_by = $4, \
             closure_requested_at = CASE WHEN $4::uuid IS NULL THEN NULL ELSE coalesce(closure_requested_at, now()) END, \
             closed_by = CASE WHEN $2 = 'closed' THEN $5 END, closed_at = CASE WHEN $2 = 'closed' THEN now() END, \
             updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(after.status.as_str())
        .bind(i32::try_from(after.reopened).unwrap_or(i32::MAX))
        .bind(after.closure_requested_by)
        .bind(actor)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn transition(c: &Case<Uuid>, e: &Event<Uuid>) -> ApiResult<Case<Uuid>> {
    c.apply(e).map_err(|x| ApiError::Conflict(x.code()))
}

// ---------------------------------------------------------------------------------------------------------------
// Evidence facts

async fn step_content(tx: &mut Tx, id: Uuid, d: Discipline) -> ApiResult<Content> {
    let v: Value = sqlx::query_scalar("SELECT content FROM capa_steps WHERE case_id = $1 AND discipline = $2")
        .bind(id)
        .bind(i16::from(d.number()))
        .fetch_one(&mut **tx)
        .await?;
    serde_json::from_value(v).map_err(ApiError::internal)
}

#[derive(sqlx::FromRow)]
struct ActionRow {
    causes: Vec<i32>,
    done_at: Option<OffsetDateTime>,
    evidence: Value,
}

/// Resolves every cited id: when it entered KIZUNA, whether it is still usable, and whether it came from the
/// supplier's 8D response (an uploaded response document or a claim extracted from one).
async fn resolve(tx: &mut Tx, case: Uuid, ids: &BTreeSet<String>) -> ApiResult<BTreeMap<String, Cited>> {
    let uuids: Vec<Uuid> = ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
    let supplier: BTreeSet<Uuid> =
        sqlx::query_scalar::<_, Uuid>("SELECT artifact_id FROM capa_supplier_responses WHERE case_id = $1")
            .bind(case)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .collect();
    let mut out = BTreeMap::new();
    let claims: Vec<(Uuid, OffsetDateTime, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, recorded_at, superseded_by IS NULL AND review_status <> 'rejected', artifact_id \
         FROM claims WHERE id = ANY($1)",
    )
    .bind(&uuids)
    .fetch_all(&mut **tx)
    .await?;
    for (id, at, usable, art) in claims {
        let origin = if art.is_some_and(|a| supplier.contains(&a)) {
            Origin::Supplier
        } else {
            Origin::Internal
        };
        out.insert(
            id.to_string(),
            Cited {
                origin,
                recorded_at: micros(at),
                usable,
            },
        );
    }
    let arts: Vec<(Uuid, OffsetDateTime, bool)> = sqlx::query_as(
        "SELECT id, created_at, status NOT IN ('rejected', 'quarantined') FROM artifacts WHERE id = ANY($1)",
    )
    .bind(&uuids)
    .fetch_all(&mut **tx)
    .await?;
    for (id, at, usable) in arts {
        let origin = if supplier.contains(&id) {
            Origin::Supplier
        } else {
            Origin::Internal
        };
        out.insert(
            id.to_string(),
            Cited {
                origin,
                recorded_at: micros(at),
                usable,
            },
        );
    }
    Ok(out)
}

/// Numeric, current, not-rejected claims of one characteristic in time order (optionally only those recorded after
/// `after`): the SPC series.
async fn spc_series(
    tx: &mut Tx,
    stype: &str,
    skey: &str,
    pred: &str,
    after: Option<OffsetDateTime>,
) -> ApiResult<Vec<(Uuid, f64)>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, value_num::text FROM claims WHERE subject_type = $1 AND subject_key = $2 AND predicate = $3 \
         AND value_num IS NOT NULL AND superseded_by IS NULL AND review_status <> 'rejected' \
         AND ($4::timestamptz IS NULL OR recorded_at > $4) \
         ORDER BY coalesce(valid_from, recorded_at), recorded_at, id LIMIT $5",
    )
    .bind(stype)
    .bind(skey)
    .bind(pred)
    .bind(after)
    .bind(MAX_SPC_POINTS)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, v)| v.parse::<f64>().ok().map(|x| (id, x)))
        .collect())
}

async fn facts(tx: &mut Tx, id: Uuid, row: &CaseRow, contents: &BTreeMap<u8, Content>) -> ApiResult<Facts> {
    let actions: Vec<ActionRow> =
        sqlx::query_as("SELECT causes, done_at, evidence FROM capa_actions WHERE case_id = $1 ORDER BY created_at")
            .bind(id)
            .fetch_all(&mut **tx)
            .await?;
    let mut ids: BTreeSet<String> = contents
        .values()
        .flat_map(Content::citations)
        .filter_map(|c| c.key().map(str::to_string))
        .collect();
    for a in &actions {
        let cs: Vec<Citation> = serde_json::from_value(a.evidence.clone()).unwrap_or_default();
        ids.extend(cs.iter().filter_map(|c| c.key().map(str::to_string)));
    }
    let members: Vec<Uuid> = contents
        .get(&1)
        .map(|c| c.members.iter().filter_map(|m| Uuid::parse_str(&m.user).ok()).collect())
        .unwrap_or_default();
    let tenant_users: BTreeSet<String> =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id = ANY($1) AND status = 'active'")
            .bind(&members)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|u| u.to_string())
            .collect();
    let last_done = actions.iter().filter_map(|a| a.done_at).max();
    let spc = if row.source_kind == "spc" {
        Some(spc_recheck(tx, row, last_done).await?)
    } else {
        None
    };
    Ok(Facts {
        cited: resolve(tx, id, &ids).await?,
        tenant_users,
        causes: u32::try_from(contents.get(&4).map_or(0, |c| c.items.len())).unwrap_or(u32::MAX),
        actions: actions
            .iter()
            .map(|a| ActionFact {
                causes: a.causes.iter().filter_map(|c| u32::try_from(*c).ok()).collect(),
                done_at: a.done_at.map(micros),
            })
            .collect(),
        spc,
    })
}

fn spc_limits(row: &CaseRow) -> Option<(f64, f64, String, String, String)> {
    let s = &row.source;
    Some((
        s["center"].as_f64()?,
        s["sigma"].as_f64()?,
        row.subject_type.clone()?,
        row.subject_key.clone()?,
        s["predicate"].as_str()?.to_string(),
    ))
}

/// Is the characteristic back in control against the limits recorded when the case opened, using only points
/// recorded after the last corrective action was implemented?
async fn spc_recheck(tx: &mut Tx, row: &CaseRow, since: Option<OffsetDateTime>) -> ApiResult<SpcCheck> {
    let (Some((center, sigma, st, sk, pred)), Some(since)) = (spc_limits(row), since) else {
        return Ok(SpcCheck::TooFewPoints);
    };
    let pts: Vec<f64> = spc_series(tx, &st, &sk, &pred, Some(since))
        .await?
        .into_iter()
        .map(|p| p.1)
        .collect();
    if pts.len() < MIN_SPC_POINTS {
        return Ok(SpcCheck::TooFewPoints);
    }
    Ok(match check_rules(&pts, center, sigma, RuleSet::WesternElectric) {
        Ok(v) if v.is_empty() => SpcCheck::InControl,
        _ => SpcCheck::OutOfControl,
    })
}

async fn all_contents(tx: &mut Tx, id: Uuid) -> ApiResult<BTreeMap<u8, Content>> {
    let rows: Vec<(i16, Value)> = sqlx::query_as("SELECT discipline, content FROM capa_steps WHERE case_id = $1")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|(d, v)| {
            Ok((
                u8::try_from(d).map_err(ApiError::internal)?,
                serde_json::from_value(v).map_err(ApiError::internal)?,
            ))
        })
        .collect()
}

type MatchRow = (
    String,
    String,
    String,
    Vec<Uuid>,
    Vec<Uuid>,
    Vec<Uuid>,
    Option<Uuid>,
    Value,
);
type ClaimInfoRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Value,
);
type StepRow = (
    i16,
    String,
    Option<Uuid>,
    Option<OffsetDateTime>,
    Option<Uuid>,
    OffsetDateTime,
);
type EventRow = (OffsetDateTime, Option<Uuid>, String, String, Option<i16>, Value);

// ---------------------------------------------------------------------------------------------------------------
// Opening a case from a finding

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpcSource {
    pub subject_type: String,
    pub subject_key: String,
    pub predicate: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenReq {
    pub source_kind: String,
    pub source_id: Option<Uuid>,
    pub spc: Option<SpcSource>,
    pub title: Option<String>,
    pub supplier: Option<String>,
}

struct Opened {
    source_ref: String,
    source: Value,
    links: Vec<Citation>,
    subject: Option<(String, String)>,
    supplier: Option<String>,
    title: String,
}

fn claim_cite(id: Uuid) -> Citation {
    Citation {
        claim: Some(id.to_string()),
        artifact: None,
    }
}
fn artifact_cite(id: Uuid) -> Citation {
    Citation {
        claim: None,
        artifact: Some(id.to_string()),
    }
}

fn cites_in(v: &Value, out: &mut Vec<Citation>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match (k.as_str(), x.as_str().and_then(|s| Uuid::parse_str(s).ok())) {
                    ("claim", Some(u)) => out.push(claim_cite(u)),
                    ("artifact", Some(u)) => out.push(artifact_cite(u)),
                    _ => cites_in(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| cites_in(x, out)),
        _ => {}
    }
}

async fn first_supplier(tx: &mut Tx, links: &[Citation]) -> ApiResult<Option<String>> {
    let ids: Vec<Uuid> = links
        .iter()
        .filter_map(|c| c.claim.as_deref().and_then(|s| Uuid::parse_str(s).ok()))
        .collect();
    Ok(sqlx::query_scalar(
        "SELECT scope->>'supplier' FROM claims WHERE id = ANY($1) AND scope ? 'supplier' ORDER BY recorded_at LIMIT 1",
    )
    .bind(&ids)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn from_finding(tx: &mut Tx, kind: &str, id: Uuid) -> ApiResult<Opened> {
    let mut links = Vec::new();
    let (source, subject, title, supplier): (Value, Option<(String, String)>, String, Option<String>) = match kind {
        "conflict" => {
            let (k, st, sk, a, b, expl): (String, String, String, Uuid, Option<Uuid>, Value) = sqlx::query_as(
                "SELECT kind, subject_type, subject_key, claim_a, claim_b, explanation FROM conflicts WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
            links.push(claim_cite(a));
            links.extend(b.map(claim_cite));
            let title = format!("{k}: {sk}");
            (json!({ "kind": k, "explanation": expl }), Some((st, sk)), title, None)
        }
        "match_finding" => {
            let (k, sev, check, po, dr, ce, doc, expl): MatchRow = sqlx::query_as(
                "SELECT kind, severity, check_name, po_claims, drawing_claims, certificate_claims, certificate_doc, \
                 explanation FROM match_findings WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
            links.extend(po.into_iter().chain(dr).chain(ce).map(claim_cite));
            links.extend(doc.map(artifact_cite));
            let subject: Option<(String, String)> = match links.first().and_then(|c| c.claim.clone()) {
                Some(c) => {
                    sqlx::query_as("SELECT subject_type, subject_key FROM claims WHERE id = $1::uuid")
                        .bind(c)
                        .fetch_optional(&mut **tx)
                        .await?
                }
                None => None,
            };
            let title = format!("{check} ({sev})");
            (
                json!({ "kind": k, "severity": sev, "check": check, "explanation": expl }),
                subject,
                title,
                None,
            )
        }
        "integrity_signal" => {
            let (k, sup, part, ch, expl, ev, arts): (
                String,
                String,
                Option<String>,
                Option<String>,
                Value,
                Value,
                Vec<Uuid>,
            ) = sqlx::query_as(
                "SELECT kind, supplier_key, part_key, characteristic, explanation, evidence, artifact_ids \
                     FROM integrity_signals WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
            cites_in(&ev, &mut links);
            links.extend(arts.into_iter().map(artifact_cite));
            let title = format!(
                "{k}: {sup}{}",
                part.as_deref().map(|p| format!(" / {p}")).unwrap_or_default()
            );
            (
                json!({ "kind": k, "characteristic": ch, "explanation": expl }),
                part.map(|p| ("part".to_string(), p)),
                title,
                Some(sup),
            )
        }
        "drift_finding" => {
            let (k, dk, pk, ea, la, changes, expl): (String, String, String, Uuid, Uuid, Value, Value) =
                sqlx::query_as(
                    "SELECT kind, drawing_key, part_key, earlier_artifact, later_artifact, changes, explanation \
                 FROM revision_drift_findings WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or(ApiError::NotFound)?;
            links.push(artifact_cite(ea));
            links.push(artifact_cite(la));
            cites_in(&changes, &mut links);
            (
                json!({ "kind": k, "drawing": dk, "explanation": expl }),
                Some(("part".into(), pk.clone())),
                format!("{k}: {dk} / {pk}"),
                None,
            )
        }
        _ => return Err(ApiError::BadRequest("invalid_source_kind")),
    };
    let mut seen = BTreeSet::new();
    links.retain(|c| seen.insert(c.clone()));
    let supplier = match supplier {
        Some(s) => Some(s),
        None => first_supplier(tx, &links).await?,
    };
    Ok(Opened {
        source_ref: id.to_string(),
        source,
        links,
        subject,
        supplier,
        title,
    })
}

/// An SPC out-of-control finding: I-MR chart over the characteristic's series, Western Electric rules. Refused when
/// the process is in control (there is nothing to correct).
async fn from_spc(tx: &mut Tx, s: &SpcSource) -> ApiResult<Opened> {
    if s.subject_type.is_empty() || s.subject_key.is_empty() || s.predicate.is_empty() || s.subject_key.len() > 200 {
        return Err(ApiError::BadRequest("invalid_spc_source"));
    }
    let pts = spc_series(tx, &s.subject_type, &s.subject_key, &s.predicate, None).await?;
    if pts.len() < MIN_SPC_POINTS {
        return Err(ApiError::Conflict("spc_too_few_points"));
    }
    let xs: Vec<f64> = pts.iter().map(|p| p.1).collect();
    let chart = individuals_moving_range(&xs).map_err(|_| ApiError::Conflict("spc_no_variation"))?;
    let (center, sigma) = (chart.individuals.center, chart.sigma_within);
    let v = check_rules(&xs, center, sigma, RuleSet::WesternElectric)
        .map_err(|_| ApiError::Conflict("spc_no_variation"))?;
    if v.is_empty() {
        return Err(ApiError::Conflict("spc_in_control"));
    }
    // Reference limits for the later effectiveness check: recomputed without the points beyond 3σ (the special
    // causes this case is about), so they describe the process as it should run.
    let special: BTreeSet<usize> = v.iter().filter(|x| x.rule == "WE1").map(|x| x.index).collect();
    let clean: Vec<f64> = xs
        .iter()
        .enumerate()
        .filter(|(i, _)| !special.contains(i))
        .map(|(_, x)| *x)
        .collect();
    let (center, sigma) = match individuals_moving_range(&clean) {
        Ok(r) if clean.len() >= MIN_SPC_POINTS && r.sigma_within > 0.0 => (r.individuals.center, r.sigma_within),
        _ => (center, sigma),
    };
    let mut links: Vec<Citation> = v
        .iter()
        .filter_map(|x| pts.get(x.index))
        .map(|p| claim_cite(p.0))
        .collect();
    let mut seen = BTreeSet::new();
    links.retain(|c| seen.insert(c.clone()));
    let violations: Vec<Value> = v
        .iter()
        .map(|x| json!({ "index": x.index, "rule": x.rule, "claim": pts.get(x.index).map(|p| p.0) }))
        .collect();
    let supplier = first_supplier(tx, &links).await?;
    Ok(Opened {
        source_ref: format!("{}|{}|{}", s.subject_type, s.subject_key, s.predicate),
        source: json!({ "chart": "i_mr", "rules": "western_electric", "predicate": s.predicate, "center": center,
                        "sigma": sigma, "ucl": center + 3.0 * sigma, "lcl": center - 3.0 * sigma, "points": xs.len(),
                        "violations": violations }),
        links,
        subject: Some((s.subject_type.clone(), s.subject_key.clone())),
        supplier,
        title: format!(
            "SPC {} {}: {}",
            s.subject_key,
            s.predicate,
            v.first().map_or("", |x| x.rule)
        ),
    })
}

pub async fn open(State(st): State<AppState>, p: Principal, ApiJson(r): ApiJson<OpenReq>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let o = match (r.source_kind.as_str(), r.source_id, &r.spc) {
        ("spc", None, Some(s)) => from_spc(&mut tx, s).await?,
        ("spc", _, _) => return Err(ApiError::BadRequest("spc_source_required")),
        (k, Some(id), None) => from_finding(&mut tx, k, id).await?,
        _ => return Err(ApiError::BadRequest("source_id_required")),
    };
    let title = match r.title.as_deref() {
        Some(t) => text_ok(t, 200, "invalid_title")?,
        None => o.title.chars().take(200).collect(),
    };
    let supplier = match r.supplier.as_deref() {
        Some(s) => Some(text_ok(s, 200, "invalid_supplier")?),
        None => o.supplier,
    };
    let dup: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM capa_cases WHERE source_kind = $1 AND source_ref = $2 AND status <> 'closed'",
    )
    .bind(&r.source_kind)
    .bind(&o.source_ref)
    .fetch_optional(&mut *tx)
    .await?;
    if dup.is_some() {
        return Err(ApiError::Conflict("case_already_open"));
    }
    let id = Uuid::now_v7();
    let reference = format!("CA-{}", &id.simple().to_string()[24..]).to_uppercase();
    sqlx::query(
        "INSERT INTO capa_cases (id, tenant_id, reference, title, source_kind, source_ref, source, source_links, \
         subject_type, subject_key, supplier_key, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(&reference)
    .bind(&title)
    .bind(&r.source_kind)
    .bind(&o.source_ref)
    .bind(&o.source)
    .bind(json!(o.links))
    .bind(o.subject.as_ref().map(|s| &s.0))
    .bind(o.subject.as_ref().map(|s| &s.1))
    .bind(&supplier)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for d in Discipline::ALL {
        // D2 starts with the finding's evidence cited, so the problem description is anchored to it.
        let content = if d == Discipline::PROBLEM {
            json!({ "evidence": o.links })
        } else {
            json!({})
        };
        sqlx::query("INSERT INTO capa_steps (tenant_id, case_id, discipline, content) VALUES ($1,$2,$3,$4)")
            .bind(p.tenant_id)
            .bind(id)
            .bind(i16::from(d.number()))
            .bind(content)
            .execute(&mut *tx)
            .await?;
    }
    log(&mut tx, p.tenant_id, id, Some(p.user_id), "capa.opened", None,
        json!({ "reference": reference, "source_kind": r.source_kind, "source_ref": o.source_ref, "links": o.links.len(),
                "engine": CAPA_VERSION })).await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------------------------
// Reading

#[derive(Serialize, sqlx::FromRow)]
pub struct CaseOut {
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub source_kind: String,
    pub source_ref: String,
    pub subject_type: Option<String>,
    pub subject_key: Option<String>,
    pub supplier_key: Option<String>,
    pub status: String,
    pub reopened: i32,
    pub closure_requested_by: Option<Uuid>,
    pub closed_by: Option<Uuid>,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Highest completed discipline (0 = none).
    pub progress: i32,
    pub overdue_actions: i64,
}

const CASE_COLS: &str = "c.id, c.reference, c.title, c.source_kind, c.source_ref, c.subject_type, c.subject_key, \
    c.supplier_key, c.status, c.reopened, c.closure_requested_by, c.closed_by, c.created_by, c.created_at, c.updated_at, \
    (SELECT coalesce(max(discipline), 0)::int FROM capa_steps s WHERE s.case_id = c.id AND s.status = 'done') AS progress, \
    (SELECT count(*) FROM capa_actions a WHERE a.case_id = c.id AND a.status = 'open' AND a.due_date < $1) AS overdue_actions";

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<CaseOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, CaseOut>(&format!(
        "SELECT {CASE_COLS} FROM capa_cases c WHERE ($2::text IS NULL OR c.status = $2) ORDER BY c.updated_at DESC LIMIT 500"
    ))
    .bind(st.clock.now().date())
    .bind(q.status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
struct ActionOut {
    id: Uuid,
    description: String,
    owner_id: Uuid,
    owner_name: Option<String>,
    due_date: Date,
    causes: Vec<i32>,
    status: String,
    evidence: Value,
    done_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    done_at: Option<OffsetDateTime>,
    done_comment: Option<String>,
    overdue: bool,
}

#[derive(Serialize, sqlx::FromRow)]
struct RequestOut {
    id: Uuid,
    reference: String,
    supplier: String,
    disciplines: Vec<i16>,
    due_date: Date,
    status: String,
    created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    reviewed_by: Option<Uuid>,
    review_comment: Option<String>,
}

/// Display data for every id the case refers to (claims and documents), so the UI can show what is cited.
async fn evidence_index(tx: &mut Tx, case: Uuid, ids: &BTreeSet<String>) -> ApiResult<Value> {
    let cited = resolve(tx, case, ids).await?;
    let uuids: Vec<Uuid> = ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
    let claims: Vec<ClaimInfoRow> =
        sqlx::query_as(
            "SELECT c.id, c.subject_key, c.predicate, coalesce(c.value_num::text, c.value_text), c.value_unit, \
             c.review_status, a.display_name, c.source_location FROM claims c LEFT JOIN artifacts a ON a.id = c.artifact_id \
             WHERE c.id = ANY($1)",
        )
        .bind(&uuids)
        .fetch_all(&mut **tx)
        .await?;
    let arts: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, display_name, kind FROM artifacts WHERE id = ANY($1)")
            .bind(&uuids)
            .fetch_all(&mut **tx)
            .await?;
    let mut m = serde_json::Map::new();
    let info = |id: &Uuid| cited.get(&id.to_string());
    for (id, sk, pred, val, unit, rs, doc, loc) in claims {
        let c = info(&id);
        m.insert(
            id.to_string(),
            json!({ "type": "claim", "subject": sk, "predicate": pred, "value": val, "unit": unit,
            "review_status": rs, "document": doc, "location": loc, "usable": c.is_some_and(|x| x.usable),
            "origin": c.map(|x| x.origin) }),
        );
    }
    for (id, name, kind) in arts {
        let c = info(&id);
        m.insert(
            id.to_string(),
            json!({ "type": "artifact", "document": name, "kind": kind,
            "usable": c.is_some_and(|x| x.usable), "origin": c.map(|x| x.origin) }),
        );
    }
    Ok(Value::Object(m))
}

async fn detail_at(tx: &mut Tx, id: Uuid, today: Date) -> ApiResult<Value> {
    let case = sqlx::query_as::<_, CaseOut>(&format!("SELECT {CASE_COLS} FROM capa_cases c WHERE c.id = $2"))
        .bind(today)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let (row, machine) = load(tx, id, false).await?;
    let source_links: Value = sqlx::query_scalar("SELECT source_links FROM capa_cases WHERE id = $1")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    let contents = all_contents(tx, id).await?;
    let f = facts(tx, id, &row, &contents).await?;
    let steps: Vec<StepRow> = sqlx::query_as(
        "SELECT discipline, status, completed_by, completed_at, updated_by, updated_at FROM capa_steps WHERE case_id = $1 \
         ORDER BY discipline",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let steps: Vec<Value> = steps
        .into_iter()
        .filter_map(|(d, status, by, at, ub, ua)| {
            let disc = Discipline::new(u8::try_from(d).ok()?)?;
            let content = contents.get(&disc.number()).cloned().unwrap_or_default();
            let missing = if machine.is_done(disc) { vec![] } else { capa::missing(disc, &content, &f) };
            Some(json!({ "discipline": d, "code": disc.code(), "status": status, "content": content, "missing": missing,
                "completed_by": by, "completed_at": at.map(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
                "updated_by": ub, "updated_at": ua.format(&time::format_description::well_known::Rfc3339).ok() }))
        })
        .collect();
    let actions = sqlx::query_as::<_, ActionOut>(
        "SELECT a.id, a.description, a.owner_id, u.display_name AS owner_name, a.due_date, a.causes, a.status, a.evidence, \
         a.done_by, a.done_at, a.done_comment, (a.status = 'open' AND a.due_date < $2) AS overdue \
         FROM capa_actions a LEFT JOIN users u ON u.id = a.owner_id WHERE a.case_id = $1 ORDER BY a.created_at",
    )
    .bind(id)
    .bind(today)
    .fetch_all(&mut **tx)
    .await?;
    let requests = sqlx::query_as::<_, RequestOut>(
        "SELECT id, reference, supplier, disciplines, due_date, status, created_by, created_at, reviewed_by, review_comment \
         FROM capa_supplier_requests WHERE case_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let responses: Vec<(Uuid, Uuid, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT r.request_id, r.artifact_id, a.display_name, r.added_at FROM capa_supplier_responses r \
         JOIN artifacts a ON a.id = r.artifact_id WHERE r.case_id = $1 ORDER BY r.added_at",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let events: Vec<EventRow> = sqlx::query_as(
        "SELECT at, actor_id, actor_kind, kind, discipline, detail FROM capa_events WHERE case_id = $1 ORDER BY at, id",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    // Ids for display: source links, step and action citations, supplier responses, and candidates on the subject.
    let mut ids: BTreeSet<String> = serde_json::from_value::<Vec<Citation>>(source_links.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.key().map(str::to_string))
        .collect();
    ids.extend(
        contents
            .values()
            .flat_map(Content::citations)
            .filter_map(|c| c.key().map(str::to_string)),
    );
    for a in &actions {
        let cs: Vec<Citation> = serde_json::from_value(a.evidence.clone()).unwrap_or_default();
        ids.extend(cs.iter().filter_map(|c| c.key().map(str::to_string)));
    }
    ids.extend(responses.iter().map(|r| r.1.to_string()));
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM claims WHERE subject_type = $1 AND subject_key = $2 AND superseded_by IS NULL \
         ORDER BY recorded_at DESC LIMIT 100",
    )
    .bind(&row.subject_type)
    .bind(&row.subject_key)
    .fetch_all(&mut **tx)
    .await?;
    ids.extend(candidates.iter().map(Uuid::to_string));
    let evidence = evidence_index(tx, id, &ids).await?;
    let team: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, display_name, role FROM users WHERE status = 'active' ORDER BY display_name LIMIT 500",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({
        "case": case,
        "source": row.source,
        "source_links": source_links,
        "steps": steps,
        "actions": actions,
        "supplier_requests": requests,
        "supplier_responses": responses.iter().map(|r| json!({ "request": r.0, "artifact": r.1, "document": r.2,
            "added_at": r.3.format(&time::format_description::well_known::Rfc3339).ok() })).collect::<Vec<_>>(),
        "events": events.iter().map(|e| json!({ "at": e.0.format(&time::format_description::well_known::Rfc3339).ok(),
            "actor": e.1, "actor_kind": e.2, "kind": e.3, "discipline": e.4, "detail": e.5 })).collect::<Vec<_>>(),
        "evidence": evidence,
        "candidates": candidates,
        "users": team.iter().map(|u| json!({ "id": u.0, "name": u.1, "role": u.2 })).collect::<Vec<_>>(),
        "spc_recheck": f.spc.map(|s| format!("{s:?}").to_lowercase()),
    }))
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------------------------
// Steps

fn discipline(n: u8) -> ApiResult<Discipline> {
    Discipline::new(n).ok_or(ApiError::BadRequest("invalid_discipline"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveReq {
    pub content: Value,
}

/// PUT /capa/cases/{id}/steps/{d}: saves the draft content of an open step of an open case.
pub async fn save_step(
    State(st): State<AppState>,
    p: Principal,
    Path((id, n)): Path<(Uuid, u8)>,
    ApiJson(r): ApiJson<SaveReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let d = discipline(n)?;
    let content: Content = serde_json::from_value(r.content).map_err(|_| ApiError::BadRequest("invalid_content"))?;
    if let Some(code) = capa::malformed(&content) {
        return Err(ApiError::BadRequest(code));
    }
    if content
        .citations()
        .any(|c| c.key().is_some_and(|k| Uuid::parse_str(k).is_err()))
        || content.members.iter().any(|m| Uuid::parse_str(&m.user).is_err())
    {
        return Err(ApiError::BadRequest("invalid_citation"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (_, c) = load(&mut tx, id, true).await?;
    if c.status != CaseStatus::Open {
        return Err(ApiError::Conflict("case_not_open"));
    }
    if c.is_done(d) {
        return Err(ApiError::Conflict("step_already_done"));
    }
    sqlx::query(
        "UPDATE capa_steps SET content = $3, updated_by = $4, updated_at = now() WHERE case_id = $1 AND discipline = $2",
    )
    .bind(id)
    .bind(i16::from(d.number()))
    .bind(json!(content))
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.step_saved",
        Some(d),
        json!({ "citations": content.citations().count() }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// POST /capa/cases/{id}/steps/{d}/complete: refused (409 step_requirements_unmet) while evidence is missing.
pub async fn complete_step(
    State(st): State<AppState>,
    p: Principal,
    Path((id, n)): Path<(Uuid, u8)>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let d = discipline(n)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (row, c) = load(&mut tx, id, true).await?;
    let next = transition(&c, &Event::Complete(d))?;
    let contents = all_contents(&mut tx, id).await?;
    let f = facts(&mut tx, id, &row, &contents).await?;
    let missing = capa::missing(d, &contents.get(&d.number()).cloned().unwrap_or_default(), &f);
    if !missing.is_empty() {
        return Err(ApiError::Conflict("step_requirements_unmet"));
    }
    persist(&mut tx, id, &c, &next, Some(p.user_id)).await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.step_completed",
        Some(d),
        json!({}),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentReq {
    pub comment: Option<String>,
}

/// POST /capa/cases/{id}/steps/{d}/reopen: reopens a completed step and every later one (reason required).
pub async fn reopen_step(
    State(st): State<AppState>,
    p: Principal,
    Path((id, n)): Path<(Uuid, u8)>,
    ApiJson(r): ApiJson<CommentReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let d = discipline(n)?;
    let comment = text_ok(r.comment.as_deref().unwrap_or(""), 2000, "comment_required")?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (_, c) = load(&mut tx, id, true).await?;
    let next = transition(&c, &Event::ReopenStep(d))?;
    persist(&mut tx, id, &c, &next, Some(p.user_id)).await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.step_reopened",
        Some(d),
        json!({ "comment": comment }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------------------------
// Corrective actions (D5)

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReq {
    pub description: String,
    pub owner: Uuid,
    pub due_date: String,
    pub causes: Vec<u32>,
}

pub async fn add_action(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<ActionReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let description = text_ok(&r.description, 2000, "description_required")?;
    let due = parse_date(&r.due_date)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (row, c) = load(&mut tx, id, true).await?;
    if c.status != CaseStatus::Open {
        return Err(ApiError::Conflict("case_not_open"));
    }
    if !c.is_done(Discipline::ROOT_CAUSE) || c.is_done(Discipline::ACTIONS) {
        return Err(ApiError::Conflict("actions_need_root_cause_and_open_d5"));
    }
    if due < row.created_at.date() {
        return Err(ApiError::BadRequest("due_date_before_case"));
    }
    let causes = step_content(&mut tx, id, Discipline::ROOT_CAUSE).await?.items.len();
    let mut idx: Vec<i32> = Vec::new();
    for x in &r.causes {
        if usize::try_from(*x).map_or(true, |i| i >= causes) {
            return Err(ApiError::BadRequest("invalid_cause"));
        }
        idx.push(i32::try_from(*x).map_err(ApiError::internal)?);
    }
    idx.sort_unstable();
    idx.dedup();
    if idx.is_empty() {
        return Err(ApiError::BadRequest("invalid_cause"));
    }
    let owner_ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND status = 'active')")
        .bind(r.owner)
        .fetch_one(&mut *tx)
        .await?;
    if !owner_ok {
        return Err(ApiError::BadRequest("unknown_owner"));
    }
    let aid = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO capa_actions (id, tenant_id, case_id, description, owner_id, due_date, causes, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(aid)
    .bind(p.tenant_id)
    .bind(id)
    .bind(&description)
    .bind(r.owner)
    .bind(due)
    .bind(&idx)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.action_added",
        Some(Discipline::ACTIONS),
        json!({ "action": aid, "owner": r.owner, "due_date": r.due_date, "causes": idx }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDoneReq {
    pub evidence: Vec<Citation>,
    pub comment: Option<String>,
}

/// POST /capa/actions/{id}/done: implementation recorded with the evidence that shows it (after D5 is decided).
pub async fn action_done(
    State(st): State<AppState>,
    p: Principal,
    Path(aid): Path<Uuid>,
    ApiJson(r): ApiJson<ActionDoneReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    if r.evidence.is_empty() || r.evidence.len() > 50 {
        return Err(ApiError::BadRequest("evidence_required"));
    }
    if r.evidence
        .iter()
        .any(|c| c.key().is_none_or(|k| Uuid::parse_str(k).is_err()))
    {
        return Err(ApiError::BadRequest("invalid_citation"));
    }
    let comment = match r.comment.as_deref().map(str::trim) {
        Some(c) if c.chars().count() > 2000 => return Err(ApiError::BadRequest("comment_too_long")),
        Some(c) if !c.is_empty() => Some(c.to_string()),
        _ => None,
    };
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (case_id, status): (Uuid, String) =
        sqlx::query_as("SELECT case_id, status FROM capa_actions WHERE id = $1 FOR UPDATE")
            .bind(aid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let (_, c) = load(&mut tx, case_id, true).await?;
    if c.status != CaseStatus::Open {
        return Err(ApiError::Conflict("case_not_open"));
    }
    if !c.is_done(Discipline::ACTIONS) {
        return Err(ApiError::Conflict("actions_not_decided"));
    }
    if status != "open" {
        return Err(ApiError::Conflict("action_already_done"));
    }
    let keys: BTreeSet<String> = r.evidence.iter().filter_map(|c| c.key().map(str::to_string)).collect();
    let cited = resolve(&mut tx, case_id, &keys).await?;
    if keys.iter().any(|k| !cited.get(k).is_some_and(|x| x.usable)) {
        return Err(ApiError::Conflict("evidence_not_usable"));
    }
    sqlx::query(
        "UPDATE capa_actions SET status = 'done', evidence = $2, done_by = $3, done_at = now(), done_comment = $4 WHERE id = $1",
    )
    .bind(aid)
    .bind(json!(r.evidence))
    .bind(p.user_id)
    .bind(&comment)
    .execute(&mut *tx)
    .await?;
    log(
        &mut tx,
        p.tenant_id,
        case_id,
        Some(p.user_id),
        "capa.action_done",
        Some(Discipline::ACTIONS),
        json!({ "action": aid, "citations": r.evidence.len() }),
    )
    .await?;
    let out = detail_at(&mut tx, case_id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------------------------
// Closure (D8, four-eyes)

pub async fn request_closure(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (row, c) = load(&mut tx, id, true).await?;
    let next = transition(&c, &Event::RequestClosure { by: p.user_id })?;
    let contents = all_contents(&mut tx, id).await?;
    let f = facts(&mut tx, id, &row, &contents).await?;
    if !capa::missing(Discipline::CLOSURE, &contents.get(&8).cloned().unwrap_or_default(), &f).is_empty() {
        return Err(ApiError::Conflict("step_requirements_unmet"));
    }
    persist(&mut tx, id, &c, &next, Some(p.user_id)).await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.closure_requested",
        Some(Discipline::CLOSURE),
        json!({}),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn decide_closure(
    st: AppState,
    p: Principal,
    id: Uuid,
    comment: Option<String>,
    approve: bool,
) -> ApiResult<Json<Value>> {
    // Closing a corrective action is a review decision (privileged: MFA-verified session).
    p.require(&st, Permission::ReviewDecide).await?;
    let comment = match comment.as_deref().map(str::trim) {
        Some(c) if c.chars().count() > 2000 => return Err(ApiError::BadRequest("comment_too_long")),
        Some(c) if !c.is_empty() => Some(c.to_string()),
        _ if !approve => return Err(ApiError::BadRequest("comment_required")),
        _ => None,
    };
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (_, c) = load(&mut tx, id, true).await?;
    let e = if approve {
        Event::ApproveClosure { by: p.user_id }
    } else {
        Event::RejectClosure
    };
    let next = c.apply(&e).map_err(|x| match x {
        capa::TransitionError::OwnClosure => ApiError::Forbidden("own_closure_request"),
        other => ApiError::Conflict(other.code()),
    })?;
    persist(&mut tx, id, &c, &next, Some(p.user_id)).await?;
    let kind = if approve {
        "capa.closed"
    } else {
        "capa.closure_rejected"
    };
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        kind,
        Some(Discipline::CLOSURE),
        json!({ "comment": comment, "requested_by": c.closure_requested_by }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

pub async fn approve_closure(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<CommentReq>,
) -> ApiResult<Json<Value>> {
    decide_closure(st, p, id, r.comment, true).await
}

pub async fn reject_closure(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<CommentReq>,
) -> ApiResult<Json<Value>> {
    decide_closure(st, p, id, r.comment, false).await
}

// ---------------------------------------------------------------------------------------------------------------
// Supplier involvement

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplierReq {
    pub due_date: String,
    pub disciplines: Option<Vec<u8>>,
    pub supplier: Option<String>,
}

fn letter(reference: &str, supplier: &str, title: &str, problem: &str, ds: &[u8], due: &str) -> (String, String) {
    const JA: [&str; 8] = [
        "チーム編成",
        "問題の記述",
        "暫定処置",
        "根本原因（発生・流出）",
        "恒久対策",
        "効果確認",
        "再発防止・水平展開",
        "完了",
    ];
    const EN: [&str; 8] = [
        "Team",
        "Problem description",
        "Containment",
        "Root cause (occurrence and escape)",
        "Permanent corrective actions",
        "Verification of effectiveness",
        "Prevention and horizontal deployment",
        "Closure",
    ];
    let pick = |names: &[&str; 8]| {
        ds.iter()
            .filter_map(|d| {
                names
                    .get(usize::from(*d).wrapping_sub(1))
                    .map(|n| format!("  - D{d} {n}"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let ja = format!(
        "{supplier} 御中\n\n件名: 是正処置報告書（8D）ご提出のお願い（{reference}）\n\n下記の不具合について、是正処置報告書のご提出をお願いいたします。\n\n\
         件名: {title}\n問題の記述:\n{problem}\n\nご回答いただきたい項目:\n{}\n\n回答期限: {due}\n\n\
         根本原因には裏付けとなる記録（検査成績書、工程記録等）を添付してください。\nご提出いただいた資料は証拠として記録され、\
         当社の確認・承認をもって完了となります。\n",
        pick(&JA)
    );
    let en = format!(
        "To: {supplier}\n\nSubject: Request for an 8D corrective-action report ({reference})\n\nPlease submit an 8D report for the \
         problem below.\n\nCase: {title}\nProblem description:\n{problem}\n\nPlease answer:\n{}\n\nDue date: {due}\n\n\
         Attach the records that support each root cause (inspection certificates, process records).\nYour documents are \
         recorded as evidence; the case is closed only after our own review and approval.\n",
        pick(&EN)
    );
    (ja, en)
}

/// POST /capa/cases/{id}/supplier-requests: drafts a bilingual request for the supplier's 8D response.
pub async fn create_supplier_request(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<SupplierReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let due = parse_date(&r.due_date)?;
    let mut ds = r.disciplines.unwrap_or_else(|| vec![3, 4, 5]);
    ds.sort_unstable();
    ds.dedup();
    if ds.is_empty() || ds.iter().any(|d| !(3..=7).contains(d)) {
        return Err(ApiError::BadRequest("invalid_discipline"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (row, c) = load(&mut tx, id, true).await?;
    if c.status != CaseStatus::Open {
        return Err(ApiError::Conflict("case_not_open"));
    }
    if !c.is_done(Discipline::PROBLEM) {
        return Err(ApiError::Conflict("problem_not_described"));
    }
    let supplier = match r.supplier.as_deref() {
        Some(s) => text_ok(s, 200, "invalid_supplier")?,
        None => row
            .supplier_key
            .clone()
            .ok_or(ApiError::BadRequest("supplier_required"))?,
    };
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM capa_supplier_requests WHERE case_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let reference = format!("{}-S{}", row.reference, n + 1);
    let problem = step_content(&mut tx, id, Discipline::PROBLEM).await?.statement;
    let (ja, en) = letter(&reference, &supplier, &row.title, &problem, &ds, &r.due_date);
    let sha = Sha256::digest(format!("{ja}\u{0}{en}").as_bytes());
    let rid = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO capa_supplier_requests (id, tenant_id, case_id, reference, supplier, disciplines, due_date, body_ja, \
         body_en, content_sha256, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(rid)
    .bind(p.tenant_id)
    .bind(id)
    .bind(&reference)
    .bind(&supplier)
    .bind(ds.iter().map(|d| i16::from(*d)).collect::<Vec<_>>())
    .bind(due)
    .bind(&ja)
    .bind(&en)
    .bind(sha.to_vec())
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.supplier_request_drafted",
        None,
        json!({ "request": rid, "reference": reference, "disciplines": ds, "content_sha256": hex::encode(sha) }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn review_supplier_request(
    st: AppState,
    p: Principal,
    rid: Uuid,
    comment: Option<String>,
    to: &'static str,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let comment = comment.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if comment.as_deref().is_some_and(|c| c.chars().count() > 2000) {
        return Err(ApiError::BadRequest("comment_too_long"));
    }
    if to == "cancelled" && comment.is_none() {
        return Err(ApiError::BadRequest("comment_required"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (case_id, status, creator): (Uuid, String, Uuid) =
        sqlx::query_as("SELECT case_id, status, created_by FROM capa_supplier_requests WHERE id = $1 FOR UPDATE")
            .bind(rid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if status != "draft" {
        return Err(ApiError::Conflict("request_not_draft"));
    }
    if creator == p.user_id {
        return Err(ApiError::Forbidden("own_request"));
    }
    sqlx::query(
        "UPDATE capa_supplier_requests SET status = $2, reviewed_by = $3, reviewed_at = now(), review_comment = $4 WHERE id = $1",
    )
    .bind(rid)
    .bind(to)
    .bind(p.user_id)
    .bind(&comment)
    .execute(&mut *tx)
    .await?;
    let kind = if to == "approved" {
        "capa.supplier_request_approved"
    } else {
        "capa.supplier_request_cancelled"
    };
    log(
        &mut tx,
        p.tenant_id,
        case_id,
        Some(p.user_id),
        kind,
        None,
        json!({ "request": rid, "comment": comment }),
    )
    .await?;
    let out = detail_at(&mut tx, case_id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

pub async fn approve_supplier_request(
    State(st): State<AppState>,
    p: Principal,
    Path(rid): Path<Uuid>,
    ApiJson(r): ApiJson<CommentReq>,
) -> ApiResult<Json<Value>> {
    review_supplier_request(st, p, rid, r.comment, "approved").await
}

pub async fn cancel_supplier_request(
    State(st): State<AppState>,
    p: Principal,
    Path(rid): Path<Uuid>,
    ApiJson(r): ApiJson<CommentReq>,
) -> ApiResult<Json<Value>> {
    review_supplier_request(st, p, rid, r.comment, "cancelled").await
}

#[derive(Deserialize)]
pub struct DocQuery {
    pub lang: Option<String>,
}

/// GET /capa/supplier-requests/{id}/document?lang=ja|en: the letter as text; unapproved drafts are marked.
pub async fn supplier_request_document(
    State(st): State<AppState>,
    p: Principal,
    Path(rid): Path<Uuid>,
    ApiQuery(q): ApiQuery<DocQuery>,
) -> ApiResult<Response> {
    p.require(&st, Permission::DecisionRead).await?;
    let ja = match q.lang.as_deref().unwrap_or("ja") {
        "ja" => true,
        "en" => false,
        _ => return Err(ApiError::BadRequest("invalid_lang")),
    };
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (case_id, reference, status, body_ja, body_en): (Uuid, String, String, String, String) =
        sqlx::query_as("SELECT case_id, reference, status, body_ja, body_en FROM capa_supplier_requests WHERE id = $1")
            .bind(rid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    log(
        &mut tx,
        p.tenant_id,
        case_id,
        Some(p.user_id),
        "capa.supplier_request_exported",
        None,
        json!({ "request": rid, "lang": if ja { "ja" } else { "en" }, "status": status }),
    )
    .await?;
    tx.commit().await?;
    let mut body = if ja { body_ja } else { body_en };
    if status != "approved" {
        body.insert_str(
            0,
            if ja {
                "【下書き・未承認／送付不可】\n\n"
            } else {
                "[DRAFT - NOT APPROVED - DO NOT SEND]\n\n"
            },
        );
    }
    let name = format!(
        "attachment; filename=\"{reference}-{}.txt\"",
        if ja { "ja" } else { "en" }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseReq {
    pub request: Uuid,
    pub artifact: Uuid,
}

/// POST /capa/cases/{id}/supplier-responses: links an uploaded supplier 8D response document to the case. It becomes
/// citable evidence of origin "supplier"; it never completes a step or approves anything.
pub async fn attach_supplier_response(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(r): ApiJson<ResponseReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (_, c) = load(&mut tx, id, true).await?;
    if c.status == CaseStatus::Closed {
        return Err(ApiError::Conflict("case_closed"));
    }
    let status: String = sqlx::query_scalar("SELECT status FROM capa_supplier_requests WHERE id = $1 AND case_id = $2")
        .bind(r.request)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    if status != "approved" {
        return Err(ApiError::Conflict("request_not_approved"));
    }
    let art: Option<String> = sqlx::query_scalar("SELECT status FROM artifacts WHERE id = $1")
        .bind(r.artifact)
        .fetch_optional(&mut *tx)
        .await?;
    match art.as_deref() {
        None => return Err(ApiError::NotFound),
        Some("rejected" | "quarantined") => return Err(ApiError::Conflict("artifact_not_usable")),
        _ => {}
    }
    let n = sqlx::query(
        "INSERT INTO capa_supplier_responses (tenant_id, case_id, request_id, artifact_id, added_by) \
         VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
    )
    .bind(p.tenant_id)
    .bind(id)
    .bind(r.request)
    .bind(r.artifact)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::Conflict("response_already_attached"));
    }
    log(
        &mut tx,
        p.tenant_id,
        id,
        Some(p.user_id),
        "capa.supplier_response_attached",
        None,
        json!({ "request": r.request, "artifact": r.artifact, "origin": "supplier" }),
    )
    .await?;
    let out = detail_at(&mut tx, id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------------------------
// Overdue notifications

/// Raises one notification per overdue open action, recipient (action owner and case opener) and due date.
/// Returns the number of new notifications.
pub async fn sweep_overdue(tx: &mut Tx, tenant: Uuid, today: Date) -> ApiResult<usize> {
    let rows: Vec<(Uuid, Uuid, Uuid, Uuid, Date)> = sqlx::query_as(
        "SELECT a.id, a.case_id, a.owner_id, c.created_by, a.due_date FROM capa_actions a \
         JOIN capa_cases c ON c.id = a.case_id WHERE a.status = 'open' AND c.status <> 'closed' AND a.due_date < $1 \
         ORDER BY a.due_date, a.id LIMIT 1000",
    )
    .bind(today)
    .fetch_all(&mut **tx)
    .await?;
    let mut new = 0;
    for (aid, case, owner, opener, due) in rows {
        let mut raised = 0;
        for user in BTreeSet::from([owner, opener]) {
            raised += sqlx::query(
                "INSERT INTO capa_notifications (id, tenant_id, user_id, case_id, action_id, kind, due_date) \
                 VALUES ($1,$2,$3,$4,$5,'action_overdue',$6) ON CONFLICT DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(tenant)
            .bind(user)
            .bind(case)
            .bind(aid)
            .bind(due)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        }
        if raised > 0 {
            log(
                tx,
                tenant,
                case,
                None,
                "capa.action_overdue",
                Some(Discipline::ACTIONS),
                json!({ "action": aid, "owner": owner, "due_date": due.to_string() }),
            )
            .await?;
            new += usize::try_from(raised).unwrap_or(0);
        }
    }
    Ok(new)
}

pub async fn run_overdue(pool: &PgPool, today: Date) -> ApiResult<usize> {
    let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE status = 'active'")
        .fetch_all(pool)
        .await?;
    let mut n = 0;
    for t in tenants {
        let mut tx = db::tenant_tx(pool, t).await?;
        n += sweep_overdue(&mut tx, t, today).await?;
        tx.commit().await?;
    }
    Ok(n)
}

/// Server background task: overdue sweep every `every`.
pub fn spawn_overdue_worker(pool: PgPool, every: std::time::Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            if let Err(e) = run_overdue(&pool, OffsetDateTime::now_utc().date()).await {
                tracing::error!(error = ?e, "capa overdue sweep failed");
            }
        }
    });
}

/// POST /capa/overdue/run: runs the sweep for the caller's tenant now.
pub async fn overdue_now(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let n = sweep_overdue(&mut tx, p.tenant_id, st.clock.now().date()).await?;
    tx.commit().await?;
    Ok(Json(json!({ "notifications_new": n })))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct NotificationOut {
    pub id: Uuid,
    pub case_id: Uuid,
    pub reference: String,
    pub action_id: Uuid,
    pub description: String,
    pub kind: String,
    pub due_date: Date,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<OffsetDateTime>,
}

/// GET /capa/notifications: the caller's own notifications.
pub async fn notifications(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<NotificationOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, NotificationOut>(
        "SELECT n.id, n.case_id, c.reference, n.action_id, a.description, n.kind, n.due_date, n.created_at, n.read_at \
         FROM capa_notifications n JOIN capa_cases c ON c.id = n.case_id JOIN capa_actions a ON a.id = n.action_id \
         WHERE n.user_id = $1 ORDER BY n.created_at DESC LIMIT 200",
    )
    .bind(p.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

pub async fn mark_read(State(st): State<AppState>, p: Principal, Path(nid): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let n =
        sqlx::query("UPDATE capa_notifications SET read_at = coalesce(read_at, now()) WHERE id = $1 AND user_id = $2")
            .bind(nid)
            .bind(p.user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------------------------------------------
// Reopening on contradicting evidence (selective-invalidation hook)

/// Called from `decisions::on_claim_change` in the same transaction as the evidence change. A case whose
/// effectiveness check (D6) is done is reopened when (a) a claim it cites as verification — directly, or through a
/// cited document — is corrected or withdrawn, or (b) for an SPC case, a new point of the monitored characteristic
/// falls outside the control limits recorded at opening. Returns the number of cases reopened.
pub async fn on_claim_change(tx: &mut Tx, tenant: Uuid, change: &Change) -> ApiResult<usize> {
    let mut hits: Vec<(Uuid, Value)> = Vec::new();
    match change {
        Change::Superseded { old_claim, .. } | Change::Withdrawn { old_claim, .. } => {
            let Ok(old) = Uuid::parse_str(old_claim) else {
                return Ok(0);
            };
            let art: Option<Uuid> = sqlx::query_scalar("SELECT artifact_id FROM claims WHERE id = $1")
                .bind(old)
                .fetch_optional(&mut **tx)
                .await?
                .flatten();
            let cases: Vec<Uuid> = sqlx::query_scalar(
                "SELECT s.case_id FROM capa_steps s WHERE s.discipline = 6 AND s.status = 'done' \
                 AND (s.content->'evidence' @> jsonb_build_array(jsonb_build_object('claim', $1::text)) \
                      OR ($2::uuid IS NOT NULL AND s.content->'evidence' @> jsonb_build_array(jsonb_build_object('artifact', $2::text))))",
            )
            .bind(old.to_string())
            .bind(art.map(|a| a.to_string()))
            .fetch_all(&mut **tx)
            .await?;
            let kind = if matches!(change, Change::Superseded { .. }) {
                "superseded"
            } else {
                "withdrawn"
            };
            hits.extend(cases.into_iter().map(|c| {
                (
                    c,
                    json!({ "cause": format!("verification_evidence_{kind}"), "claim": old }),
                )
            }));
        }
        Change::NewEvidence { new_claim, .. } => {
            let Ok(nc) = Uuid::parse_str(new_claim) else {
                return Ok(0);
            };
            let Some((st, sk, pred, val)): Option<(String, String, String, Option<String>)> = sqlx::query_as(
                "SELECT subject_type, subject_key, predicate, value_num::text FROM claims WHERE id = $1",
            )
            .bind(nc)
            .fetch_optional(&mut **tx)
            .await?
            else {
                return Ok(0);
            };
            let Some(x) = val.and_then(|v| v.parse::<f64>().ok()) else {
                return Ok(0);
            };
            let cases: Vec<(Uuid, Value)> = sqlx::query_as(
                "SELECT c.id, c.source FROM capa_cases c JOIN capa_steps s ON s.case_id = c.id AND s.discipline = 6 \
                 WHERE c.source_kind = 'spc' AND s.status = 'done' AND c.subject_type = $1 AND c.subject_key = $2 \
                 AND c.source->>'predicate' = $3",
            )
            .bind(&st)
            .bind(&sk)
            .bind(&pred)
            .fetch_all(&mut **tx)
            .await?;
            for (cid, src) in cases {
                let (Some(center), Some(sigma)) = (src["center"].as_f64(), src["sigma"].as_f64()) else {
                    continue;
                };
                if (x - center).abs() > 3.0 * sigma {
                    hits.push((
                        cid,
                        json!({ "cause": "spc_point_beyond_limits", "claim": nc, "value": x.to_string() }),
                    ));
                }
            }
        }
    }
    let mut n = 0;
    for (cid, detail) in hits {
        let (_, c) = load(tx, cid, true).await?;
        let next = transition(&c, &Event::Contradicted)?;
        if next == c {
            continue;
        }
        persist(tx, cid, &c, &next, None).await?;
        log(
            tx,
            tenant,
            cid,
            None,
            "capa.reopened",
            Some(Discipline::VERIFICATION),
            detail,
        )
        .await?;
        n += 1;
    }
    Ok(n)
}
