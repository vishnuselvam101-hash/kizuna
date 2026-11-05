//! Old (frozen v1) vs new selective invalidation on synthetic graphs.
//! Usage: inval_bench <old|new|bounded|build> <fanout|narrow|evidence> <edges> [reps]
//! Prints one JSON line. Run each case in its own process (scripts/bench-invalidation.sh) to read peak RSS.

use std::collections::BTreeMap;
use std::time::Instant;

use dependency_engine::{Change, Dep, Materiality, Snapshot, assess, assess_bounded, reference};

/// splitmix64: deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn snap(v: u64) -> Snapshot {
    Snapshot {
        num: Some(format!("{}.{}", 10 + v, v % 10)),
        unit: Some("mm".into()),
        revision: Some("C".into()),
        ..Default::default()
    }
}

/// Random DAG: decisions D0..n, each with one claim edge and ~3 parent edges to earlier decisions (1 in 8
/// informational). `claims` claims; claim values come from 16 distinct snapshots (realistic sharing).
fn graph(edges: usize, claims: u64, rng: &mut Rng) -> Vec<Dep> {
    let n = (edges / 4).max(2) as u64;
    let mut deps = Vec::with_capacity(edges);
    let app = BTreeMap::from([("revision".to_string(), "C".to_string())]);
    for i in 0..n {
        let c = rng.below(claims);
        deps.push(Dep {
            decision: format!("D{i:08}"),
            claim: Some(format!("c{c}")),
            on_decision: None,
            subject: format!("P{}", c % 64),
            predicate: Some("len".into()),
            applicability: app.clone(),
            materiality: if c.is_multiple_of(3) {
                Materiality::NumericAbs {
                    threshold: "0.5".into(),
                }
            } else {
                Materiality::AnyChange
            },
            snapshot: snap(rng.below(16)),
        });
        if i == 0 {
            continue;
        }
        for _ in 0..3 {
            if deps.len() >= edges {
                break;
            }
            deps.push(Dep {
                decision: format!("D{i:08}"),
                claim: None,
                on_decision: Some(format!("D{:08}", rng.below(i))),
                subject: String::new(),
                predicate: None,
                applicability: BTreeMap::new(),
                materiality: if rng.below(8) == 0 {
                    Materiality::Informational
                } else {
                    Materiality::AnyChange
                },
                snapshot: Snapshot::default(),
            });
        }
    }
    deps
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (algo, scenario, edges) = (a[1].as_str(), a[2].as_str(), a[3].parse::<usize>().expect("edges"));
    let reps: usize = a.get(4).map_or(1, |r| r.parse().expect("reps"));
    let mut rng = Rng(0x4b49_5a55_4e41);
    let (deps, change) = match scenario {
        // A claim used by ~1/500 of decisions, early in the order: propagation reaches most of the graph.
        "fanout" => (
            graph(edges, 500, &mut rng),
            Change::Withdrawn {
                old_claim: "c7".into(),
                reason: "rejected".into(),
            },
        ),
        // Many claims, a correction to one: a handful of direct impacts and a short downstream tail.
        "narrow" => {
            let claims = (edges as u64 / 40).max(1);
            (
                graph(edges, claims, &mut rng),
                Change::Superseded {
                    old_claim: "c7".into(),
                    new_claim: "cN".into(),
                    new: snap(3),
                },
            )
        }
        // Every decision depends on the same subject/predicate: direct-phase (materiality) bound, little propagation.
        "evidence" => {
            let mut d = graph(edges, 64, &mut rng);
            for x in d.iter_mut().filter(|x| x.claim.is_some()) {
                x.subject = "P0".into();
                if x.materiality == Materiality::AnyChange {
                    x.materiality = Materiality::NumericAbs {
                        threshold: "0.2".into(),
                    };
                }
            }
            for x in d.iter_mut().filter(|x| x.claim.is_none()) {
                x.materiality = Materiality::Informational;
            }
            (
                d,
                Change::NewEvidence {
                    new_claim: "cN".into(),
                    subject: "P0".into(),
                    predicate: "len".into(),
                    new: snap(5),
                },
            )
        }
        s => panic!("unknown scenario {s}"),
    };
    let mut times = Vec::new();
    let (mut affected, mut material) = (0, 0);
    for _ in 0..reps {
        let t = Instant::now();
        let r = match algo {
            "old" => reference::assess(&deps, &change),
            "new" => assess(&deps, &change),
            // The server's bound (10 000 decisions): returns the review-batch result instead of the full set.
            "bounded" => assess_bounded(&deps, &change, 10_000).unwrap_or_default(),
            "build" => Vec::new(),
            x => panic!("unknown algo {x}"),
        };
        times.push(t.elapsed().as_secs_f64() * 1e3);
        affected = r.len();
        material = r.iter().filter(|i| i.material).count();
        std::hint::black_box(r);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{{\"algo\":\"{algo}\",\"scenario\":\"{scenario}\",\"edges\":{},\"reps\":{reps},\"median_ms\":{:.3},\"min_ms\":{:.3},\"affected\":{affected},\"material\":{material}}}",
        deps.len(),
        times[times.len() / 2],
        times[0]
    );
}
