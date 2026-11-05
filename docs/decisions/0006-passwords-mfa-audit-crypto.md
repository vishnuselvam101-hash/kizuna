# ADR 0006: Cryptographic building blocks (passwords, MFA, audit chain, secrets at rest)

Status: accepted (2026-11-05)

## Decision
- Passwords: argon2id via RustCrypto `argon2` 0.5, m=19456 KiB, t=2, p=1 (OWASP Password Storage Cheat Sheet
  minimum). Unknown users still run a dummy verification to flatten timing. Password policy: 12–128 characters, no
  composition rules (ASVS 5.0 V6). Breached-password screening is not implemented yet (known gap).
- TOTP (RFC 6238, SHA-1, 6 digits, 30 s, ±1 step) built on RustCrypto `hmac` + `sha1`; verified against the RFC 6238
  test vectors. Last accepted step is stored to block replay. Secrets encrypted at rest with AES-256-GCM (`aes-gcm`),
  key from `KIZUNA_DATA_KEY`, random 96-bit nonce per encryption, AAD = tenant id + user id.
- Privileged permissions require a session that passed MFA. WebAuthn (phishing-resistant) is designed as the
  `SecondFactor` interface; not implemented in M1 (`webauthn-rs` is MPL-2.0 and needs an origin/TLS deployment; ADR
  revisit before first pilot with admins).
- Audit chain: per-tenant sequence; `hash = SHA-256(canonical encoding incl. prev_hash)` with RustCrypto `sha2`,
  computed in Rust (`audit-engine`), verified in Rust. Canonical encoding is length-prefixed; JSON details are
  canonicalised (sorted keys, floats rejected). DB triggers forbid UPDATE/DELETE/TRUNCATE.
- A hash proves integrity since capture, never truthfulness or authenticity of the content (directive §9).
- Signed checkpoints: deferred to M5/M6 (Ed25519 via `ed25519-dalek`) behind a key-management interface.

## Consequences
No home-made primitives; protocol code (TOTP, chain encoding) is small and vector/property tested.
