# Conflicts and review

A conflict is a contradiction or gap that KIZUNA found in the evidence. It is a prompt for human review, not a
verdict. Screen: **Conflicts › Conflict review**.

## Find conflicts
1. Select **Run verification** (Administrator, Reviewer, Analyst). The message reports new conflicts and those that
   no longer apply.
2. Filter by **Status**: **Open**, **Needs evidence**, **Resolved: approved**, **Resolved: rejected**, **No longer
   applies**.
3. Select a conflict to open it.

Kinds: Revision mismatch, Incompatible units, Value mismatch, Tolerance violation, Missing approval, Expired
evidence, Wrong applicability, Conflicting supplier record, Conflicting inspection.

## Read a conflict
| Section | What it tells you |
|---|---|
| **What was detected** | The kind, identifier and attribute. |
| **Why** | The rule's explanation. |
| **Supporting source** / **Contradicting source** | Value, document and location on each side. Some rules use a single source, for example a missing approval or expired evidence. |
| **Confidence and limitations** | The lowest source confidence. |
| **Human review** | Whether a decision is still needed. Nothing proceeds on the record until a reviewer decides. |
| **Decisions that may be affected** | Recorded decisions that rely on these claims. |

## Record a review
Who: Administrator or Reviewer with an MFA session, who did not supply the evidence (four-eyes).
1. Write a **Reason**. It is required to approve or reject and is stored in the audit trail.
2. Choose one:
   - **Approve (documented waiver)**: accept the difference as a recorded exception. Status becomes
     **Resolved: approved**.
   - **Reject evidence**: the evidence is wrong. Status becomes **Resolved: rejected**; the faulty claims are
     rejected and withdrawn from decisions, which then show as **Potentially affected**.
   - **Request more evidence**: status becomes **Needs evidence** until new evidence arrives.

**Known limitation:** the server needs the faulty claim IDs for **Reject evidence**, and the screen does not send
them yet, so the button fails with "The request failed (faulty_claims_required)." Until this is fixed, reject through
the API: `POST /api/v1/conflicts/{id}/review` with `decision: "reject"`, a `comment` and `faulty_claims`.

"This finding is no longer open" means someone else reviewed it first. Reload the list.
