# Verification-run performance and upload staging — 2026-11-05

Base: `main` c215db0, branch `perf-verify` (not pushed). Follow-up to two findings of
[CHAOS_SOAK_2026-11-05](CHAOS_SOAK_2026-11-05.md): the whole-tenant, serial verification run, and orphan objects
when the database dies mid-upload. All database work ran on a **private** PostgreSQL 18 (`initdb` as `nobody`,
127.0.0.1:54044, databases `kizuna_test_perf` and `kizuna_test_perf_base`). The system PostgreSQL was not used.

Host: the shared 8-core dev VPS, under load from other jobs during every measurement (1-min load average 10–22).
Absolute numbers are therefore noisy. Before and after were run interleaved, three rounds each, on the same data
shape, so the comparison is fair, but treat single figures as ±30 %.

## 1. Incremental verification run

### Benchmark

`kizuna-verify-bench` (new, `crates/kizuna-bench/src/bin/kizuna-verify-bench.rs`) seeds one tenant directly in the
database. Each of 3 900 parts gets a spec, a drawing, a purchase order and an inspection report: 65 968 claims and
15 600 artifacts. About half the parts carry one injected fault (wrong revision, missing approval, conflicting
spec, wrong unit, expired certificate or out-of-tolerance measurement), which gives 2 317 findings. The bench then
times the work of `POST /verification/run` (detection plus commit, in a runtime-role tenant transaction):

- **first run**: no findings stored yet.
- **re-run without change**.
- **typical change + run**: one new inspection document for one part (4 claims), then a run, repeated 9×.

The "before" binary was built from c215db0 against its own database (migrations 0001–0021).

### Profile of the old full run (EXPLAIN ANALYZE, 66 k claims)

| step | cost |
|---|---|
| fact load (`claims` ⟕ `artifacts`) | 230 ms: index scan of 66 k claims plus 66 k artifact primary-key probes. The RLS predicate is evaluated per row. |
| finding writes | one `INSERT … ON CONFLICT` round trip per finding: about 2 300 statements, most of the remaining ~600 ms |
| stale-finding scan | 1 ms |
| rule evaluation | about 10 ms, but de-duplication was O(findings²) across the whole tenant (a linear `any()` over every earlier finding) |

**Valid-time GiST index (`tstzrange(valid_from, valid_to)`): not added.** Neither verification nor any other query
in `kizuna-api` filters on valid time; `valid_from`/`valid_to` are only stored and returned. An index nothing reads
would only slow every claim insert. The new indexes are the ones the incremental path needs (below).

### Changes

- **Change log + watermark** (migration `0022_incremental_verification.sql`). Triggers append a row to
  `verification_changes` in the same transaction as every claim insert, supersession or review-status change
  (`subject_type`, raw `subject_key`, `claim_id`). Every alias / entity-decision change appends a NULL-subject row.
  A run consumes the log with `DELETE … RETURNING` before it reads any facts. It records the highest consumed id as
  the tenant's `watermark` in `verification_state`, together with `last_day`, `last_full_at` and `runs_since_full`.
  The log is consumed by delete rather than read as "id > watermark" because a `bigserial` id can commit out of
  order, and an id-range read would silently skip that change.
