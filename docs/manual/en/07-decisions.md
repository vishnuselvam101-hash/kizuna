# Decisions and affected-decision review

A decision records an outcome (for example a supplier part release) together with the exact claims it relied on.
KIZUNA keeps a snapshot of that evidence and flags the decision when the evidence changes later.

## Record a decision
Who: Administrator or Reviewer. Screen: **Decisions › Decision trace**, panel **Record a decision**.
1. Fill in **Title**, **Component** (the part identifier), **Decision type** (Supplier part release, First article
   approval, Deviation acceptance, Other) and **Outcome** (Approved, Rejected, Conditional).
2. Write the **Rationale**.
3. Enter the **Evidence claim IDs**, separated by spaces or commas (see
   [Evidence › Claim IDs](04-evidence-and-sources.md#claim-ids)).
4. Select **Record decision**.

The server refuses a decision without evidence, with an unknown claim ID, or citing a claim that was rejected,
superseded, or is still an unreviewed AI suggestion.

## Read a decision trace
Select a decision in the list.
- **Derivation hash**: a hash over the decision and its evidence.
- **Evidence trace**: a graph of the evidence feeding the decision; changed items are highlighted.
- **Evidence at decision time and now**: each claim **At decision** and **Now**, with **Changed** Yes/No.
- **Changes since the decision**: each change with its cause (**Claim superseded**, **Claim rejected**, **Upstream
  decision**) and whether it is **Material**.

Statuses: **Valid**, **Potentially affected**, **Superseded**, **Revoked**.

## Review affected decisions
Screen: **Decisions › Affected decisions**. It lists decisions whose evidence changed after they were made.
Each one must be reconfirmed or revoked.

Who: Administrator or Reviewer with an MFA session, who is not the decision's author.
1. Open the decision and read **Changes since the decision**.
2. Write a **Reason** (required, 2,000 characters at most).
3. Choose:
   - **Reconfirm with current evidence**: records a new version on the current evidence. The old version becomes
     **Superseded**.
   - **Revoke…**, then **Revoke decision**: final, cannot be undone.

Revoking is also possible for a **Valid** decision. A superseded or revoked decision cannot change.
