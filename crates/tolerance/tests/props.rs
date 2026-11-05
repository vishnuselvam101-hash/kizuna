//! Property tests: exact unit round trips, WC ≥ MRSS/RSS, ISO 14253-1 monotonicity, parser never panics.

use proptest::prelude::*;
use rust_decimal::Decimal;
use tolerance::*;

fn dec(m: i64, scale: u32) -> Decimal {
    Decimal::new(m, scale)
}

fn unit() -> impl Strategy<Value = Unit> {
    proptest::sample::select(Unit::ALL.to_vec())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn unit_round_trip_is_exact(m in -1_000_000_000_000_000i64..1_000_000_000_000_000, s in 0u32..=12, u in unit()) {
        let v = dec(m, s);
        let q = Quantity::new(v, u).expect("in-range values convert exactly");
        prop_assert_eq!(q.value_in(u), Ok(v));
        // Into the canonical unit and back through it is exact as well.
        let c = Quantity::new(q.canonical(), Unit::canonical(u.dimension())).expect("canonical");
        prop_assert_eq!(c.value_in(u), Ok(v));
    }

    #[test]
    fn worst_case_bounds_statistical_widths(
        links in proptest::collection::vec((1i64..1_000_000, 0i64..5_000, any::<bool>(), 0i64..3_000), 1..20),
    ) {
        let chain: Vec<Link> = links.iter().enumerate().map(|(i, &(n, up, plus, lo))| Link {
            name: format!("L{i}"),
            direction: if plus { Direction::Plus } else { Direction::Minus },
            spec: Spec::asymmetric(dec(n, 3), dec(up, 4), dec(-lo, 4), Unit::Mm).expect("spec"),
            process: None,
        }).collect();
        let gap = Spec::symmetric(Decimal::ZERO, Decimal::ONE, Unit::Mm).expect("gap");
        let r = analyze(&chain, &gap, &StackOptions::default()).expect("report");
        let wc = (r.worst_case.1 - r.worst_case.0).to_string().parse::<f64>().expect("f64") / 2.0;
        let rss = (r.rss.1 - r.rss.0) / 2.0;
        let mrss = (r.mrss.1 - r.mrss.0) / 2.0;
        prop_assert!(wc + 1e-9 >= rss, "WC {wc} < RSS {rss}");
        prop_assert!(wc + 1e-9 >= mrss && mrss + 1e-9 >= rss);
        prop_assert!((0.0..=1.0).contains(&r.predicted_yield));
        prop_assert!(r.worst_case.0 <= r.nominal && r.nominal <= r.worst_case.1);
    }

    #[test]
    fn smaller_uncertainty_never_weakens_a_decision(
        y in -2_000i64..2_000, u1 in 0i64..500, u2 in 0i64..500, t in 1i64..1_000,
    ) {
        let spec = Spec::symmetric(Decimal::ZERO, dec(t, 3), Unit::Mm).expect("spec");
        let (small, big) = (u1.min(u2), u1.max(u2));
        let at = |u: i64| judge(&spec, Quantity::new(dec(y, 3), Unit::Mm).expect("q"), Quantity::new(dec(u, 3), Unit::Mm).expect("u")).expect("v").outcome;
        let (vs, vb) = (at(small), at(big));
        if vb != Outcome::Indeterminate { prop_assert_eq!(vs, vb); }
        // Pass implies the value is inside the spec; Fail implies outside.
        let inside = y.abs() <= t;
        if vs == Outcome::Pass { prop_assert!(inside); }
        if vs == Outcome::Fail { prop_assert!(!inside); }
    }

    #[test]
    fn describe_then_parse_gives_the_same_limits(n in 1i64..10_000_000, up in 0i64..100_000, lo in 0i64..100_000) {
        let s = Spec::asymmetric(dec(n, 3), dec(up, 4), dec(-lo, 4), Unit::Mm).expect("spec");
        let back = parse_spec(&s.describe()).expect("own rendering parses");
        prop_assert_eq!((back.lower, back.upper), (s.lower, s.upper));
    }

    #[test]
    fn parser_never_panics_on_arbitrary_text(text in "\\PC{0,40}") {
        let _ = parse(&text);
    }

    #[test]
    fn parser_never_panics_on_mutated_notations(
        seed in 0usize..12,
        ops in proptest::collection::vec((0usize..64, 0usize..4, 0usize..24), 0..6),
    ) {
        const CORPUS: [&str; 12] = ["φ10 +0.1/−0", "10±0.05", "10 h7", "M8×1.25 6g", "１０±０．０５", "9.9〜10.1mm",
            "400MPa以上", "10以上10.2以下", "≥ 400 N/mm²", "0.5 ± 0.002 in", "25 JS7", "10 mm +50/-0 um"];
        const ALPHABET: [char; 24] = ['0', '9', '.', '+', '-', '/', '±', '~', ' ', 'h', 'H', 'M', 'x', '×', 'm', 'u',
            '以', '上', '下', '未', '満', '\u{3000}', '\u{FF10}', '\u{2212}'];
        let mut chars: Vec<char> = CORPUS[seed].chars().collect();
        for (pos, op, c) in ops {
            let p = if chars.is_empty() { 0 } else { pos % (chars.len() + 1) };
            match op {
                0 => chars.insert(p, ALPHABET[c]),
                1 if p < chars.len() => { chars.remove(p); }
                2 if p < chars.len() => chars[p] = ALPHABET[c],
                _ => { let dup = chars.clone(); chars.extend(dup); }
            }
            if chars.len() > 400 { chars.truncate(400); }
        }
        let text: String = chars.into_iter().collect();
        if let Ok(Parsed::Spec(s)) = parse(&text)
            && let (Some(l), Some(h)) = (s.lower, s.upper) { prop_assert!(l.value <= h.value, "{text}"); }
    }
}
