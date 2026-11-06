#![allow(clippy::unwrap_used)]
//! Delivery receipts: final / pending verification, openings, and that every single-bit change is caught.

#[path = "common/receipt_fixture.rs"]
mod fixture;

use fixture::*;
use kizuna_verify_pack::VerifyOptions;
use kizuna_verify_pack::receipt::*;
use proptest::prelude::*;

fn check(b: &[u8], file: Option<&[u8]>) -> ReceiptReport {
    verify_receipt(b, file.map(FileDigest::of), &VerifyOptions::default())
}

fn codes(r: &ReceiptReport) -> Vec<&'static str> {
    r.report.failures().map(|c| c.code).collect()
}

#[test]
fn final_receipt_verifies_and_states_what_it_proves() {
    let c = chain();
    let b = bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]));
    let r = check(&b, Some(FILE));
    assert_eq!(r.status, "final", "{:?}", codes(&r));
    let s = r.summary.as_ref().unwrap();
    assert_eq!(s.checkpoint_seq, Some(N));
    assert_eq!(s.size, Some(FILE.len() as u64));
    assert_eq!(s.request_id, Some(REQUEST));
    let proven: String = r.report.proven.iter().map(|m| m.en.clone()).collect();
    assert!(proven.contains("receiving exactly the file with SHA-256"), "{proven}");
    let not: String = r.report.not_proven.iter().map(|m| m.en.clone()).collect();
    assert!(
        not.contains("not acceptance") && not.contains("server clock") && not.contains("--trusted-key"),
        "{not}"
    );
    let pinned = verify_receipt(
        &b,
        None,
        &VerifyOptions {
            trusted_key: Some(s.key_id.clone().unwrap()),
        },
    );
    assert_eq!(pinned.status, "final");
    let other = verify_receipt(
        &b,
        None,
        &VerifyOptions {
            trusted_key: Some("00".repeat(8)),
        },
    );
    assert!(other.report.failed("trusted_key_mismatch"));
}

#[test]
fn pending_receipt_is_consistent_but_proves_nothing() {
    let c = chain();
    let b = bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![]));
    let r = check(&b, Some(FILE));
    assert_eq!(r.status, "pending", "{:?}", codes(&r));
    assert!(r.report.proven.is_empty());
    assert!(r.report.not_proven[0].en.starts_with("PENDING"));
    // A proof without a checkpoint is refused.
    let mut p = receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![]);
    p.event.proof = entry(&c, RECEIPT_SEQ, true).proof;
    assert!(check(&bytes(&p), None).report.failed("pending_with_proof"));
}

#[test]
fn pending_becomes_final_with_the_same_event() {
    let c = chain();
    let p = check(&bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![])), None);
    let f = check(&bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![])), None);
    assert_eq!((p.status, f.status), ("pending", "final"));
    let (p, f) = (p.summary.unwrap(), f.summary.unwrap());
    assert_eq!(
        (p.sha256, p.received_at, p.event_seq),
        (f.sha256, f.received_at, f.event_seq)
    );
}

#[test]
fn wrong_file_and_wrong_checkpoint_fail() {
    let c = chain();
    let b = bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]));
    assert!(check(&b, Some(b"other bytes")).report.failed("file_mismatch"));
    let mut early = receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]);
    early.checkpoint.as_mut().unwrap().seq = RECEIPT_SEQ - 1;
    let r = check(&bytes(&early), None);
    assert!(r.report.failed("checkpoint_signature_invalid") && r.report.failed("event_proof_invalid"));
    // A receipt event moved to another receipt id is refused.
    let mut moved = receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]);
    moved.receipt_id = CLAIMS;
    assert!(check(&bytes(&moved), None).report.failed("receipt_event_invalid"));
    // A non-receipt event (the capture event) is not a receipt.
    let mut cap = receipt(&c, RECEIPT, 2, true, vec![]);
    cap.event = entry(&c, 2, true);
    assert!(check(&bytes(&cap), None).report.failed("receipt_event_invalid"));
    // A portal delivery must name the request it answers.
    let mut no_req = receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]);
    no_req.event.details.as_object_mut().unwrap().remove("request_id");
    assert!(check(&bytes(&no_req), None).report.failed("receipt_event_invalid"));
}

