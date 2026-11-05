# Roles and permissions

Each user has one role. The server enforces permissions; the screen only hides what your role cannot use. Anything
not granted is denied.

| Can… | Administrator | Reviewer | Analyst | Auditor | Viewer |
|---|:-:|:-:|:-:|:-:|:-:|
| See documents, claims and decisions; download a decision report | ✓ | ✓ | ✓ | ✓ | ✓ |
| Upload documents, extract and re-extract claims, **Run verification** | ✓ | ✓ | ✓ | | |
| Review conflicts, decide identifier pairs, reconfirm or revoke decisions ¹ | ✓ | ✓ | | | |
| Record decisions | ✓ | ✓ | | | |
| See users and their sessions | ✓ | | | ✓ | |
| Read the audit trail, verify the hash chain, generate the organisation audit report | ✓ | | | ✓ | |
| See **Security monitoring** | ✓ | | | ✓ | |
| Manage users, import settings, AI-assisted extraction, detection thresholds ¹ | ✓ | | | | |
| Approve or reject incident actions, release quarantined documents ¹ | ✓ | | | | |

¹ Needs a session verified with your authenticator (MFA).

## Four-eyes rules
KIZUNA stops people from checking their own work:
- You cannot review a conflict if you entered one of its claims by hand or uploaded one of its source documents.
- You cannot reconfirm or revoke a decision you recorded.
- You cannot release a quarantined document you uploaded.

If a rule blocks you, ask another reviewer or administrator.

## Role changes
When an administrator changes your role or disables your account, all your sessions end at once. Sign in again to
get the new permissions. The last active administrator cannot be removed or disabled.
