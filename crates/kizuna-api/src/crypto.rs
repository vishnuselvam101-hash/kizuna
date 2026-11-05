//! Identity crypto: argon2id passwords, opaque tokens, AES-256-GCM secret box, RFC 6238 TOTP.
//! Primitives come from RustCrypto crates; only protocol glue lives here.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const PASSWORD_MIN_CHARS: usize = 12;
pub const PASSWORD_MAX_CHARS: usize = 128;

fn argon2() -> Argon2<'static> {
    // OWASP minimum for argon2id: m=19 MiB, t=2, p=1.
    // Constant, valid parameters: a failure here is a programming error, never a silent fallback (KZ-C5).
    #[allow(clippy::expect_used)]
    let params = Params::new(19_456, 2, 1, None).expect("argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rng().fill_bytes(&mut b);
    b
}

#[derive(Debug, PartialEq, Eq)]
pub enum PasswordPolicyError {
    TooShort,
    TooLong,
    /// On the bundled common/breached password list.
    Common,
    /// Contains the account's email local part or the product name.
    ContainsContext,
}

impl PasswordPolicyError {
    pub fn code(&self) -> &'static str {
        match self {
            PasswordPolicyError::TooShort | PasswordPolicyError::TooLong => "password_policy",
            PasswordPolicyError::Common => "password_common",
            PasswordPolicyError::ContainsContext => "password_contains_context",
        }
    }
}

/// Lower-cased passwords of 12+ characters from the SecLists xato-net 1M list (MIT; see
/// `data/common-passwords.LICENSE` and docs/research/dependencies.md). Bundled: screening makes no network calls.
static COMMON_PASSWORDS: &str = include_str!("../data/common-passwords.txt");

fn is_common_password(pw: &str) -> bool {
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| COMMON_PASSWORDS.lines().collect())
        .contains(pw.trim().to_lowercase().as_str())
}

/// ASVS 5.0 V6: length-based policy, no composition rules, screened against common/breached passwords and
/// context words (`email` may be empty when unknown).
pub fn check_password_policy(pw: &str, email: &str) -> Result<(), PasswordPolicyError> {
    let n = pw.chars().count();
    if n < PASSWORD_MIN_CHARS {
        return Err(PasswordPolicyError::TooShort);
    } else if n > PASSWORD_MAX_CHARS {
        return Err(PasswordPolicyError::TooLong);
    }
    if is_common_password(pw) {
        return Err(PasswordPolicyError::Common);
    }
    let lower = pw.to_lowercase();
    let local = email.split('@').next().unwrap_or_default().to_lowercase();
    if lower.contains("kizuna") || (local.chars().count() >= 4 && lower.contains(&local)) {
        return Err(PasswordPolicyError::ContainsContext);
    }
    Ok(())
}

