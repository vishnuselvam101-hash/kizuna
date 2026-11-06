//! Paged evidence graph for the Overview (documents → claims → records → decisions/findings).
//!
//! The web client streams pages into a WebGL scene, so a tenant with 100k claims never has to
//! arrive in one response. Keyset paging on the current (non-superseded) claim id; the first page
//! also carries every document, decision and open finding. Node ids match the client's scheme:
//! `doc:`, `claim:`, `subj:<type>:<key>`, `dec:`, `fnd:`. Edges may reference nodes that arrive
//! on a later page; the client holds them until both ends are known.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::State;
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::Principal;
use crate::db;
use crate::error::{ApiQuery, ApiResult};

#[derive(Deserialize)]
pub struct GraphQuery {
    pub after: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub tone: &'static str,
}

#[derive(Serialize)]
pub struct GraphPage {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(String, String)>,
    /// Pass as `after` for the next page; absent on the last page.
    pub next: Option<Uuid>,
    /// Current claims in the tenant (first page only), for a progress indicator.
    pub claims_total: Option<i64>,
}

pub const PAGE_DEFAULT: i64 = 5000;
pub const PAGE_MAX: i64 = 20000;

fn subj_id(t: &str, k: &str) -> String {
    format!("subj:{t}:{k}")
}

type ClaimRow = (Uuid, Option<Uuid>, String, String, String, String);

pub async fn overview(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<GraphQuery>,
) -> ApiResult<Json<GraphPage>> {
    p.require(&st, Permission::ClaimRead).await?;
    p.require(&st, Permission::DecisionRead).await?;
    let limit = q.limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let rows: Vec<ClaimRow> = sqlx::query_as(
        "SELECT id, artifact_id, subject_type, subject_key, predicate, review_status FROM claims \
         WHERE superseded_at IS NULL AND ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2",
    )
    .bind(q.after)
    .bind(limit + 1)
    .fetch_all(&mut *tx)
    .await?;
    let more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let next = more.then(|| rows.last().map(|r| r.0)).flatten();

    let mut nodes = Vec::with_capacity(rows.len() * 2);
    let mut edges = Vec::with_capacity(rows.len() * 3);
    let open_subjects: HashSet<(String, String)> =
        sqlx::query_as("SELECT DISTINCT subject_type, subject_key FROM conflicts WHERE status = 'open'")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let mut claims_total = None;
    if q.after.is_none() {
        claims_total = Some(
            sqlx::query_scalar("SELECT count(*) FROM claims WHERE superseded_at IS NULL")
                .fetch_one(&mut *tx)
                .await?,
        );
        let docs: Vec<(Uuid, String, String)> =
            sqlx::query_as("SELECT id, display_name, status FROM artifacts ORDER BY created_at, id")
                .fetch_all(&mut *tx)
                .await?;
        for (id, name, status) in docs {
            let tone = if status == "accepted" { "base" } else { "warn" };
            nodes.push(GraphNode {
                id: format!("doc:{id}"),
                kind: "document",
                label: name,
                tone,
            });
        }
        let decs: Vec<(Uuid, String, String, String, String, bool)> = sqlx::query_as(
            "SELECT id, title, status, subject_type, subject_key, EXISTS (SELECT 1 FROM decision_dependencies dd \
               WHERE dd.decision_id = decisions.id AND dd.depends_on_claim IS NOT NULL) FROM decisions ORDER BY decided_at, id",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (id, title, status, stype, skey, cites) in decs {
            let tone = match status.as_str() {
                "valid" => "ok",
                "potentially_affected" => "crit",
                _ => "base",
            };
            let did = format!("dec:{id}");
            // Exact links arrive with each claim page; the subject link is only a fallback.
            if !cites {
                edges.push((subj_id(&stype, &skey), did.clone()));
            }
            nodes.push(GraphNode {
                id: did,
                kind: "decision",
                label: title,
                tone,
            });
        }
        let fnds: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT id, subject_type, subject_key FROM conflicts WHERE status = 'open' ORDER BY detected_at, id",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (id, stype, skey) in fnds {
            let fid = format!("fnd:{id}");
            edges.push((subj_id(&stype, &skey), fid.clone()));
            nodes.push(GraphNode {
                id: fid,
                kind: "finding",
                label: skey,
                tone: "crit",
            });
        }
    }

    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let cites: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT depends_on_claim, decision_id FROM decision_dependencies WHERE depends_on_claim = ANY($1) ORDER BY 1, 2",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut cited: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (c, d) in cites {
        cited.entry(c).or_default().push(d);
    }
    let mut seen_subj = HashSet::new();
    for (id, artifact, stype, skey, pred, review) in rows {
        let cid = format!("claim:{id}");
        let sid = subj_id(stype, skey);
        let tone = match review.as_str() {
            "rejected" => "crit",
            "accepted" => "ok",
            _ => "base",
        };
        nodes.push(GraphNode {
            id: cid.clone(),
            kind: "claim",
            label: format!("{skey} {pred}"),
            tone,
        });
        if seen_subj.insert(sid.clone()) {
            let tone = if open_subjects.contains(&(stype.clone(), skey.clone())) {
                "crit"
            } else {
                "info"
            };
            nodes.push(GraphNode {
                id: sid.clone(),
                kind: "subject",
                label: skey.clone(),
                tone,
            });
        }
        if let Some(a) = artifact {
            edges.push((format!("doc:{a}"), cid.clone()));
        }
        for d in cited.get(id).into_iter().flatten() {
            edges.push((cid.clone(), format!("dec:{d}")));
        }
        edges.push((cid, sid));
    }
    Ok(Json(GraphPage {
        nodes,
        edges,
        next,
        claims_total,
    }))
}
