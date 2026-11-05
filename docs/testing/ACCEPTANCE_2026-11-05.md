# KIZUNA browser acceptance test — 2026-11-05

Base: `main` c9d58a4, tested on branch `qa` (detached worktree `/root/kizuna-qa`). Not pushed.

## How it was run

`scripts/e2e-browser.sh` (new) does everything a real deployment does, with no mocks in the product path:

- **Server**: release `kizuna-server` (`cargo build --release`), own throwaway PostgreSQL database
  (`kizuna_e2e_browser_<ts>_<pid>`, created the way `scripts/lib/throwaway.sh` does, dropped on exit),
  real bwrap extraction sandbox, real Tesseract OCR, signed audit checkpoints (random key), WebAuthn
  RP `localhost`.
- **Web**: production build (`tsc` + `vite build`) served by `vite preview`, `/api` proxied to the server.
- **AI provider**: `scripts/e2e/mock_ai.py`, a local OpenAI-compatible endpoint on 127.0.0.1 (the only
  stand-in; the operator AI integration is otherwise real).
- **Browser**: Playwright 1.64.1 / Chromium 155, `web/e2e/`. Two tenants: `e2e-en` (English UI, 1440×900,
  dark) and `e2e-ja` (Japanese UI, 390×844 touch, light), each with admin, admin2, reviewer, reviewer2,
  analyst, auditor, viewer, keyuser, victim.
- **Documents**: the E2E generator set (`gen_dataset.py --parts 7`, English tenant) plus realistic Japanese
  documents created for this run (`scripts/e2e/gen_ja_docs.py`, `web/e2e/global-setup.ts`):
  検査成績書 PDF, ミルシート PDF (SUS316 against a SUS304 spec), 補足仕様 rev C PDF, Shift_JIS 発注一覧 CSV,
  仕様書 XLSX, DXF drawing with a Japanese title block, a scanned (image-only) drawing PDF for OCR, and a
  certificate PDF with an embedded `/JS` action. PDFs are printed by Chromium (CJK fonts, ToUnicode), as
  office tools produce them.
- Login rate limit per IP raised to 1000/min (all browser sessions share 127.0.0.1); everything else default.

```
scripts/e2e-browser.sh                 # full run, then stop server + drop DB
scripts/e2e-browser.sh --up            # leave it running; --restart-server WORK; --down WORK
```

Results: `web/e2e/results/` (gitignored): `report.json`, `screenshots/` (every page state per language,
viewport and theme, `<project>-<state>[-<lang>-<width>-<theme>].png`), traces of failures.

## Journeys × result (final run)

