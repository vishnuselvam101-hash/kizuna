# Chaos, soak, mutation and fuzz campaign — 2026-11-05

Base: `main` 0758242. Branch `chaos` (not pushed). Host: shared dev VPS, release build, **private** PostgreSQL 18
instance (`initdb` as `nobody`, 127.0.0.1:54033) — the system PostgreSQL was never touched. Fakes: `scripts/chaos/fakes.py`
(clamd on a unix socket, OpenAI-compatible AI endpoint; modes switched at runtime). Load: `scripts/chaos/soak.py`
(stdlib, 8 workers, E2E generator data: 400 seeds × 52 clean documents, reviewers enrolled in TOTP).

## 1. Soak — 30 min mixed load

Mix: upload+extract 30 %, claim review 15 %, conflict list/get/review 10 %, verification run 4 %, decision
create/trace/report 10 %, supersede 8 %, org report 3 %, dashboard/artifact list 15 %, audit verify 2 %.

| metric | value |
|---|---|
| requests | 49 617 (27.5 req/s) |
| 5xx / transport errors | **0** |
| documents uploaded + extracted | 8 340 (68 079 claims, 21 850 audit events) |
| decisions recorded / reported | 1 129 |
| overall latency p50 / p95 / p99 | 31.6 / 1 397 / 2 616 ms |
| upload p50 / p95 / p99 | 30.7 / 92 / 181 ms |
| extract (sandboxed worker) p50 / p95 / p99 | 967 / 2 069 / 2 799 ms |
| verification run p50 / p95 / p99 | 1 310 / 7 998 / 10 834 ms |
| dashboard p50 / p95 | 198 / 716 ms |
| decision report p50 / p95 | 264 / 864 ms |
| server RSS | 32 MB → 552 MB (flattening: +6 MB in the last 3 min) |
| open fds | 12 → 33–41 (no growth) |
| DB connections | 10 → 14–16 (pool bound, no growth) |
| extraction children at end | 0 |
| after run: half-written state | 0 artifacts without capture, 0 claims without a run, 0 runs without claims |
| orphan objects / missing objects / temp files | 0 / 0 / 0 |
| audit chains + artifact hashes (`kizuna-restore-verify`) | intact, 8 340 / 8 340 |

RSS check: a fresh server against the same 68 k-claim database reached 264 MB within 10 s and plateaued at ~500 MB over
4 min — the footprint follows the data size (whole-tenant reports/detection), not request count: no leak found.

Findings (not fixed, performance): `POST /verification/run` re-evaluates the whole tenant and is serialised by an
advisory lock; at 68 k claims it took p50 13.8 s / max 22 s (4-min re-run). Dashboard p50 677 ms and decision report p50
691 ms at that size. Worth incremental detection before tenants reach ~10 k documents.

Test-tool notes: the first soak attempt tripped the (correct) `authorization_probing` rule because the driver had the
reviewer review claims it had recorded itself (`no_self_review`); the driver now supersedes as the analyst. Audit
verify and org report are rate-limited (429s in the table above are expected).

## 2. Chaos — each scenario on a fresh database, invariants + `kizuna-restore-verify` after each

| scenario | result before fix | after fix |
|---|---|---|
| private PostgreSQL `pg_ctl stop -m fast`, 6 s down, mid-load | 4× 500 `internal_error`; recovered; no half-written state; chain intact | 1× **503 `database_unavailable`** + Retry-After; same integrity |
| `pg_terminate_backend` of all app backends, 15× every 2 s (144 killed) | 16× 500; recovered; chain intact; 2 orphan objects (blob written, tx rolled back) | 503 instead of 500 |
| storage disk full (1 MB tmpfs) | 23× 500, **23 `.tmp-*` partial files left**, disk stuck at 100 % | 507 `storage_full`, 0 temp files |
| extraction worker SIGKILLed (70 kills during load) | 69 runs `failed`, 400 `extraction_failed`, no stray processes, chain intact | — |
| extraction worker hangs (timeout 5 s) | 400 after 5.1 s, queued ones after 10/15 s (2 permits), `timeout` recorded, 0 children left | — |
| clamd ok / hang / reset / ERROR / down / socket missing | fail closed (quarantined, ≤ timeout); **after 5 outage quarantines the uploader was auto-restricted (`upload_abuse` containment)** | outage recorded as `ingest.scanner_unavailable`; uploads keep working |
| AI slow / garbage / >256 KB / 500 / 429 | extraction 200 with rules-only fallback + `ai_fallback:<code>` warning, bounded by the AI timeout | — |
| client disconnects during extraction (slow AI) | **advisory lock leaked on a pooled connection**: `extraction_in_progress` for that artifact until the connection died | lock released; retry succeeds |
| SIGTERM during an upload | **process died (exit 143), in-flight request cut off** | request completes (200), exit 0 |
| clock jumps ±days (app clock vs DB clock, `OffsetClock` test) | expired tokens refused, fresh login works, upload/extract work, audit intact | — (test added) |

Orphan objects on connection loss remain by design (object is stored before the DB commit; never a missing object).
They are unreferenced blobs; a report-only orphan sweep would be a useful follow-up.

## 3. Mutation testing

`cargo-mutants` is not installed: 21 targeted manual mutants of critical conditions (`/scratchpad mutate.py` runner:
apply, `cargo test -p <crate>`, revert). First pass: 9 killed, **12 survived**: policy (review decisions not MFA-gated;
auditor gains ClaimWrite), dependency (informational withdrawal material; threshold `>`→`>=`; material-wins dedup),
conflict (empty revisions comparable; duplicate findings), evidence (PDF without `%%EOF`; zip ratio bomb; zip entry
limit off-by-one; CSV line limit). Ten new tests (commit "Tests: kill 11 surviving mutants") kill all 11 rechecked
survivors; P2/P1/P4, A1–A5, D3, E5 were already killed. Survivors left: 0 of 21.

## 4. Fuzzing (`scripts/fuzz.sh` properties, PROPTEST_CASES=300000, release)

`scripts/fuzz.sh 300000` exceeds its 600 s cargo budget, so the same packages were run one by one: audit-engine 7 (34 s),
evidence-core 16 (33 s), entity-resolution 5 (264 s), conflict-engine 4 (18 s), domain-manufacturing 25 (214 s),
dependency-engine 9 (5 s), kizuna-api lib 43, extraction worker pytest 60 — all passed, no crash or shrink.

## 5. Fixes on `chaos` (each with a failing test first)

1. DB outage → 503 `database_unavailable` + Retry-After; full store → 507 `storage_full` (`error.rs` unit tests).
2. `FsStore::put` removes its temp file when write/sync/rename fails (`storage.rs` unit test).
3. Extraction lock connection is closed (not pooled) when the request is cancelled (`cancelled_extraction_releases_its_lock`).
4. `kizuna-server` drains and exits 0 on SIGTERM (`sigterm_drains_in_flight_requests_and_exits_cleanly`).
5. Scanner-outage quarantines are `ingest.scanner_unavailable`, not upload abuse (`scanner_outage_quarantines_do_not_restrict_uploaders`).

Full workspace suite after fixes: 222 passed, 0 failed (private PostgreSQL); clippy `-D warnings` and fmt clean.
