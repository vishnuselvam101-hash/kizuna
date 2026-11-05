//! Versioned data keys for secrets at rest (TOTP seeds, WebAuthn credentials and challenge state).
//!
//! Envelope: `"KZ1" || key_id (u32 BE) || nonce || ciphertext+tag`; the 7-byte header is part of the AAD, so the
//! key id cannot be swapped. Blobs written before versioning (bare `nonce || ct`) are read with key id 1.
//! Rotation: configure a new current key, keep old keys in `KIZUNA_DATA_KEYS_PREVIOUS`, run
//! `kizuna-admin rotate-data-key`, confirm no stale blobs remain in any tenant, then drop the old key.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::ConfigError;
use crate::crypto;
use crate::db::{self, Tx};

const MAGIC: &[u8; 3] = b"KZ1";
const HEADER: usize = 7;
/// Key id assumed for pre-versioning blobs.
pub const LEGACY_KEY_ID: u32 = 1;

#[derive(Clone)]
pub struct DataKeys {
    current: u32,
    keys: BTreeMap<u32, [u8; 32]>,
}

impl fmt::Debug for DataKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DataKeys(current={}, ids={:?}, [REDACTED])",
            self.current,
            self.keys.keys()
        )
    }
}

pub fn parse_key(b64: &str, var: &'static str) -> Result<[u8; 32], ConfigError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| ConfigError::Invalid(var))?;
    raw.try_into().map_err(|_| ConfigError::Invalid(var))
}

fn parse_id(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|n| (1..=65_535).contains(n))
}

fn header(id: u32) -> [u8; HEADER] {
    let b = id.to_be_bytes();
    [MAGIC[0], MAGIC[1], MAGIC[2], b[0], b[1], b[2], b[3]]
}

fn envelope_id(blob: &[u8]) -> Option<u32> {
    (blob.len() > HEADER && blob.starts_with(MAGIC)).then(|| u32::from_be_bytes([blob[3], blob[4], blob[5], blob[6]]))
}

impl DataKeys {
    pub fn new(current: u32, key: [u8; 32]) -> Self {
        Self {
            current,
            keys: BTreeMap::from([(current, key)]),
        }
    }

    /// Adds a retired key that stays readable until rotation completes.
    pub fn with_previous(mut self, id: u32, key: [u8; 32]) -> Self {
        self.keys.entry(id).or_insert(key);
        self
    }

    /// `KIZUNA_DATA_KEY` (current, base64 of 32 bytes), `KIZUNA_DATA_KEY_ID` (1..=65535, default 1),
    /// `KIZUNA_DATA_KEYS_PREVIOUS` (optional `id:base64,id:base64`).
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let cur = get("KIZUNA_DATA_KEY")
            .filter(|v| !v.is_empty())
            .ok_or(ConfigError::Missing("KIZUNA_DATA_KEY"))?;
        let id = match get("KIZUNA_DATA_KEY_ID") {
            None => 1,
            Some(v) => parse_id(&v).ok_or(ConfigError::Invalid("KIZUNA_DATA_KEY_ID"))?,
        };
        let mut keys = Self::new(id, parse_key(&cur, "KIZUNA_DATA_KEY")?);
        const PREV: &str = "KIZUNA_DATA_KEYS_PREVIOUS";
        for item in get(PREV)
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
        {
            let (i, k) = item.split_once(':').ok_or(ConfigError::Invalid(PREV))?;
            let i = parse_id(i).ok_or(ConfigError::Invalid(PREV))?;
            if keys.keys.contains_key(&i) {
                return Err(ConfigError::Invalid(PREV));
            }
            keys.keys.insert(i, parse_key(k, PREV)?);
        }
        Ok(keys)
    }

    pub fn current_id(&self) -> u32 {
        self.current
    }

    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
        let header = header(self.current);
        let full_aad = [aad, &header].concat();
        let ct = crypto::seal(&self.keys[&self.current], plaintext, &full_aad)?;
        Ok([&header[..], &ct].concat())
    }

    pub fn open(&self, blob: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
        if let Some(id) = envelope_id(blob) {
            let full_aad = [aad, &blob[..HEADER]].concat();
            if let Some(pt) = self
                .keys
                .get(&id)
                .and_then(|k| crypto::open(k, &blob[HEADER..], &full_aad))
            {
                return Some(pt);
            }
            // A legacy blob whose random nonce happens to start with the magic falls through.
        }
        crypto::open(self.keys.get(&LEGACY_KEY_ID)?, blob, aad)
    }

    /// True when the blob is not sealed under the current key.
    pub fn is_stale(&self, blob: &[u8]) -> bool {
        envelope_id(blob) != Some(self.current)
    }
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct RotationStats {
    pub rewritten: u64,
    /// Blobs no configured key can open (left untouched; investigate before dropping any key).
    pub unreadable: u64,
    pub expired_challenges_deleted: u64,
}