| Spec | Journey | EN 1440 dark | JA 390 light | Other |
|---|---|---|---|---|
| `01-auth.spec.ts` | sign-in: wrong password is refused with a clear message; correct password signs in; sign-out ends the session | pass | pass |  |
| `01-auth.spec.ts` | TOTP: admins enrol an authenticator from the account page and sign in with a code (MFA-verified session) | pass | pass |  |
| `01-auth.spec.ts` | WebAuthn: enrol a security key, keep the recovery codes, sign in with the key | pass | pass |  |
| `01-auth.spec.ts` | recovery code: a user who lost the key signs in once with a code | pass | pass |  |
| `02-documents.spec.ts` | analyst uploads each Japanese document type; a hostile certificate is quarantined, never processed | pass | pass |  |
| `02-documents.spec.ts` | reviewer2 uploads the supplier mill sheet (they may not review conflicts built on it later) | pass | pass |  |
| `02-documents.spec.ts` | English tenant also ingests the synthetic supplier set (specs, drawings, POs, inspection lots) | pass | n/a |  |
| `02-documents.spec.ts` | analyst extracts claims from every accepted document, including OCR of the scanned drawing and the DXF | pass | pass |  |
| `03-evidence.spec.ts` | evidence explorer: a component's claims show document, exact location, method and disagreement | pass | pass |  |
| `03-evidence.spec.ts` | entity resolution: reviewer confirms one identifier variant as the same part and rejects another | pass | pass |  |
| `04-conflicts.spec.ts` | analyst runs detection: the mill sheet's SUS316 contradicts the specified SUS304 | pass | pass |  |
| `04-conflicts.spec.ts` | four-eyes: reviewer2 uploaded the mill sheet and is refused when reviewing that finding | pass | pass |  |
| `04-conflicts.spec.ts` | reviewer: reject needs a reason and the faulty evidence; request evidence; approve a waiver | pass | pass |  |
| `05-decisions.spec.ts` | revision comparison: rev B → C of JA-BRK-1001 shows the changed length | pass | pass |  |
| `05-decisions.spec.ts` | reviewer records a decision citing claim IDs copied from the evidence table | pass | pass |  |
| `05-decisions.spec.ts` | analyst supersedes the cited length; the decision becomes potentially affected | pass | pass |  |
| `05-decisions.spec.ts` | four-eyes: the decision's author cannot reconfirm it; a second reviewer re-reviews from the trace | pass | pass |  |
| `06-reports.spec.ts` | decision report: generate Markdown and JSON, download both | pass | pass |  |
| `06-reports.spec.ts` | auditor: organisation report downloads with its SHA-256; chain verifies; checkpoint signed and verified | pass | pass |  |
| `07-admin.spec.ts` | users & roles: add a user, change role, view and end sessions, disable and re-enable | pass | pass |  |
| `07-admin.spec.ts` | import settings: save Shift_JIS CSV settings; analyst re-extracts the CSV with a recorded reason | pass | pass |  |
| `07-admin.spec.ts` | AI-assisted extraction: admin accepts the notice and turns it on; re-extraction maps 製造者 via the local provider | pass | pass |  |
| `07-admin.spec.ts` | detection thresholds: an inconsistent lock threshold is refused; lowered thresholds are saved | pass | pass |  |
| `08-security.spec.ts` | brute force: failed sign-ins raise an incident; admin approves the account lock, then reverts it and resolves | pass | pass |  |
| `08-security.spec.ts` | quarantine release: the uploading admin is refused (four-eyes); a second admin releases it | pass | pass |  |
| `keyboard.spec.ts` | keyboard only: sign in, upload, extract, review a conflict, sign out |  |  | keyboard: pass |
| `matrix.spec.ts` | every page state in EN/JA at 1440/390 in dark/light passes axe without horizontal scroll |  |  | matrix: pass |

Final run: 51 passed, 1 skipped (English-only bulk upload, not applicable in the JA tenant), 0 failed, 21.5 min (`scripts/e2e-browser.sh`, exit 0).

Every journey step runs axe-core (WCAG 2.0/2.1/2.2 A+AA rule tags) on the page state it reaches and
checks there is no horizontal page scroll. The matrix project visits 13 page states (login, overview,
documents, evidence explorer with a part selected, revision comparison result, conflict detail, decision
trace + record form, affected decisions, reports, users, import settings + AI panel, security with an
incident open, account, command palette) × EN/JA × 1440/390 × dark/light = 104 states + 8 login states.

## Bugs found and fixed (each: failing test first, then fix, one commit on `qa`)

