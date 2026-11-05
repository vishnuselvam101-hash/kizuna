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
    use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
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

    /// KZCP2: like KZCP1 plus the SHA-256 of the previous checkpoint's signature (all-zero for the first).
    pub fn message_v2(tenant: Uuid, seq: i64, head: &[u8], signed_at_us: i64, prev_sig_sha256: &[u8; 32]) -> Vec<u8> {
        let mut m = b"KZCP2".to_vec();
        m.extend_from_slice(&message(tenant, seq, head, signed_at_us)[5..]);
        m.extend_from_slice(prev_sig_sha256);
        m
    }

    /// KZCP3: like KZCP2 plus the RFC 6962 Merkle root over the chain hashes of events `1..=seq` (leaf `i-1` is
    /// the hash of event `i`), so single events can be proven included without disclosing the others.
    pub fn message_v3(
        tenant: Uuid,
        seq: i64,
        head: &[u8],
        signed_at_us: i64,
        prev_sig_sha256: &[u8; 32],
        merkle_root: &[u8; 32],
    ) -> Vec<u8> {
        let mut m = b"KZCP3".to_vec();
        m.extend_from_slice(&message_v2(tenant, seq, head, signed_at_us, prev_sig_sha256)[5..]);
        m.extend_from_slice(merkle_root);
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
        // Strict: rejects small-order keys and non-canonical signatures (deep review KZ-C2).
        vk.verify_strict(msg, &Signature::from_bytes(&sig)).is_ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn small_order_key_and_non_canonical_signature_are_rejected() {
            // Identity point (small order) as public key with an all-zero-ish signature: accepted by lax verify.
            let mut identity = [0u8; 32];
            identity[0] = 1;
            let mut sig = [0u8; 64];
            sig[0] = 1;
            assert!(!verify(&identity, b"m", &sig));
            // A valid signature with S + L (non-canonical scalar) is refused.
            let seed = [7u8; 32];
            let mut good = sign(&seed, b"m");
            assert!(verify(&public_key(&seed), b"m", &good));
            const L: [u8; 32] = [
                0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
            ];
            let mut carry = 0u16;
            for i in 0..32 {
                let v = u16::from(good[32 + i]) + u16::from(L[i]) + carry;
                good[32 + i] = (v & 0xff) as u8;
                carry = v >> 8;
            }
            assert!(!verify(&public_key(&seed), b"m", &good));
        }

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

/// RFC 6962 (Certificate Transparency) Merkle tree over chain hashes, via the `ct-merkle` crate. Leaf `i` is the
/// chain hash of the event with `seq = i + 1`; leaf and node hashes are domain-separated (0x00 / 0x01 prefixes).
pub mod merkle {
    use ct_merkle::mem_backed_tree::MemoryBackedTree;
    use ct_merkle::{InclusionProof, RootHash};
    use sha2_11::Sha256;

    pub struct Tree(MemoryBackedTree<Sha256, [u8; 32]>);

    impl Tree {
        pub fn from_leaves(leaves: impl IntoIterator<Item = [u8; 32]>) -> Self {
            let mut t = MemoryBackedTree::new();
            for l in leaves {
                t.push(l);
            }
            Tree(t)
        }

        pub fn len(&self) -> u64 {
            self.0.len()
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        pub fn root(&self) -> [u8; 32] {
            self.0.root().as_bytes().as_slice().try_into().unwrap_or([0u8; 32])
        }

        /// Audit path for leaf `idx` (sibling hashes from the leaf up), or `None` when out of range.
        pub fn prove(&self, idx: u64) -> Option<Vec<[u8; 32]>> {
            if idx >= self.len() {
                return None;
            }
            let p = self.0.prove_inclusion(usize::try_from(idx).ok()?);
            Some(p.as_bytes().chunks(32).filter_map(|c| c.try_into().ok()).collect())
        }
    }

    /// Why an inclusion proof was refused.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ProofError {
        IndexOutOfRange,
        WrongLength,
        RootMismatch,
    }

    /// Checks that `leaf` is leaf `idx` of the tree of `size` leaves with root `root`.
    pub fn verify(root: &[u8; 32], size: u64, idx: u64, leaf: &[u8; 32], path: &[[u8; 32]]) -> Result<(), ProofError> {
        if size == 0 || idx >= size {
            return Err(ProofError::IndexOutOfRange);
        }
        if path.len() != ct_merkle::indices_for_inclusion_proof(size, idx).len() {
            return Err(ProofError::WrongLength);
        }
        let digest = ct_merkle::digest::Output::<Sha256>::from(*root);
        let rh = RootHash::<Sha256>::new(digest, size);
        let proof = InclusionProof::<Sha256>::from_bytes(path.concat());
        rh.verify_inclusion(leaf, idx, &proof)
            .map_err(|_| ProofError::RootMismatch)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use proptest::prelude::*;

        fn leaf(i: u64) -> [u8; 32] {
            let mut l = [0u8; 32];
            l[..8].copy_from_slice(&i.to_be_bytes());
            l
        }

        #[test]
        fn rfc6962_empty_and_single() {
            // RFC 6962: MTH({d0}) = SHA-256(0x00 || d0).
            use sha2_11::Digest;
            let t = Tree::from_leaves([leaf(0)]);
            let want: [u8; 32] = Sha256::new_with_prefix([0u8]).chain_update(leaf(0)).finalize().into();
            assert_eq!(t.root(), want);
            assert_eq!(t.prove(0), Some(vec![]));
            assert_eq!(t.prove(1), None);
        }

        proptest! {
            #[test]
            fn every_leaf_proves_and_tamper_fails(n in 1u64..80, pick in 0u64..80, flip in 0usize..32) {
                let t = Tree::from_leaves((0..n).map(leaf));
                let i = pick % n;
                let path = t.prove(i).unwrap_or_default();
                prop_assert!(verify(&t.root(), n, i, &leaf(i), &path).is_ok());
                prop_assert!(verify(&t.root(), n, i, &leaf(i + 1000), &path).is_err());
                if !path.is_empty() {
                    let mut bad = path.clone();
                    let k = flip % bad.len();
                    bad[k][flip] ^= 1;
                    prop_assert_eq!(verify(&t.root(), n, i, &leaf(i), &bad), Err(ProofError::RootMismatch));
                    prop_assert_eq!(verify(&t.root(), n, i, &leaf(i), &path[1..]), Err(ProofError::WrongLength));
                }
                prop_assert_eq!(verify(&t.root(), n, n, &leaf(i), &path), Err(ProofError::IndexOutOfRange));
            }
        }
    }
}

#[cfg(test)]
mod tests;
