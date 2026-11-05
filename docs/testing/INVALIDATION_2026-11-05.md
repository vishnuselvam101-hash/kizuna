# Selective invalidation: old vs new algorithm (2026-11-05)

Branch `algo-invalidation`, rebased onto main `0eac43c` (benchmarks ran before the rebase on base `2345619`; the engine code is the same). Crate `crates/dependency-engine`; server use in
`crates/kizuna-api/src/decisions.rs`. All numbers below come from real runs of
`scripts/bench-invalidation.sh 1000000 300` on this host (8 cores, 31 GB RAM, rustc 1.95.0, release build).
The host was shared with other work (load average 11–15 during the run), so single timings are noisy;
median and minimum of the repetitions are both given.

## What changed

| | old (`reference`, dep-engine@1) | new |
|---|---|---|
| Direct impacts | one pass over all edges, materiality computed per edge | one pass; materiality verdict memoised per (materiality, snapshot, applicability) for the change, so edges that recorded the same claim value cost one evaluation |
| Propagation | BFS; for each queued decision, a scan of **all** edges (`O(queued × edges)`) | same BFS order over a CSR reverse index built once (`O(edges)`); decisions interned to integer ids |
| State | `BTreeMap<String, Impact>`, path `Vec` cloned per edge | per-decision dirty slot (empty / immaterial / material) + parent pointer; paths, reasons and JSON built once at the end |
| Cycles | terminate (decision queued once when it first becomes material) | same rule: a material slot is final, each decision is queued at most once |
| Order | output sorted by decision id; BFS from seeds in id order, children in input order | identical (determines which path/reason a decision gets) |
| Bound | none | `assess_bounded` / `upstream_withdrawn_bounded(.., max_affected)` return `Err(NeedsReviewBatch { limit, reached })` as soon as more than `max_affected` decisions would be affected; deterministic stop point |

Semantics are unchanged. The order of evaluation stays breadth-first rather than a strict topological order:
BFS is what fixes each decision's explanation path (shortest from a seed) and reason in v1, and the graph may
contain cycles, where no topological order exists. Changing the order would change paths and reasons.

Server: an evidence change or revocation is applied inline when it affects at most
`KIZUNA_INVALIDATION_INLINE_LIMIT` decisions (default 10 000). Above that bound it is never refused. The change
commits and an audited **review batch** is recorded with it (`review_batches`, migration 0021). A background job
(`review_batches::spawn_worker`, every 2 s) first computes and stores the full impact list, then applies it in
chunks of `KIZUNA_REVIEW_BATCH_CHUNK` (default 500) and marks decisions potentially_affected as it goes. Each
chunk applies its impacts and marks them applied in one transaction, so a restart resumes exactly where the last
committed chunk ended and nothing is applied twice. Progress (`total`, `processed`, `flagged`, `status`) is visible
at `GET /api/v1/review-batches[/{id}]`. While a batch is open its subject (canonical key and aliases) is "impact
analysis in progress": creating a decision about that subject, citing evidence about it or depending on a decision
about it is refused with `409 impact_analysis_in_progress`. Tested in `crates/kizuna-api/tests/review_batches.rs`,
including a chunk whose transaction never commits (crash mid-batch) followed by a fresh worker run.
Limitation: the job materialises the impact list in one transaction. That is linear and fast (≈ 2 s for
250k impacts in the engine), but it is not chunked.

## Equivalence and tests

* `new_equals_reference` (proptest, 512 cases): random graphs (DAGs and graphs with cycles), up to 30 decisions and
  120 edges, all three materiality kinds, unit and text snapshots, revisions, scopes and applicability; every
  change kind plus revocation. It asserts that the full `Vec<Impact>` is equal (order, materiality, cause, reason,
  path, semantic change) and that the bound is exact: `Ok` with limit = true fan-out, `Err { reached: limit + 1 }` with one less.
* Mutation check: reversing the seed order in the new BFS makes the property fail on a 7-node graph with cycles.
* `bounded_fan_out_reports_review_batch`: a 1 000-way fan-out at limit 100.
* All existing engine tests pass unchanged.

## Synthetic graphs

Decisions `D0..n` with n ≈ edges / 4. Each decision has one claim edge and up to 3 edges to earlier decisions
(a DAG; 1 in 8 is informational). Claim values come from 16 distinct snapshots.
* **fanout**: a claim withdrawn that ~1/500 of decisions use; propagation reaches about a quarter of the graph.
* **narrow**: a correction to one of edges/40 claims; a few hundred decisions are affected.
* **evidence**: new evidence on a subject/predicate that every claim edge uses (threshold materiality); a
  direct-phase-bound case with no propagation.