- **Incremental run** (`verification::detect`, mode `auto`, the endpoint default):
  1. Canonicalise the changed subjects (`safe_normalize` plus aliases, exactly as before).
  2. Find every raw spelling of those subjects (`SELECT DISTINCT subject_key` over the tenant's part claims).
  3. Load only those subjects' facts and run the rules on them.
  4. Treat as stale only the open findings that reference a re-evaluated or changed claim. The lookup is by claim
     (new partial indexes `conflicts_open_claim_a/_b`), never by the stored subject key. A finding keeps the
     subject key of its first detection, so a key-based lookup could wrongly obsolete it after an alias change.
- **Full run** stays available as `POST /verification/run?mode=full` and `verification::detect_in_tx`. An `auto`
  run falls back to a full run in these cases:
  - no state yet (first run);
  - the tenant's calendar day changed (expiry findings depend on "today");
  - an alias or entity decision changed;
  - the last full run is older than 24 h;
  - every 100th run, as a periodic consistency pass.
- **Parallel by subject, bounded**: `conflict_engine::run_parallel` evaluates contiguous chunks of the ordered
  subject list on at most `min(cores, 4)` threads. It runs inside `block_in_place` on the multi-threaded runtime
  and merges results in subject order, so the output does not depend on the thread count. De-duplication now
  uses a hash set: first occurrence wins, as before.
- **Batched writes**: all findings go in one `INSERT … SELECT unnest(…) ON CONFLICT … RETURNING` statement.
  Reopened findings are still audited one by one (`conflict.reopened`), in finding order.
- **Fact-load query plan**: the first version used a single statement with `($2 IS NULL OR subject_key = ANY($2))`.
  On freshly loaded tables the planner estimated one row and chose a nested loop that rescanned the tenant's
  artifacts for every claim: **120 s** at 66 k claims, reproduced twice (once with autovacuum off). It is now two
  fixed statements that join on the artifact primary key. On the same freshly loaded data, that first run took
  0.95 s.
- `RunResp` gains `mode`, `subjects_evaluated`, `changes_consumed` and `watermark`. `findings_total` counts the
  findings of the evaluated subjects, which is every subject in a full run. The web UI reads only `findings_new`
  and `marked_obsolete`, which mean the same as before.

### Equivalence (incremental ∘ changes == full)

- `crates/domain-manufacturing/tests/incremental_property.rs` (proptest, 256 cases, real rule set). Random facts,
  then 1–8 steps of random additions and removals. Applying "drop the dirty subjects' findings, re-run only their
  facts in parallel (1–4 threads)" after each step equals a full run as a set (subject, kind, claims,
  explanation). `run_parallel` equals `run` exactly. Mutation check: no longer marking a removed fact's subject
  as dirty makes the property fail.
- `crates/kizuna-api/tests/incremental_verification.rs::incremental_matches_full` (real database, 4 seeds × 30
  steps). The steps mix:
  - claim inserts in several spellings (`BRK-1`, `brk-1`, full-width `ＢＲＫ-1`, `HSG3`/`HSG-3`), from four source
    kinds and manual entry;
  - supersessions, rejections and re-acceptances;
  - human conflict reviews (`needs_evidence` / resolved);
  - alias decisions and calendar-day advances.

  At every step the conflicts table after the `auto` run equals the table a rolled-back full run produces from the
  same state: kind, claims, status, subject key and explanation. A full run afterwards inserts and obsoletes
  nothing. More than 60 of the runs were incremental. Mutation check: dropping changed claim ids from the stale
  lookup fails the test.
- `endpoint_modes_and_watermark`:
  - first run is `full`;
  - a run with no change evaluates 0 subjects;
  - one new claim → 1 subject, watermark advances;
  - `?mode=full` works.

### Results: 65 968 claims, 3 900 parts, 2 317 findings (three interleaved rounds)

| | before (c215db0, always full) | after: `auto` (incremental) | after: `?mode=full` |
|---|---|---|---|
| typical change + run, p50 per round | 907 / 740 / 971 ms | **34 / 23 / 19 ms** | 268 / 347 / 302 ms |
| typical change + run, max per round | 1 008 / 987 / 1 153 ms | **47 / 33 / 29 ms** | 287 / 412 / 399 ms |
| re-run without change, p50 | 987 / 872 / 745 ms | 8 / 9 / 3 ms | 273 / 313 / 307 ms |
| first run (inserts all findings) | 1 807 ms | 508 ms (falls back to full) | 341 ms |

The target was under 1 s for a typical incremental run at 68 k claims. Measured: 19–34 ms p50, at most 47 ms, which
is about 30× faster than the old full run on the same data. The full run itself is about 2.8× faster (batched
writes, hash-set de-duplication).

Scale check, after only, 15 600 parts / 263 877 claims:

| run | time |
|---|---|
| incremental change + run, p50 | 121 ms |
| full re-run, p50 | 1 029 ms |
| full change + run, p50 | 1 294 ms |

The incremental figure grows with the tenant because of step 2 (the distinct-key scan). The rest of the
incremental run is proportional to the change.

**About the soak's 13.8 s.** Run in isolation, the old code needs only 0.75–0.99 s at this size, so most of the
soak's 13.8 s p50 / 22 s max was time spent queued behind the per-tenant advisory lock under the 8-worker mixed
load. Runs still serialise on that lock, so after this change a run holds it for about 20–50 ms instead of about
1 s. The soak was not re-run, so its figure under load has not been re-measured.

## 2. Orphan objects on database failure during upload

### Before

The object was written to its final path, then the database transaction ran. If the database died before COMMIT,
the stored blob had no row: the chaos campaign found 2 orphans after 144 connection kills.

