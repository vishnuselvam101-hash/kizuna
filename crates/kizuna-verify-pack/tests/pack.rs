#![allow(clippy::unwrap_used)]
//! Round-trip and tamper tests: every manipulation must fail with its own specific check.

use std::io::{Cursor, Read, Write};

use audit_engine::{Event, canonical_json, chain_hash, checkpoint, merkle};
use kizuna_verify_pack::*;
use proptest::prelude::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SEED: [u8; 32] = [7u8; 32];
const OTHER: [u8; 32] = [9u8; 32];
const TENANT: Uuid = Uuid::from_u128(0x1234);
const DECISION: Uuid = Uuid::from_u128(0xdec);
const A1: Uuid = Uuid::from_u128(0xa1);
const A2: Uuid = Uuid::from_u128(0xa2);
const N: i64 = 13;

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

struct Fixture {
    events: Vec<(Event, [u8; 32], [u8; 32])>,
    tree: merkle::Tree,
    derivation: Vec<u8>,
    files: [Vec<u8>; 2],
}

fn fixture() -> Fixture {
    let files = [
        b"%PDF-1.7 mill certificate BRK-100 tensile 520 MPa".to_vec(),
        "見積書 BRK-100 単価 1200円".as_bytes().to_vec(),
    ];
    let derivation = canonical_json(&json!({
        "engine": "test", "outcome": "accept",
        "evidence": [
            { "claim_id": Uuid::from_u128(0xc1), "artifact_id": A1, "artifact_sha256": hex::encode(sha(&files[0])), "source_location": { "page": 1, "line": 3 } },
            { "claim_id": Uuid::from_u128(0xc2), "artifact_id": A2, "artifact_sha256": hex::encode(sha(&files[1])), "source_location": { "page": 2 } },
            { "claim_id": Uuid::from_u128(0xc3), "artifact_id": null, "source_location": null },
        ],
    }))
    .unwrap()
    .into_bytes();
    let mut prev = audit_engine::GENESIS;
    let mut events = Vec::new();
    for seq in 1..=N {
        let (action, ttype, target, details) = match seq {
            2 => (
                "artifact.uploaded",
                "artifact",
                Some(A1.to_string()),
                json!({ "sha256": hex::encode(sha(&files[0])), "size": files[0].len() }),
            ),
            4 => (
                "artifact.uploaded",
                "artifact",
                Some(A2.to_string()),
                json!({ "sha256": hex::encode(sha(&files[1])), "size": files[1].len() }),
            ),
            7 => (
                "decision.recorded",
                "decision",
                Some(DECISION.to_string()),
                json!({ "derivation_sha256": hex::encode(sha(&derivation)), "kind": "x" }),
            ),
            _ => ("claim.recorded", "claim", Some(format!("c{seq}")), json!({ "n": seq })),
        };
        let e = Event {
            tenant_id: TENANT,
            seq,
            occurred_at_us: 1_790_000_000_000_000 + seq * 1_000_000,
            actor_user_id: Some(Uuid::from_u128(0x5)),
            actor_kind: "user".into(),
            action: action.into(),
            target_type: ttype.into(),
            target_id: target,
            details,
        };
        let h = chain_hash(&e, &prev).unwrap();
        events.push((e, prev, h));
        prev = h;
    }
    let tree = merkle::Tree::from_leaves(events.iter().map(|e| e.2));
    Fixture {
        events,
        tree,
        derivation,
        files,
    }
}

fn entry(f: &Fixture, seq: i64) -> EventEntry {
    let (e, prev, h) = &f.events[(seq - 1) as usize];
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
        proof: f.tree.prove((seq - 1) as u64).unwrap().into_iter().map(H32).collect(),
    }
}

fn checkpoint_entry(f: &Fixture, seed: &[u8; 32]) -> CheckpointEntry {
    let head = f.events.last().unwrap().2;
    let root = f.tree.root();
    let at = 1_790_000_100_000_000;
    let prev = [3u8; 32];
    let public = checkpoint::public_key(seed);
    let sig = checkpoint::sign(seed, &checkpoint::message_v3(TENANT, N, &head, at, &prev, &root));
    CheckpointEntry {
        message_version: "KZCP3".into(),
        seq: N,
        head_hash: H32(head),
        signed_at_us: at,
        prev_signature_sha256: H32(prev),
        merkle_root: H32(root),
        key_id: key_id(&public),
        public_key: H32(public),
        signature: H64(sig),
        head_proof: f.tree.prove((N - 1) as u64).unwrap().into_iter().map(H32).collect(),
    }
}

fn artifact(id: Uuid, name: &str, b: &[u8], seq: i64) -> ArtifactEntry {
    ArtifactEntry {
        id,
        name: name.into(),
        media_type: "application/pdf".into(),
        sha256: H32(sha(b)),
        size: b.len() as u64,
        included: true,
        excluded_reason: None,
        path: None,
        capture_event_seq: seq,
    }
}

