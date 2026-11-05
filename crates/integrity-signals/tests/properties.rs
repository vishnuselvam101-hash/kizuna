#![allow(clippy::unwrap_used)]
//! Property tests: determinism, input-order independence, bounds, and "no signal without a pattern".

use integrity_signals::Settings;
use integrity_signals::cert::{SignalKind, analyze};
use integrity_signals::drift::detect;
use integrity_signals::synth::{certificates, drawings};
use proptest::prelude::*;

fn kinds() -> impl Strategy<Value = Vec<Option<SignalKind>>> {
    prop::collection::vec(prop::option::of(prop::sample::select(SignalKind::ALL.to_vec())), 1..5)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn certificate_signals_are_deterministic_and_order_independent(seed in any::<u64>(), inject in kinds(), rot in 0usize..50) {
        let case = certificates(seed, &inject, 16);
        let s = Settings::default();
        let a = analyze(&case.certs, &case.limits, &s);
        let mut shuffled = case.certs.clone();
        let len = shuffled.len();
        shuffled.rotate_left(rot % len);
        shuffled.reverse();
        let b = analyze(&shuffled, &case.limits, &s);
        prop_assert_eq!(&a, &b);
        for sig in &a {
            prop_assert!(!sig.evidence.is_empty());
            prop_assert!(sig.evidence.len() <= integrity_signals::cert::MAX_EVIDENCE);
            prop_assert!(!sig.certificates.is_empty());
            prop_assert_eq!(sig.fingerprint.len(), 64);
            prop_assert!(sig.why.contains("not a finding of wrongdoing"));
        }
        let mut fps: Vec<&String> = a.iter().map(|s| &s.fingerprint).collect();
        fps.dedup();
        prop_assert_eq!(fps.len(), a.len(), "fingerprints are unique");
    }

    #[test]
    fn a_single_certificate_never_yields_cross_certificate_signals(seed in any::<u64>()) {
        let case = certificates(seed, &[None], 1);
        for sig in analyze(&case.certs, &case.limits, &Settings::default()) {
            prop_assert!(matches!(sig.kind, SignalKind::ChemistrySum | SignalKind::DateOrder | SignalKind::ExcessPrecision));
        }
    }

    #[test]
    fn stricter_thresholds_never_remove_signals(seed in any::<u64>(), inject in kinds()) {
        // Monotonicity of the threshold knobs that only filter (identical/table counts, min samples, percentages).
        let case = certificates(seed, &inject, 20);
        let base = Settings::default();
        let looser = Settings { min_identical_values: 3, min_samples: 8, at_limit_pct: 20, excess_pct: 30, min_excess_values: 2, ..base.clone() };
        let strict = analyze(&case.certs, &case.limits, &base);
        let loose = analyze(&case.certs, &case.limits, &looser);
        for s in &strict {
            if s.kind == SignalKind::TableCopy || s.kind == SignalKind::IdenticalValues {
                continue; // table copy vs identical-values classification may shift between the two kinds
            }
            prop_assert!(loose.iter().any(|l| l.fingerprint == s.fingerprint), "{:?} lost", s.kind);
        }
    }

    #[test]
    fn drift_is_deterministic_and_order_independent(seed in any::<u64>(), rot in 0usize..100) {
        let case = drawings(seed, 6, 6);
        let s = Settings::default();
        let a = detect(&case.versions, &s);
        let mut v = case.versions.clone();
        let len = v.len();
        v.rotate_left(rot % len);
        prop_assert_eq!(&a, &detect(&v, &s));
        for d in &a {
            prop_assert!(d.earlier != d.later);
        }
    }

    #[test]
    fn identical_copies_never_drift(seed in any::<u64>()) {
        // The same content captured again (new file) under the same revision is never a finding by default.
        let case = drawings(seed, 3, 0);
        let mut v = case.versions.clone();
        for x in &case.versions {
            let mut y = x.clone();
            y.id = format!("{}-copy", x.id);
            y.sha256 = format!("{}-copy", x.sha256);
            y.captured_at += 10;
            v.push(y);
        }
        prop_assert!(detect(&v, &Settings::default()).is_empty());
    }

    #[test]
    fn settings_validation_matches_bounds(v in 0u32..400) {
        let s = Settings { min_samples: v, ..Settings::default() };
        let [lo, hi] = Settings::bounds()["min_samples"];
        prop_assert_eq!(s.validate().is_ok(), (lo..=hi).contains(&v));
    }
}