### After

Write order: stage → commit the row → promote (`storage.rs`, `artifacts.rs`).

- **Stage**: `ObjectStore::stage` writes and fsyncs the object to `<root>/.staging/<tenant>.<sha256>.<uuid>`, on
  the same filesystem as the store. A staged object is not visible under its key.
- **Promote**: after `COMMIT`, `promote` renames it to `<root>/<tenant>/<hh>/<sha256>` and fsyncs both
  directories. Promotion is idempotent: if an intact copy is already stored, it just drops the staged one.
- **Failure before COMMIT**: the rollback is certain, so a drop guard discards the staged copy immediately.
- **COMMIT fails**: the outcome is unknown (the connection may have died after the server committed). The staged
  copy is kept.
- **Promotion fails after a successful COMMIT**: the error is logged and the client still gets its success.
- **Reads**: `get` serves an intact staged copy while the final object is missing (the instant between COMMIT and
  rename, or after a crash in between). Callers only ever ask for keys that have a row.
- **Staging sweep** (`storage::sweep_staging_job`, every 10 min in `kizuna-server`):
  - a staged copy whose `(tenant, sha256)` has an artifact row is promoted;
  - a copy without a row and older than `KIZUNA_STAGING_MAX_AGE_HOURS` (default 6, range 1–720) is deleted;
  - younger row-less copies are left alone, since an upload may still be in flight;
  - each tenant with work done gets one audit event, `storage.staging_swept`, with counts only:
    `{promoted, deleted, max_age_secs}`.
- `put` remains, as stage plus promote, and discards the staged copy if promotion fails. The disk-full guarantee
  (no partial temp files) still holds for staging writes.

### Tests

- `storage.rs` unit tests:
  - staged objects are invisible until promoted, are still served by `get`, and promote idempotently;
  - the sweep promotes copies that have a row, keeps young orphans and deletes old ones;
  - a failed `put` leaves neither a temp file nor a staging copy.
- `staging_orphans.rs::failed_commits_leave_no_orphans_and_sweep_promotes_committed`. A deferred constraint
  trigger makes one upload fail at COMMIT (500). Checked:
  - its staging copy is kept and later deleted;
  - a committed-but-unpromoted object is promoted;
  - afterwards every row has an intact object, no object lacks a row, and staging is empty;
  - two count-only audit events were written.
- **Chaos**: `staging_orphans.rs::chaos_connection_kills_during_uploads_leave_no_orphans` (`--ignored`; needs
  `KIZUNA_CHAOS_PG_SUPERUSER_URL` of a private PostgreSQL). 8 workers × 40 uploads run while a task calls
  `pg_terminate_backend` on every app backend every 150 ms. A sweep with no age cut-off then runs, and the test
  checks for orphans and missing objects:

| run | uploads ok / failed | backends killed | staging copies deleted by the sweep | orphans | missing objects |
|---|---|---|---|---|---|
| 1 | 255 / 65 | 87 | 1 | **0** | **0** |
| 2 | 274 / 46 | 56 | 0 | **0** | **0** |
| 3 | 251 / 69 | 90 | 3 | **0** | **0** |
| 4 | 270 / 50 | 60 | 4 | **0** | **0** |

The deleted staging copies are uploads whose COMMIT outcome was unknown. Under the old write order each of these
would have been an orphan object. Every acknowledged upload has its row and its object.

## 3. Gate

- `scripts/check.sh` (with `KIZUNA_ENV_FILE` pointing at the private database), final tree: all steps passed
  (fmt, clippy `-D warnings`, tests, cargo audit/deny, pnpm audit, pip-audit, Python, web, shellcheck, ops script
  tests, gitleaks). Rust tests: 460 passed / 0 failed / 2 ignored. The 2 ignored are the chaos test above and one
  that existed before.

## 4. Reproduce

```
initdb (as an unprivileged user) + roles kizuna / kizuna_app + database, then:
KIZUNA_BENCH_OWNER_URL=… KIZUNA_BENCH_APP_URL=… target/release/kizuna-verify-bench --runs 9 --mode incremental|full [--parts 3900] [--slug S]
KIZUNA_CHAOS_PG_SUPERUSER_URL=postgres://<superuser>@127.0.0.1:<private port>/<db> \
  cargo test -p kizuna-api --test staging_orphans -- --ignored --nocapture
```

Never point the chaos test at a shared PostgreSQL: it terminates every `kizuna_app` backend of the database.
