// Used by tests/receipt.rs: a small audit chain with one delivery receipt event and
// one claim-commitment receipt event, a KZCP3 checkpoint over it, and receipts built from them.
#![allow(dead_code, clippy::unwrap_used)]

use audit_engine::{Event, chain_hash, checkpoint, merkle};
use kizuna_verify_pack::receipt::*;
use kizuna_verify_pack::{CheckpointEntry, EventEntry, H32, H64, key_id};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const SEED: [u8; 32] = [7u8; 32];
pub const TENANT: Uuid = Uuid::from_u128(0x1234);
pub const SUPPLIER: Uuid = Uuid::from_u128(0x5);
pub const RECEIPT: Uuid = Uuid::from_u128(0xe0);
pub const CLAIMS: Uuid = Uuid::from_u128(0xc0);
pub const ARTIFACT: Uuid = Uuid::from_u128(0xa1);
pub const REQUEST: Uuid = Uuid::from_u128(0xb7);
pub const FILE: &[u8] = b"%PDF-1.7 mill certificate BRK-100 tensile 520 MPa";
pub const RECEIPT_SEQ: i64 = 3;
pub const CLAIMS_SEQ: i64 = 7;
pub const N: i64 = 9;

pub fn openings() -> Vec<Opening> {
    vec![
        Opening {
            label: "lot/L-77/tensile_strength".into(),
            value: "520 MPa".into(),
            salt: H32([0x11; 32]),
        },
        Opening {
            label: "lot/L-77/heat_number".into(),
            value: "H-9".into(),
            salt: H32([0x22; 32]),
        },
    ]
}

pub struct Chain {
    pub events: Vec<(Event, [u8; 32], [u8; 32])>,
    pub tree: merkle::Tree,
    pub cp: CheckpointEntry,
}

fn details(seq: i64) -> (String, String, Option<String>, serde_json::Value) {
    let sha = hex::encode(Sha256::digest(FILE));
    match seq {
        2 => (
            "artifact.uploaded".into(),
            "artifact".into(),
            Some(ARTIFACT.to_string()),
            json!({ "sha256": sha, "size": FILE.len() }),
        ),
        RECEIPT_SEQ => (
            ISSUED.into(),
            TARGET_TYPE.into(),
            Some(RECEIPT.to_string()),
            serde_json::to_value(ReceiptDetails {
                receipt_id: RECEIPT,
                supplier_id: SUPPLIER,
                artifact_id: ARTIFACT,
                sha256: H32(Sha256::digest(FILE).into()),
                commitments: vec![],
                channel: Some("supplier_portal".into()),
                uploader: Some(UploaderRef {
                    kind: "supplier_user".into(),
                    id: Uuid::from_u128(0x5a),
                }),
                size: Some(FILE.len() as u64),
                filename: Some("ミルシート BRK-100.pdf".into()),
                capture_event_seq: Some(2),
                request_id: Some(REQUEST),
                delivery_receipt_id: None,
            })
            .unwrap(),
        ),
        CLAIMS_SEQ => (
            CLAIMS_COMMITTED.into(),
            TARGET_TYPE.into(),
            Some(CLAIMS.to_string()),
            serde_json::to_value(ReceiptDetails {
                receipt_id: CLAIMS,
                supplier_id: SUPPLIER,
                artifact_id: ARTIFACT,
                sha256: H32(Sha256::digest(FILE).into()),
                commitments: openings()
                    .iter()
                    .map(|o| Commitment {
                        label: o.label.clone(),
                        commitment: H32(commitment(&o.label, &o.value, &o.salt.0)),
                    })
                    .collect(),
                channel: None,
                uploader: None,
                size: None,
                filename: None,
                capture_event_seq: None,
                request_id: None,
                delivery_receipt_id: Some(RECEIPT),
            })
            .unwrap(),
        ),
        _ => ("other.event".into(), "x".into(), None, json!({ "n": seq })),
    }
}

