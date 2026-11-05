# ADR 0011: Upload validation, static scanning and quarantine

Status: accepted (2026-11-05)

## Decision
- Uploads are typed by content (magic bytes) against an allowlist: PDF, XLSX, CSV (UTF-8 or Shift_JIS/CP932 text), PNG, JPEG, TIFF.
  Declared MIME type and file name are recorded but never trusted; names are display-only and sanitised.
- `evidence-core::scan` (static, no parsing/execution): PDF active-content markers (/JavaScript, /JS, /Launch,
  /EmbeddedFile, /RichMedia, /XFA, /SubmitForm) → quarantine; XLSX central directory checks (entry count, total and
  ratio of declared sizes = zip-bomb heuristic, `..`/absolute names, VBA macros, external links) → quarantine;
  malformed ZIP or non-XLSX ZIP → reject; anything else → reject.
- Verdicts: clean → `accepted`; suspicious → `quarantined` (kept, not served, not extracted; release requires
  `IncidentManage` + MFA, audited); not allowed → refused, `ingest.upload_rejected` security event.
- This is **not antivirus**. ClamAV (GPL-2.0, separate daemon) or a commercial scanner can be added behind the same
  verdict interface; not installed on this host and not adopted without a further ADR.
- Per-user upload rate limit; upload size limit (default 25 MiB) enforced while streaming.

## Consequences
False positives possible (e.g. benign PDFs with form JavaScript); a human releases them. Static markers can be
obfuscated (e.g. PDF name escapes `/J#61vaScript`); the sandboxed extraction worker (M3) is the second line.
