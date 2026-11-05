#![allow(clippy::unwrap_used)]
//! One test per certificate signal (fires on a clear pattern) and its false-positive guards.

use std::str::FromStr;

use integrity_signals::cert::{Certificate, DateField, Limit, Limits, Reading, Signal, SignalKind, analyze};
use integrity_signals::{Settings, Strength};
use rust_decimal::Decimal;
use serde_json::json;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn reading(cert: &str, ch: &str, v: &str, unit: Option<&str>) -> Reading {
    Reading {
        characteristic: ch.into(),
        value: d(v),
        unit: unit.map(str::to_string),
        claim: format!("{cert}:{ch}"),
        location: json!({"page": 1}),
    }
}

fn date(v: &str) -> Option<DateField> {
    Some(DateField {
        date: v.into(),
        claim: format!("date:{v}"),
        location: json!({"page": 1}),
    })
}

fn cert(id: &str, lot: &str, issued: &str, values: &[(&str, &str, Option<&str>)]) -> Certificate {
    Certificate {
        id: id.into(),
        artifact: id.into(),
        document: format!("{id}.pdf"),
        supplier: "ACME".into(),
        part: "P-1".into(),
        lot: Some(lot.into()),
        heat: None,
        issued: date(issued),
        shipped: date(issued),
        inspected: date(issued),
        readings: values.iter().map(|(c, v, u)| reading(id, c, v, *u)).collect(),
    }
}

/// `n` honest certificates with distinct, well spread values (lot L100.., one per week).
fn history(n: usize) -> Vec<Certificate> {
    (0..n)
        .map(|i| {
            let f = i as f64;
            let v = |base: f64, step: f64| format!("{:.3}", base + ((f * step) % 0.18) - 0.09);
            cert(
                &format!("C{i:02}"),
                &format!("L{}", 100 + i),
                &iso(20_000 + 7 * i as i64),
                &[
                    ("qa.measured.length", &v(120.0, 0.037), Some("mm")),
                    ("qa.measured.width", &v(40.0, 0.053), Some("mm")),
                    ("qa.measured.height", &v(15.0, 0.071), Some("mm")),
                    ("qa.measured.thickness", &v(3.0, 0.029), Some("mm")),
                    ("qa.measured.diameter", &v(8.0, 0.061), Some("mm")),
                ],
            )
        })
        .collect()
}

