# Stability: idempotency, retry-safe jobs, backpressure, metrics, 30-minute soak — 2026-11-06

Base: `main` 3c3df2a, branch `stability` (main 9d1a3b6 merged in; not pushed). Builds on
[CHAOS_SOAK_2026-11-05](CHAOS_SOAK_2026-11-05.md) and [PERF_VERIFY_2026-11-05](PERF_VERIFY_2026-11-05.md): graceful
SIGTERM, staged uploads and 503 on database outage were already there and are not repeated here. All database work
ran on a **private** PostgreSQL 18 (`initdb` as `nobody`, first under `var/pg` of the worktree, later moved to
`/var/tmp/kizuna-stab-pg` because `/root` is not traversable by `nobody`; 127.0.0.1:54066; databases
`kizuna_test_stab`, `kizuna_stab_smoke`, `kizuna_stab_soak2`). The system PostgreSQL and system services were not
touched. Shared 8-core dev VPS with other jobs running: treat latencies as ±30 %.

## 1. Idempotency-Key (`idempotency.rs`, migration `0031_stability.sql`)

- Every POST/PUT/PATCH/DELETE under `/api/v1` with an `Idempotency-Key` header (1–200 visible ASCII) from a live
  user **or supplier** session. Stored per (tenant, principal kind, principal, key) with SHA-256 of method, path,
  query and body, for **24 h**. RLS-forced table.
- First request claims the key `in_progress` (lock 120 s), runs the handler, stores status, content type/disposition
  and the body **sealed with the data key** (AES-GCM, AAD = tenant/principal/key).
- Same key + same request, completed → stored response replayed (`idempotent-replayed: true`), handler not run.
- Same key + different request (body, path or method) → **422 `idempotency_key_reused`**.
- Same key while the first is running → 409 `idempotency_in_progress` + `Retry-After: 1`.
- "Not executed" answers (401, 403, 408, 429, 5xx) are not stored: the key is released and a retry runs.
- A dead holder's `in_progress` row is taken over by the same request after its lock lapses. GC job deletes
  expired keys.
- Excluded: `/auth/*`, portal login/logout/invite (no principal yet, credential responses).
- Known gap (documented in the module): a crash after the handler committed and before the response was stored lets
  a retry after 120 s run the handler again (separate transactions). Bodies > 4 MiB are not kept (replay → 409
  `idempotency_response_not_stored`).
- Admission runs before idempotency (so overload never buffers bodies): a replay can get 429 `server_busy`, which
  means "retry later", not a different answer.

## 2. Retry-safe background jobs (`jobs.rs`)

- `jobs` table (tenant RLS; owner-defined `kz_jobs_ready_tenants()` / `kz_jobs_depth()` return only ids/counts
  across tenants). Claim = `UPDATE … WHERE id = (SELECT … FOR UPDATE SKIP LOCKED)` with a **lease** (300 s);
  completion only by the lease holder; an expired lease is reclaimed. `attempts` counts claims, so a job that kills
  its worker is counted too.
- Failure → backoff 2^attempts s (≤ 5 min). After **5** attempts → `quarantined` + incident `job_quarantined` (high)
  with a pre-authorised `notify_admins` action (delivered by the alert job) + audit `job.quarantined`. A handler
  panic or overrun fails the attempt instead of killing the worker. `GET /jobs?state=…`, `GET /jobs/{id}`,
  `POST /jobs/{id}/requeue` (incident managers).
- Idempotent handlers: `review_batch.run` (each step commits atomically; enqueued in the batch's own transaction —
  outbox — plus a scheduler safety net), `verification.run` (`POST /verification/jobs` → 202; detection is
  idempotent), `gc.tenant` (expired keys, finished jobs > 24 h), `alerts.deliver` (each executed `notify_admins`
  action marked `alerted_at` after sending; at-least-once). Enqueue is deduplicated per (tenant, kind, key) while
  queued.
- Server: 2 workers + scheduler (30 s, GC hourly) on a **separate job pool** (default 6); the incident, staging and
  CAPA-overdue sweeps moved onto it too. The 2 s review-batch poller and the in-memory alert cursor are gone.

## 3. Pool sizing, statement timeouts, backpressure (`admission.rs`)

- Route classes by method + path: auth, read, write, heavy (verification run, reports, evidence pack, audit verify,
  simulations, detection, dashboard, decision report), upload, extract, portal, ops; job for workers.
- `statement_timeout` per class, set in the same `set_config` round trip as the RLS tenant (task-local scope, no
  handler changes): auth 5 s, read/write 15 s, heavy 55 s (below the 60 s request timeout), upload/extract 30 s,
  portal 10 s, ops 2 s, job 120 s. SQLSTATE 57014 → **503 `database_timeout`** + Retry-After. `KIZUNA_STATEMENT_TIMEOUTS`.
- Bounded admission (semaphore + bounded wait queue + max wait) → **429 `server_busy`** with `gate` and Retry-After:
  upload 8 running/16 waiting/15 s, extract 2/6/20 s (= sandbox workers), heavy 3/6/20 s. Heavy may hold at most
  half the request pool (config refuses more). `KIZUNA_GATE_*`, `KIZUNA_DB_POOL_SIZE` (16),
  `KIZUNA_DB_JOB_POOL_SIZE` (6), `KIZUNA_JOB_WORKERS` (2).

## 4. Metrics and health (`metrics.rs`)

- Prometheus text on `KIZUNA_METRICS_BIND` (default 127.0.0.1:9464, **loopback only** — config refuses anything
  else; `off` disables): request counts by class/method/status class, latency histograms per class, in-flight,
  admission in-use/waiting/rejected, request and job pool idle/in-use/max, job queue depth by state, job runs by
  kind/outcome and duration, idempotency outcomes, RSS, open fds, uptime, draining. Bounded labels only.
