# Reports and audit verification

Screen: **Reports › Audit reports**.

## Decision verification report
Who: every role.
1. Under **Decision verification report**, choose a **Decision** and a **Format** (**Markdown** or **JSON**).
2. Select **Generate**, check the **Report preview**, then **Download**.

The report lists the decision, its evidence with the source documents' hashes, open conflicts and reviews, and a
SHA-256 over its content. Keep that value with the file; a changed file will no longer match it.

## Organisation audit report
Who: Administrator, Auditor.
1. Choose the **Scope**: **Open conflicts and affected decisions** or **All conflicts and decisions**.
2. Optionally set **From** and **To** dates (both included, read in the organisation's time zone).
3. Select **Generate organisation report**, then **Download report JSON**.

The summary shows the number of conflicts and decisions, the audit chain state, the **Report SHA-256** and the
**Signature**. "Signed (Ed25519), public key …" means the server signed the report. "Not signed" means the operator
has not configured a signing key. Large or frequent exports can raise an **Unusual exports** incident.

## Audit trail
Who: Administrator, Auditor.
- The **Audit trail** lists every recorded action: **#**, **Time**, **Actor**, **Action**, **Target**, **Hash**.
  Use **Load more events** for older entries and **Export JSON** to save them.
- **Verify hash chain** checks that no event was changed, removed or reordered. "Hash chain intact: n events
  checked" is the expected result. "Hash chain broken" raises an **Audit chain broken** incident; tell your operator
  at once.
- The **Audit chain** figure on **Overview** shows the same check.

## For operators
- `kizuna-admin verify-audit <organisation>` checks the chain from the command line (exit code 2 = broken). Run it
  daily.
- Signed checkpoints (`POST /api/v1/audit/checkpoints`, **API only**) exported to another system detect even a
  database owner re-chaining the log. Keep the signing key outside the database host.