| # | Severity | Bug | Test | Fix |
|---|---|---|---|---|
| 1 | High | The web app had no way to enrol MFA. Privileged actions (user management, review decisions, AI opt-in, thresholds, quarantine release) need an MFA-verified session, so a web-only organisation could never use them. Security keys and recovery codes existed only in the API; a key-enrolled user could not sign in on the web at all (`webauthn_required` shown as "sign-in failed"). | `01-auth` | New *Account & security* page (TOTP setup with secret + otpauth link, WebAuthn enrolment, one-time recovery codes), security-key and recovery-code steps on the login form, `/auth/me` reports `webauthn_enabled`. |
| 2 | High | Upload scan quarantined every PDF printed by Chromium / Office: `/AA` and `/JS` were substring-matched and hit font subset tags (`/AAAAAA+NotoSansCJKjp`). | `02-documents`, `evidence-core` unit test | PDF names match only up to a PDF delimiter. |
| 3 | High | Japanese PDFs produced zero claims: per-glyph fragments joined with spaces (`品 番`), full-width widths under-estimated (labels split into two cells), CJK radical code points from font ToUnicode maps (`⼊ ⾦ ⻑`) never matched labels or other documents (北陸⾦属 ≠ 北陸金属). | `02-documents`, `test_japanese_pdf.py` | CJK-aware fragment joining, per-glyph runs, radical folding in `layout.py`. |
| 4 | Medium | Japanese dimension / property / tolerance labels (長さ 幅 高さ 厚さ 板厚 直径 外径 内径 硬さ 引張強さ 耐力 表面粗さ 公差) were unmapped; values lost. | `02-documents`, `mapping.rs` test | Aliases added. |
| 5 | High | A mill sheet / inspection certificate certifying another material than the specification raised no finding (only an unflagged disagreement in the explorer). | `04-conflicts`, `rules.rs` test | `certified_material` rule (kind `value_mismatch`). |
| 6 | High | *Reject* in conflict review always failed: the server requires the faulty evidence (`faulty_claims`), the UI never sent it (the unit test mocked the wrong contract). | `04-conflicts`, `workflow.test.tsx` | Reviewer must tick the faulty evidence; sent with the review. |
| 7 | High | Decisions need evidence claim IDs "copied from the evidence table", but the table never showed claim IDs. | `05-decisions` | Claim ID column. |
| 8 | High | No way in the UI to supersede (correct) evidence, so the affected-decision / re-review loop could not be driven by a user. | `05-decisions` | *Supersede* action with corrected value + required reason in the evidence explorer. |
| 9 | High | Quarantined documents could not be released from the UI. | `08-security` | Two-step *Release* for incident managers; server keeps four-eyes. |
| 10 | Medium | Signed audit checkpoints had no UI. | `06-reports` | *Create signed checkpoint* (downloads the JSON) and *Verify checkpoints* for auditors. |
| 11 | Medium | AI-assisted mapping dropped Japanese maker labels (製造者, 製造元) as implausible for *supplier*. | `07-admin`, `ai_guard.rs` test | Stems added. |
| 12 | Medium | Four-eyes refusals showed "The request failed (403 no_self_review)". | `04-conflicts`, `05-decisions`, `08-security` | Translated message (EN/JA). |
| 13 | Medium | 390 px: the incident action table overflowed the page, *Approve*/*Revert* were unreachable. | `08-security` (JA, 390) | Table in a scroll region. |
| 14 | Medium (a11y) | Horizontally scrollable evidence, duplicate-candidate, trace and comparison tables were not keyboard-focusable (axe `scrollable-region-focusable`); incident actions table had no headers; incident panels had no accessible name. (Two regressions of these fixes — 16 px reject checkboxes failing target-size, a duplicate landmark name — were caught by the same checks and fixed.) | `03-evidence`, keyboard, matrix, `a11y.test.tsx` | `tabIndex=0` uniquely labelled regions, `<thead>`, panel ids, 32 px checkbox rows. |
| 15 | Low | 390 px: top bar did not wrap, *Sign out* off-screen once the account button was added. | `01-auth` (JA) | `.topbar-right` wraps. |

## Observations (not changed — need a product decision)

- Server-generated texts are English only in the Japanese UI: incident summaries ("5 failed credential
  checks …"), finding explanations (`why`), PDF metadata keys (`Creator`, `Producer`, `CreationDate`,
  `ModDate`) listed as "unrecognised labels" after every PDF extraction.
- Rules and mappings were extended (bugs 4, 5, 11) without bumping `mfg-rules@1` / `mfg-map@1`; bumping
  changes decision derivations, so it is left to the maintainer.
- *Extract claims* stays visible after extraction; clicking again returns `already_extracted`.
- Theme is not taken from `prefers-color-scheme` and resets on reload (by design: no storage).
- Native file input shows the browser's own language ("Choose File") inside the Japanese UI.
- A user with only a password (no MFA) sees privileged buttons (e.g. *Approve* on a finding) and learns
  about the MFA requirement only from the error after clicking.

## Not covered

- Real hardware keys / platform passkeys (virtual CTAP2 authenticator only); Firefox/Safari.
- ClamAV (not configured on this host: static scan only).
- Real AI providers (local mock only).