fn iso(day: i64) -> String {
    // Small local inverse of day_number for test data (dates after 2024 only).
    let mut y = 1970;
    let mut left = day;
    loop {
        let len = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if left < len {
            break;
        }
        left -= len;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut m = 1;
    for len in [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31] {
        if left < len {
            break;
        }
        left -= len;
        m += 1;
    }
    format!("{y:04}-{m:02}-{:02}", left + 1)
}

fn limits() -> Limits {
    let mut l = Limits::new();
    for (ch, nom) in [
        ("qa.measured.length", "120"),
        ("qa.measured.width", "40"),
        ("qa.measured.height", "15"),
        ("qa.measured.thickness", "3"),
        ("qa.measured.diameter", "8"),
    ] {
        l.insert(
            ("P-1".into(), ch.into()),
            Limit {
                lower: Some(d(nom) - d("0.1")),
                upper: Some(d(nom) + d("0.1")),
                unit: Some("mm".into()),
                claims: vec![format!("spec:{ch}")],
            },
        );
    }
    l
}

fn of(sigs: &[Signal], k: SignalKind) -> Vec<&Signal> {
    sigs.iter().filter(|s| s.kind == k).collect()
}

#[test]
fn honest_history_produces_no_signals() {
    let sigs = analyze(&history(30), &limits(), &Settings::default());
    assert!(
        sigs.is_empty(),
        "{:#?}",
        sigs.iter().map(|s| (&s.kind, &s.why)).collect::<Vec<_>>()
    );
}

#[test]
fn whole_table_copy_between_lots() {
    let mut h = history(30);
    h[20].readings = h[5].readings.clone();
    let sigs = analyze(&h, &limits(), &Settings::default());
    let t = of(&sigs, SignalKind::TableCopy);
    assert_eq!(t.len(), 1, "{sigs:#?}");
    assert_eq!(t[0].certificates, vec!["C05".to_string(), "C20".to_string()]);
    assert!(t[0].strength >= Strength::Moderate);
    assert_eq!(t[0].evidence.len(), 10, "both certificates' five values are cited");
    assert!(t[0].why.contains("L105") && t[0].why.contains("L120"));
}

#[test]
fn identical_subset_of_values() {
    let mut h = history(30);
    for k in 0..4 {
        h[22].readings[k].value = h[3].readings[k].value;
    }
    let sigs = analyze(&h, &limits(), &Settings::default());
    let t = of(&sigs, SignalKind::IdenticalValues);
    assert_eq!(t.len(), 1, "{sigs:#?}");
    assert_eq!(t[0].statistic["identical"], 4);
    assert_eq!(t[0].statistic["shared"], 5);
}

#[test]
fn reissued_certificate_for_the_same_lot_is_not_a_copy() {
    let mut h = history(30);
    let mut re = h[5].clone();
    re.id = "C05-R".into();
    re.artifact = "C05-R".into();
    h.push(re);
    assert!(analyze(&h, &limits(), &Settings::default()).is_empty());
}

#[test]
fn copies_without_lot_or_heat_are_not_compared() {
    let mut h = history(30);
    h[20].readings = h[5].readings.clone();
    h[20].lot = None;
    assert!(of(&analyze(&h, &limits(), &Settings::default()), SignalKind::TableCopy).is_empty());
}

#[test]
fn coincidences_of_coarse_common_values_are_not_reported() {
    // Hardness is reported as one of three integers: identical hardness on many pairs is expected.
    let h: Vec<Certificate> = (0..40)
        .map(|i| {
            cert(
                &format!("C{i:02}"),
                &format!("L{}", 100 + i),
                &iso(20_000 + 7 * i),
                &[
                    ("qa.property.hardness", ["160", "161", "162"][(i % 3) as usize], None),
                    ("qa.property.hardness_core", ["150", "151"][(i % 2) as usize], None),
                    (
                        "qa.property.elongation",
                        ["22", "23"][((i / 2) % 2) as usize],
                        Some("%"),
                    ),
                    ("qa.property.reduction", ["60", "61"][((i / 3) % 2) as usize], Some("%")),
                ],
            )
        })
        .collect();
    assert!(analyze(&h, &Limits::new(), &Settings::default()).is_empty());
}

#[test]
fn narrow_band_needs_limits_samples_and_resolution() {
    let mut h = history(12);
    for (i, c) in h.iter_mut().enumerate() {
        c.readings[0].value = d("120.000") + Decimal::new(i as i64 % 3, 3);
    }
    let s = Settings::default();
    let sigs = analyze(&h, &limits(), &s);
    let n = of(&sigs, SignalKind::NarrowBand);
    assert_eq!(n.len(), 1, "{sigs:#?}");
    assert_eq!(n[0].characteristic.as_deref(), Some("qa.measured.length"));
    assert_eq!(n[0].evidence.len(), 12);
    // Guards: no limits; fewer samples than min_samples; tolerance too coarse for the resolution.
    assert!(of(&analyze(&h, &Limits::new(), &s), SignalKind::NarrowBand).is_empty());
    assert!(of(&analyze(&h[..9], &limits(), &s), SignalKind::NarrowBand).is_empty());
    let coarse = Settings {
        resolutions: vec![integrity_signals::settings::Resolution {
            prefix: "qa.measured.".into(),
            unit: Some("mm".into()),
            step: "0.05".into(),
        }],
        ..Settings::default()
    };
    assert!(of(&analyze(&h, &limits(), &coarse), SignalKind::NarrowBand).is_empty());
}

#[test]
fn values_piled_at_a_spec_limit() {
    let mut h = history(20);
    for c in h.iter_mut().step_by(2) {
        c.readings[1].value = d("40.1");
    }
    h[1].readings[1].value = d("40.099");
    let sigs = analyze(&h, &limits(), &Settings::default());
    let a = of(&sigs, SignalKind::AtLimit);
    assert_eq!(a.len(), 1, "{sigs:#?}");
    assert_eq!(a[0].statistic["near_limit"], 11);
    assert_eq!(a[0].statistic["exactly_at_limit"], 10);
    assert_eq!(a[0].evidence.len(), 11, "only the values at the limit are cited");
}

#[test]
fn out_of_spec_values_are_not_counted_as_at_limit() {
    let mut h = history(20);
    for c in h.iter_mut().step_by(2) {
        c.readings[1].value = d("40.2"); // out of tolerance: a conformance matter, not this signal
    }
    assert!(of(&analyze(&h, &limits(), &Settings::default()), SignalKind::AtLimit).is_empty());
}

#[test]
fn precision_finer_than_resolution() {
    let mut h = history(10);
    for (i, c) in h.iter_mut().enumerate() {
        c.readings[2].value += Decimal::new(3 + i as i64, 5);
    }
    let sigs = analyze(&h, &limits(), &Settings::default());
    let e = of(&sigs, SignalKind::ExcessPrecision);
    assert_eq!(e.len(), 1, "{sigs:#?}");
    assert_eq!(e[0].strength, Strength::Strong);
    assert_eq!(e[0].statistic["resolution"], "0.001");
    // Unknown resolution (unit not configured): never guessed.
    for c in &mut h {
        c.readings[2].unit = Some("in".into());
    }
    assert!(
        of(
            &analyze(&h, &limits(), &Settings::default()),
            SignalKind::ExcessPrecision
        )
        .is_empty()
    );
}

#[test]
fn trailing_zeros_are_not_extra_precision() {
    let mut h = history(10);
    for c in &mut h {
        c.readings[2].value = d("15.0200");
    }
    assert!(
        of(
            &analyze(&h, &limits(), &Settings::default()),
            SignalKind::ExcessPrecision
        )
        .is_empty()
    );
}

fn chem(id: &str, vals: &[(&str, &str)]) -> Certificate {
    let v: Vec<(String, &str)> = vals.iter().map(|(e, v)| (format!("qa.chem.{e}"), *v)).collect();
    let refs: Vec<(&str, &str, Option<&str>)> = v.iter().map(|(c, x)| (c.as_str(), *x, Some("%"))).collect();
    cert(id, "L1", "2026-01-10", &refs)
}

#[test]
fn chemistry_sum_above_100_percent() {
    let ok = chem("ok", &[("c", "0.15"), ("mn", "1.2"), ("fe", "98.6")]);
    let bad = chem("bad", &[("c", "0.15"), ("mn", "12"), ("fe", "98.6")]);
    let neg = chem("neg", &[("c", "-0.1"), ("mn", "1.2"), ("si", "0.3")]);
    let two = chem("two", &[("c", "60"), ("mn", "60")]);
    let sigs = analyze(&[ok, bad, neg, two], &Limits::new(), &Settings::default());
    let c = of(&sigs, SignalKind::ChemistrySum);
    let ids: Vec<&str> = c.iter().map(|s| s.certificates[0].as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.contains(&"bad") && ids.contains(&"neg"));
    let bad = c.iter().find(|s| s.certificates[0] == "bad").unwrap();
    assert_eq!(bad.statistic["sum_pct"], "110.75");
    // Within the tolerance (rounding of a full analysis): not reported.
    let edge = chem("edge", &[("c", "0.2"), ("mn", "1.5"), ("fe", "98.7")]);
    assert!(analyze(&[edge], &Limits::new(), &Settings::default()).is_empty());
}

#[test]
fn certificate_dated_after_shipment_and_before_inspection() {
    let mut c = cert("C1", "L1", "2026-03-20", &[]);
    c.shipped = date("2026-03-01");
    c.inspected = date("2026-03-25");
    let sigs = analyze(&[c], &Limits::new(), &Settings::default());
    let checks: Vec<&str> = sigs.iter().filter_map(|s| s.characteristic.as_deref()).collect();
    assert_eq!(checks.len(), 3, "{checks:?}");
    let after = sigs
        .iter()
        .find(|s| s.characteristic.as_deref() == Some("issued_after_shipment"))
        .unwrap();
    assert_eq!(
        (after.statistic["days"].as_i64(), after.strength),
        (Some(19), Strength::Moderate)
    );
    assert_eq!(after.evidence.len(), 2);
}

#[test]
fn dates_within_grace_or_unparseable_are_not_reported() {
    let mut c = cert("C1", "L1", "2026-03-02", &[]);
    c.shipped = date("2026-03-01");
    let s = Settings {
        date_grace_days: 1,
        ..Settings::default()
    };
    assert!(analyze(std::slice::from_ref(&c), &Limits::new(), &s).is_empty());
    c.shipped = date("2026-02-30");
    assert!(analyze(&[c], &Limits::new(), &Settings::default()).is_empty());
}

#[test]
fn lot_number_far_ahead_of_its_date() {
    let mut h = history(20);
    h[3].lot = Some("L999".into());
    let sigs = analyze(&h, &limits(), &Settings::default());
    let l = of(&sigs, SignalKind::LotSequence);
    assert_eq!(l.len(), 1, "{sigs:#?}");
    assert_eq!(l[0].certificates[0], "C03");
}

#[test]
fn old_stock_a_week_late_is_not_out_of_sequence() {
    let mut h = history(20);
    let (a, b) = (h[7].lot.clone(), h[8].lot.clone());
    h[7].lot = b;
    h[8].lot = a;
    assert!(of(&analyze(&h, &limits(), &Settings::default()), SignalKind::LotSequence).is_empty());
}

#[test]
fn wording_is_neutral() {
    let mut h = history(30);
    h[20].readings = h[5].readings.clone();
    h[3].lot = Some("L999".into());
    let mut c = chem("bad", &[("c", "0.15"), ("mn", "120"), ("fe", "98.6")]);
    c.shipped = date("2025-01-01");
    h.push(c);
    let sigs = analyze(&h, &limits(), &Settings::default());
    assert!(sigs.len() >= 4);
    for s in &sigs {
        let all = format!("{} {}", s.why, s.why_ja).to_lowercase();
        for w in [
            "fraud",
            "falsif",
            "fake",
            "forged",
            "cheat",
            "liar",
            "不正を",
            "改ざん",
            "偽造",
            "捏造",
        ] {
            assert!(!all.contains(w), "{w} in {}", s.why);
        }
        assert!(s.why.contains("not a finding of wrongdoing"));
        assert!(s.why_ja.contains("不正の判断ではありません"));
    }
}