pub fn hash_password(pw: &str) -> Result<String, String> {
    let salt = SaltString::encode_b64(&random_bytes::<16>()).map_err(|e| e.to_string())?;
    argon2()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// PHC prefix of hashes made with the current parameters; older hashes are upgraded at the next login (KZ-C5).
pub const PHC_CURRENT_PREFIX: &str = "$argon2id$v=19$m=19456,t=2,p=1$";

pub fn needs_rehash(phc: &str) -> bool {
    !phc.starts_with(PHC_CURRENT_PREFIX)
}

pub fn verify_password(pw: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(h) => argon2().verify_password(pw.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

/// Burns the same CPU as a real verification so unknown accounts are not distinguishable by timing.
pub fn dummy_verify(pw: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let h = DUMMY.get_or_init(|| hash_password("kizuna-dummy-password-not-used").unwrap_or_default());
    let _ = verify_password(pw, h);
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Refresh,
}

impl TokenKind {
    fn prefix(self) -> &'static str {
        match self {
            TokenKind::Access => "kza",
            TokenKind::Refresh => "kzr",
        }
    }
}

/// A freshly minted token. `value` goes to the client once; only `hash` is stored.
pub struct IssuedToken {
    pub value: String,
    pub hash: [u8; 32],
}

pub fn issue_token(kind: TokenKind, tenant: Uuid) -> IssuedToken {
    let secret = random_bytes::<32>();
    IssuedToken {
        value: format!(
            "{}.{}.{}",
            kind.prefix(),
            tenant.simple(),
            URL_SAFE_NO_PAD.encode(secret)
        ),
        hash: sha256(&secret),
    }
}

/// Strictly parses `prefix.tenant.secret`; returns the tenant and the SHA-256 of the secret.
pub fn parse_token(kind: TokenKind, value: &str) -> Option<(Uuid, [u8; 32])> {
    if value.len() > 128 {
        return None;
    }
    let mut it = value.split('.');
    let (p, t, s) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || p != kind.prefix() || t.len() != 32 || s.len() != 43 {
        return None;
    }
    let tenant = Uuid::try_parse(t).ok()?;
    let secret = URL_SAFE_NO_PAD.decode(s).ok()?;
    (secret.len() == 32).then(|| (tenant, sha256(&secret)))
}

/// AES-256-GCM with a random 96-bit nonce; output = nonce || ciphertext+tag. AAD binds the value to its owner.
pub fn seal(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce = random_bytes::<12>();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| "encrypt failed".to_string())?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open(key: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let (nonce, ct) = sealed.split_at(12);
    cipher.decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad }).ok()
}

/// AAD for a TOTP secret awaiting confirmation; distinct from the active one, so blobs cannot be swapped between
/// `totp_pending_enc` and `totp_secret_enc` unnoticed (deep review KZ-C4).
pub fn totp_pending_aad(tenant: Uuid, user: Uuid) -> Vec<u8> {
    let mut v = b"kizuna-totp-pending-v1".to_vec();
    v.extend_from_slice(tenant.as_bytes());
    v.extend_from_slice(user.as_bytes());
    v
}

pub fn totp_aad(tenant: Uuid, user: Uuid) -> Vec<u8> {
    let mut v = b"kizuna-totp-v1".to_vec();
    v.extend_from_slice(tenant.as_bytes());
    v.extend_from_slice(user.as_bytes());
    v
}