fn pack_with(seed: &[u8; 32], hash_only_a2: bool) -> Vec<u8> {
    let f = fixture();
    let input = BuildInput {
        generated_at_us: 1_790_000_200_000_000,
        tenant_id: TENANT,
        subject: Subject {
            kind: "decision".into(),
            id: DECISION,
            derivation_sha256: H32(sha(&f.derivation)),
            recorded_event_seq: 7,
        },
        artifacts: vec![
            (artifact(A1, "mill-cert.pdf", &f.files[0], 2), Some(f.files[0].clone())),
            (
                artifact(A2, "見積書.pdf", &f.files[1], 4),
                (!hash_only_a2).then(|| f.files[1].clone()),
            ),
        ],
        events: [2, 4, 7].iter().map(|s| entry(&f, *s)).collect(),
        checkpoint: checkpoint_entry(&f, seed),
        documents: vec![
            (DERIVATION.into(), f.derivation.clone()),
            ("decision/record.json".into(), br#"{"title":"accept BRK-100"}"#.to_vec()),
        ],
    };
    build(input, seed).unwrap()
}

fn pack() -> Vec<u8> {
    pack_with(&SEED, false)
}

fn check(b: &[u8]) -> Report {
    verify(Cursor::new(b), &VerifyOptions::default())
}

fn entries(b: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut za = zip::ZipArchive::new(Cursor::new(b)).unwrap();
    (0..za.len())
        .map(|i| {
            let mut f = za.by_index(i).unwrap();
            let mut v = Vec::new();
            f.read_to_end(&mut v).unwrap();
            (f.name().unwrap().into_owned(), v)
        })
        .collect()
}

fn rezip(items: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (n, b) in items {
        zw.start_file(n.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

/// Rewrites one entry.
fn tamper(b: &[u8], name: &str, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut items = entries(b);
    let it = items.iter_mut().find(|(n, _)| n == name).unwrap();
    f(&mut it.1);
    rezip(&items)
}

/// Edits the manifest as JSON and writes it back canonically (an attacker's best effort).
fn tamper_manifest(b: &[u8], f: impl FnOnce(&mut Value)) -> Vec<u8> {
    tamper(b, MANIFEST, |m| {
        let mut v: Value = serde_json::from_slice(m).unwrap();
        f(&mut v);
        *m = canonical_json(&v).unwrap().into_bytes();
    })
}

fn assert_fails(r: &Report, code: &str, needle: &str) {
    assert!(!r.ok, "must fail");
    let c = r
        .failures()
        .find(|c| c.code == code)
        .unwrap_or_else(|| panic!("no {code} failure in {:#?}", r.failures().collect::<Vec<_>>()));
    assert!(c.msg.en.contains(needle), "{code}: {:?} lacks {needle:?}", c.msg.en);
    assert!(!c.msg.ja.is_empty());
    assert!(r.proven.is_empty(), "a failed pack proves nothing");
}

#[test]
fn round_trip_verifies_and_states_limits() {
    let r = check(&pack());
    assert!(r.ok, "{}", render(&r, Lang::En));
    let text = render(&r, Lang::Both);
    assert!(text.contains("RESULT: VERIFIED"));
    assert!(text.contains("byte-identical"));
    assert!(text.contains("signed with Ed25519 key"));
    assert!(text.contains("included in the audit chain up to checkpoint #13"));
    assert!(text.contains("exactly the one recorded at decision time"));
    // Directive §9: the report says explicitly that content truth is not proven.
    assert!(text.contains("That any document's content is true"));
    assert!(text.contains("文書の内容が真実"));
    assert!(text.contains("--trusted-key"), "unpinned key must be flagged");
    // Pinned by key id and by full key.
    let kid = key_id(&checkpoint::public_key(&SEED));
    let pinned = verify(Cursor::new(pack()), &VerifyOptions { trusted_key: Some(kid) });
    assert!(pinned.ok);
    assert!(!render(&pinned, Lang::En).contains("rerun with --trusted-key"));
    let full = hex::encode(checkpoint::public_key(&SEED));
    assert!(
        verify(
            Cursor::new(pack()),
            &VerifyOptions {
                trusted_key: Some(full)
            }
        )
        .ok
    );
}

#[test]
fn build_is_deterministic() {
    assert_eq!(pack(), pack());
}

#[test]
fn hash_only_redaction_verifies_without_the_file() {
    let p = pack_with(&SEED, true);
    assert!(
        !entries(&p).iter().any(|(n, _)| n.contains(&A2.to_string())),
        "redacted file must not be shipped"
    );
    let r = check(&p);
    assert!(r.ok, "{}", render(&r, Lang::En));
    let text = render(&r, Lang::En);
    assert!(text.contains("The file is not in this pack (hash only)"));
    assert!(text.contains("The content of hash-only documents"));
}

#[test]
fn tamper_file_byte() {
    let path = format!("artifacts/{A1}/mill-cert.pdf");
    let r = check(&tamper(&pack(), &path, |b| b[5] ^= 1));
    assert_fails(&r, "file_hash_mismatch", "was modified");
    assert_fails(&r, "artifact_file_mismatch", "differs from the captured");
}

#[test]
fn tamper_derivation_document_even_with_consistent_manifest() {
    // Changing a source location and fixing the manifest's file hash still breaks the export signature and
    // the hash recorded in the audit chain at decision time.
    let p = pack();
    let mut items = entries(&p);
    let d = items.iter_mut().find(|(n, _)| n == DERIVATION).unwrap();
    d.1 = String::from_utf8(d.1.clone())
        .unwrap()
        .replace("\"line\":3", "\"line\":4")
        .into_bytes();
    let new = sha(&d.1);
    let p = rezip(&items);
    let p = tamper_manifest(&p, |m| {
        for f in m["files"].as_array_mut().unwrap() {
            if f["path"] == DERIVATION {
                f["sha256"] = json!(hex::encode(new));
            }
        }
    });
    let r = check(&p);
    assert_fails(
        &r,
        "derivation_hash_mismatch",
        "derivation hash recorded at decision time",
    );
    assert_fails(&r, "export_signature_invalid", "changed after export");
}

#[test]
fn tamper_manifest_field() {
    let r = check(&tamper_manifest(&pack(), |m| {
        m["artifacts"][0]["name"] = json!("other.pdf")
    }));
    assert_fails(&r, "export_signature_invalid", "manifest signature is invalid");
    let r = check(&tamper_manifest(&pack(), |m| {
        m["artifacts"][0]["sha256"] = json!("00".repeat(32))
    }));
    assert_fails(&r, "artifact_capture_event_mismatch", "did not capture");
    let r = check(&tamper_manifest(&pack(), |m| {
        m["checkpoint"]["signed_at_us"] = json!(1_790_000_000_000_001i64)
    }));
    assert_fails(&r, "checkpoint_signature_invalid", "signature is invalid");
    let r = check(&tamper_manifest(&pack(), |m| {
        m["subject"]["recorded_event_seq"] = json!(2)
    }));
    assert_fails(&r, "decision_event_mismatch", "does not record decision");
    let r = check(&tamper_manifest(&pack(), |m| m["version"] = json!(2)));
    assert_fails(&r, "manifest_unsupported", "unsupported pack format");
    let r = check(&tamper_manifest(&pack(), |m| m["extra"] = json!(1)));
    assert_fails(&r, "manifest_invalid", "unknown field");
    // Valid JSON, same content, not canonical (whitespace).
    let r = check(&tamper(&pack(), MANIFEST, |m| m.insert(1, b' ')));
    assert_fails(&r, "manifest_not_canonical", "canonical");
}

#[test]
fn tamper_audit_event_content() {
    let r = check(&tamper_manifest(&pack(), |m| {
        m["events"][2]["details"]["kind"] = json!("y")
    }));
    assert_fails(
        &r,
        "event_hash_mismatch",
        "audit event #7 (decision.recorded) was changed",
    );
    assert_fails(&r, "decision_event_missing", "#7");
}

#[test]
fn tamper_proof_node() {
    let r = check(&tamper_manifest(&pack(), |m| {
        let s = m["events"][0]["proof"][1].as_str().unwrap().to_string();
        let first = if s.starts_with('0') { '1' } else { '0' };
        let flipped = format!("{first}{}", s.get(1..).unwrap());
        m["events"][0]["proof"][1] = json!(flipped);
    }));
    assert_fails(
        &r,
        "event_proof_invalid",
        "audit event #2 does not lead to the signed checkpoint root",
    );
    assert_fails(&r, "artifact_capture_event_missing", "#2");
}

#[test]
fn tamper_signatures() {
    let r = check(&tamper_manifest(&pack(), |m| {
        let s = m["checkpoint"]["signature"].as_str().unwrap().to_string();
        m["checkpoint"]["signature"] = json!(format!("{}{}", if s.starts_with('0') { "1" } else { "0" }, &s[1..]));
    }));
    assert_fails(
        &r,
        "checkpoint_signature_invalid",
        "checkpoint #13 signature is invalid",
    );
    let r = check(&tamper(&pack(), SIGNATURE, |s| {
        let mut v: Value = serde_json::from_slice(s).unwrap();
        let sig = v["signature"].as_str().unwrap().to_string();
        v["signature"] = json!(format!("{}{}", &sig[..127], if sig.ends_with('0') { "1" } else { "0" }));
        *s = canonical_json(&v).unwrap().into_bytes();
    }));
    assert_fails(&r, "export_signature_invalid", "manifest signature is invalid");
    let mut items = entries(&pack());
    items.retain(|(n, _)| n != SIGNATURE);
    assert_fails(&check(&rezip(&items)), "export_signature_missing", "missing");
}

#[test]
fn wrong_key() {
    // A complete forgery under another key is self-consistent: only pinning the tenant key catches it.
    let forged = pack_with(&OTHER, false);
    assert!(check(&forged).ok);
    let kid = key_id(&checkpoint::public_key(&SEED));
    let r = verify(
        Cursor::new(&forged),
        &VerifyOptions {
            trusted_key: Some(kid.clone()),
        },
    );
    assert_fails(&r, "trusted_key_mismatch", &format!("not by the trusted key {kid}"));
    // Swapping in another public key (and its id) breaks both signatures.
    let other = checkpoint::public_key(&OTHER);
    let r = check(&tamper_manifest(&pack(), |m| {
        m["checkpoint"]["public_key"] = json!(hex::encode(other));
        m["checkpoint"]["key_id"] = json!(key_id(&other));
    }));
    assert_fails(&r, "checkpoint_signature_invalid", "signature is invalid");
    assert_fails(&r, "export_key_mismatch", "but the checkpoint by key");
    // A key id that does not belong to the key.
    let r = check(&tamper_manifest(&pack(), |m| {
        m["checkpoint"]["key_id"] = json!("0011223344556677")
    }));
    assert_fails(&r, "key_id_mismatch", "does not belong to public key");
}

#[test]
fn truncated_chain() {
    // Proof path cut short.
    let r = check(&tamper_manifest(&pack(), |m| {
        m["events"][2]["proof"].as_array_mut().unwrap().pop();
    }));
    assert_fails(&r, "event_proof_truncated", "proof truncated or padded");
    // Chain claimed shorter than the signed checkpoint.
    let r = check(&tamper_manifest(&pack(), |m| m["checkpoint"]["seq"] = json!(7)));
    assert_fails(&r, "checkpoint_signature_invalid", "signature is invalid");
    // An event dropped from the pack.
    let r = check(&tamper_manifest(&pack(), |m| {
        m["events"].as_array_mut().unwrap().remove(0);
    }));
    assert_fails(
        &r,
        "artifact_capture_event_missing",
        "capture audit event #2 is not in the pack",
    );
    // Head hash no longer shown to be the last leaf of the signed tree.
    let r = check(&tamper_manifest(&pack(), |m| m["checkpoint"]["head_proof"] = json!([])));
    assert_fails(&r, "checkpoint_head_not_in_tree", "not the last leaf");
}

#[test]
fn hostile_zip_structure() {
    let p = pack();
    let mut items = entries(&p);
    items.push(("../evil.sh".into(), b"rm -rf /".to_vec()));
    assert_fails(&check(&rezip(&items)), "zip_bad_path", "unsafe path");
    let mut items = entries(&p);
    items.push(("extra.txt".into(), b"x".to_vec()));
    assert_fails(&check(&rezip(&items)), "zip_unlisted_entry", "not in the manifest");
    let mut items = entries(&p);
    items.retain(|(n, _)| !n.starts_with("artifacts/"));
    assert_fails(&check(&rezip(&items)), "file_missing", "missing from the pack");
    let mut items = entries(&p);
    items.retain(|(n, _)| n != MANIFEST);
    assert_fails(&check(&rezip(&items)), "manifest_missing", "missing");
    assert_fails(&check(b"not a zip"), "zip_unreadable", "not a readable ZIP");
}

proptest! {
    #[test]
    fn fuzz_manifest_parser_arbitrary_bytes(b in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse_manifest(&b);
    }

    #[test]
    fn fuzz_manifest_parser_mutations(pos in any::<usize>(), byte in any::<u8>(), cut in any::<usize>()) {
        let p = pack();
        let m = entries(&p).into_iter().find(|(n, _)| n == MANIFEST).unwrap().1;
        let mut x = m.clone();
        let i = pos % x.len();
        x[i] = byte;
        if x != m {
            // Any single-byte change of a canonical manifest is rejected or changes the parsed value.
            if let Ok(parsed) = parse_manifest(&x) {
                prop_assert_ne!(parsed, parse_manifest(&m).unwrap());
            }
        }
        let _ = parse_manifest(&m[..cut % m.len()]);
    }

    #[test]
    fn fuzz_verify_never_accepts_a_mutated_pack(pos in any::<usize>(), byte in any::<u8>()) {
        let mut p = pack();
        let i = pos % p.len();
        if p[i] != byte {
            p[i] = byte;
            let r = check(&p);
            // A flipped byte either breaks the pack or lands in ZIP metadata that carries no evidence.
            if r.ok {
                prop_assert_eq!(entries(&p), entries(&pack()));
            }
        }
    }
}