#[test]
fn openings_match_commitments_and_may_be_partial() {
    let c = chain();
    let all = check(&bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings())), None);
    assert_eq!(all.status, "final", "{:?}", codes(&all));
    assert_eq!(all.summary.unwrap().opened.len(), 2);
    let one = check(
        &bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings()[1..].to_vec())),
        None,
    );
    assert_eq!(one.status, "final");
    let none = check(&bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, vec![])), None);
    assert_eq!(none.status, "final");
    for f in [
        |o: &mut Opening| o.value = "521 MPa".into(),
        |o: &mut Opening| o.salt.0[31] ^= 1,
        |o: &mut Opening| o.label = "lot/L-77/elongation".into(),
    ] {
        let mut os = openings();
        f(&mut os[0]);
        assert!(
            check(&bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, os)), None)
                .report
                .failed("opening_mismatch")
        );
    }
    let mut dup = openings();
    dup[1] = dup[0].clone();
    assert!(
        check(&bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, dup)), None)
            .report
            .failed("opening_mismatch")
    );
}

#[test]
fn non_canonical_or_unknown_fields_are_refused() {
    let c = chain();
    let r = receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![]);
    let pretty = serde_json::to_vec_pretty(&r).unwrap();
    assert!(check(&pretty, None).report.failed("receipt_format"));
    let mut v = serde_json::to_value(&r).unwrap();
    v["status"] = "final".into();
    let extra = kizuna_verify_pack::to_canonical(&v).unwrap();
    assert!(check(&extra, None).report.failed("receipt_format"));
}

/// Every single-bit change anywhere in a final receipt (with openings) and a pending receipt fails. Exhaustive.
#[test]
fn every_single_bit_flip_fails_exhaustive() {
    let c = chain();
    for b in [
        bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings())),
        bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![])),
        bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![])),
    ] {
        let base = check(&b, None).status;
        assert_ne!(base, "failed");
        for bit in 0..b.len() * 8 {
            let mut t = b.clone();
            t[bit / 8] ^= 1 << (bit % 8);
            let r = check(&t, None);
            assert_eq!(
                r.status,
                "failed",
                "bit {bit} (byte {:?}) not caught",
                b[bit / 8] as char
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    /// Property form: any bit of any of the three receipts, with or without the file check.
    #[test]
    fn single_bit_tamper_fails(which in 0usize..3, pos in any::<prop::sample::Index>(), bit in 0u8..8, with_file in any::<bool>()) {
        let c = chain();
        let b = match which {
            0 => bytes(&receipt(&c, CLAIMS, CLAIMS_SEQ, true, openings())),
            1 => bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, true, vec![])),
            _ => bytes(&receipt(&c, RECEIPT, RECEIPT_SEQ, false, vec![])),
        };
        let mut t = b.clone();
        let i = pos.index(t.len());
        t[i] ^= 1 << bit;
        let f = with_file.then_some(FILE);
        prop_assert_eq!(check(&t, f).status, "failed");
        prop_assert_ne!(check(&b, f).status, "failed");
    }

    /// Random bytes never panic and never verify.
    #[test]
    fn random_input_never_verifies(b in prop::collection::vec(any::<u8>(), 0..512)) {
        prop_assert_eq!(check(&b, None).status, "failed");
    }
}

#[test]
fn vectors_have_expected_status() {
    let want = [
        ("delivery_final", "final"),
        ("delivery_final_with_file", "final"),
        ("delivery_final_wrong_file", "failed"),
        ("delivery_pending", "pending"),
        ("claims_final_all_opened", "final"),
        ("claims_final_one_opened", "final"),
        ("claims_wrong_opening", "failed"),
        ("bit_flipped", "failed"),
        ("checkpoint_before_event", "failed"),
        ("garbage", "failed"),
    ];
    let v = vectors();
    assert_eq!(v.len(), want.len());
    for ((name, b, f), (n2, st)) in v.iter().zip(want) {
        assert_eq!(*name, n2);
        assert_eq!(check(b, f.as_deref()).status, st, "{name}");
    }
}
