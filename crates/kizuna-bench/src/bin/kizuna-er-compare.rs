//! ER v1 vs v2 on labelled data: the E2E generator ground truth (one or more `--e2e name=ground_truth.json`) and
//! the deterministic Japanese supplier/part sets (dense and sparse part masters, with synthetic evidence context),
//! including the typo-rule modes (off / broad / strict and a strict floor sweep). Prints a Markdown summary and writes JSON with provenance.
//!
//! Usage: kizuna-er-compare [--seed N] [--runs N] [--e2e NAME=PATH]... [--out reports/er/er_v2_compare.json]
//! Build with --release; timings are wall-clock medians of `--runs` runs on the same input.

use entity_resolution::v2;
use kizuna_bench::er_eval::{self, Dataset, pair, score};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

fn median(mut v: Vec<Duration>) -> f64 {
    v.sort();
    v[v.len() / 2].as_secs_f64() * 1000.0
}

fn evaluate(d: &Dataset, runs: usize) -> Value {
    let keys = d.keys();
    let truth = d.truth();
    // v1 (rule-based; the same function the API used for every subject type).
    let mut t1 = Vec::new();
    let (mut c1, mut compared1) = (Vec::new(), 0);
    for _ in 0..runs {
        let t = Instant::now();
        (c1, compared1) = entity_resolution::candidates_with_stats(&keys);
        t1.push(t.elapsed());
    }
    let p1: BTreeSet<_> = c1.iter().map(|c| pair(&c.left, &c.right)).collect();
    // v2 in the default mode (typo rule `strict` for parts; the mode does not apply to suppliers).
    let ctx: HashMap<String, v2::PartContext> = d.context.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let cfg = v2::Config::default();
    let mut t2 = Vec::new();
    let mut r2 = None;
    for _ in 0..runs {
        let t = Instant::now();
        r2 = Some(v2::resolve_with_context(d.kind, &keys, &ctx, &cfg));
        t2.push(t.elapsed());
    }
    let r2 = r2.expect("runs >= 1");
    let p2: BTreeSet<_> = r2.candidates.iter().map(|c| pair(&c.left, &c.right)).collect();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for c in &r2.candidates {
        let k = serde_json::to_value(c.outcome)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        *outcomes.entry(k).or_default() += 1;
    }
    // Typo-rule modes (parts only): off = no rule (v2 before the rule), broad = every one-character pair to review
    // (v2 before tuning), strict at the default floor and a floor sweep. One timed run each.
    let variant_of: HashMap<&str, &str> = d.items.iter().map(|i| (i.text.as_str(), i.variant)).collect();
    let typo_truth: BTreeSet<(String, String)> = truth
        .keys()
        .filter(|(a, b)| {
            [a, b]
                .iter()
                .any(|s| variant_of.get(s.as_str()).is_some_and(|v| v.starts_with("keying")))
        })
        .cloned()
        .collect();
    let review_metrics = |cands: &[(String, String, entity_resolution::Outcome, bool)]| {
        let review: BTreeSet<_> = cands
            .iter()
            .filter(|c| c.2 == entity_resolution::Outcome::RequiresHumanReview)
            .map(|c| pair(&c.0, &c.1))
            .collect();
        let listed = cands.iter().filter(|c| !c.3).count();
        json!({
            "review_load": review.len(),
            "review_true": review.iter().filter(|p| truth.contains_key(*p)).count(),
            "listed_for_decision": listed,
            "typo_pairs": typo_truth.len(),
            "typo_surfaced": typo_truth.iter().filter(|p| cands.iter().any(|c| pair(&c.0, &c.1) == **p)).count(),
            "typo_in_review": typo_truth.iter().filter(|p| review.contains(*p)).count(),
        })
    };
    let v1_rows: Vec<_> = c1
        .iter()
        .map(|c| {
            (
                c.left.clone(),
                c.right.clone(),
                c.outcome,
                c.outcome == entity_resolution::Outcome::Match,
            )
        })
        .collect();
    let mut modes = serde_json::Map::new();
    if d.kind == v2::Kind::Part {
        let sweep: [(&str, v2::TypoReview, f64); 6] = [
            ("off", v2::TypoReview::Off, cfg.context_floor),
            ("broad", v2::TypoReview::Broad, cfg.context_floor),
            ("strict", v2::TypoReview::Strict, cfg.context_floor),
            ("strict_floor_0.4", v2::TypoReview::Strict, 0.4),
            ("strict_floor_0.5", v2::TypoReview::Strict, 0.5),
            ("strict_floor_0.8", v2::TypoReview::Strict, 0.8),
        ];
        for (name, mode, floor) in sweep {
            let c = v2::Config {
                typo_review: mode,
                context_floor: floor,
                ..cfg
            };
            let t = Instant::now();
            let r = v2::resolve_with_context(d.kind, &keys, &ctx, &c);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let p: BTreeSet<_> = r.candidates.iter().map(|c| pair(&c.left, &c.right)).collect();
            let rows: Vec<_> = r
                .candidates
                .iter()
                .map(|c| (c.left.clone(), c.right.clone(), c.outcome, c.safe))
                .collect();
            let mut m = review_metrics(&rows);
            m["mode"] = json!(mode);
            m["context_floor"] = json!(floor);
            m["score"] = json!(score(&truth, &p));
            m["typo_reviews"] = json!(r.stats.typo_reviews);
            m["typo_reviews_true"] = json!(
                r.candidates
                    .iter()
                    .filter(|c| c.reasons.contains(&"possible_typo_or_different_part")
                        && truth.contains_key(&pair(&c.left, &c.right)))
                    .count()
            );
            m["typo_left_to_fs"] = json!(r.stats.typo_left_to_fs);
            m["time_ms"] = json!(ms);
            modes.insert(name.to_string(), m);
        }
    }
    // Per-variant recall (pairs labelled by their non-canonical side).
    let mut per_variant: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for (p, v) in &truth {
        let e = per_variant.entry(v).or_default();
        e.0 += 1;
        e.1 += usize::from(p1.contains(p));
        e.2 += usize::from(p2.contains(p));
    }
    // Pairs between a kanji form and a non-kanji form need a reading dictionary neither version has.
    let reachable: BTreeMap<_, _> = truth
        .iter()
        .filter(|((a, b), _)| {
            let k = |s: &str| v2::normalize::has_kanji(&v2::normalize::canonical_company(s));
            d.kind == v2::Kind::Part || k(a) == k(b)
        })
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let n = keys.len();
    json!({
        "dataset": d.name,
        "kind": d.kind,
        "keys": n,
        "entities": d.items.iter().map(|i| i.entity).collect::<BTreeSet<_>>().len(),
        "all_pairs": n * n.saturating_sub(1) / 2,
        "v1": { "score": score(&truth, &p1), "score_same_script": score(&reachable, &p1.iter().filter(|p| !truth.contains_key(*p) || reachable.contains_key(*p)).cloned().collect()),
                "pairs_compared": compared1, "time_ms_median": median(t1), "review": review_metrics(&v1_rows) },
        "v2": { "score": score(&truth, &p2), "score_same_script": score(&reachable, &p2.iter().filter(|p| !truth.contains_key(*p) || reachable.contains_key(*p)).cloned().collect()),
                "pairs_compared": r2.stats.pairs_compared, "time_ms_median": median(t2), "outcomes": outcomes,
                "em": r2.stats.em, "t_match": r2.stats.t_match, "t_possible": r2.stats.t_possible, "typo_review": r2.stats.typo_review, "typo_reviews": r2.stats.typo_reviews, "typo_left_to_fs": r2.stats.typo_left_to_fs, "typo_reviews_true": r2.candidates.iter().filter(|c| c.reasons.contains(&"possible_typo_or_different_part") && truth.contains_key(&pair(&c.left, &c.right))).count(), "params": r2.stats.params },
        "v2_missed_same_script_sample": reachable.keys().filter(|p| !p2.contains(*p)).take(12).map(|(a, b)| format!("{a} | {b}")).collect::<Vec<_>>(),
        "v2_false_positive_sample": p2.iter().filter(|p| !truth.contains_key(*p)).take(12).map(|(a, b)| format!("{a} | {b}")).collect::<Vec<_>>(),
        "typo_modes": modes,
        "per_variant_recall": per_variant.iter().map(|(k, (n, a, b))| (k.to_string(), json!({"pairs": n, "v1": a, "v2": b}))).collect::<serde_json::Map<_, _>>(),
    })
}

