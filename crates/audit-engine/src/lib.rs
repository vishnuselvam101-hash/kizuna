//! Tamper-evident, per-tenant hash-chained audit log (pure logic, no I/O).
//!
//! `hash_n = SHA-256(encode(event_n) || prev_hash)` where `prev_hash` of the first event is 32 zero bytes.
//! The encoding is length-prefixed and versioned so no two different events share an encoding.
//! A valid chain proves the stored events are unchanged since they were written (and that none were removed
//! from the middle); it does **not** prove that what they describe is true.

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const GENESIS: [u8; 32] = [0u8; 32];
const MAGIC: &[u8; 4] = b"KZA1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuditError {
    #[error("floating-point numbers are not allowed in audit details")]
    FloatNotAllowed,
    #[error("NUL characters are not allowed in audit data")]
    NulNotAllowed,
}

/// One audit event as recorded. `details` must be canonicalisable (no floats).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub tenant_id: Uuid,
    pub seq: i64,
    /// Microseconds since the Unix epoch (PostgreSQL `timestamptz` precision).
    pub occurred_at_us: i64,
    pub actor_user_id: Option<Uuid>,
    pub actor_kind: String,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub details: Value,
}

/// An event together with its stored chain links.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    pub event: Event,
    pub prev_hash: Vec<u8>,
    pub hash: Vec<u8>,
}

/// Canonical JSON: object keys sorted by UTF-8 bytes, no whitespace, integers only.
pub fn canonical_json(v: &Value) -> Result<String, AuditError> {
    let mut out = String::new();
    write_canonical(v, &mut out)?;
    Ok(out)
}

fn write_str(s: &str, out: &mut String) -> Result<(), AuditError> {
    if s.contains('\0') {
        return Err(AuditError::NulNotAllowed);
    }
    // serde_json string escaping is deterministic.
    out.push_str(&serde_json::to_string(s).unwrap_or_default());
    Ok(())
}

