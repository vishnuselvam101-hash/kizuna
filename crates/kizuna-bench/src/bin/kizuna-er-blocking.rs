//! Blocking evaluation for ER v2: pair recall (true pairs blocking produces / all true pairs), candidate-pair count,
//! true pairs surfaced after scoring, and a breakdown of the true pairs blocking misses by the script of each side
//! (kanji / kana / latin) and by variant label. Synthetic labelled sets only.
//!
//! Usage: kizuna-er-blocking [--seed N] [--out reports/er/er_blocking.json]

use entity_resolution::v2;
use kizuna_bench::er_eval::{self, Dataset, pair};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Script of the name without its legal form (株式会社 etc. would make every name "kanji").
fn script(s: &str) -> &'static str {
    let core = v2::normalize::company_key(s).core;
    let s = core.as_str();
    if v2::normalize::has_kanji(s) {
        "kanji"
    } else if s
        .chars()
        .any(|c| ('\u{30A0}'..='\u{30FF}').contains(&c) || ('\u{FF66}'..='\u{FF9F}').contains(&c))
    {
        "kana"
    } else {
        "latin"
    }
}

fn evaluate(d: &Dataset, cfg: &v2::Config) -> Value {
    let keys = d.keys();
    let truth = d.truth();
    let n = keys.len();
    let blocked: BTreeSet<(String, String)> = v2::blocked_pairs(d.kind, &keys, cfg)
        .into_iter()
        .map(|(a, b)| pair(&a, &b))
        .collect();
    let ctx: HashMap<String, v2::PartContext> = d.context.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let r = v2::resolve_with_context(d.kind, &keys, &ctx, cfg);
    let surfaced: BTreeSet<(String, String)> = r.candidates.iter().map(|c| pair(&c.left, &c.right)).collect();
    let true_blocked = truth.keys().filter(|p| blocked.contains(*p)).count();
    let true_surfaced = truth.keys().filter(|p| surfaced.contains(*p)).count();
    let mut missed_by_script: BTreeMap<String, usize> = BTreeMap::new();
    let mut missed_by_variant: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dropped_by_script: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: Vec<Value> = Vec::new();
    for ((a, b), label) in &truth {
        let mut s = [script(a), script(b)];
        s.sort();
        let class = format!("{}-{}", s[0], s[1]);
        if !blocked.contains(&(a.clone(), b.clone())) {
            *missed_by_script.entry(class.clone()).or_default() += 1;
            *missed_by_variant.entry(label).or_default() += 1;
            if examples.len() < 20 {
                examples.push(json!([a, b, label]));
            }
        } else if !surfaced.contains(&(a.clone(), b.clone())) {
            *dropped_by_script.entry(class).or_default() += 1;
        }
    }
    let surfaced_true_review = r
        .candidates
        .iter()
        .filter(|c| !c.safe && truth.contains_key(&pair(&c.left, &c.right)))
        .count();
    json!({
        "dataset": d.name,
        "keys": n,
        "all_pairs": n * n.saturating_sub(1) / 2,
        "blocked_pairs": blocked.len(),
        "true_pairs": truth.len(),
        "true_blocked": true_blocked,
        "pair_recall": if truth.is_empty() { 1.0 } else { true_blocked as f64 / truth.len() as f64 },
        "candidates_surfaced": r.candidates.len(),
        "true_surfaced": true_surfaced,
        "surfaced_recall": if truth.is_empty() { 1.0 } else { true_surfaced as f64 / truth.len() as f64 },
        "true_in_review_queue": surfaced_true_review,
        "missed_by_blocking_by_script": missed_by_script,
        "missed_by_blocking_by_variant": missed_by_variant,
        "blocked_but_dropped_by_scoring_by_script": dropped_by_script,
        "missed_examples": examples,
        "em": r.stats.em,
        "t_match": r.stats.t_match,
        "t_possible": r.stats.t_possible,
        "params": r.stats.params,
    })
}

fn git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seed = 20261105u64;
    let mut out: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = v.parse().expect("--seed N"),
            "--out" => out = Some(v),
            _ => panic!("unknown argument {a}"),
        }
    }
    let cfg = v2::Config::default();
    let sets = [
        er_eval::ja_suppliers(seed, 300),
        er_eval::ja_suppliers(seed + 1, 300),
        er_eval::ja_suppliers(seed + 2, 450),
        er_eval::ja_suppliers_unseen(seed, 200),
        er_eval::ja_parts(seed, 1000),
        er_eval::ja_parts(seed + 1, 5000),
        er_eval::ja_parts_sparse(seed, 1000),
        er_eval::ja_parts_sparse(seed + 1, 5000),
    ];
    let mut results = Vec::new();
    println!(
        "| dataset | keys | all pairs | blocked pairs | true pairs | pair recall | surfaced (true) | surfaced recall |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for d in &sets {
        let r = evaluate(d, &cfg);
        println!(
            "| {} | {} | {} | {} | {} | {:.4} | {} ({}) | {:.4} |",
            r["dataset"].as_str().unwrap_or(""),
            r["keys"],
            r["all_pairs"],
            r["blocked_pairs"],
            r["true_pairs"],
            r["pair_recall"].as_f64().unwrap_or(0.0),
            r["candidates_surfaced"],
            r["true_surfaced"],
            r["surfaced_recall"].as_f64().unwrap_or(0.0),
        );
        eprintln!(
            "  missed by blocking: {} | by variant: {} | dropped by scoring: {}",
            r["missed_by_blocking_by_script"],
            r["missed_by_blocking_by_variant"],
            r["blocked_but_dropped_by_scoring_by_script"]
        );
        eprintln!("  examples: {}", r["missed_examples"]);
        eprintln!("  em: {} t: {} {}", r["em"], r["t_match"], r["t_possible"]);
        results.push(r);
    }
    if let Some(out) = out {
        let report = json!({
            "tool": "kizuna-er-blocking",
            "command": std::env::args().collect::<Vec<_>>().join(" "),
            "commit": git(&["rev-parse", "--short", "HEAD"]),
            "worktree_dirty": !git(&["status", "--porcelain", "--untracked-files=no"]).is_empty(),
            "seed": seed,
            "environment": kizuna_bench::environment(),
            "note": "synthetic labelled sets; nothing is merged",
            "results": results,
        });
        if let Some(dir) = std::path::Path::new(&out).parent() {
            std::fs::create_dir_all(dir).expect("create report dir");
        }
        std::fs::write(&out, serde_json::to_string_pretty(&report).expect("json") + "\n").expect("write report");
        eprintln!("wrote {out}");
    }
}
