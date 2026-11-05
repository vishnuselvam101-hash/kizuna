# ADR 0006: Cryptographic building blocks (passwords, MFA, audit chain, secrets at rest)

Status: accepted (2026-11-05)

## Decision
- Passwords: argon2id via RustCrypto `argon2` 0.5, m=19456 KiB, t=2, p=1 (OWASP Password Storage Cheat Sheet
  minimum). Unknown users still run a dummy verification to flatten timing. Password policy: 12–128 characters, no
  composition rules (ASVS 5.0 V6). Screened offline against a bundled common/breached list (46k entries ≥12 chars,
  SecLists, MIT) and context words (email local part, product name): errors `password_common`,
  `password_contains_context`. No k-anonymity HIBP call (no external calls by design).
- TOTP (RFC 6238, SHA-1, 6 digits, 30 s, ±1 step) built on RustCrypto `hmac` + `sha1`; verified against the RFC 6238
  test vectors. Last accepted step is stored to block replay. Secrets encrypted at rest with AES-256-GCM (`aes-gcm`),
  random 96-bit nonce per encryption, AAD = purpose label + tenant id + user id (+ row id).
- Data keys are versioned (`keyring.rs`): envelope `"KZ1" || key_id || nonce || ct`, header inside the AAD.
  `KIZUNA_DATA_KEY` + `KIZUNA_DATA_KEY_ID` is the current key; `KIZUNA_DATA_KEYS_PREVIOUS` keeps old keys readable;
  unversioned M1 blobs are read as key 1. `kizuna-admin rotate-data-key [slug]` re-encrypts per tenant (RLS scope,
  idempotent, audited `crypto.data_key_rotated`) and exits 2 while any blob is not under the current key.
- Privileged permissions require a session that passed MFA. WebAuthn (`webauthn-rs` 0.6, MPL-2.0, user
  verification required, 5-min single-use server-side challenges sealed with the data key). Once a user enrols a
  key, password/TOTP login is refused (`webauthn_required`) and all existing sessions are revoked; only WebAuthn
  login yields MFA sessions. Applies to every enrolled account, so to every privileged admin who enrols.
  Relying party from `KIZUNA_WEBAUTHN_RP_ID` / `KIZUNA_WEBAUTHN_ORIGIN` (validated at start-up).
- Lock-out-proof recovery: 10 one-time recovery codes at first enrolment (128-bit, SHA-256 stored); a code + password
  clears the user's keys and gives a non-MFA session (re-enrol, then log in again); a peer tenant admin can reset MFA
  (never their own); the operator CLI `reset-mfa` works when no admin can sign in. Recovery is audited and raises
  `auth.mfa_reset`.
- Self-service (2026-11-05, web gaps): `GET /auth/mfa/status` lists the caller's factors (labels and dates, never key
  material) and unused recovery codes; `POST /auth/mfa/webauthn/credentials/{id}/remove` and
  `POST /auth/mfa/recovery-codes` (new set, shown once, old set void) need the password **and** a session signed in
  with a key. Removing the last key turns key sign-in off, deletes the recovery codes, ends the user's other sessions
  and raises `auth.mfa_reset`. The web sign-in screen runs the TOTP, security-key and recovery-code paths.
- All expiry decisions read an injected clock (`clock.rs`, `AppState::clock`) passed to SQL as a parameter.
- Audit chain: per-tenant sequence; `hash = SHA-256(canonical encoding incl. prev_hash)` with RustCrypto `sha2`,
  computed in Rust (`audit-engine`), verified in Rust. Canonical encoding is length-prefixed; JSON details are
  canonicalised (sorted keys, floats rejected). DB triggers forbid UPDATE/DELETE/TRUNCATE.
- A hash proves integrity since capture, never truthfulness or authenticity of the content (directive §9).
- Signed checkpoints: deferred to M5/M6 (Ed25519 via `ed25519-dalek`) behind a key-management interface.

- **Founder decision 2026-11-05: WebAuthn stays optional for tenant administrators.** TOTP satisfies the MFA
  requirement for privileged permissions; an administrator who enrols a security key is held to it (above). Not
  mandated because pilot users may lack authenticators; revisit before production or for defence customers.

## Consequences
No home-made primitives; protocol code (TOTP, chain encoding) is small and vector/property tested.