pub fn chain() -> Chain {
    let mut prev = audit_engine::GENESIS;
    let mut events = Vec::new();
    for seq in 1..=N {
        let (action, target_type, target_id, details) = details(seq);
        let e = Event {
            tenant_id: TENANT,
            seq,
            occurred_at_us: 1_790_000_000_000_000 + seq * 1_000_000,
            actor_user_id: Some(Uuid::from_u128(0x5a)),
            actor_kind: "supplier_user".into(),
            action,
            target_type,
            target_id,
            details,
        };
        let h = chain_hash(&e, &prev).unwrap();
        events.push((e, prev, h));
        prev = h;
    }
    let tree = merkle::Tree::from_leaves(events.iter().map(|e| e.2));
    let public = checkpoint::public_key(&SEED);
    let head = events.last().unwrap().2;
    let at = 1_790_000_100_000_000;
    let root = tree.root();
    let prev_sig = [0u8; 32];
    let sig = checkpoint::sign(&SEED, &checkpoint::message_v3(TENANT, N, &head, at, &prev_sig, &root));
    let cp = CheckpointEntry {
        message_version: "KZCP3".into(),
        seq: N,
        head_hash: H32(head),
        signed_at_us: at,
        prev_signature_sha256: H32(prev_sig),
        merkle_root: H32(root),
        key_id: key_id(&public),
        public_key: H32(public),
        signature: H64(sig),
        head_proof: tree.prove(N as u64 - 1).unwrap().into_iter().map(H32).collect(),
    };
    Chain { events, tree, cp }
}

pub fn entry(c: &Chain, seq: i64, with_proof: bool) -> EventEntry {
    let (e, prev, h) = &c.events[(seq - 1) as usize];
    EventEntry {
        seq,
        occurred_at_us: e.occurred_at_us,
        actor_user_id: e.actor_user_id,
        actor_kind: e.actor_kind.clone(),
        action: e.action.clone(),
        target_type: e.target_type.clone(),
        target_id: e.target_id.clone(),
        details: e.details.clone(),
        prev_hash: H32(*prev),
        hash: H32(*h),
        proof: if with_proof {
            c.tree.prove(seq as u64 - 1).unwrap().into_iter().map(H32).collect()
        } else {
            vec![]
        },
    }
}

pub fn receipt(c: &Chain, id: Uuid, seq: i64, final_: bool, openings: Vec<Opening>) -> Receipt {
    Receipt {
        format: RECEIPT_FORMAT.into(),
        version: RECEIPT_VERSION,
        tenant_id: TENANT,
        receipt_id: id,
        event: entry(c, seq, final_),
        checkpoint: final_.then(|| c.cp.clone()),
        openings,
    }
}

pub fn bytes(r: &Receipt) -> Vec<u8> {
    build_receipt(r).unwrap()
}

/// A named test vector: (name, receipt bytes, optional file bytes).
pub type Vector = (&'static str, Vec<u8>, Option<Vec<u8>>);

pub fn vectors() -> Vec<Vector> {
    let c = chain();
    let delivery_final = bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]));
    let delivery_pending = bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![]));
    let claims_final = bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings()));
    let claims_one = bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings()[..1].to_vec()));
    let mut wrong = openings();
    wrong[0].value = "530 MPa".into();
    let claims_wrong = bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, wrong));
    let mut flipped = delivery_final.clone();
    let i = flipped.len() / 2;
    flipped[i] ^= 0x01;
    let mut other_cp = c.cp.clone();
    other_cp.seq = RECEIPT_SEQ - 1;
    let mut early = receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]);
    early.checkpoint = Some(other_cp);
    vec![
        ("delivery_final", delivery_final.clone(), None),
        ("delivery_final_with_file", delivery_final.clone(), Some(FILE.to_vec())),
        (
            "delivery_final_wrong_file",
            delivery_final,
            Some(b"another file".to_vec()),
        ),
        ("delivery_pending", delivery_pending, Some(FILE.to_vec())),
        ("claims_final_all_opened", claims_final, None),
        ("claims_final_one_opened", claims_one, None),
        ("claims_wrong_opening", claims_wrong, None),
        ("bit_flipped", flipped, None),
        ("checkpoint_before_event", bytes(&early), None),
        ("garbage", b"{\"format\":1}".to_vec(), None),
    ]
}
