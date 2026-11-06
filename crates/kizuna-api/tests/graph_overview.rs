#![allow(clippy::unwrap_used)]
//! Paged evidence graph for the Overview WebGL scene (web/PERF_GRAPH.md).
mod common;

use std::collections::HashSet;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

fn ids(v: &Value) -> Vec<String> {
    v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn overview_graph_pages_cover_every_claim_once_with_links() {
    let c = ctx().await;
    let (t, slug, _, _) = c.admin().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &e).await;
    c.ingest(
        &tok,
        "po.csv",
        b"PO Number,Part No,Rev\nPO-1,BRK-1,A\nPO-2,BRK-2,A\nPO-3,BRK-3,B\n",
        "purchase_order",
    )
    .await;
    let all = c.get("/api/v1/claims?limit=1000", &tok).await;
    let claims: HashSet<String> = all
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| format!("claim:{}", c["id"].as_str().unwrap()))
        .collect();
    assert!(claims.len() >= 3);
    let cid = all.body[0]["id"].as_str().unwrap();
    let key = all.body[0]["subject_key"].as_str().unwrap();
    let dec = c
        .post("/api/v1/decisions", Some(&tok), json!({"kind":"po_release","title":"t","subject_type":"part","subject_key":key,"outcome":"approved","rationale":"r","evidence":[{"claim_id": cid}]}))
        .await;
    let did = format!("dec:{}", dec.body["id"].as_str().unwrap());

    // Page through two claims at a time; the first page carries documents and decisions.
    let first = c.get("/api/v1/graph/overview?limit=2", &tok).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.body["claims_total"].as_i64().unwrap() as usize, claims.len());
    let n0 = ids(&first.body);
    assert!(n0.iter().any(|i| i.starts_with("doc:")));
    assert!(n0.contains(&did));
    let mut seen_claims = HashSet::new();
    let mut edges = Vec::new();
    let mut page = first.body;
    let mut pages = 1;
    loop {
        for i in ids(&page) {
            if i.starts_with("claim:") {
                assert!(seen_claims.insert(i), "claim repeated across pages");
            }
        }
        edges.extend(page["edges"].as_array().unwrap().iter().cloned());
        let Some(next) = page["next"].as_str() else { break };
        page = c
            .get(&format!("/api/v1/graph/overview?limit=2&after={next}"), &tok)
            .await
            .body;
        assert!(page["claims_total"].is_null());
        pages += 1;
    }
    assert_eq!(seen_claims, claims);
    assert_eq!(pages, claims.len().div_ceil(2));
    let cited = format!("claim:{cid}");
    assert!(
        edges.contains(&json!([cited, did])),
        "decision cites its evidence claim"
    );
    assert!(
        edges
            .iter()
            .any(|e| e[0].as_str().unwrap().starts_with("doc:") && e[1] == json!(cited))
    );
    assert!(edges.contains(&json!([cited, format!("subj:part:{key}")])));
}

#[tokio::test]
async fn overview_graph_is_tenant_scoped_and_validates_cursor() {
    let c = ctx().await;
    let (_, _, _, tok) = c.admin().await;
    c.ingest(
        &tok,
        "po.csv",
        b"PO Number,Part No,Rev\nPO-9,ZZZ-9,A\n",
        "purchase_order",
    )
    .await;
    let (_, _, _, tok2) = c.admin().await;
    let other = c.get("/api/v1/graph/overview", &tok2).await;
    assert_eq!(other.status, StatusCode::OK);
    assert_eq!(other.body["claims_total"], 0);
    assert!(other.body["nodes"].as_array().unwrap().is_empty());
    assert_eq!(
        c.get("/api/v1/graph/overview?limit=0", &tok).await.status,
        StatusCode::OK
    );
    let bad = c.get("/api/v1/graph/overview?after=not-a-uuid", &tok).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}