fn git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seed = 20261105u64;
    let mut runs = 5usize;
    let mut out = "reports/er/er_v2_compare.json".to_string();
    let mut e2e: Vec<(String, String)> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = v.parse().expect("--seed N"),
            "--runs" => runs = v.parse::<usize>().expect("--runs N").max(1),
            "--out" => out = v,
            "--e2e" => {
                let (n, p) = v.split_once('=').expect("--e2e NAME=PATH");
                e2e.push((n.to_string(), p.to_string()));
            }
            _ => panic!("unknown argument {a}"),
        }
    }
    let mut sets: Vec<Dataset> = Vec::new();
    for (name, path) in &e2e {
        let truth: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("read e2e truth")).expect("json");
        sets.push(er_eval::from_e2e_truth(name, &truth));
    }
    sets.push(er_eval::ja_suppliers(seed, 300));
    sets.push(er_eval::ja_parts(seed, 1000));
    sets.push(er_eval::ja_parts(seed + 1, 5000));
    sets.push(er_eval::ja_parts_sparse(seed, 1000));
    sets.push(er_eval::ja_parts_sparse(seed + 1, 5000));
    let results: Vec<Value> = sets
        .iter()
        .map(|d| {
            eprintln!("evaluating {} ({} keys)", d.name, d.keys().len());
            evaluate(d, runs)
        })
        .collect();
    println!(
        "| dataset | keys | true pairs | ver | precision | recall | F1 | same-script recall | pairs compared | time ms (median) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for r in &results {
        for v in ["v1", "v2"] {
            let s = &r[v]["score"];
            println!(
                "| {} | {} | {} | {v} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {:.1} |",
                r["dataset"].as_str().unwrap_or(""),
                r["keys"],
                s["true_pairs"],
                s["precision"].as_f64().unwrap_or(0.0),
                s["recall"].as_f64().unwrap_or(0.0),
                s["f1"].as_f64().unwrap_or(0.0),
                r[v]["score_same_script"]["recall"].as_f64().unwrap_or(0.0),
                r[v]["pairs_compared"],
                r[v]["time_ms_median"].as_f64().unwrap_or(0.0),
            );
        }
    }
    println!();
    println!(
        "| dataset | mode | precision | recall | F1 | review load | of which true | typo pairs | typo in review | typo rule pairs (true) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for r in &results {
        let row = |label: &str, m: &Value, s: &Value, rule: String| {
            println!(
                "| {} | {label} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {} | {rule} |",
                r["dataset"].as_str().unwrap_or(""),
                s["precision"].as_f64().unwrap_or(0.0),
                s["recall"].as_f64().unwrap_or(0.0),
                s["f1"].as_f64().unwrap_or(0.0),
                m["review_load"],
                m["review_true"],
                m["typo_pairs"],
                m["typo_in_review"],
            );
        };
        let Some(modes) = r["typo_modes"].as_object().filter(|m| !m.is_empty()) else {
            continue;
        };
        row("v1", &r["v1"]["review"], &r["v1"]["score"], "—".into());
        for (name, m) in modes {
            row(
                &format!("v2 {name}"),
                m,
                &m["score"],
                format!("{} ({})", m["typo_reviews"], m["typo_reviews_true"]),
            );
        }
    }
    let report = json!({
        "tool": "kizuna-er-compare",
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "commit": git(&["rev-parse", "--short", "HEAD"]),
        "worktree_dirty": !git(&["status", "--porcelain", "--untracked-files=no"]).is_empty(),
        "seed": seed,
        "runs": runs,
        "environment": kizuna_bench::environment(),
        "kind_note": "predicted = every pair v1/v2 surfaces (match, possible_match, requires_human_review); nothing is merged automatically",
        "results": results,
    });
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("create report dir");
    }
    std::fs::write(&out, serde_json::to_string_pretty(&report).expect("json") + "\n").expect("write report");
    eprintln!("wrote {out}");
}
