# Getting started

## Sign in
1. Open the KIZUNA address your administrator gave you.
2. Enter **Organisation** (the organisation ID, for example `acme`), **Email** and **Password**.
3. Select **Sign in**.
4. If you have enrolled an authenticator app, an **Authenticator code** field appears. Enter the 6-digit code and
   sign in again.

Notes:
- After repeated failures you see "Too many attempts. Wait a minute and try again." Repeated failed sign-ins also
  lock the account; an administrator can unlock it (see [Administration](09-admin.md)).
- Your session is kept in memory only. Closing the tab signs you out.
- After **30 minutes without activity** the session ends and you sign in again ("Your session timed out").
- Privileged actions (user management, settings, reviews, incident containment) also need a sign-in from the
  **last 15 minutes**. Otherwise they fail with "Please sign in again"; sign out and in, then repeat the action.
- The sign-in screen does not support security keys yet. If your account has a security key, see
  [Administration › Security keys](09-admin.md#security-keys-api-only).
- Some actions need a session verified with your authenticator (MFA). If you signed in without a code, those
  actions fail with "This action needs a session verified with your authenticator." Sign out and sign in with your
  code.

## Find your way
The top bar has six sections. Each has its own sub-navigation.

| Section | Screens |
|---|---|
| **Overview** | **Overview** |
| **Evidence** | **Documents**, **Suppliers & components**, **Revision comparison** |
| **Conflicts** | **Conflict review** |
| **Decisions** | **Decision trace**, **Affected decisions** |
| **Reports** | **Audit reports** |
| **Admin** | **Users & access**, **Import settings**, **Security monitoring** (shown only to roles that may use them) |

- **Command palette:** press ⌘K or Ctrl+K to jump to any screen, switch language or theme, or sign out.
- **Language:** the **日本語 / English** button switches the interface language. Document names and server
  explanations are shown as recorded.
- **Theme:** toggle light and dark.
- **Tables:** type in **Filter rows**, move with J / K or the arrow keys, press Enter to open, click a header to sort.

## The Overview screen
- **Key figures:** documents (and how many are not accepted), open conflicts (and how many await evidence), valid
  decisions, potentially affected decisions, and the audit chain state (only for roles that may verify it).
- **Evidence graph:** documents → claims → parts and suppliers → decisions and conflicts. Select a node to open it.
  Large data sets show only the first claims.
- **Conflicts needing review** and **Decisions to re-examine** link to the screens where you act on them.

## Typical flow
1. Upload documents ([page 3](03-uploading-documents.md)) and extract claims.
2. Check the evidence per part or supplier ([page 4](04-evidence-and-sources.md)).
3. **Run verification** and review conflicts ([page 6](06-conflicts-and-review.md)).
4. Record a decision on the evidence ([page 7](07-decisions.md)).
5. Generate a report and verify the audit chain ([page 8](08-reports-and-audit.md)).
