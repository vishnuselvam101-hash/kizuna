//! What-if simulation (`whatif@1`): before a correction (spec, revision or value change) or an evidence withdrawal
//! is committed, preview its consequences with the same engines the real change runs: which decisions it would
//! flag (selective invalidation, bounded), which three-way-match findings and integrity/drift signals would
//! appear or clear, and the minimal supplier re-request it would trigger.
//!
//! Nothing is written. The analysis runs in a READ ONLY, REPEATABLE READ tenant transaction, so the database
//! itself refuses any write; the change is applied as an in-memory [`Overlay`] over the loaded claims. The only
//! optional write is an audited `simulation.run` record, in its own transaction after the analysis.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::State;
use dependency_engine::{Change, ENGINE_VERSION, Impact};
use policy_engine::Permission;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::claims::{self, ClaimOut, SupersedeReq};
use crate::db::{self, Tx};
use crate::decisions::{self, Cause};
use crate::error::{ApiError, ApiJson, ApiResult};

pub const SIM_VERSION: &str = "whatif@1";
/// Stand-in id of the claim a simulated correction would create (the real one gets a fresh UUIDv7).
pub const SIM_CLAIM: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_5151_0000_0001);
/// Bound on the decisions one simulation evaluates (the linear engine stops deterministically beyond it).
pub const SIM_LIMIT: usize = 250_000;
/// Most entries listed per section; totals are always exact.
const LIST_CAP: usize = 200;
const SAMPLE_DEFAULT: usize = 50;
const SAMPLE_MAX: usize = 500;
const WITHDRAW_MAX: usize = 200;

/// A simulated evidence change applied on top of the loaded claims.
#[derive(Default)]
pub struct Overlay {
    /// Claims that stop being current evidence (corrected or withdrawn).
    pub removed: BTreeSet<Uuid>,
    /// The claim a correction would add.
    pub added: Option<SimClaim>,
}

impl Overlay {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_none()
    }
    pub fn keeps(&self, id: &Uuid) -> bool {
        !self.removed.contains(id)
    }
}

pub struct SimClaim {
    pub claim: ClaimOut,
    pub artifact: Option<SimArtifact>,
}