- `/healthz` = liveness (unchanged); `/readyz` = not draining + DB answers within 2 s + object store present (503
  with the failing check). SIGTERM sets draining before connections drain.

## 5. Tests

New `tests/stability.rs` (9): replay/mismatch/per-user scope/sealed body/invalid key; in-progress 409, lock
takeover, 403 not stored, 400 stored and replayed, expiry and GC; idempotent upload (no second audit row); poison job
backoff → quarantine → incident + notify action + audit → requeue; lease expiry/takeover/lost lease and
crash-looping job quarantined without running; verification + GC as jobs (re-run harmless); statement timeout per
class (`SHOW statement_timeout`, 57014 → `DbTimeout`); 429 with Retry-After on full upload/extract queues;
metrics content and readiness/draining. Plus portal Idempotency-Key (`supplier_portal.rs`), review-batch outbox job
(`review_batches.rs`), concurrent CAPA open (`capa_8d.rs`), extraction lease tests (`chaos_regressions.rs`), unit
tests in `admission.rs`, `metrics.rs`, `idempotency.rs`, `error.rs`.

Gate on this branch (2026-11-06, private DB `kizuna_test_stab`, `scripts/check.sh` steps with `--no-fail-fast`):
fmt + clippy `-D warnings` pass; **564 Rust tests pass, 2 failed, 2 ignored** — `kizuna-messages` catalog (six new
error codes had no EN/JA web text; added in `web/src/i18n.tsx`, then 3/3 pass) and `whatif::hundred_thousand_edges_…`
(`database_timeout` after the 55 s heavy statement timeout while the host load average was 9–16; passes alone,
5/5 in 28 s; the same test was already known to exceed the 60 s request timeout on a loaded host). cargo audit/deny,
pnpm audit, pip-audit 0; ruff + 132 pytest; tsc + 111 vitest; shellcheck; ops script tests; gitleaks: pass.

## 6. Soak (measured)

`scripts/chaos/stability-soak.sh WORKDIR MINUTES` (release build, fresh database, 8 driver workers, `--stability` mix,
200 synthetic datasets). Runs on this host:

| Run | Length | Outcome |
|---|---|---|
| smoke (`var/smoke`) | 3.0 min | 5 443 requests, **1 × 500** (`POST /verification/jobs`: enqueue raced a worker claiming the queued twin → fixed in `jobs::enqueue`, retry loop) |
| first long run (`var/soak`) | **aborted after ~41 min** | the session running it died; the private PostgreSQL was shut down at 01:09 and the driver never wrote `result.json`. No results are claimed from it. |
| **30-minute soak** (`var/soak30`, after the fix) | 30.03 min | below |

The planned 2-hour soak was **not** completed; the longest finished run is 30 minutes.

30-minute soak (2026-11-06 01:19–01:49 UTC). The host was not idle: two release/debug cargo builds and the
seccomp-profile replay of another lane ran during the soak, so latencies are pessimistic and not comparable with
PERF_VERIFY.

| Metric | Result |
|---|---|
| requests | 29 706 (16.5 req/s), **0 × 5xx or transport errors** |
| work done | 2 489 uploads extracted, 455 decisions, 358 verification-job requests (all 202; deduplicated into 335 jobs, all `done`), 214 CAPA cases opened, 707 supplier-portal uploads |
| overall latency | p50 31.8 ms, p95 4 249 ms, p99 7 217 ms (the tail is extraction: p50 4.5 s, p95 8.6 s, max 14.7 s, queued behind the 2-slot extract gate — `admission_waiting{gate="extract"}` 5–6 throughout) |
| other ops p95 | claims list 174 ms, dashboard 276 ms, decision report 617 ms, verification run 174 ms, upload 124 ms |
| idempotency | 4 685 keyed mutations, 998 same-key retries replayed, **0 mismatches** (server: 4 656 stored, 998 replayed, 0 mismatch, 0 in-progress) |
| backpressure / limits | admission gates rejected **0** (upload, extract, heavy: callers waited, extract queue 0–6 deep). All 429s came from the pre-existing per-user rate limits, not the new gates: audit verify 161/243 and org report 194/277 (heavy limit 6/min per user, 20/min per tenant), supplier portal reads/uploads 72–149 each |
| business refusals (expected) | decision create 409 × 634 (duplicate/four-eyes), CAPA open 409 × 187 (`case_already_open`), review 409 × 4 |
| pools | request pool peak 11 open of 16, job pool peak 5 of 6 (metrics sampled every 10 s); job queue 0 queued / 0 running at the end, **0 quarantined** |
| server RSS | 60 MB → 81 MB (first 15 min) → step to 203 MB at ~16 min → 220 MB at 30 min. Fds 19–46, threads 11–19. 30 min is too short to rule out slow growth; not called leak-free |
| drain + shutdown | jobs drained, SIGTERM → exit 0 |
| integrity (`kizuna-restore-verify`) | ok: 3 196/3 196 objects intact, all audit chains intact |
| server log | 0 ERROR lines |

Driver note: the sampler's DB-connection count (`db_conns`) was empty in this run (psql printed a table, not a bare
number); fixed afterwards (`-qAt`). Pool usage above comes from the metrics endpoint instead.

## 7. Open gaps

- 2-hour (or longer) soak still to run on an otherwise idle host; RSS trend beyond 30 min unknown.
- Idempotency crash window (handler committed, response not stored) — see §1.
- The object store was not changed here: staged uploads + rename + directory fsync (CHAOS_SOAK_2026-11-05) remain the
  crash-consistency mechanism; no new crash test in this lane.
