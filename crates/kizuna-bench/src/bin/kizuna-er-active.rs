//! Active learning for ER review: reviews-to-F1 on the labelled Japanese part/supplier sets.
//!
//! For each dataset and config, the non-safe candidates v2 surfaces form the review queue; a simulated reviewer
//! answers from the ground truth. Strategies:
//! - `before`: current queue order (FS weight descending); unreviewed pairs keep the static EM decision
//!   (predicted match only when the outcome is `match`).
//! - `weight_model`: the `before` order, but unreviewed pairs take the active model's suggestion (isolates the
//!   effect of the order from the effect of the prediction rule).
//! - `random`: seeded random order; unreviewed pairs take the active model's suggestion (posterior ≥ 0.5).
//! - `after`: uncertainty sampling (|log-odds| ascending); unreviewed pairs take the model's suggestion.
//!
//! The model (`entity_resolution::v2::active`) is refit after EVERY decision (closed-form counts). Safe pairs are
//! predicted matches in every strategy; true pairs v2 never surfaces are false negatives in every strategy.
//! F1 is over all true pairs. Nothing is merged; the reviewer's answers are the only decisions.
//!
//! Usage: kizuna-er-active [--seed N] [--prior-strength S] [--out reports/er/er_active.json]

use entity_resolution::Outcome;
use entity_resolution::v2::{self, active};
use kizuna_bench::er_eval::{self, Dataset, pair};
use serde_json::{Value, json};
use std::collections::HashMap;

const CHECKPOINTS: [usize; 7] = [0, 10, 25, 50, 100, 200, 500];
const TARGETS: [f64; 3] = [0.90, 0.95, 0.99];

struct Queue {
    gammas: Vec<v2::em::Gamma>,
    weights: Vec<f64>,
    static_match: Vec<bool>,
    truth: Vec<bool>,
    /// True pairs among the safe (auto) pairs, and the number of safe pairs.
    safe_tp: usize,
    safe_n: usize,
    true_pairs: usize,
}

impl Queue {
    fn f1(&self, label: impl Fn(usize) -> bool) -> f64 {
        let (mut tp, mut pred) = (self.safe_tp, self.safe_n);
        for i in 0..self.truth.len() {
            if label(i) {
                pred += 1;
                tp += usize::from(self.truth[i]);
            }
        }
        let precision = if pred == 0 { 1.0 } else { tp as f64 / pred as f64 };
        let recall = if self.true_pairs == 0 {
            1.0
        } else {
            tp as f64 / self.true_pairs as f64
        };
        if precision + recall == 0.0 {
            0.0
        } else {
            2.0 * precision * recall / (precision + recall)
        }
    }
}

/// F1 after k = 0..=n reviews for one strategy.
fn curve(q: &Queue, strategy: &str, prior: &v2::em::FsParams, strength: f64, seed: u64) -> Vec<f64> {
    let n = q.truth.len();
    let mut reviewed = vec![false; n];
    let mut model = active::ActiveModel::new(prior, strength);
    let label = |reviewed: &[bool], sugg: &[bool], i: usize| if reviewed[i] { q.truth[i] } else { sugg[i] };
    let mut out = Vec::with_capacity(n + 1);
    let order: Vec<usize> = match strategy {
        "before" | "weight_model" => active::baseline_order(&q.weights, &[]),
        "random" => {
            let mut v: Vec<usize> = (0..n).collect();
            let mut rng = v2::Rng::new(seed);
            for i in (1..n).rev() {
                v.swap(i, rng.below(i as u64 + 1) as usize);
            }
            v
        }
        _ => Vec::new(),
    };
    let mut fixed = order.into_iter();
    for k in 0..=n {
        let sugg = if strategy == "before" {
            q.static_match.clone()
        } else {
            model.suggest_all(&q.gammas)
        };
        out.push(q.f1(|i| label(&reviewed, &sugg, i)));
        if k == n {
            break;
        }
        let i = if strategy == "after" {
            active::next_item(&model, &q.gammas, &reviewed, seed).expect("unreviewed item left")
        } else {
            fixed.next().expect("fixed order covers every item")
        };
        reviewed[i] = true;
        // The reviewer answers from the ground truth; the decision is recorded as given.
        model.record(&q.gammas[i], q.truth[i]);
    }
    out
}

