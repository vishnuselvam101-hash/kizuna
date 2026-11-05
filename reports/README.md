# Run reports (M6)

Every file here was written by a real run of the named script; nothing is hand-edited. Provenance (command, commit,
host, time) is inside each JSON. `dirty: true` / `worktree_dirty: true` on 2026-11-05 means only that earlier
untracked report files were present; code was at the recorded commit.

| Dir | Script | 2026-11-05 result (commit 8308073) |
|---|---|---|
| `e2e/` | `scripts/e2e.sh` | seed 7 / 24 parts and seed 11 / 48 parts: rule-finding precision 1.00, fault recall 1.00, duplicate ids, malicious uploads and unauthorised requests all as expected |
| `bench/` | `scripts/bench.sh` | see the `.md`; MEASURED on a shared 8-vCPU host with load average ~10 (other builds running) — treat as an upper bound and re-measure on idle pilot hardware |
| `drill/` | `scripts/restore-drill.sh --tamper-test` | backup of the seed-7 E2E database: restore 1.2 s, verify 0.1 s, 2 tenants, 125 audit events, 54/54 artifacts intact; altered object detected; wrong passphrase refused |

Findings worth acting on:
- Entity-resolution candidate search is all-pairs (O(n²)): 2 000 part keys take ~3.6 s (P50). `GET /entities/candidates`
  reads up to 5 000 keys, which extrapolates to ~20 s+ per request on this host (60 s request timeout). Needs
  blocking/indexing before tenants with thousands of part numbers.
- Under 64 concurrent keep-alive connections evidence lookups stayed error-free; latency is dominated by the
  16-connection database pool and host load.
- First precision run was 0.83: the scorer had not listed `missing_approval` as an expected consequence of a
  wrong-revision PO (the ordered revision genuinely has no approval). The answer key was corrected; the rule was not.
