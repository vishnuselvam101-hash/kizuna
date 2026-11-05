# One-hour pilot guide for a factory quality manager

Goal: in one hour, a quality manager sees whether KIZUNA Verify can answer three questions about a supplier part:
**What do our documents actually say? Where do they disagree? Which approvals must be re-checked when something
changes?** Japanese version: [PILOT_GUIDE_ja.md](PILOT_GUIDE_ja.md). Manual: [en/README.md](en/README.md).

Use synthetic data only (or documents the customer has approved in writing). The current build is not
production-ready.

## Before the session (facilitator, about 30 minutes)
- Build the synthetic data set: `scripts/e2e.sh --seed 7 --keep` (24 parts; spec PDFs, drawings, a purchase-order
  CSV and an inspection CSV; twelve parts have one planted fault). Details: [../demo/PILOT_DEMO.md](../demo/PILOT_DEMO.md).
  For a clean hands-on run, start an empty organisation instead and keep the generated files ready to upload.
- Accounts: the quality manager as **Reviewer** with an authenticator enrolled
  ([Administration](en/09-admin.md#authenticator-app-totp--enrolment-is-api-only)); an **Analyst** account for
  uploads; a second **Reviewer** (the facilitator) for four-eyes steps; an **Auditor** account.
- Have a terminal ready for the steps marked **API**: the screen does not yet show claim IDs, release quarantined
  files or supersede claims.
- Pick one clean part for the decision in step 5 and note two of its claim IDs (`GET /api/v1/evidence/part/<id>`).

## Timeline
| Time | Step | Who |
|---|---|---|
| 0:00–0:05 | 1. Sign in and orient | QM |
| 0:05–0:15 | 2. Upload and extract | Analyst account |
| 0:15–0:25 | 3. Inspect evidence | QM |
| 0:25–0:40 | 4. Find and review conflicts | QM |
| 0:40–0:50 | 5. Decide, change evidence, re-check | QM + facilitator |
| 0:50–0:57 | 6. Report and audit | QM + Auditor |
| 0:57–1:00 | 7. Wrap-up | all |

## 1. Sign in and orient (5 min)
1. Sign in with organisation, email, password and **Authenticator code**. Switch to 日本語 if preferred.
2. On **Overview**, read the key figures and the evidence graph. Press Ctrl+K to show the command palette.

Point to make: every number comes from the organisation's own records.

## 2. Upload and extract (10 min)
Signed in as the Analyst:
1. **Evidence › Documents › Upload a document**: upload one spec PDF, its drawing, the purchase-order CSV and the
   inspection CSV with the right **Document kind**.
2. Upload `malicious/supplier_cert_js.pdf`: it is **Quarantined** with a scan finding. Upload
   `malicious/inspection_report.pdf` (a program renamed to .pdf): it is refused and nothing is stored.
3. Select **Extract claims** on each accepted document. Note the claim count and any unrecognised labels.
4. Select **Verify** on one document.

Points to make: files are hashed and kept unchanged; **Verify** proves the bytes are unchanged, not that the content
is right; every extracted claim starts **Unreviewed**.

## 3. Inspect evidence (10 min)
1. **Suppliers & components**, **Components**: open the uploaded part.
2. For two attributes, read the **Source** column: document name plus page, row or cell. Open the document and find
   the same value.
3. Look for **sources disagree** badges.
4. **Possible duplicate identifiers**: find a pair like `BRK-1234` / `BRK1234`. Decide **Same component** (needs the
   MFA session).
5. **Revision comparison**: compare two revisions of one part.

Points to make: confidence rates the method, not the truth; similar identifiers are never merged automatically.

## 4. Find and review conflicts (15 min)
1. **Conflict review › Run verification**.
2. Open one conflict of each kind you have: revision mismatch, value mismatch, missing approval, expired evidence.
   For each, read **Why**, both sources and **Decisions that may be affected**.
3. On one conflict, write a reason and select **Request more evidence** → **Needs evidence**.
4. On another, write a reason (for example a deviation number) and select **Approve (documented waiver)**.
5. Ask the QM: "Would your team have found these by hand? How long would it take?"

Known limitation: **Reject evidence** from the screen fails in this build; skip it or show it through the API
([Conflicts and review](en/06-conflicts-and-review.md)).

## 5. Decide, change evidence, re-check (10 min)
1. **Decision trace › Record a decision**: title, the clean part, **Supplier part release**, **Approved**, a
   rationale, and the two claim IDs you prepared. Select **Record decision**.
2. Open the decision: show the **Derivation hash** and **Evidence at decision time and now**.
3. Facilitator (**API**) supersedes one of the cited claims with a materially different value:
   `POST /api/v1/claims/<id>/supersede` with a reason.
4. **Affected decisions**: the decision is **Potentially affected**; **Changes since the decision** shows the cause.
5. The QM, as author, cannot reconfirm it (four-eyes). The facilitator signs in as the second reviewer, writes a
   reason and selects **Reconfirm with current evidence** (or **Revoke…**).

Point to make: only decisions that relied on the changed claim are flagged; others stay **Valid**.

## 6. Report and audit (7 min)
1. **Audit reports › Decision verification report**: generate the Markdown report for the decision and show its
   SHA-256.
2. Signed in as the Auditor: **Generate organisation report**, then **Verify hash chain** → "Hash chain intact".
3. Show the **Audit trail**: every upload, review and decision with actor and time.

## 7. Wrap-up (3 min)
Ask and write down:
- Which documents and conflict kinds matter most in your plant?
- Who would upload, who would review, who would approve?
- What would you need before using real supplier documents (data agreement, on-premises hosting, AI off)?

State the limits honestly: rules were measured on synthetic data only; OCR of poor scans depends on the server
setup; conflicts are prompts for review, not verdicts.
