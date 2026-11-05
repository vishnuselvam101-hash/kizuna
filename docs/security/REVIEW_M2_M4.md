# Security and correctness review: M2 (bd00639), M3 (6301e19), M4 (375126e)

Date: 2026-11-05. Read-only review. No code was changed.
Checklists used: security-and-hardening (threat model, uploads, authz, headers, errors, SSRF, sandbox) and
code-review-and-quality (correctness first, tests, how findings are labelled).
I checked each finding by reading the code path. Line numbers refer to HEAD = 375126e.

Severity: **High** = must be fixed before merge or release (the skill's "Critical/required"). **Medium** = required.
**Low** = should be fixed ("Consider"). **Info** = for information only.

Counts: High 2 · Medium 7 · Low 9 · Info 4

---

## High

### H1. Four-eyes review can be bypassed: one Reviewer can approve their own evidence and waive their own findings
- **Where:** `crates/kizuna-api/src/verification.rs:600` (`owner = if method == "manual" {recorded_by} else {None}`), `verification.rs:547` (`review_conflict` has no owner check), `crates/kizuna-api/src/decisions.rs:673` and `:781` (reconfirm and revoke have no check against `decided_by`), `crates/policy-engine/src/lib.rs:74-79` (the Reviewer role has ArtifactUpload, ClaimWrite, ReviewDecide and DecisionWrite).
- **Scenario:** A single Reviewer does all of the following:
  1. Uploads a CSV they wrote.
  2. Calls `POST /artifacts/{id}/extract`. The claims get `extraction_method = worker:...`, so `NoSelfReview` sees no owner.
  3. Approves those claims with `POST /claims/{id}/review`.
  4. Runs detection, then waives every finding on those claims with `POST /conflicts/{id}/review approve`. Waivers have no owner rule at all.
  5. Records a decision on them and later reconfirms that decision themselves.

  The only rule that stops self-review is the one for hand-entered claims, and step 2 avoids it.
- **Fix:**
  - Set the claim owner to the uploader of the source artifact (`artifacts.created_by`, and every `artifact_captures.uploaded_by`) and to whoever triggered the extraction.
  - For conflicts, deny review when the reviewer recorded either claim or uploaded either source artifact.
  - For decisions, deny reconfirm and revoke when `decided_by == reviewer`.
  - Enforce all of these through `require_on` with a `Resource` owner set, so the rule sits in the policy engine.
- **Tests to add:**
  - Reviewer uploads, extracts, then approves an extracted claim → 403 `no_self_review`.
  - Reviewer waives a conflict on a claim from their own upload → 403.
  - Decision author reconfirms their own decision → 403.

### H2. A finding that became `obsolete` can never fire again, so findings can be suppressed permanently
- **Where:** `migrations/0003_verification.sql:63` (the `conflicts_dedup` key does not include status), `verification.rs:337-339` (`ON CONFLICT ... DO NOTHING`), `verification.rs:367` (marks findings obsolete), `verification.rs:614-624` (a claim can be re-reviewed from any status).
- **Scenario:** Conflict X exists between claims A and B. A reviewer rejects A, so A drops out of `load_facts`. Running `/verification/run` (needs only ClaimWrite) then marks X `obsolete`. The reviewer approves A again. The next detection run finds the same `(kind, A, B)`, but the insert is a no-op, so X stays `obsolete`. No conflict review ever happened. The same thing happens for any temporary state change, for example an alias or a third claim, without anyone intending it.
- **Fix:** When a finding fires again and its existing row is `obsolete`, set it back to `open` (`ON CONFLICT ... DO UPDATE SET status='open' WHERE conflicts.status='obsolete'`, plus an audit event `conflict.reopened`). Also give claim review a state machine: rejected → accepted only through supersession, or with a second reviewer.
- **Test to add:** Create conflict → reject claim A → run detection (finding is obsolete) → approve A → run detection → the conflict is `open` again.

## Medium

### M1. A correction that changes revision, scope, subject or predicate is judged "not material"
- **Where:** `crates/dependency-engine/src/lib.rs:94` (`materiality` compares only the value), `lib.rs:139` (the Superseded branch never calls `applicable` and never compares revision or scope). `Snapshot` has no subject or predicate (`decisions.rs:26`).
- **Scenario:** A decision depends on claim "length = 10 mm, rev C, supplier S1". The claim is superseded by "length = 10 mm, rev D", or by one with scope supplier S2, or even by a claim about a different part or predicate with the same value. The engine returns `same quantity` and the decision stays `valid`.
- **Fix:**
  - In the Superseded branch, treat it as material when `!applicable(&d.applicability, new)`, when the revision differs, or when the scope differs.
  - Add subject and predicate to `Change::Superseded` and treat any change of either as material.
  - Alternatively, forbid supersession across subject or predicate in `claims::supersede`.
- **Test to add:** Supersede with the same value but a new revision, or a new supplier scope → the decision becomes `potentially_affected`.

### M2. Reconfirming a decision breaks the links between dependent decisions
- **Where:** `decisions.rs:712` (an upstream decision that is not `valid` is dropped) and `decisions.rs:746` (the old version is set to `superseded`). Reconfirm does not move dependants over to the new version.
- **Scenario:** D2 depends on D1, and both are flagged.
  - If D2 is reconfirmed first, D1 is `potentially_affected`, so the D2→D1 edge is dropped with only a note.
  - If D1 is reconfirmed first, D1 becomes `superseded`, and reconfirming D2 drops the edge anyway, because the status is not `valid`.

  Either way, a later revoke or invalidation of D1' never reaches D2'. Selective invalidation silently stops working.
- **Fix:** In reconfirm, follow `supersedes` forward to the current version of each upstream decision (as `current_version` does for claims). Refuse to reconfirm while an upstream decision is still `potentially_affected` (409 `upstream_unresolved`) instead of dropping it.
- **Test to add:** D1 ← D2 → change D1's evidence → reconfirm D1, then D2 → revoke D1' → D2' becomes `potentially_affected`.

### M3. Race: a decision can be committed on a claim that a concurrent request is superseding or rejecting
- **Where:** `decisions.rs:473` (`claims::fetch` takes no lock, and upstream `fetch` at `:491` takes none either). `claims::supersede` and `review_claim` lock the claim `FOR UPDATE` and run `on_claim_change` in their own transaction.
- **Scenario:** The transactions run at READ COMMITTED. A supersede commits after `create` has read the claim but before `create` commits. The supersede's invalidation scan cannot see the uncommitted dependency row, so the new decision depends on a superseded claim and is never flagged.
- **Fix:** Use `SELECT ... FOR SHARE` on each evidence claim and on each upstream decision in `create` and `reconfirm`. Re-check `superseded_at IS NULL` and the review status after taking the lock.
- **Test to add:** Two concurrent transactions (barrier): create a decision and supersede its claim at the same time → the final decision status is `potentially_affected`, or the create fails with 409.

### M4. A hand-entered claim can pretend to come from a document
- **Where:** `crates/kizuna-api/src/claims.rs:145` (`artifact_id` comes from the client), `claims.rs:86-91` (checks only that the artifact exists; quarantined artifacts are allowed), `evidence-core/src/claim.rs` (`source_location` and `confidence_bps` come from the client), `verification.rs:267` (`source_kind = a.kind` drives the rules, see `domain-manufacturing/src/rules.rs:25`).
- **Scenario:** An Analyst records a manual claim with `artifact_id` set to a purchase order or inspection report, plus a made-up `source_location` and `confidence_bps: 10000`. The rules treat it as purchase-order or inspection data. Trace and report then show the document's SHA-256 next to it, which makes the claim look backed by evidence.
- **Fix:**
  - For `extraction_method='manual'`, use `source_kind='manual'` in `load_facts`, whatever the `artifact_id`.
  - Refuse `artifact_id` on artifacts that are not `accepted`.
  - Cap manual confidence, or ignore the client value.
  - Label manual links as "cited by user" in trace and report.
- **Test to add:** A manual claim that cites a PO artifact does not take part in PO-specific rules, and its report row says "manual".

### M5. The PDF active-content scan is easy to get past, so an active PDF is marked Clean and served
- **Where:** `crates/evidence-core/src/scan.rs:49-83` (raw byte search for `/JavaScript`, `/Launch` and the like).
- **Scenario:** The scan misses PDF name escapes (`/J#61vaScript`, `/L#61unch`) and dictionaries hidden inside compressed object streams (`/ObjStm` with FlateDecode). The file gets verdict `clean`, status `accepted`, and is downloadable through `/artifacts/{id}/content` with no warning. ADR 0011 says the scan is not antivirus, but the verdict is presented as one.
- **Fix:**
  - Decode `#xx` name escapes before matching.
  - Quarantine any PDF that contains `/ObjStm`, `/EmbeddedFile`, `/OpenAction` or `/AA` unless a real parser (inside the sandbox) confirms no actions.
  - Or rename the verdict to "structurally plausible" in the UI and on download.
- **Test to add:** Upload a PDF containing `/J#61vaScript`, and one whose `/OpenAction` sits inside a compressed ObjStm → both are `quarantined`.

### M6. Gaps in the extraction sandbox, which runs C image parsers on attacker files
- **Where:** `crates/kizuna-api/src/extraction.rs:105-160`. prlimit sets no `--nproc`, there is no seccomp filter, and nested user namespaces are allowed (bwrap 0.11.1 is installed and supports `--disable-userns`). `documents.py` runs `tesseract` (leptonica, libtiff, libpng, libjpeg) on images uploaded by users.
- **Scenario:** A parser memory-corruption bug in tesseract or libtiff gives code execution inside the sandbox. From there:
  - The attacker can fork-bomb. RLIMIT_NPROC counts per real UID on the host, which is the same UID as the API server, so the API can no longer spawn threads.
  - The attacker can create nested user namespaces, a common route to kernel privilege escalation.
  - All syscalls are reachable.
- **Fix:**
  - Add `--nproc=32` (or a pids cgroup through `systemd-run --scope -p TasksMax`).
  - Add `--disable-userns` (it needs `--unshare-user`).
  - Add a seccomp filter (`--seccomp FD`) that blocks ptrace, keyctl, bpf, unshare, mount and perf_event_open.
  - Consider running the worker as a dedicated UID.
- **Test to add:** In `sandbox_argv_isolates_network_and_filesystem`, assert that `--disable-userns` and `--nproc=` are present. Add an integration test where a worker that forks in a loop is refused.

### M7. Confirming a finding ("reject") does not reach decisions that depend on its claims
- **Where:** `verification.rs:558-568`. `review_conflict` sets `resolved_rejected` but never calls `on_claim_change`. Its own doc comment says reject means "the evidence/item is rejected".
- **Scenario:** A reviewer confirms that a value_mismatch finding is real. Decisions built on either claim stay `valid`, and `affected_by_claims` keeps listing them as merely "potentially" affected.
- **Fix:** On `reject`, require the reviewer to name the faulty claim(s), then reject them through the same path as `review_claim`. That emits `Withdrawn`. Or, at minimum, flag dependent decisions with cause `finding_confirmed`.
- **Test to add:** A decision on claim A, a conflict A/B is rejected → the decision becomes `potentially_affected`, and an invalidation event cites the conflict.

## Low

### L1. New-evidence matching ignores normalisation and aliases
- **Where:** `decisions.rs` `load_deps` matches `d.subject_key = $2` on the raw key. `new_evidence` uses `c.subject_key` as given.
- **Scenario:** A decision depends on `PN-100`. New disagreeing evidence about `pn 100`, or about a confirmed alias, flags nothing, even though conflict detection treats them as the same part.
- **Fix:** Store `safe_normalize` and alias-canonical keys in `decision_dependencies.subject_key`, and look up with the same canonicalisation.
- **Test to add:** Make an alias decision, then add new evidence under the alias key → the dependent decision is flagged.

### L2. Alias chains split entities instead of merging them
- **Where:** `verification.rs:729-745` and `:261-264`. The lookup is one level deep, and the right-hand key can already be canonical for other keys.
- **Scenario:**
  1. `same(A,B)` creates B→A.
  2. `same(C,A)` creates A→C.
  3. `load_facts` now maps B to A and A to C, so facts end up split across A and C.
- **Fix:** When inserting r→canonical, also re-point every alias whose canonical is r (`UPDATE entity_aliases SET canonical_key=$new WHERE canonical_key=$r`, which needs an UPDATE grant), or resolve to a fixed point. Reject cycles.
- **Test to add:** same(A,B), then same(C,A) → facts for A, B and C share one canonical subject.

### L3. `KIZUNA_EXTRACTION_SANDBOX=none` is accepted silently
- **Where:** `crates/kizuna-api/src/config.rs:133-136`.
- **Scenario:** One mistaken environment variable in production runs untrusted parsers with rlimits only. There is no log line and no health signal.
- **Fix:** Refuse `none` unless `KIZUNA_DEV=1` (or similar), log a warning at startup, and expose the mode in `/healthz` or a security event.
- **Test to add:** Config with `none` and no dev flag → `ConfigError::Invalid`.

### L4. The XLSX DTD/ENTITY refusal can be bypassed
- **Where:** `services/extraction/src/kizuna_extraction/tabular.py:67`.
- **Scenario:** Two ways past the check:
  - XML encoded as UTF-16 (with a BOM) defeats the byte search, because there are NUL bytes between the characters.
  - A `<!DOCTYPE` placed after more than 4 KB of comments is not searched.

  Expat's amplification limits and the sandbox limit the damage, but the ADR's claim that DTDs are refused does not hold.
- **Fix:** Decode using the XML declaration's encoding before the check, or use expat with `SetParamEntityParsing(NEVER)` and a handler that raises on `StartDoctypeDecl`. Alternatively, vendor `defusedxml` (pure Python).
- **Test to add:** A UTF-16 `sharedStrings.xml` containing `<!ENTITY` → `xlsx_dtd_refused`.

### L5. XLSX cell references are not bounded, so one crafted sheet can tie up an extraction slot
- **Where:** `tabular.py:78` (`_ref_to_col` has no limit) and `tabular.py:124` (`range(1, width+1)`). Sheets can also point to the same target up to 50 times.
- **Scenario:** One cell with `r="ZZZZZZZZZZ1"` makes the worker allocate until it hits the 3 GiB address-space limit. That holds one of the 2 worker permits for up to 45 s, and an Analyst can repeat it to starve extraction.
- **Fix:**
  - Reject columns past 16384 (XFD) and rows past 1048576.
  - Store rows sparsely.
  - Skip duplicate sheet targets.
  - Add a per-user rate limit on extraction.
- **Test to add:** A cell at `ZZZZ1` → `xlsx_bad_cell_ref` returned quickly.

### L6. Review actions do not require MFA
- **Where:** `crates/policy-engine/src/lib.rs:60-62` (`is_privileged` covers only UserManage and IncidentManage).
- **Scenario:** Waiving findings, merging entities, and reconfirming or revoking decisions all work from a session without MFA, for example a stolen password-only session.
- **Fix:** Treat ReviewDecide (or at least waive, reconfirm and revoke) as privileged, or require step-up re-authentication.
- **Test to add:** A Reviewer without MFA → 403 on `/conflicts/{id}/review`.

### L7. Gaps in the audit trail
- **Where:** `artifacts.rs:366` (content download writes only an integrity row, with no `artifact.downloaded` audit event); `decisions.rs:888` (the export security event leaves out `report_sha256`, so an issued report cannot be checked later); `artifacts.rs:229` (a duplicate upload logs the requester's `kind`, but the existing artifact keeps its original kind).
- **Fix:** Audit downloads, including size and hash. Put `report_sha256` and the format in the export event. For a recapture, log the stored kind.
- **Test to add:** After a download and a report export, the audit and security events contain the artifact id and the report hash.

### L8. The integrity claims are self-consistency checks
- **Where:** `decisions.rs:582`. `derivation_reproducible` re-hashes a value stored in the same row as its hash. `report_sha256` is unsigned, and the Markdown body is not covered by it.
- **Scenario:** Anyone able to change the row, which bypasses the trigger, can recompute the hash too. Anyone holding a report can edit it and recompute `report_sha256`.
- **Fix:** Cross-check `derivation_sha256` against the `decision.recorded` audit event, which is chained. Sign reports with `KIZUNA_AUDIT_SIGNING_KEY` when it is configured. Hash the Markdown output separately.
- **Test to add:** Change the stored derivation as superuser (trigger disabled) → the trace reports a mismatch with the audit chain.

### L9. Markdown injection in the verification report
- **Where:** `decisions.rs:922` (`md_escape` handles only `|`, newlines and `<`).
- **Scenario:** A decision title or rationale, or an uploaded `display_name`, contains `![x](https://attacker/beacon)` or `[click](javascript:...)`. Any Markdown viewer that opens the report then loads a tracking pixel or shows a phishing link.
- **Fix:** Escape `[ ] ( ) ! * _ \``, or wrap user text in inline code with backticks escaped.
- **Test to add:** A title containing `![a](http://x)` → the rendered Markdown contains no image syntax.

## Info

- **I1.** Two concurrent extracts of the same artifact both pass the "already_extracted" check (`verification.rs:114-125`). The loser hits the unique index and returns 500 `internal_error` instead of 409. Data stays consistent. Fix: map 23505 to Conflict, or take an advisory lock per artifact.
- **I2.** The worker's `MAX_INPUT` is 64 MiB of base64 JSON (`worker.py:26`), which is about 47 MiB of raw file, but `KIZUNA_MAX_UPLOAD_BYTES` can be up to 200 MiB (`config.rs:109`). Larger accepted uploads always fail extraction with `io_error`. Fix: tie the two limits together, or refuse extraction early with a clear code.
- **I3.** The CSV formula-prefix check (`scan.rs:138`) misses `-`, tab and CR, and it misses quoted fields (`"=..."`). Any future export must escape cells itself. `artifact_captures.client_ip` is stored forever in an append-only table and needs a stated retention period.
- **I4.** `POST /artifacts/{id}/verify` has no rate limit, and each call appends an `integrity_checks` row and reads the whole object.

## Checked and dropped (no finding)
- **RLS:** All 10 new tables have ENABLE and FORCE RLS with tenant policies. Updates are limited by column grants. Storage keys come from `p.tenant_id`. The M3 and M4 tests cover isolation.
- **Upload path traversal:** Objects are stored by content address (tenant UUID and sha hex). File names are for display only and are sanitised. Content-Disposition strips non-ASCII characters, quotes and control characters. `nosniff` is set globally.
- **ZIP bombs:** Python `zipfile` stops at the declared `file_size`, and `_read_member` caps reads. The Rust scanner never decompresses.
- **SSRF:** None of these commits fetches anything outbound. The worker runs in an unshared network namespace.
- **Error leakage:** `ApiError::Internal` is logged and returned as a generic `internal_error`.
- **Env and fd leakage into the worker:** `env_clear`, `--clearenv` and stderr set to null. Rust fds are CLOEXEC.

## Resolution status (integrator, 2026-11-05)

Regression tests: `crates/kizuna-api/tests/review_m2_m4_regressions.rs` unless noted. Schema changes: `migrations/0007_review_fixes.sql`.

| ID | Status | Fix / test |
|---|---|---|
| H1 | fixed | owners = claim recorder + source uploaders (`claim_owners`) for claim and conflict review; decision author may not reconfirm/revoke (`four_eyes`); `h1_*` |
| H2 | fixed | re-firing finding re-opens an obsolete row (`conflict.reopened` audit); rejected claims are final (supersede instead); `h2_*` |
| M1 | fixed | revision/scope/applicability change on supersession is material; supersession across subject/predicate refused; `dependency-engine` `regression_review_m1_*`, `m2_*` |
| M2 | fixed | reconfirm follows upstream decisions forward; refuses with `upstream_unresolved`; `m2_*` |
| M3 | fixed | `FOR SHARE` on evidence claims and upstream decisions in create; `m3_*` (lock-ordering test) |
| M4 | fixed | manual claims count as `manual` in rules, may only cite `accepted` artifacts, client confidence ignored; `m4_*` |
| M5 | fixed | PDF name-escape decoding; `/ObjStm`, `/OpenAction`, `/AA` quarantine (conservative: more false positives, human release); `evidence-core` `regression_review_m5_*` |
| M6 | partly fixed | `--unshare-user --disable-userns`, `--fsize=0`, `--nproc=64` when not running as root; **open:** seccomp filter, dedicated worker UID (deployment doc) |
| M7 | fixed | rejecting a finding requires `faulty_claims`; they are rejected and withdrawn from decisions; `m7_*` |
| L1 | fixed | dependencies stored with canonical key; new evidence matched through aliases; `l1_l2_*` |
| L2 | fixed | alias chains flattened on merge, cycles refused; `l1_l2_*` |
| L3 | fixed | `none` sandbox needs `KIZUNA_DEV=1`; `config::tests::regression_review_l3_*` |
| L4 | see lane-python merge | handled in the Python worker (BUILD_STATUS) |
| L5 | see lane-python merge | handled in the Python worker (BUILD_STATUS) |
| L6 | fixed | `ReviewDecide` is privileged (MFA); `l6_*` |
| L7 | fixed | `artifact.downloaded` audit; export event carries `report_sha256`; recapture logs `requested_kind`; `l6_l7_*` |
| L8 | fixed | trace cross-checks derivation hash against the chained audit event; reports Ed25519-signed when a key is configured; `l6_l7_l8_*` |
| L9 | fixed | Markdown escaping of `[]()!*_`#\|<`; `l6_l7_l8_l9_*` |
| I1 | fixed | per-artifact advisory lock → 409 `extraction_in_progress` |
| I2 | fixed | > 40 MiB → 400 `document_too_large_for_extraction` |
| I3 | documented | formula-prefix detection is informational; exports must escape; capture IP retention to be set by deployment policy |
| I4 | fixed | 60 verifications/min per user |