pub struct SimArtifact {
    pub id: Uuid,
    pub kind: String,
    pub status: String,
    pub display_name: String,
    pub sha256: String,
    pub created_ts: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimChange {
    /// A correction exactly as `POST /claims/{claim_id}/supersede` would take it.
    Supersede {
        claim_id: Uuid,
        correction: Box<SupersedeReq>,
    },
    /// Evidence withdrawal, as rejecting the claims in review would do: listed claims and/or every current claim of
    /// one document.
    Withdraw {
        #[serde(default)]
        claim_ids: Vec<Uuid>,
        artifact_id: Option<Uuid>,
    },
}

#[derive(Deserialize)]
pub struct SimReq {
    pub change: SimChange,
    /// Affected decisions listed in detail (default 50, at most 500); totals are always complete.
    pub sample: Option<usize>,
    /// Record an audited `simulation.run` (the only write a simulation can make).
    #[serde(default)]
    pub record: bool,
}

/// The engine-level changes, the overlay and the claims as they are now.
struct Prepared {
    changes: Vec<Change>,
    overlay: Overlay,
    before: Vec<ClaimOut>,
}

async fn current_claim(tx: &mut Tx, id: Uuid) -> ApiResult<ClaimOut> {
    claims::fetch(tx, id).await
}

async fn prepare(tx: &mut Tx, change: &SimChange) -> ApiResult<Prepared> {
    match change {
        SimChange::Supersede { claim_id, correction } => {
            // Same validation, in the same order, as claims::supersede.
            let reason = correction.reason.trim();
            if reason.is_empty() || reason.chars().count() > 500 {
                return Err(ApiError::BadRequest("reason_required"));
            }
            let v = claims::validate_typed(&correction.create.claim)?;
            let old = current_claim(tx, *claim_id).await?;
            if old.superseded_at.is_some() {
                return Err(ApiError::Conflict("already_superseded"));
            }
            if old.subject_type != v.subject_type
                || entity_resolution::safe_normalize(&old.subject_key) != v.subject_key
                || old.predicate != v.predicate
            {
                return Err(ApiError::BadRequest("supersede_changes_identity"));
            }
            let valid_from = claims::parse_time(&correction.create.valid_from)?;
            let valid_to = claims::parse_time(&correction.create.valid_to)?;
            if let (Some(f), Some(t)) = (valid_from, valid_to)
                && t <= f
            {
                return Err(ApiError::BadRequest("invalid_valid_time"));
            }
            let artifact = match correction.create.artifact_id {
                None => None,
                Some(a) => {
                    let row: Option<(String, String, String, String, OffsetDateTime)> = sqlx::query_as(
                        "SELECT kind, status, display_name, encode(sha256, 'hex'), created_at FROM artifacts WHERE id = $1",
                    )
                    .bind(a)
                    .fetch_optional(&mut **tx)
                    .await?;
                    let (kind, status, display_name, sha256, created_ts) =
                        row.ok_or(ApiError::BadRequest("unknown_artifact"))?;
                    if status != "accepted" {
                        return Err(ApiError::Conflict("artifact_not_released"));
                    }
                    Some(SimArtifact {
                        id: a,
                        kind,
                        status,
                        display_name,
                        sha256,
                        created_ts,
                    })
                }
            };
            // The number exactly as PostgreSQL would store and print it.
            let value_num: Option<String> = match &v.value_num {
                Some(n) => Some(
                    sqlx::query_scalar("SELECT $1::numeric::text")
                        .bind(n)
                        .fetch_one(&mut **tx)
                        .await?,
                ),
                None => None,
            };
            let claim = ClaimOut {
                id: SIM_CLAIM,
                model_version: old.model_version,
                subject_type: v.subject_type.clone(),
                subject_key: v.subject_key.clone(),
                predicate: v.predicate.clone(),
                value_text: v.value_text.clone(),
                value_num,
                value_unit: v.value_unit.clone(),
                scope: v.scope.clone(),
                valid_from,
                valid_to,
                revision: v.revision.clone(),
                artifact_id: correction.create.artifact_id,
                source_location: v.source_location.clone(),
                extraction_method: "manual".into(),
                confidence_bps: 10_000,
                review_status: "unreviewed".into(),
                provenance: json!({ "simulated": true, "supersedes": claim_id, "reason": reason }),
                recorded_by: None,
                recorded_at: OffsetDateTime::now_utc(),
                superseded_at: None,
                superseded_by: None,
                artifact_display_name: artifact.as_ref().map(|a| a.display_name.clone()),
            };
            let change = Change::Superseded {
                old_claim: claim_id.to_string(),
                new_claim: SIM_CLAIM.to_string(),
                new: decisions::snapshot_of(&claim),
            };
            Ok(Prepared {
                changes: vec![change],
                overlay: Overlay {
                    removed: BTreeSet::from([*claim_id]),
                    added: Some(SimClaim { claim, artifact }),
                },
                before: vec![old],
            })
        }
        SimChange::Withdraw { claim_ids, artifact_id } => {
            let mut ids: BTreeSet<Uuid> = claim_ids.iter().copied().collect();
            if let Some(a) = artifact_id {
                let of_doc: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM claims WHERE artifact_id = $1 AND superseded_at IS NULL AND review_status <> 'rejected' \
                     ORDER BY id LIMIT $2",
                )
                .bind(a)
                .bind((WITHDRAW_MAX + 1) as i64)
                .fetch_all(&mut **tx)
                .await?;
                ids.extend(of_doc);
            }
            if ids.is_empty() {
                return Err(ApiError::BadRequest("nothing_to_withdraw"));
            }
            if ids.len() > WITHDRAW_MAX {
                return Err(ApiError::BadRequest("too_many_claims"));
            }
            let mut before = Vec::new();
            for id in &ids {
                // Same refusals as rejecting the claim in review (verification::review_claim).
                let c = current_claim(tx, *id).await?;
                if c.superseded_at.is_some() {
                    return Err(ApiError::Conflict("claim_superseded"));
                }
                if c.review_status == "rejected" {
                    return Err(ApiError::Conflict("claim_rejected_supersede_instead"));
                }
                before.push(c);
            }
            Ok(Prepared {
                changes: ids
                    .iter()
                    .map(|id| Change::Withdrawn {
                        old_claim: id.to_string(),
                        reason: "rejected in review".into(),
                    })
                    .collect(),
                overlay: Overlay {
                    removed: ids,
                    added: None,
                },
                before,
            })
        }
    }
}

/// Selective invalidation of every engine change, merged per decision (a material impact wins, then the first).
struct Impacts {
    /// Every impact in the order the real change would record them (several per decision when several claims go).
    all: Vec<Impact>,
    merged: BTreeMap<String, Impact>,
    /// Some change needed more than [`SIM_LIMIT`] decisions: totals are lower bounds and nothing is listed.
    bounded: Option<usize>,
    /// The real change would exceed the inline bound and be applied by a review batch.
    review_batch: bool,
}

