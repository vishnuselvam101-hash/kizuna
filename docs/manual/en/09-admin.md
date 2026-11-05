# Administration

All changes on this page need the **Administrator** role and an MFA session, and all are audited. Auditors can see
**Users & access** without changing anything.

## Users
Screen: **Admin › Users & access**.

| Task | How |
|---|---|
| Add a user | **Add a user**: name, email, role, **Initial password** (12–128 characters). Share the password through a separate channel. |
| Change a role | Pick a new role in the row. The user's sessions end at once. |
| Disable / enable | **Disable** signs the user out at once and blocks sign-in. **Enable** restores access and also clears a lockout. |
| End sessions | **End sessions** signs the user out everywhere. |
| See sessions | **Sessions** lists active and ended sessions and whether each was MFA-verified. |
| Reset authenticator | **Reset authenticator** removes the user's authenticator app and security keys. They enrol again. You cannot reset your own. |

The last active administrator cannot be removed or disabled. **Locked until** shows a lockout after failed
sign-ins.

## Authenticator app (TOTP) — enrolment is API only
There is no enrolment screen yet.
1. `POST /api/v1/auth/mfa/totp/setup` with your current password returns the secret for your authenticator app.
2. `POST /api/v1/auth/mfa/totp/confirm` with a code from the app finishes enrolment.
3. From then on, sign in with **Authenticator code**.

## Security keys (API only)
Security keys (WebAuthn) are an optional, phishing-resistant second factor.
- Enrol: `POST /api/v1/auth/mfa/webauthn/register/start` (needs your current password), then `…/register/finish`.
  Finishing ends all your sessions.
- After enrolment, password + code sign-in is refused. Sign in through `/api/v1/auth/webauthn/login/start` and
  `…/login/finish`. **The web sign-in screen does not support this yet**, so do not enrol a key for anyone who must
  use the web screen.

## Recovery codes
- At the first security-key enrolment KIZUNA returns **10 one-time recovery codes, once**. Store them offline.
- Lost key: `POST /api/v1/auth/webauthn/recover` with your password and one code. It removes all your keys, ends your
  sessions and gives a session that is **not** MFA-verified. Enrol a factor again and sign in again before
  privileged work.
- Other ways back in: another administrator uses **Reset authenticator**; if no administrator can sign in, the
  operator runs `kizuna-admin reset-mfa <organisation> <email>` (or `kizuna-admin unlock <organisation> <email>`
  for a lockout).

## Import settings
Screen: **Admin › Import settings**. These settings control how files are read at extraction time.

| Group | Setting | Range / default |
|---|---|---|
| General | **Default document kind** | default Specification |
| General | **Upload limit (MB)** | 1–200, default 25 (stored; the server limit is set by the operator) |
| CSV | **Delimiter** | Comma, Semicolon, Tab |
| CSV | **Encoding** | UTF-8 or Shift_JIS |
| CSV | **Header row (0 = none)** | 0–100, default 1 |
| CSV | **Maximum records** | 1–20,000 |
| XLSX | **Sheet name** | blank = **All sheets** |
| XLSX | **Header row**, **Maximum sheets**, **Maximum records** | 0–100, 1–50, 1–20,000 |
| PDF and scanned images | **Run OCR on pages without a text layer** | on by default |
| PDF and scanned images | **OCR language** | Japanese + English, Japanese, English |
| PDF and scanned images | **Maximum pages** | 1–500 |
| Column mapping | Component identifier, Attribute, Value, Unit, Revision columns | header names; Unit and Revision optional |

- **Save settings** stores a new version ("Saved as version n."). If someone else saved first, the latest version is
  loaded; check it and save again.
- New settings apply to the next extraction. Use **Re-extract** on documents already processed.
- **Export settings (JSON)** downloads the form. **Reset to defaults** discards unsaved changes.

## Time zone

**Integrations › Time zone** sets the organisation's IANA time zone (default `Asia/Tokyo`). It decides which
calendar day is "today" for expiry checks and how report **From**/**To** dates are read. Administrator with an MFA
session; every change is versioned, audited and raises a security event.

## AI-assisted extraction
Panel at the bottom of **Import settings**. Off by default.

It can send labels that the rules did not recognise to an external AI provider, which suggests which field they
mean. It needs two switches: the operator configures a provider on the server, and an administrator turns it on for
the organisation. "Not configured by the operator" means the first switch is off.

**Data notice — read before turning it on.** The panel shows the provider's data-processing notice exactly as the
operator published it, with its version. In this build:
- What is sent: up to 100 unrecognised labels with at most two example values each. Email addresses, URLs, phone
  numbers, postal addresses and values of person-related labels are replaced with `[personal]`. Recognised labels
  (part numbers, suppliers, approvers…) are never sent. Redaction is heuristic: a name inside free text may slip
  through.
- Free providers may log, keep and train on what they receive. There is no data-processing agreement.
- Do not turn it on for confidential, export-controlled or personal data.

To turn it on: tick "I have read the provider data-processing notice (version n) and accept it for this
organisation", then **Turn on AI-assisted extraction**. If the notice changes, the status shows "Turned on but not
active"; accept the new notice with **Accept notice and keep on**. **Turn off AI-assisted extraction** stops it at
once. The panel also shows the provider, model, host, **Daily budget** and **Used today**.

What AI can and cannot do:
- It only maps labels to fields; values always come from the document.
- It never maps governance fields (approval, review status, expiry, revision, inspection result).
- Its claims are stored as **ai suggested** with confidence at most 70 %. They are excluded from conflict detection
  and cannot be cited by a decision until a reviewer (not the uploader) accepts them (**API only** in this version).
- If the provider fails or the budget is used up, extraction continues with rules only.

## Detection thresholds
Panel on **Admin › Security monitoring** (Administrator only).
- Each rule raises an incident when its count within the rule's window reaches the **Threshold**: failed sign-ins per
  account, failed sign-ins before lock, password spray failures and accounts, permission denials, privileged
  operations, uploads, extraction failures, exports. Each field shows its range and default; **Active now** shows the
  value in force.
- "Failed sign-ins before lock" must be at least the brute-force threshold.
- **Adaptive thresholds** only raise volume rules from your recent baseline, never lower them, up to the **Maximum
  factor**. **Baseline (days)** sets the period.
- **Save thresholds** stores a new version; **Reset to defaults** restores the defaults.
