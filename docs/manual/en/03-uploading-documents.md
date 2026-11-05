# Uploading documents

Who: Administrator, Reviewer, Analyst. Screen: **Evidence › Documents**.

## Upload a file
1. Under **Upload a document**, choose the **Document kind**: Specification, Drawing, Purchase order, Inspection
   report, Spreadsheet or Other.
2. Choose the **File** and select **Upload**.
3. The message shows the status. If the same bytes were uploaded before, KIZUNA keeps one copy and records a new
   capture.

Every file is hashed (SHA-256), scanned and stored unchanged. Nothing in it is trusted until reviewed.

## Supported types and limits
- Types are detected from the file content, not the extension: **PDF, XLSX, CSV, PNG, JPEG, TIFF, DXF**.
  Anything else (for example DOCX, ZIP, executables) is rejected: "This file type is not accepted."
- Size: the server limit set by the operator (default 25 MB, at most 200 MB). Larger files fail with "The file is
  larger than the upload limit." The **Upload limit (MB)** field in **Import settings** is saved with the settings
  but does not change the server limit in this version.
- Too many uploads in a short time fail with "Too many requests. Wait a minute and try again." and can raise an
  **Upload abuse** incident.

## Statuses
| Status | Meaning |
|---|---|
| **Scanning** | The scan has not finished. |
| **Accepted** | Passed the scan. Claims can be extracted. |
| **Quarantined** | Stored but never extracted or served. See below. |
| **Rejected** | Type not allowed or file malformed. |

The library shows the reasons as **Scan findings**.

## Why a file is quarantined
- **PDF** with active content: `/JavaScript`, `/JS`, `/Launch`, `/EmbeddedFile`, `/RichMedia`, `/XFA`,
  `/SubmitForm`, `/OpenAction`, `/AA`, or compressed object streams (`/ObjStm`). The last one is conservative and
  catches many harmless PDFs.
- **XLSX** with macros, external links, unsafe entry names, more than 2,000 entries, ZIP64, or a suspected zip
  bomb (over 200 MB uncompressed or a compression ratio over 100).
- **CSV** with more than 200,000 lines. (Cells starting with `=`, `+` or `@` are only noted as a finding;
  KIZUNA never evaluates formulas.)
- **Antivirus** (when the operator has configured ClamAV): a detection, a file too large to scan, or the scanner
  being unavailable. KIZUNA fails closed.
- A security incident (for example an integrity failure) can also quarantine a document.

**Releasing a quarantined file (API only).** There is no release button yet. An administrator with an MFA session,
who did not upload the file, calls `POST /api/v1/artifacts/{id}/release`. A document quarantined by an integrity
incident must pass the integrity check again first.

## Verify integrity
Select **Verify** on a row. "Stored bytes are unchanged since capture" means the file was not altered in storage. It
does not say the content is correct. A failure raises a **Document integrity failure** incident.

## Extract claims
Who: Administrator, Reviewer, Analyst.
1. Select **Extract claims** on an accepted document.
2. The message reports how many claims were created and which labels were not recognised.

All extracted claims start as **Unreviewed**. A document can be extracted once; to run it again use **Re-extract**.

## Re-extract
Use this after changing **Import settings**, or when a document was extracted with the wrong settings.
1. Select **Re-extract** and enter a reason (required, recorded in the audit log).
2. Select **Re-extract now**.

Changed claims are superseded (old versions are kept), claims no longer found are flagged for evidence, and
affected decisions are re-checked. The result lists new, superseded, unchanged and flagged claims.

If extraction fails, the message says the document may be malformed or unsupported. Repeated failures can raise a
**Repeated extraction failures** incident.