async fn impacts(tx: &mut Tx, changes: &[Change]) -> ApiResult<Impacts> {
    let inline = decisions::inline_limit();
    let mut out = Impacts {
        all: Vec::new(),
        merged: BTreeMap::new(),
        bounded: None,
        review_batch: false,
    };
    for ch in changes {
        match decisions::impacts_of(tx, &Cause::Claim { change: ch.clone() }, SIM_LIMIT).await? {
            Err(e) => {
                out.bounded = Some(out.bounded.unwrap_or(0).max(e.reached));
                out.review_batch = true;
            }
            Ok(c) => {
                out.review_batch |= c.impacts.len() > inline;
                for i in c.impacts {
                    match out.merged.get(&i.decision) {
                        Some(prev) if prev.material || !i.material => {}
                        _ => {
                            out.merged.insert(i.decision.clone(), i.clone());
                        }
                    }
                    out.all.push(i);
                }
            }
        }
    }
    Ok(out)
}

fn parse_ids(ids: &[&String]) -> Vec<Uuid> {
    ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect()
}

type DecRow = (Uuid, String, String, String, String, String);

async fn decision_rows(tx: &mut Tx, ids: &[Uuid]) -> ApiResult<BTreeMap<Uuid, DecRow>> {
    let rows: Vec<DecRow> =
        sqlx::query_as("SELECT id, kind, title, subject_type, subject_key, status FROM decisions WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows.into_iter().map(|r| (r.0, r)).collect())
}

fn status_after(before: &str, material: bool) -> &str {
    if material && before == "valid" {
        "potentially_affected"
    } else {
        before
    }
}

/// Keyed, comparable view of one three-way-match finding.
fn match_findings(facts_docs: &(Vec<conflict_engine::Fact>, BTreeMap<String, String>)) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (l, m) in crate::matching::all_findings(&facts_docs.0, &facts_docs.1) {
        for f in &m.findings {
            let key = serde_json::to_string(&(
                &l.purchase_order,
                &l.line,
                &l.part,
                f.kind,
                &f.check,
                &f.certificate_doc,
            ))
            .unwrap_or_default();
            out.insert(
                key,
                json!({
                    "purchase_order": l.purchase_order, "po_line": l.line, "part": l.part, "kind": f.kind,
                    "severity": f.severity, "check": f.check, "certificate_doc": f.certificate_doc,
                    "explanation": f.explanation,
                }),
            );
        }
    }
    out
}

/// Entries that appear, clear or change between two keyed sets (lists capped, totals exact).
fn diff(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
    same: impl Fn(&Value, &Value) -> bool,
) -> Value {
    let appear: Vec<&Value> = after
        .iter()
        .filter(|(k, _)| !before.contains_key(*k))
        .map(|x| x.1)
        .collect();
    let clear: Vec<&Value> = before
        .iter()
        .filter(|(k, _)| !after.contains_key(*k))
        .map(|x| x.1)
        .collect();
    let changed: Vec<Value> = after
        .iter()
        .filter_map(|(k, a)| {
            before
                .get(k)
                .filter(|b| !same(b, a))
                .map(|b| json!({ "before": b, "after": a }))
        })
        .collect();
    json!({
        "appear_total": appear.len(), "clear_total": clear.len(), "changed_total": changed.len(),
        "appear": appear.into_iter().take(LIST_CAP).collect::<Vec<_>>(),
        "clear": clear.into_iter().take(LIST_CAP).collect::<Vec<_>>(),
        "changed": changed.into_iter().take(LIST_CAP).collect::<Vec<_>>(),
    })
}

fn signal_view(g: &integrity_signals::cert::Signal) -> Value {
    json!({ "fingerprint": g.fingerprint, "kind": g.kind.as_str(), "supplier": g.supplier, "part": g.part,
            "characteristic": g.characteristic, "strength": g.strength.as_str(), "why": g.why, "why_ja": g.why_ja })
}

fn drift_view(d: &integrity_signals::drift::Drift) -> Value {
    json!({ "fingerprint": d.fingerprint, "kind": d.kind.as_str(), "key": d.key, "part": d.part,
            "earlier_document": d.earlier_document, "later_document": d.later_document,
            "earlier_revision": d.earlier_revision, "later_revision": d.later_revision, "changes": d.changes.len(),
            "strength": d.strength.as_str(), "why": d.why, "why_ja": d.why_ja })
}

type Live = (BTreeMap<String, Value>, BTreeMap<String, Value>);

