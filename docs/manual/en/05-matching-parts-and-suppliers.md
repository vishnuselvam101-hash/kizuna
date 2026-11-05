# Matching parts and suppliers

Documents often name the same part or supplier differently, for example `KZ-1001` and `KZ1001`, or `KZ-1001-B`
with the revision in the identifier. KIZUNA flags such pairs but **never merges them automatically**. A reviewer
decides.

Screen: **Evidence › Suppliers & components**, panel **Possible duplicate identifiers**.

## Review a pair
1. Choose the **Record type** (Components or Suppliers).
2. Each row shows **Identifier A**, **Identifier B**, the **Assessment** and **Why flagged**.
   - Assessment: **Possible match**, **Needs review (ambiguous)**, **Match** or **No match**.
   - Reasons: **separator differs**, **revision suffix in identifier**, **one character differs**.
3. Select **Same component** / **Same supplier** or **Different components** / **Different suppliers**.

Who: Administrator or Reviewer with an MFA session. The decision is audited.

## What happens next
- **Same:** the identifiers are linked. Claims under either identifier count as one record, so verification can
  compare them.
- **Different:** the pair is closed and no longer listed.
- "This identifier is already linked to another record." means one side is already linked elsewhere. Check that
  link first.
- After linking, select **Run verification** on **Conflict review** so new comparisons are made.
