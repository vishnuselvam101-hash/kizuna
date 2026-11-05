# Security incidents and containment

Screen: **Admin › Security monitoring**. Administrators manage incidents; Auditors can read them.

KIZUNA watches its own security events (the last hour, re-checked every minute) and opens an incident when a rule
fires. Responses only act on KIZUNA sessions, accounts, uploads and documents. Nothing is done to external systems.

## Read the dashboard
- Counts: **open incidents**, **actions awaiting approval**, **active address blocks**.
- **Incidents** table: **Severity** (Critical, High, Medium, Low), **Rule**, **Subject**, **Summary**, **Status**,
  **Last seen**.
- Rules: Repeated failed sign-ins, Password spraying, Session token reuse, Permission probing, Burst of admin
  operations, Document integrity failure, Audit chain broken, Upload abuse, Repeated extraction failures, Unusual
  exports.

## Containment
| Action | Default |
|---|---|
| **End user sessions** | automatic |
| **Block address** (60 min, sign-in only) | automatic, but needs approval if an active administrator used the address in the last 24 h |
| **Quarantine document** | automatic |
| **Restrict uploads** (60 min) | automatic |
| **Pause extraction** (30 min) | automatic |
| **Notify administrators** (in-app) | automatic |
| **Lock account** | needs approval |
| End an administrator's sessions after a burst of admin operations | needs approval |

Action statuses: **Awaiting approval**, **Done**, **Rejected**, **Reverted**, **Refused by policy** (for example
locking the last active administrator).

## Handle an incident
Who: Administrator with an MFA session.
1. Open the incident and read the summary and actions.
2. Write a **Comment**.
3. For actions **Awaiting approval**, choose **Approve** or **Reject**.
4. Done actions such as **Lock account**, **Block address**, **Restrict uploads** and **Pause extraction** can be
   undone with **Revert**.
5. Close the incident with **Resolve** or mark it **False positive**.

Incident statuses: **Open**, **Contained**, **Resolved**, **False positive**. Nothing is deleted.

## Emergencies
If administrators are locked out or blocked, the operator runs `kizuna-admin emergency-unlock <organisation>`. It
re-enables administrators and lifts blocks, and is audited.

To release a document quarantined by an incident, see
[Uploading documents](03-uploading-documents.md#why-a-file-is-quarantined).