async fn integrity_state(tx: &mut Tx, ov: &Overlay) -> ApiResult<Live> {
    let (_, signals, drifts) = crate::integrity::live(tx, ov).await?;
    Ok((
        signals
            .iter()
            .map(|g| (g.fingerprint.clone(), signal_view(g)))
            .collect(),
        drifts.iter().map(|d| (d.fingerprint.clone(), drift_view(d))).collect(),
    ))
}

/// Match findings, integrity signals and drift findings the engines produce over the current claims (no overlay).
/// Used by tests to compare a simulation with the state after the change was really applied.
pub async fn engine_state(tx: &mut Tx) -> ApiResult<Value> {
    let ov = Overlay::default();
    let m = match_findings(&crate::matching::load_with(tx, &ov).await?);
    let (s, d) = integrity_state(tx, &ov).await?;
    Ok(
        json!({ "match": m.keys().collect::<Vec<_>>(), "signals": s.keys().collect::<Vec<_>>(),
               "drift": d.keys().collect::<Vec<_>>() }),
    )
}

/// Graph of the sampled impacts: changed claims → decisions along each explanation path.
fn impact_graph(sample: &[&Impact], titles: &BTreeMap<Uuid, DecRow>, before: &[ClaimOut]) -> Value {
    let claims: BTreeMap<String, &ClaimOut> = before.iter().map(|c| (c.id.to_string(), c)).collect();
    let mut nodes: BTreeMap<String, Value> = BTreeMap::new();
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    let material: BTreeMap<&str, bool> = sample.iter().map(|i| (i.decision.as_str(), i.material)).collect();
    for i in sample {
        for w in i.path.windows(2) {
            edges.insert((w[0].clone(), w[1].clone()));
        }
        for id in &i.path {
            nodes.entry(id.clone()).or_insert_with(|| {
                if let Some(c) = claims.get(id) {
                    json!({ "id": id, "kind": "claim", "label": format!("{} (before)", c.predicate), "tone": "warn", "mark": "changed" })
                } else if id == &SIM_CLAIM.to_string() {
                    json!({ "id": id, "kind": "claim", "label": "simulated value", "tone": "info", "mark": "changed" })
                } else {
                    let label = Uuid::parse_str(id)
                        .ok()
                        .and_then(|u| titles.get(&u))
                        .map_or_else(|| id.clone(), |r| r.2.clone());
                    let m = material.get(id.as_str()).copied().unwrap_or(true);
                    json!({ "id": id, "kind": "decision", "label": label, "tone": if m { "crit" } else { "ok" },
                            "mark": if m { "affected" } else { "unaffected" } })
                }
            });
        }
    }
    json!({ "nodes": nodes.into_values().collect::<Vec<_>>(), "edges": edges.into_iter().map(|(a, b)| [a, b]).collect::<Vec<_>>() })
}