/// Returns the re-sealed blob if it was stale and readable.
fn rewrap(keys: &DataKeys, blob: Option<Vec<u8>>, aad: &[u8], s: &mut RotationStats) -> Option<Vec<u8>> {
    let blob = blob.filter(|b| keys.is_stale(b))?;
    match keys.open(&blob, aad).and_then(|pt| keys.seal(&pt, aad).ok()) {
        Some(new) => {
            s.rewritten += 1;
            Some(new)
        }
        None => {
            s.unreadable += 1;
            None
        }
    }
}

/// Re-encrypts every secret of one tenant under the current key. Idempotent; runs in the tenant's RLS scope.
pub async fn reencrypt_tenant(
    tx: &mut Tx,
    keys: &DataKeys,
    tenant: Uuid,
    now: OffsetDateTime,
) -> Result<RotationStats, sqlx::Error> {
    db::set_tenant(tx, tenant).await?;
    let mut s = RotationStats {
        expired_challenges_deleted: sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at <= $1")
            .bind(now)
            .execute(&mut **tx)
            .await?
            .rows_affected(),
        ..Default::default()
    };

    type TotpBlobs = (Uuid, Option<Vec<u8>>, Option<Vec<u8>>);
    let users: Vec<TotpBlobs> = sqlx::query_as("SELECT id, totp_secret_enc, totp_pending_enc FROM users FOR UPDATE")
        .fetch_all(&mut **tx)
        .await?;
    for (uid, secret, pending) in users {
        let aad = crypto::totp_aad(tenant, uid);
        let secret = rewrap(keys, secret, &aad, &mut s);
        let pending = rewrap(keys, pending, &crypto::totp_pending_aad(tenant, uid), &mut s);
        if secret.is_some() || pending.is_some() {
            sqlx::query(
                "UPDATE users SET totp_secret_enc = COALESCE($2, totp_secret_enc), \
                 totp_pending_enc = COALESCE($3, totp_pending_enc) WHERE id = $1",
            )
            .bind(uid)
            .bind(secret)
            .bind(pending)
            .execute(&mut **tx)
            .await?;
        }
    }

    for (table, col, aad_fn) in [
        (
            "webauthn_credentials",
            "passkey_enc",
            crate::webauthn::credential_aad as fn(Uuid, Uuid, Uuid) -> Vec<u8>,
        ),
        ("webauthn_challenges", "state_enc", crate::webauthn::challenge_aad),
    ] {
        let rows: Vec<(Uuid, Uuid, Vec<u8>)> =
            sqlx::query_as(&format!("SELECT id, user_id, {col} FROM {table} FOR UPDATE"))
                .fetch_all(&mut **tx)
                .await?;
        for (id, uid, blob) in rows {
            if let Some(new) = rewrap(keys, Some(blob), &aad_fn(tenant, uid, id), &mut s) {
                sqlx::query(&format!("UPDATE {table} SET {col} = $2 WHERE id = $1"))
                    .bind(id)
                    .bind(new)
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }
    Ok(s)
}

/// Counts blobs in the current tenant scope not yet under the current key (0 = old keys can be removed).
pub async fn stale_count(tx: &mut Tx, keys: &DataKeys) -> Result<u64, sqlx::Error> {
    let blobs: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT b FROM (SELECT totp_secret_enc AS b FROM users UNION ALL SELECT totp_pending_enc FROM users \
         UNION ALL SELECT passkey_enc FROM webauthn_credentials UNION ALL SELECT state_enc FROM webauthn_challenges) x \
         WHERE b IS NOT NULL",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(blobs.iter().filter(|b| keys.is_stale(b)).count() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_key_roundtrip_and_header_binding() {
        let k = DataKeys::new(2, [2u8; 32]);
        let b = k.seal(b"seed", b"aad").unwrap();
        assert_eq!(&b[..HEADER], &header(2));
        assert_eq!(k.open(&b, b"aad").as_deref(), Some(&b"seed"[..]));
        assert_eq!(k.open(&b, b"other"), None, "AAD bound");
        let mut swapped = b.clone();
        swapped[6] = 3;
        let k2 = k.clone().with_previous(3, [2u8; 32]);
        assert_eq!(k2.open(&swapped, b"aad"), None, "key id is authenticated");
        assert!(!k.is_stale(&b));
    }

    #[test]
    fn old_and_legacy_blobs_stay_readable_until_key_removed() {
        let old = DataKeys::new(1, [1u8; 32]);
        let enveloped = old.seal(b"a", b"x").unwrap();
        let legacy = crypto::seal(&[1u8; 32], b"b", b"x").unwrap();
        let rotated = DataKeys::new(2, [2u8; 32]).with_previous(1, [1u8; 32]);
        assert_eq!(rotated.open(&enveloped, b"x").as_deref(), Some(&b"a"[..]));
        assert_eq!(rotated.open(&legacy, b"x").as_deref(), Some(&b"b"[..]));
        assert!(rotated.is_stale(&enveloped) && rotated.is_stale(&legacy));
        let dropped = DataKeys::new(2, [2u8; 32]);
        assert_eq!(dropped.open(&enveloped, b"x"), None);
        assert_eq!(dropped.open(&legacy, b"x"), None);
        assert_eq!(dropped.open(b"KZ1", b"x"), None, "short input");
        assert_eq!(dropped.open(b"", b"x"), None);
    }

    #[test]
    fn config_parsing_and_rejections() {
        let b64 = |b: u8| base64::engine::general_purpose::STANDARD.encode([b; 32]);
        let env = |pairs: Vec<(&'static str, String)>| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
        };
        let k = DataKeys::from_lookup(&env(vec![
            ("KIZUNA_DATA_KEY", b64(2)),
            ("KIZUNA_DATA_KEY_ID", "2".into()),
            ("KIZUNA_DATA_KEYS_PREVIOUS", format!("1:{}", b64(1))),
        ]))
        .unwrap();
        assert_eq!(k.current_id(), 2);
        assert!(!format!("{k:?}").contains("AgIC"), "debug redacts key material");
        let short = b64(1)[..10].to_owned();
        for bad in [
            vec![("KIZUNA_DATA_KEY", b64(2)), ("KIZUNA_DATA_KEY_ID", "0".into())],
            vec![
                ("KIZUNA_DATA_KEY", b64(2)),
                ("KIZUNA_DATA_KEYS_PREVIOUS", format!("1{}", b64(1))),
            ],
            vec![
                ("KIZUNA_DATA_KEY", b64(2)),
                ("KIZUNA_DATA_KEYS_PREVIOUS", format!("3:{short}")),
            ],
            // Duplicate id (current defaults to 1).
            vec![
                ("KIZUNA_DATA_KEY", b64(2)),
                ("KIZUNA_DATA_KEYS_PREVIOUS", format!("1:{}", b64(1))),
            ],
        ] {
            assert!(DataKeys::from_lookup(&env(bad)).is_err());
        }
        assert!(DataKeys::from_lookup(&env(vec![])).is_err());
    }
}