fn evaluate(d: &Dataset, label: &str, cfg: &v2::Config, strength: f64, seed: u64) -> Value {
    let keys = d.keys();
    let truth = d.truth();
    let ctx: HashMap<String, v2::PartContext> = d.context.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let r = v2::resolve_with_context(d.kind, &keys, &ctx, cfg);
    let mut q = Queue {
        gammas: Vec::new(),
        weights: Vec::new(),
        static_match: Vec::new(),
        truth: Vec::new(),
        safe_tp: 0,
        safe_n: 0,
        true_pairs: truth.len(),
    };
    for c in &r.candidates {
        let t = truth.contains_key(&pair(&c.left, &c.right));
        if c.safe {
            q.safe_n += 1;
            q.safe_tp += usize::from(t);
            continue;
        }
        q.gammas.push(v2::gamma(
            d.kind,
            &v2::prepare(d.kind, &c.left),
            &v2::prepare(d.kind, &c.right),
        ));
        q.weights.push(c.weight);
        q.static_match.push(c.outcome == Outcome::Match);
        q.truth.push(t);
    }
    let n = q.truth.len();
    let strategies: Vec<Value> = ["before", "weight_model", "random", "after"]
        .iter()
        .map(|s| {
            let f = curve(&q, s, &r.stats.params, strength, seed);
            let at: serde_json::Map<String, Value> = CHECKPOINTS
                .iter()
                .filter(|&&k| k <= n)
                .map(|&k| (k.to_string(), json!(f[k])))
                .chain(std::iter::once(("all".to_string(), json!(f[n]))))
                .collect();
            let reach: serde_json::Map<String, Value> = TARGETS
                .iter()
                .map(|&t| (format!("{t:.2}"), json!(f.iter().position(|&x| x >= t))))
                .collect();
            json!({"strategy": s, "f1_at": at, "reviews_to_f1": reach})
        })
        .collect();
    json!({
        "dataset": d.name,
        "config": label,
        "keys": keys.len(),
        "true_pairs": truth.len(),
        "safe_pairs": q.safe_n,
        "safe_true": q.safe_tp,
        "queue": n,
        "queue_true": q.truth.iter().filter(|t| **t).count(),
        "true_not_surfaced": truth.len() - q.safe_tp - q.truth.iter().filter(|t| **t).count(),
        "em": r.stats.em.used,
        "strategies": strategies,
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
    let mut strength = 10.0f64;
    let mut out: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = v.parse().expect("--seed N"),
            "--prior-strength" => strength = v.parse().expect("--prior-strength S"),
            "--out" => out = Some(v),
            _ => panic!("unknown argument {a}"),
        }
    }
    let base = v2::Config::default();
    let broad = v2::Config {
        typo_review: v2::TypoReview::Broad,
        ..base
    };
    let sets = [
        er_eval::ja_parts(seed, 1000),
        er_eval::ja_parts(seed + 1, 5000),
        er_eval::ja_parts_sparse(seed, 1000),
        er_eval::ja_parts_sparse(seed + 1, 5000),
        er_eval::ja_suppliers(seed, 300),
    ];
    let mut results = Vec::new();
    for d in &sets {
        eprintln!("evaluating {} ({} keys)", d.name, d.keys().len());
        results.push(evaluate(d, "default", &base, strength, seed));
        if d.kind == v2::Kind::Part {
            results.push(evaluate(d, "broad", &broad, strength, seed));
        }
    }
    println!(
        "Active learning for ER review (seed {seed}, prior strength {strength}, model refit after every decision). \
         F1 over all true pairs after k reviews; unsurfaced true pairs are FN for every strategy.\n"
    );
    let ks: Vec<String> = CHECKPOINTS
        .iter()
        .map(|k| k.to_string())
        .chain(["all".into()])
        .collect();
    println!(
        "| dataset | config | queue (true) | not surfaced | strategy | {} | k→0.90 | k→0.95 | k→0.99 |",
        ks.iter().map(|k| format!("F1@{k}")).collect::<Vec<_>>().join(" | ")
    );
    println!("|---|---|---|---|---|{}---|---|---|", "---|".repeat(ks.len()));
    for r in &results {
        for s in r["strategies"].as_array().into_iter().flatten() {
            let cells: Vec<String> = ks
                .iter()
                .map(|k| s["f1_at"][k].as_f64().map_or("—".into(), |f| format!("{f:.3}")))
                .collect();
            let reach: Vec<String> = TARGETS
                .iter()
                .map(|t| {
                    s["reviews_to_f1"][format!("{t:.2}")]
                        .as_u64()
                        .map_or("not reached".into(), |k| k.to_string())
                })
                .collect();
            println!(
                "| {} | {} | {} ({}) | {} | {} | {} | {} |",
                r["dataset"].as_str().unwrap_or(""),
                r["config"].as_str().unwrap_or(""),
                r["queue"],
                r["queue_true"],
                r["true_not_surfaced"],
                s["strategy"].as_str().unwrap_or(""),
                cells.join(" | "),
                reach.join(" | "),
            );
        }
    }
    if let Some(out) = out {
        let report = json!({
            "tool": "kizuna-er-active",
            "command": std::env::args().collect::<Vec<_>>().join(" "),
            "commit": git(&["rev-parse", "--short", "HEAD"]),
            "worktree_dirty": !git(&["status", "--porcelain", "--untracked-files=no"]).is_empty(),
            "seed": seed,
            "prior_strength": strength,
            "refit": "after every decision (closed-form conjugate counts)",
            "environment": kizuna_bench::environment(),
            "note": "simulated reviewer answers from ground truth; nothing is merged automatically",
            "results": results,
        });
        if let Some(dir) = std::path::Path::new(&out).parent() {
            std::fs::create_dir_all(dir).expect("create report dir");
        }
        std::fs::write(&out, serde_json::to_string_pretty(&report).expect("json") + "\n").expect("write report");
        eprintln!("wrote {out}");
    }
}
