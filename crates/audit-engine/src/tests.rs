use super::*;
use proptest::prelude::*;
use serde_json::json;

fn ev(tenant: Uuid, seq: i64, action: &str, details: Value) -> Event {
    Event {
        tenant_id: tenant,
        seq,
        occurred_at_us: 1_700_000_000_000_000 + seq,
        actor_user_id: Some(Uuid::from_u128(7)),
        actor_kind: "user".into(),
        action: action.into(),
        target_type: "user".into(),
        target_id: Some(format!("t{seq}")),
        details,
    }
}

fn build(tenant: Uuid, n: i64) -> Vec<StoredEvent> {
    let mut prev = GENESIS;
    (1..=n)
        .map(|s| {
            let e = ev(tenant, s, "auth.login", json!({"n": s, "ip": "127.0.0.1"}));
            let h = chain_hash(&e, &prev).unwrap();
            let se = StoredEvent {
                event: e,
                prev_hash: prev.to_vec(),
                hash: h.to_vec(),
            };
            prev = h;
            se
        })
        .collect()
}

#[test]
fn canonical_json_sorts_and_rejects_floats() {
    assert_eq!(
        canonical_json(&json!({"b": 1, "a": [true, null, "x"]})).unwrap(),
        r#"{"a":[true,null,"x"],"b":1}"#
    );
    assert_eq!(canonical_json(&json!({"x": 1.5})), Err(AuditError::FloatNotAllowed));
    assert_eq!(canonical_json(&json!("a\u{0}b")), Err(AuditError::NulNotAllowed));
}

#[test]
fn intact_chain_verifies() {
    let t = Uuid::from_u128(1);
    let r = verify_chain(t, &build(t, 50));
    assert!(r.is_intact(), "{r:?}");
    assert_eq!(r.events_checked, 50);
}

#[test]
fn detects_modification_deletion_reorder_and_relink() {
    let t = Uuid::from_u128(1);
    let chain = build(t, 10);

    let mut c = chain.clone();
    c[4].event.details = json!({"n": 999});
    assert_eq!(verify_chain(t, &c).first_break, Some(Break::HashMismatch { seq: 5 }));

    let mut c = chain.clone();
    c.remove(3);
    assert_eq!(
        verify_chain(t, &c).first_break,
        Some(Break::SequenceGap { expected: 4, found: 5 })
    );

    let mut c = chain.clone();
    c.swap(2, 3);
    assert!(!verify_chain(t, &c).is_intact());

    // Attacker rewrites one event and recomputes its own hash: next link breaks.
    let mut c = chain.clone();
    c[4].event.action = "auth.logout".into();
    c[4].hash = chain_hash(&c[4].event, &c[4].prev_hash).unwrap().to_vec();
    assert_eq!(verify_chain(t, &c).first_break, Some(Break::BrokenLink { seq: 6 }));

    assert_eq!(
        verify_chain(Uuid::from_u128(2), &chain).first_break,
        Some(Break::WrongTenant { seq: 1 })
    );
}

#[test]
fn encoding_is_injective_across_field_boundaries() {
    let t = Uuid::from_u128(1);
    let mut a = ev(t, 1, "ab", json!({}));
    a.target_type = "c".into();
    let mut b = ev(t, 1, "a", json!({}));
    b.target_type = "bc".into();
    assert_ne!(encode(&a).unwrap(), encode(&b).unwrap());
    let mut c = a.clone();
    c.target_id = None;
    let mut d = a.clone();
    d.target_id = Some(String::new());
    assert_ne!(encode(&c).unwrap(), encode(&d).unwrap());
}

proptest! {
    #[test]
    fn any_field_mutation_is_detected(idx in 0usize..8, field in 0u8..6, salt in "[a-z]{1,8}") {
        let t = Uuid::from_u128(9);
        let mut c = build(t, 8);
        let e = &mut c[idx].event;
        match field {
            0 => e.action.push_str(&salt),
            1 => e.target_type.push_str(&salt),
            2 => e.target_id = Some(salt.clone()),
            3 => e.occurred_at_us += 1,
            4 => e.actor_user_id = None,
            _ => e.details = json!({"tampered": salt}),
        }
        prop_assert!(!verify_chain(t, &c).is_intact());
    }

    #[test]
    fn canonical_json_is_key_order_independent(keys in proptest::collection::btree_set("[a-z]{1,6}", 1..8)) {
        let ks: Vec<String> = keys.into_iter().collect();
        let mut fwd = serde_json::Map::new();
        for (i, k) in ks.iter().enumerate() { fwd.insert(k.clone(), json!(i)); }
        let mut rev = serde_json::Map::new();
        for (i, k) in ks.iter().enumerate().rev() { rev.insert(k.clone(), json!(i)); }
        prop_assert_eq!(canonical_json(&Value::Object(fwd)).unwrap(), canonical_json(&Value::Object(rev)).unwrap());
    }
}