fn write_canonical(v: &Value, out: &mut String) -> Result<(), AuditError> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return Err(AuditError::FloatNotAllowed);
            }
        }
        Value::String(s) => write_str(s, out)?,
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out)?;
                out.push(':');
                write_canonical(&m[k.as_str()], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn put(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    buf.extend_from_slice(bytes);
}

fn put_opt(buf: &mut Vec<u8>, bytes: Option<&[u8]>) {
    match bytes {
        None => buf.push(0),
        Some(b) => {
            buf.push(1);
            put(buf, b);
        }
    }
}

/// Deterministic, injective byte encoding of an event (without chain links).
pub fn encode(e: &Event) -> Result<Vec<u8>, AuditError> {
    for s in [&e.actor_kind, &e.action, &e.target_type] {
        if s.contains('\0') {
            return Err(AuditError::NulNotAllowed);
        }
    }
    if e.target_id.as_deref().is_some_and(|s| s.contains('\0')) {
        return Err(AuditError::NulNotAllowed);
    }
    let details = canonical_json(&e.details)?;
    let mut b = Vec::with_capacity(128 + details.len());
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(e.tenant_id.as_bytes());
    b.extend_from_slice(&e.seq.to_be_bytes());
    b.extend_from_slice(&e.occurred_at_us.to_be_bytes());
    put_opt(&mut b, e.actor_user_id.as_ref().map(|u| u.as_bytes().as_slice()));
    put(&mut b, e.actor_kind.as_bytes());
    put(&mut b, e.action.as_bytes());
    put(&mut b, e.target_type.as_bytes());
    put_opt(&mut b, e.target_id.as_deref().map(str::as_bytes));
    put(&mut b, details.as_bytes());
    Ok(b)
}

/// Chain hash of an event given the previous event's hash.
pub fn chain_hash(e: &Event, prev_hash: &[u8]) -> Result<[u8; 32], AuditError> {
    let mut h = Sha256::new();
    h.update(encode(e)?);
    h.update(prev_hash);
    Ok(h.finalize().into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Break {
    /// Sequence numbers are not 1, 2, 3, ... (an event was removed or inserted).
    SequenceGap { expected: i64, found: i64 },
    /// `prev_hash` does not equal the previous event's hash.
    BrokenLink { seq: i64 },
    /// Recomputed hash differs from the stored hash (event content changed).
    HashMismatch { seq: i64 },
    /// Event cannot be encoded (e.g. a float was written into details).
    Unencodable { seq: i64 },
    /// Event belongs to a different tenant than the chain.
    WrongTenant { seq: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub events_checked: usize,
    pub first_break: Option<Break>,
    pub head_hash: [u8; 32],
}

impl VerifyReport {
    pub fn is_intact(&self) -> bool {
        self.first_break.is_none()
    }
}

/// Incremental verifier: feed events in `seq` order (e.g. in database batches) without holding the chain in memory.
pub struct ChainVerifier {
    tenant_id: Uuid,
    prev: [u8; 32],
    checked: usize,
    base: usize,
    broken: Option<Break>,
}

impl ChainVerifier {
    pub fn new(tenant_id: Uuid) -> Self {
        ChainVerifier {
            tenant_id,
            prev: GENESIS,
            checked: 0,
            base: 0,
            broken: None,
        }
    }

    /// Continues a chain after a trusted anchor (e.g. a verified signed checkpoint at `after_seq` with `hash`).
    pub fn resume(tenant_id: Uuid, after_seq: i64, hash: [u8; 32]) -> Self {
        let base = usize::try_from(after_seq.max(0)).unwrap_or(0);
        ChainVerifier {
            tenant_id,
            prev: hash,
            checked: base,
            base,
            broken: None,
        }
    }

    /// Returns false once the chain is broken (further events are ignored).
    pub fn feed(&mut self, se: &StoredEvent) -> bool {
        if self.broken.is_some() {
            return false;
        }
        let expected = i64::try_from(self.checked).unwrap_or(i64::MAX).saturating_add(1);
        let b = if se.event.tenant_id != self.tenant_id {
            Some(Break::WrongTenant { seq: se.event.seq })
        } else if se.event.seq != expected {
            Some(Break::SequenceGap {
                expected,
                found: se.event.seq,
            })
        } else if se.prev_hash.as_slice() != self.prev.as_slice() {
            Some(Break::BrokenLink { seq: se.event.seq })
        } else {
            match chain_hash(&se.event, &self.prev) {
                Err(_) => Some(Break::Unencodable { seq: se.event.seq }),
                Ok(h) if se.hash.as_slice() != h.as_slice() => Some(Break::HashMismatch { seq: se.event.seq }),
                Ok(h) => {
                    self.prev = h;
                    None
                }
            }
        };
        match b {
            Some(b) => {
                self.broken = Some(b);
                false
            }
            None => {
                self.checked += 1;
                true
            }
        }
    }

    pub fn report(&self) -> VerifyReport {
        VerifyReport {
            events_checked: self.checked - self.base,
            first_break: self.broken.clone(),
            head_hash: self.prev,
        }
    }
}

/// Verifies a full chain for one tenant, ordered by `seq`, starting at seq 1.
/// Truncation of the tail is only detectable against an externally kept head (checkpoint).
pub fn verify_chain(tenant_id: Uuid, events: &[StoredEvent]) -> VerifyReport {
    let mut v = ChainVerifier::new(tenant_id);
    for e in events {
        if !v.feed(e) {
            break;
        }
    }
    v.report()
}

/// Signed checkpoints: an Ed25519 signature over (tenant, seq, head hash, time). Exported outside the database they
/// make a full re-chaining by a database owner detectable, as long as the signing key is not in the database.
pub mod checkpoint {
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
    use uuid::Uuid;

    pub fn message(tenant: Uuid, seq: i64, head: &[u8], signed_at_us: i64) -> Vec<u8> {
        let mut m = b"KZCP1".to_vec();
        m.extend_from_slice(tenant.as_bytes());
        m.extend_from_slice(&seq.to_be_bytes());
        m.extend_from_slice(&u32::try_from(head.len()).unwrap_or(u32::MAX).to_be_bytes());
        m.extend_from_slice(head);
        m.extend_from_slice(&signed_at_us.to_be_bytes());
        m
    }

    pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
        SigningKey::from_bytes(seed).verifying_key().to_bytes()
    }

    pub fn sign(seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(seed).sign(msg).to_bytes()
    }

    pub fn verify(public: &[u8; 32], msg: &[u8], sig: &[u8]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(public) else {
            return false;
        };
        let Ok(sig) = <[u8; 64]>::try_from(sig) else {
            return false;
        };
        vk.verify(msg, &Signature::from_bytes(&sig)).is_ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sign_verify_and_tamper() {
            let seed = [7u8; 32];
            let t = Uuid::from_u128(3);
            let m = message(t, 10, &[1u8; 32], 99);
            let s = sign(&seed, &m);
            assert!(verify(&public_key(&seed), &m, &s));
            assert!(!verify(&public_key(&seed), &message(t, 11, &[1u8; 32], 99), &s));
            assert!(!verify(&public_key(&[8u8; 32]), &m, &s));
            assert!(!verify(&public_key(&seed), &m, &s[..63]));
        }
    }
}

#[cfg(test)]
mod tests;