`bounded` = new algorithm with the server's 10 000 limit. A result of 0 affected means it returned
`NeedsReviewBatch`. `build` = graph construction only (the RSS baseline). Peak RSS from `/usr/bin/time`, one
process per case, so it includes the graph.

## Results

Time in ms, median / min (reps: 21 up to 10k edges, 5 at 100k, 3 at 1M; old runs once at 100k and above).
DNF = killed at the 300 s timeout.

| scenario | edges | affected | old | new | bounded (10k) | speed-up new vs old |
|---|---:|---:|---:|---:|---:|---:|
| fanout | 1k | 17 | 0.107 / 0.079 | 0.032 / 0.031 | 0.034 / 0.030 | 3.3× |
| fanout | 10k | 1 576 | 65.5 / 52.1 | 3.24 / 2.36 | 3.07 / 1.73 | 20× |
| fanout | 100k | 23 951 | 40 928 | 91.0 / 65.5 | 31.1 (review batch) | 450× |
| fanout | 1M | 248 951 | DNF (> 300 000) | 1 909 / 1 789 | 522 (review batch) | > 157× |
| narrow | 1k | 231 | 0.909 / 0.806 | 0.205 / 0.174 | 0.203 / 0.162 | 4.4× |
| narrow | 10k | 1 748 | 67.5 / 63.2 | 1.78 / 1.51 | 2.01 / 1.56 | 38× |
| narrow | 100k | 126 | 158.2 | 25.8 / 14.4 | 15.9 / 15.2 | 6.1× |
| narrow | 1M | 489 | 12 241 | 507 / 428 | 898 / 560 | 24× |
| evidence | 1k | 250 | 1.91 / 1.71 | 0.466 / 0.425 | 0.453 / 0.394 | 4.1× |
| evidence | 10k | 2 500 | 100.4 / 89.1 | 12.4 / 9.0 | 9.06 / 7.47 | 8.1× |
| evidence | 100k | 24 999 | 36 738 | 178 / 149 | 14.8 (review batch) | 206× |
| evidence | 1M | 249 996 | DNF (> 300 000) | 3 073 / 2 573 | 24.2 (review batch) | > 98× |

Peak RSS (MB):

| scenario | edges | graph only | old | new | bounded |
|---|---:|---:|---:|---:|---:|
| fanout | 100k | 55 | 95 | 95 | 63 |
| fanout | 1M | 527 | DNF | 924 | 606 |
| narrow | 1M | 528 | 528 | 606 | 606 |
| evidence | 100k | 55 | 95 | 115 | 69 |
| evidence | 1M | 533 | DNF | 1 147 | 546 |

## Reading the numbers

* The old algorithm is quadratic in practice: work grows with affected × edges. It takes 41 s for a 100k-edge
  graph with 24k affected decisions and does not finish a 1M-edge graph in 5 minutes. The new algorithm's work
  is linear in edges plus the size of its output (the explanation paths).
* With large fan-out the new run is dominated by building the output: 250k impacts with full paths, reasons and
  JSON (about 0.4 GB above the graph at 1M edges). The old algorithm builds the same output. Where it finished, its peak is equal
  (fanout 100k: 95 vs 95 MB) or lower (evidence 100k: 95 vs 115 MB; narrow 1M: 528 vs 606 MB). The difference is
  the new algorithm's reverse index and memo, about 80 MB at 1M edges.
* With few affected decisions (narrow), the time goes to indexing the edges (interning parent ids, one hash per
  edge), not to propagation. Profiling showed SipHash and the hash-table probes as the top cost. They were
  replaced by an Fx-style hasher with a murmur finaliser, which roughly halved narrow 1M (≈ 0.9 s → 0.42 s in a
  spot check). Decision ids are server-generated UUIDs, so HashDoS resistance is not needed there.
* The bounded variant stops at the 10 000th affected decision. When the seeds alone exceed the bound
  (evidence), it stops during the first pass (24 ms at 1M). Otherwise it stops after the index is built
  (fanout 1M: 0.52 s, ≈ 0.08 GB above the graph).
* In the server, the edges given to the engine are only the rows loaded for the change (direct matches plus
  decision→decision edges), so real graphs are much smaller than these synthetic ones.

## Reproduce

```
scripts/bench-invalidation.sh [max_edges=1000000] [old_timeout_s=300]
cargo test -p dependency-engine
```
