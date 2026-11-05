# Evidence and source locations

Screen: **Evidence › Suppliers & components**.

## Open a record
1. Choose the **Record type**: **Components** or **Suppliers**.
2. Type in **Filter by identifier** and select a record. **Load more records** fetches further pages.
3. **Evidence for {identifier}** lists the current claims.

## Read the evidence table
| Column | Meaning |
|---|---|
| **Attribute** | What the claim is about, for example a dimension or material. |
| **Value** | The value with its unit. |
| **Revision** | The document revision the value belongs to. |
| **Source** | The document name and exact location, or **Manual entry**. |
| **Method / confidence** | How the claim was extracted and the method's confidence. |
| **Review** | **Unreviewed**, **Accepted**, **Rejected** or **Needs evidence**. |

- **Source locations** read like `page 3`, `sheet Spec, cell B7`, `row 12, col 4` or `line 40`. "no location
  recorded" means the extractor could not give one.
- **sources disagree** marks an attribute whose current claims differ. **{n} superseded** counts older versions
  that were replaced.
- **Confidence describes the extraction method, not the probability that a value is true.** CSV and XLSX
  extraction is rated 90 %, PDF and DXF 80 %. AI-suggested claims are capped at 70 %.

## Claim review
Claims change their review state through conflict review (see [page 6](06-conflicts-and-review.md)). There is no
button to accept or reject a single claim in this version (**API only**: `POST /api/v1/claims/{id}/review`).

## Claim IDs
Recording a decision needs claim IDs. The evidence table does not show them yet. Read them from the API, for example
`GET /api/v1/evidence/part/{identifier}`, or ask your administrator.

## Compare revisions
Screen: **Evidence › Revision comparison**.
1. Choose the **Record type**, enter the **Identifier**, **From revision** and **To revision**.
2. Select **Compare**.

Each attribute is shown as **Changed**, **Added**, **Removed** or **Unchanged**. "No specification differences
between these revisions." means the claims match.
