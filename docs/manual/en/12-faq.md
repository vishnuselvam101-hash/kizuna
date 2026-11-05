# FAQ

**I signed in but some buttons are missing.**
Your role does not include that action ([Roles](02-roles-and-permissions.md)). The Admin section is hidden from
roles that cannot use it.

**"This action needs a session verified with your authenticator."**
Sign out and sign in again with your **Authenticator code**. If you have no authenticator yet, see
[Administration](09-admin.md#authenticator-app-totp--enrolment-is-api-only).

**"Your role does not allow this action."** — but I am a reviewer.
A four-eyes rule may apply: you supplied the evidence, recorded the decision, or uploaded the document. Ask a
colleague.

**My PDF was quarantined but it is harmless.**
Many PDFs contain compressed object streams (`/ObjStm`), which KIZUNA quarantines conservatively. An administrator
who did not upload it can release it (API only). See [Uploading documents](03-uploading-documents.md).

**Does "Accepted" mean the document is correct?**
No. It means the file passed the scan. Likewise, **Verify** only proves the stored bytes are unchanged.

**Is 90 % confidence a 90 % chance the value is right?**
No. Confidence rates the extraction method, not the truth of the value.

**Why were two similar part numbers not merged?**
KIZUNA never merges identifiers on its own. A reviewer decides in **Possible duplicate identifiers**.

**"Reject evidence" fails with "faulty_claims_required".**
Known limitation of this version. Use the API ([Conflicts and review](06-conflicts-and-review.md)).

**Where do I find claim IDs for a decision?**
They are not shown on screen yet; read them from the API ([Evidence](04-evidence-and-sources.md#claim-ids)).

**A decision became "Potentially affected". What now?**
Evidence it relied on changed. Open **Affected decisions**, read the changes, then reconfirm or revoke. Another
reviewer must do this, not the author.

**Can a revoked decision be restored?**
No. Revoking is final. Record a new decision instead.

**I changed Import settings but nothing changed.**
Settings apply to the next extraction. Use **Re-extract** with a reason.

**Does AI decide anything?**
No. AI only suggests which field an unknown label means. Its claims stay unreviewed and cannot support a decision
until a reviewer accepts them.

**Can I use the security key I enrolled to sign in on the web?**
Not yet. The web sign-in screen supports password and authenticator code only.

**Does KIZUNA send notifications by email?**
Incident notifications are in-app only. Operators can configure email or Telegram alerts for server failures such
as a failed backup.

**I closed the tab and was signed out.**
Expected: sessions are kept in memory only.