/// Runs a simulation in `tx` (read-only by the caller's choice; nothing here writes).
pub async fn simulate(tx: &mut Tx, req: &SimReq) -> ApiResult<Value> {
    let sample_n = req.sample.unwrap_or(SAMPLE_DEFAULT).clamp(1, SAMPLE_MAX);
    let p = prepare(tx, &req.change).await?;
    let im = impacts(tx, &p.changes).await?;

    // Decisions.
    let material_ids: Vec<Uuid> = parse_ids(
        &im.merged
            .values()
            .filter(|i| i.material)
            .map(|i| &i.decision)
            .collect::<Vec<_>>(),
    );
    let newly_flagged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM decisions WHERE id = ANY($1) AND status = 'valid'")
            .bind(&material_ids)
            .fetch_one(&mut **tx)
            .await?;
    let mut ordered: Vec<&Impact> = im.merged.values().collect();
    ordered.sort_by(|a, b| {
        b.material
            .cmp(&a.material)
            .then(a.path.len().cmp(&b.path.len()))
            .then(a.decision.cmp(&b.decision))
    });
    let sample: Vec<&Impact> = ordered.into_iter().take(sample_n).collect();
    let rows = decision_rows(tx, &parse_ids(&sample.iter().map(|i| &i.decision).collect::<Vec<_>>())).await?;
    let path_ids: Vec<Uuid> = sample
        .iter()
        .flat_map(|i| i.path.iter())
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let mut titles = decision_rows(tx, &path_ids).await?;
    titles.extend(rows.clone());
    let listed: Vec<Value> = sample
        .iter()
        .map(|i| {
            let r = Uuid::parse_str(&i.decision).ok().and_then(|u| rows.get(&u));
            let before = r.map_or("valid", |r| r.5.as_str());
            json!({
                "decision_id": i.decision, "kind": r.map(|r| &r.1), "title": r.map(|r| &r.2),
                "subject_type": r.map(|r| &r.3), "subject_key": r.map(|r| &r.4),
                "status_before": before, "status_after": status_after(before, i.material),
                "material": i.material, "cause_kind": i.cause_kind, "reason": i.reason, "path": i.path,
                "semantic_change": i.semantic_change,
            })
        })
        .collect();

    // Three-way match: the same engine over the claims before and after the change.
    let m_before = match_findings(&crate::matching::load_with(tx, &Overlay::default()).await?);
    let m_after = match_findings(&crate::matching::load_with(tx, &p.overlay).await?);
    // Integrity signals and revision drift.
    let (s_before, d_before) = integrity_state(tx, &Overlay::default()).await?;
    let (s_after, d_after) = integrity_state(tx, &p.overlay).await?;
    let same_strength = |a: &Value, b: &Value| a["strength"] == b["strength"] && a["why"] == b["why"];

    // Minimal supplier re-request for the decisions the change would leave potentially affected.
    let resupply = if im.bounded.is_some() {
        Value::Null
    } else {
        let mut extra: BTreeMap<Uuid, Vec<(String, Value)>> = BTreeMap::new();
        for i in im.all.iter().filter(|i| i.material) {
            if let Ok(d) = Uuid::parse_str(&i.decision) {
                extra
                    .entry(d)
                    .or_default()
                    .push((i.cause_kind.to_string(), i.semantic_change.clone()));
            }
        }
        let mut flagged = material_ids.clone();
        flagged.sort();
        flagged.truncate(crate::evidence_requests::PLAN_DECISIONS);
        if flagged.is_empty() {
            json!({ "engine": domain_manufacturing::resupply::RESUPPLY_VERSION, "decisions_considered": 0,
                    "suppliers": [], "no_supplier_action": [] })
        } else {
            json!(crate::evidence_requests::compute_with(tx, &flagged, &extra, &p.overlay).await?)
        }
    };

    let graph = impact_graph(&sample, &titles, &p.before);
    let after_claim = p.overlay.added.as_ref().map(|s| {
        let mut v = json!(s.claim);
        v["id"] = Value::Null;
        v
    });
    Ok(json!({
        "simulation": SIM_VERSION,
        "engine": ENGINE_VERSION,
        "read_only": true,
        "change": {
            "type": match req.change { SimChange::Supersede { .. } => "supersede", SimChange::Withdraw { .. } => "withdraw" },
            "before": p.before.iter().take(LIST_CAP).collect::<Vec<_>>(),
            "before_total": p.before.len(),
            "after": after_claim,
            "simulated_claim_id": p.overlay.added.as_ref().map(|_| SIM_CLAIM),
        },
        "decisions": {
            "affected_total": im.merged.len(),
            "material_total": material_ids.len(),
            "newly_flagged_total": newly_flagged,
            "bounded": im.bounded.is_some(),
            "reached": im.bounded,
            "limit": SIM_LIMIT,
            "review_batch": im.review_batch,
            "truncated": im.merged.len() > listed.len(),
            "sample": listed,
        },
        "matching": diff(&m_before, &m_after, |a, b| a == b),
        "integrity": {
            "signals": diff(&s_before, &s_after, same_strength),
            "drift": diff(&d_before, &d_after, same_strength),
        },
        "resupply": resupply,
        "graph": graph,
    }))
}

/// POST /simulations — preview a change without committing it.
pub async fn run(State(st): State<AppState>, p: Principal, ApiJson(req): ApiJson<SimReq>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let mut out = simulate(&mut tx, &req).await?;
    tx.rollback().await?;
    if req.record {
        let id = Uuid::now_v7();
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&out).map_err(ApiError::internal)?));
        let mut w = db::tenant_tx(&st.pool, p.tenant_id).await?;
        audit::append(
            &mut w,
            p.tenant_id,
            Record::user(
                p.user_id,
                "simulation.run",
                "simulation",
                id,
                json!({
                    "simulation": SIM_VERSION, "change": out["change"]["type"],
                    "claims": out["change"]["before"].as_array().map(|a| a.iter().map(|c| c["id"].clone()).collect::<Vec<_>>()),
                    "affected": out["decisions"]["affected_total"], "material": out["decisions"]["material_total"],
                    "match_appear": out["matching"]["appear_total"], "match_clear": out["matching"]["clear_total"],
                    "signals_appear": out["integrity"]["signals"]["appear_total"],
                    "signals_clear": out["integrity"]["signals"]["clear_total"],
                    "result_sha256": digest,
                }),
            ),
        )
        .await?;
        w.commit().await?;
        out["run_id"] = json!(id);
        out["result_sha256"] = json!(digest);
    }
    Ok(Json(out))
}
