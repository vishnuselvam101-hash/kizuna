# KIZUNA Verify user manual

KIZUNA Verify collects the documents a supplier-part decision depends on, extracts claims with their exact
sources, flags contradictions for human review, and keeps a tamper-evident record of who decided what on which
evidence. When evidence changes, it tells you which decisions must be checked again.

This manual describes the current build (main `c9d58a4`). It is not production-ready; use synthetic or approved
data only. Japanese version: [../ja/README.md](../ja/README.md).

| # | Page | For |
|---|---|---|
| 1 | [Getting started](01-getting-started.md) | everyone |
| 2 | [Roles and permissions](02-roles-and-permissions.md) | everyone |
| 3 | [Uploading documents](03-uploading-documents.md) | analysts, reviewers |
| 4 | [Evidence and source locations](04-evidence-and-sources.md) | everyone |
| 5 | [Matching parts and suppliers](05-matching-parts-and-suppliers.md) | reviewers |
| 6 | [Conflicts and review](06-conflicts-and-review.md) | reviewers |
| 7 | [Decisions and affected-decision review](07-decisions.md) | reviewers |
| 8 | [Reports and audit verification](08-reports-and-audit.md) | reviewers, auditors |
| 9 | [Administration](09-admin.md) | administrators |
| 10 | [Security incidents and containment](10-security-incidents.md) | administrators, auditors |
| 11 | [Backups and restore](11-backup-restore.md) | operators |
| 12 | [FAQ](12-faq.md) | everyone |
| 13 | [Glossary](13-glossary.md) | everyone |

One-hour pilot walkthrough: [../PILOT_GUIDE_en.md](../PILOT_GUIDE_en.md).

Screen and button names are written in **bold** exactly as the English UI shows them. Some functions exist only in
the API (base path `/api/v1`); those are marked **API only**.