/// RFC 4648 base32 without padding (for authenticator apps).
pub fn base32(data: &[u8]) -> String {
    const A: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(A[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub const TOTP_STEP: i64 = 30;

/// RFC 4226 HOTP with HMAC-SHA1.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([h[off] & 0x7f, h[off + 1], h[off + 2], h[off + 3]]);
    bin % 10u32.pow(digits)
}

pub fn totp_now(secret: &[u8], unix: i64) -> String {
    format!("{:06}", hotp(secret, (unix / TOTP_STEP) as u64, 6))
}

/// Verifies a 6-digit code within ±1 step, rejecting steps at or before `last_step` (replay).
/// Returns the matched step.
pub fn totp_verify(secret: &[u8], code: &str, unix: i64, last_step: Option<i64>) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let cur = unix / TOTP_STEP;
    let mut matched = None;
    for step in [cur - 1, cur, cur + 1] {
        if step < 0 || last_step.is_some_and(|l| step <= l) {
            continue;
        }
        let want = format!("{:06}", hotp(secret, step as u64, 6));
        if bool::from(want.as_bytes().ct_eq(code.as_bytes())) {
            matched = Some(step);
        }
    }
    matched
}

pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        let k = b"12345678901234567890";
        for (t, want) in [
            (59, 94287082),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
        ] {
            assert_eq!(hotp(k, (t / 30) as u64, 8), want, "t={t}");
        }
    }

    #[test]
    fn totp_window_and_replay() {
        let k = [3u8; 20];
        let now = 1_800_000_000;
        let code = totp_now(&k, now);
        let step = totp_verify(&k, &code, now, None).unwrap();
        assert_eq!(totp_verify(&k, &code, now, Some(step)), None, "replay rejected");
        assert!(totp_verify(&k, &totp_now(&k, now - 30), now, None).is_some());
        assert!(totp_verify(&k, &totp_now(&k, now - 90), now, None).is_none());
        assert!(totp_verify(&k, "12345", now, None).is_none());
        assert!(totp_verify(&k, "12a456", now, None).is_none());
    }

    #[test]
    fn base32_rfc4648_vectors() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b""), "");
    }

    #[test]
    fn password_hash_roundtrip_and_policy() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(!needs_rehash(&h));
        assert!(needs_rehash("$argon2id$v=19$m=4096,t=3,p=1$c2FsdA$aGFzaA"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong horse battery", &h));
        assert!(!verify_password("x", "not-a-phc"));
        assert_eq!(check_password_policy("short", ""), Err(PasswordPolicyError::TooShort));
        assert_eq!(
            check_password_policy(&"x".repeat(129), ""),
            Err(PasswordPolicyError::TooLong)
        );
        assert!(check_password_policy("twelve chars", "").is_ok());
    }

    #[test]
    fn common_and_context_passwords_rejected() {
        for pw in ["0987654321qwerty", "123123QWERTY", " 123321qwerty "] {
            assert_eq!(check_password_policy(pw, ""), Err(PasswordPolicyError::Common), "{pw}");
        }
        assert!(
            COMMON_PASSWORDS
                .lines()
                .all(|l| l.chars().count() >= PASSWORD_MIN_CHARS && l == l.to_lowercase())
        );
        assert_eq!(
            check_password_policy("my Kizuna login 2026", ""),
            Err(PasswordPolicyError::ContainsContext)
        );
        assert_eq!(
            check_password_policy("hello-Alice.Smith-99", "alice.smith@example.test"),
            Err(PasswordPolicyError::ContainsContext)
        );
        assert!(
            check_password_policy("bob plus long phrase", "bob@example.test").is_ok(),
            "short local part ignored"
        );
        assert_eq!(PasswordPolicyError::Common.code(), "password_common");
    }

    #[test]
    fn tokens_roundtrip_and_kinds_are_distinct() {
        let t = Uuid::now_v7();
        let a = issue_token(TokenKind::Access, t);
        assert_eq!(parse_token(TokenKind::Access, &a.value), Some((t, a.hash)));
        assert_eq!(parse_token(TokenKind::Refresh, &a.value), None);
        let b = issue_token(TokenKind::Access, t);
        assert_ne!(a.value, b.value);
    }

    #[test]
    fn seal_open_binds_aad() {
        let k = [9u8; 32];
        let s = seal(&k, b"seed", b"aad1").unwrap();
        assert_eq!(open(&k, &s, b"aad1").as_deref(), Some(&b"seed"[..]));
        assert_eq!(open(&k, &s, b"aad2"), None);
        assert_eq!(open(&[8u8; 32], &s, b"aad1"), None);
        let mut t = s.clone();
        t[14] ^= 1;
        assert_eq!(open(&k, &t, b"aad1"), None);
        // KZ-C4: a pending TOTP blob does not open as the active secret (and vice versa).
        let (tn, u) = (Uuid::now_v7(), Uuid::now_v7());
        let pending = seal(&k, b"seed", &totp_pending_aad(tn, u)).unwrap();
        assert_eq!(open(&k, &pending, &totp_aad(tn, u)), None);
        let active = seal(&k, b"seed", &totp_aad(tn, u)).unwrap();
        assert_eq!(open(&k, &active, &totp_pending_aad(tn, u)), None);
    }

    proptest! {
        #[test]
        fn parse_token_never_panics(s in ".{0,200}") {
            let _ = parse_token(TokenKind::Access, &s);
            let _ = parse_token(TokenKind::Refresh, &s);
        }
    }
}
