//! Synthetic certificate histories and drawing version histories with known ground truth, and the precision /
//! recall evaluation of both engines on them. Deterministic (seeded); no external data. Honest data includes
//! benign quirks that must not be reported (re-issued certificates for the same lot, very capable processes,
//! coarse integer hardness values, old stock shipped a week late, re-scans, single-field extraction gaps,
//! formatting differences such as "120.0" vs "120.00").

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Serialize;
use serde_json::json;

use crate::Settings;
use crate::cert::{self, Certificate, DateField, Limit, Limits, Reading, SignalKind};
use crate::drift::{self, DocVersion, DriftKind, FieldValue};

/// SplitMix64: small, seedable, good enough for test data.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
    pub fn normal(&mut self) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

fn dec(v: f64, dp: u32) -> Decimal {
    Decimal::from_f64(v).unwrap_or_default().round_dp(dp)
}

fn iso(day: i64) -> String {
    // Inverse of day_number (civil_from_days).
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// What was injected, for scoring.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Truth {
    Pair(SignalKind, String, String),
    Series(SignalKind, String, String),
    Cert(SignalKind, String),
}

impl Truth {
    fn kind(&self) -> SignalKind {
        match self {
            Truth::Pair(k, ..) | Truth::Series(k, ..) | Truth::Cert(k, ..) => *k,
        }
    }
}

pub struct CertCase {
    pub certs: Vec<Certificate>,
    pub limits: Limits,
    pub truth: Vec<Truth>,
    pub honest_certs: usize,
}

struct Dim {
    name: String,
    nominal: f64,
    tol: f64,
}

/// One supplier history per entry of `inject` (None = honest only), `n` certificates each.
pub fn certificates(seed: u64, inject: &[Option<SignalKind>], n: usize) -> CertCase {
    let mut rng = Rng::new(seed);
    let mut certs = Vec::new();
    let mut limits = Limits::new();
    let mut truth = Vec::new();
    let mut honest_certs = 0;
    for (si, inj) in inject.iter().enumerate() {
        let supplier = format!("SUPPLIER-{si:03}");
        let part = format!("PART-{si:03}");
        let capable = rng.unit() < 0.3;
        let dims: Vec<Dim> = ["length", "width", "height", "thickness", "diameter", "inner_diameter"]
            .iter()
            .map(|d| Dim {
                name: format!("qa.measured.{d}"),
                nominal: (10.0 + rng.unit() * 190.0).round(),
                tol: [0.05, 0.1, 0.2, 0.5][rng.below(4)],
            })
            .collect();
        for d in &dims {
            limits.insert(
                (part.clone(), d.name.clone()),
                Limit {
                    lower: Some(dec(d.nominal - d.tol, 3)),
                    upper: Some(dec(d.nominal + d.tol, 3)),
                    unit: Some("mm".into()),
                    claims: vec![format!("spec-{part}-{}", d.name)],
                },
            );
        }
        limits.insert(
            (part.clone(), "qa.property.tensile_strength".into()),
            Limit {
                lower: Some(Decimal::from(490)),
                upper: None,
                unit: Some("MPa".into()),
                claims: vec![],
            },
        );
        let start = 20_000 + rng.below(300) as i64;
        let mut lot_no = 1000 + rng.below(1000) as u64;
        let mut list: Vec<Certificate> = Vec::new();
        for i in 0..n {
            lot_no += 1 + rng.below(3) as u64;
            let issued = start + (i as i64) * 7 + rng.below(3) as i64;
            let id = format!("{supplier}-C{i:03}");
            let mut readings = Vec::new();
            let push = |ch: &str, v: Decimal, unit: Option<&str>, readings: &mut Vec<Reading>| {
                readings.push(Reading {
                    characteristic: ch.into(),
                    value: v,
                    unit: unit.map(str::to_string),
                    claim: format!("{id}-{ch}"),
                    location: json!({ "page": 1, "row": readings.len() + 1 }),
                });
            };
            for d in &dims {
                let sd = if capable { d.tol / 10.0 } else { d.tol / 4.0 };
                let v = loop {
                    let v = d.nominal + rng.normal() * sd;
                    if (v - d.nominal).abs() < d.tol * 0.98 {
                        break v;
                    }
                };
                push(&d.name, dec(v, 3), Some("mm"), &mut readings);
            }
            push(
                "qa.property.tensile_strength",
                dec(560.0 + rng.normal() * 15.0, 0),
                Some("MPa"),
                &mut readings,
            );
            push(
                "qa.property.hardness",
                dec(160.0 + rng.normal() * 2.0, 0),
                None,
                &mut readings,
            );
            for (el, mean, sd) in [
                ("c", 0.15, 0.02),
                ("si", 0.3, 0.05),
                ("mn", 1.2, 0.1),
                ("p", 0.02, 0.004),
                ("s", 0.01, 0.002),
            ] {
                push(
                    &format!("qa.chem.{el}"),
                    dec((mean + rng.normal() * sd).abs(), 3),
                    Some("%"),
                    &mut readings,
                );
            }
            let date = |d: i64, f: &str| DateField {
                date: iso(d),
                claim: format!("{id}-{f}"),
                location: json!({ "page": 1, "field": f }),
            };
            list.push(Certificate {
                id: id.clone(),
                artifact: id.clone(),
                document: format!("{id}.pdf"),
                supplier: supplier.clone(),
                part: part.clone(),
                lot: Some(format!("L{lot_no}")),
                heat: Some(format!("H{}", 10_000 + rng.below(89_999))),
                issued: Some(date(issued, "issued")),
                shipped: Some(date(issued + rng.below(4) as i64, "shipped")),
                inspected: Some(date(issued - rng.below(3) as i64, "inspected")),
                readings,
            });
        }
        // Benign: old stock a week late (lot numbers of neighbours swapped, 7 days apart).
        for i in 1..n {
            if rng.unit() < 0.05 {
                let l = list[i - 1].lot.clone();
                list[i - 1].lot = list[i].lot.clone();
                list[i].lot = l;
            }
        }
        // Benign: a re-issued certificate for the same lot (same values) a few days later.
        if n > 3 && rng.unit() < 0.5 {
            let k = rng.below(n - 1);
            let mut re = list[k].clone();
            re.id = format!("{}-R", re.id);
            re.artifact = re.id.clone();
            re.document = format!("{}.pdf", re.id);
            for r in &mut re.readings {
                r.claim = format!("{}-R", r.claim);
            }
            list.push(re);
        }
        let pick = |rng: &mut Rng| 2 + rng.below(n.saturating_sub(4).max(1));
        match inj {
            None => honest_certs += list.len(),
            Some(SignalKind::TableCopy) => {
                let (i, j) = (pick(&mut rng), pick(&mut rng));
                let j = if i == j { (j + 1) % n } else { j };
                let src = list[i].readings.clone();
                for (r, s) in list[j].readings.iter_mut().zip(src) {
                    r.value = s.value;
                }
                truth.push(pair(SignalKind::TableCopy, &list[i].id, &list[j].id));
            }
            Some(SignalKind::IdenticalValues) => {
                let (i, j) = (pick(&mut rng), pick(&mut rng));
                let j = if i == j { (j + 1) % n } else { j };
                for k in 0..dims.len() {
                    list[j].readings[k].value = list[i].readings[k].value;
                }
                truth.push(pair(SignalKind::IdenticalValues, &list[i].id, &list[j].id));
            }
            Some(SignalKind::NarrowBand) => {
                let k = rng.below(dims.len());
                for c in &mut list {
                    c.readings[k].value = dec(dims[k].nominal + rng.normal() * dims[k].tol / 150.0, 3);
                }
                truth.push(Truth::Series(
                    SignalKind::NarrowBand,
                    supplier.clone(),
                    dims[k].name.clone(),
                ));
            }
            Some(SignalKind::AtLimit) => {
                let k = rng.below(dims.len());
                for c in &mut list {
                    if rng.unit() < 0.5 {
                        let up = dims[k].nominal + dims[k].tol - if rng.unit() < 0.5 { 0.0 } else { 0.001 };
                        c.readings[k].value = dec(up, 3);
                    }
                }
                truth.push(Truth::Series(
                    SignalKind::AtLimit,
                    supplier.clone(),
                    dims[k].name.clone(),
                ));
            }
            Some(SignalKind::ExcessPrecision) => {
                let k = rng.below(dims.len());
                for c in &mut list {
                    c.readings[k].value += dec(0.0001 + rng.unit() * 0.0008, 5);
                }
                truth.push(Truth::Series(
                    SignalKind::ExcessPrecision,
                    supplier.clone(),
                    dims[k].name.clone(),
                ));
            }
            Some(SignalKind::ChemistrySum) => {
                let i = pick(&mut rng);
                if let Some(r) = list[i].readings.iter_mut().find(|r| r.characteristic == "qa.chem.mn") {
                    r.value *= Decimal::from(100); // decimal point lost: 1.2 → 120
                }
                truth.push(Truth::Cert(SignalKind::ChemistrySum, list[i].id.clone()));
            }
            Some(SignalKind::DateOrder) => {
                let i = pick(&mut rng);
                let c = &mut list[i];
                let ship = c
                    .shipped
                    .as_ref()
                    .and_then(|d| crate::day_number(&d.date))
                    .unwrap_or(start);
                if let Some(d) = c.issued.as_mut() {
                    d.date = iso(ship + 10 + rng.below(30) as i64);
                }
                truth.push(Truth::Cert(SignalKind::DateOrder, c.id.clone()));
            }
            Some(SignalKind::LotSequence) => {
                let i = 1 + rng.below((n / 4).max(1));
                list[i].lot = Some(format!("L{}", lot_no + 50));
                truth.push(Truth::Cert(SignalKind::LotSequence, list[i].id.clone()));
            }
        }
        certs.extend(list);
    }
    CertCase {
        certs,
        limits,
        truth,
        honest_certs,
    }
}

fn pair(k: SignalKind, a: &str, b: &str) -> Truth {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    Truth::Pair(k, a.to_string(), b.to_string())
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Score {
    pub injected: usize,
    pub detected: usize,
    pub signals: usize,
    pub true_positive_signals: usize,
    pub precision: f64,
    pub recall: f64,
}

impl Score {
    fn finish(&mut self) {
        self.precision = if self.signals == 0 {
            1.0
        } else {
            self.true_positive_signals as f64 / self.signals as f64
        };
        self.recall = if self.injected == 0 {
            1.0
        } else {
            self.detected as f64 / self.injected as f64
        };
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CertReport {
    pub certificates: usize,
    pub honest_certificates: usize,
    pub per_kind: BTreeMap<&'static str, Score>,
    pub overall: Score,
    /// Signals on suppliers with no injected pattern, per 1,000 honest certificates.
    pub false_signals_per_1000_honest: f64,
}

fn matches(s: &cert::Signal, t: &Truth) -> bool {
    if s.kind != t.kind() {
        return false;
    }
    match t {
        Truth::Pair(_, a, b) => s.certificates == [a.clone(), b.clone()],
        Truth::Series(_, sup, ch) => &s.supplier == sup && s.characteristic.as_ref() == Some(ch),
        Truth::Cert(_, c) => s.certificates.first() == Some(c),
    }
}

/// Scores the certificate signals on a generated case.
pub fn score_certificates(case: &CertCase, s: &Settings) -> CertReport {
    let signals = cert::analyze(&case.certs, &case.limits, s);
    let mut per: BTreeMap<&'static str, Score> =
        SignalKind::ALL.iter().map(|k| (k.as_str(), Score::default())).collect();
    let mut overall = Score::default();
    let honest: BTreeSet<&str> = case
        .certs
        .iter()
        .map(|c| c.supplier.as_str())
        .filter(|sup| !case.truth.iter().any(|t| truth_supplier(t, &case.certs) == Some(*sup)))
        .collect();
    let mut honest_signals = 0;
    for sig in &signals {
        let e = per.entry(sig.kind.as_str()).or_default();
        e.signals += 1;
        overall.signals += 1;
        if case.truth.iter().any(|t| matches(sig, t)) {
            e.true_positive_signals += 1;
            overall.true_positive_signals += 1;
        }
        if honest.contains(sig.supplier.as_str()) {
            honest_signals += 1;
        }
    }
    for t in &case.truth {
        let e = per.entry(t.kind().as_str()).or_default();
        e.injected += 1;
        overall.injected += 1;
        if signals.iter().any(|sig| matches(sig, t)) {
            e.detected += 1;
            overall.detected += 1;
        }
    }
    per.values_mut().for_each(Score::finish);
    overall.finish();
    CertReport {
        certificates: case.certs.len(),
        honest_certificates: case.honest_certs,
        per_kind: per,
        overall,
        false_signals_per_1000_honest: if case.honest_certs == 0 {
            0.0
        } else {
            honest_signals as f64 * 1000.0 / case.honest_certs as f64
        },
    }
}

fn truth_supplier<'c>(t: &Truth, certs: &'c [Certificate]) -> Option<&'c str> {
    let id = match t {
        Truth::Pair(_, a, _) | Truth::Cert(_, a) => a,
        Truth::Series(_, sup, _) => return certs.iter().find(|c| &c.supplier == sup).map(|c| c.supplier.as_str()),
    };
    certs.iter().find(|c| &c.id == id).map(|c| c.supplier.as_str())
}

// ---- Drawing versions -----------------------------------------------------------------------------------------

/// (predicate, value, numeric)
type Fld = (String, String, bool);

pub struct DriftCase {
    pub versions: Vec<DocVersion>,
    /// (kind, earlier id, later id)
    pub truth: Vec<(DriftKind, String, String)>,
}

fn next_rev(r: &str) -> String {
    let c = r.as_bytes()[0];
    ((c + 1) as char).to_string()
}

/// `drawings` drawings with `events` version events each; each event is honest or injected at random.
pub fn drawings(seed: u64, drawings: usize, events: usize) -> DriftCase {
    let mut rng = Rng::new(seed);
    let mut versions = Vec::new();
    let mut truth = Vec::new();
    for di in 0..drawings {
        let key = format!("DWG-{di:04}");
        let mut fields: Vec<(String, String, bool)> = Vec::new(); // (predicate, value, numeric)
        for d in ["length", "width", "height", "thickness", "diameter", "inner_diameter"] {
            fields.push((
                format!("mfg.dimension.{d}"),
                format!("{:.1}", 10.0 + rng.unit() * 190.0),
                true,
            ));
            fields.push((
                format!("mfg.tolerance.{d}.plus"),
                ["0.05", "0.1", "0.2"][rng.below(3)].into(),
                true,
            ));
        }
        fields.push((
            "mfg.material".into(),
            ["S355JR", "SS400", "SUS304"][rng.below(3)].into(),
            false,
        ));
        fields.push(("mfg.drawing_number".into(), key.clone(), false));
        fields.push(("mfg.approval.status".into(), "approved".into(), false));
        let mut rev = "A".to_string();
        let mut prev_rev: Option<(String, Vec<Fld>)> = None;
        let mut t = 0i64;
        let mut vi = 0usize;
        let mk = |fields: &[(String, String, bool)], rev: &str, t: i64, vi: usize, gap: Option<usize>| {
            let id = format!("{key}-V{vi:02}");
            DocVersion {
                id: id.clone(),
                document: format!("{id}.pdf"),
                key: key.clone(),
                part: format!("P-{di:04}"),
                revision: Some(rev.to_string()),
                sha256: format!("sha-{id}"),
                captured_at: t,
                fields: fields
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != gap)
                    .map(|(i, (p, v, num))| FieldValue {
                        predicate: p.clone(),
                        text: (!num).then(|| v.clone()),
                        num: if *num { v.parse().ok() } else { None },
                        unit: num.then(|| "mm".to_string()),
                        claim: format!("{id}-f{i}"),
                        location: json!({ "page": 1, "line": i + 1 }),
                    })
                    .collect(),
            }
        };
        versions.push(mk(&fields, &rev, t, vi, None));
        let mut last_id = format!("{key}-V{vi:02}");
        for _ in 0..events {
            t += 86_400 * (1 + rng.below(60) as i64);
            vi += 1;
            let id = format!("{key}-V{vi:02}");
            let roll = rng.unit();
            let mut gap = None;
            if roll < 0.25 {
                // Honest revision: bump and change one or two dimensions.
                prev_rev = Some((rev.clone(), fields.clone()));
                rev = next_rev(&rev);
                for _ in 0..1 + rng.below(2) {
                    let k = 2 * rng.below(6);
                    fields[k].1 = format!("{:.1}", 10.0 + rng.unit() * 190.0);
                }
            } else if roll < 0.40 {
                // Honest re-scan, sometimes with one field not extracted or formatted differently.
                if rng.unit() < 0.4 {
                    gap = Some(rng.below(fields.len()));
                } else if rng.unit() < 0.5 {
                    let k = 2 * rng.below(6);
                    fields[k].1 = format!("{}0", fields[k].1);
                }
            } else if roll < 0.55 {
                // Injected silent change: a dimension, tolerance or the material changes, revision stays.
                match rng.below(3) {
                    0 => {
                        let k = 2 * rng.below(6);
                        fields[k].1 = format!("{:.1}", fields[k].1.parse::<f64>().unwrap_or(10.0) + 0.5);
                    }
                    1 => {
                        let k = 2 * rng.below(6) + 1;
                        fields[k].1 = if fields[k].1 == "0.05" {
                            "0.1".into()
                        } else {
                            "0.05".into()
                        };
                    }
                    _ => {
                        fields[12].1 = if fields[12].1 == "SS400" {
                            "S355JR".into()
                        } else {
                            "SS400".into()
                        };
                    }
                }
                truth.push((DriftKind::SilentChange, last_id.clone(), id.clone()));
            } else if roll < 0.65 {
                // Injected revision bump with identical content.
                prev_rev = Some((rev.clone(), fields.clone()));
                rev = next_rev(&rev);
                truth.push((DriftKind::RevisionWithoutChange, last_id.clone(), id.clone()));
            } else if roll < 0.72 && prev_rev.is_some() {
                // Injected regression: an older revision (and its content) arrives again.
                if let Some((r, f)) = prev_rev.take() {
                    let back = mk(&f, &r, t, vi, None);
                    versions.push(back.clone());
                    truth.push((DriftKind::RevisionRegression, last_id.clone(), id.clone()));
                    // The current revision is re-sent afterwards (honest correction).
                    t += 86_400;
                    vi += 1;
                    let id2 = format!("{key}-V{vi:02}");
                    let resent = mk(&fields, &rev, t, vi, None);
                    if drift::diff(&back, &resent).is_empty() {
                        truth.push((DriftKind::RevisionWithoutChange, id.clone(), id2.clone()));
                    }
                    versions.push(resent);
                    last_id = id2;
                    continue;
                }
            } else {
                continue; // no new version
            }
            versions.push(mk(&fields, &rev, t, vi, gap));
            last_id = id;
        }
    }
    DriftCase { versions, truth }
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub versions: usize,
    pub per_kind: BTreeMap<&'static str, Score>,
    pub overall: Score,
}

pub fn score_drift(case: &DriftCase, s: &Settings) -> DriftReport {
    let found = drift::detect(&case.versions, s);
    let mut per: BTreeMap<&'static str, Score> = [
        DriftKind::SilentChange,
        DriftKind::RevisionWithoutChange,
        DriftKind::RevisionRegression,
    ]
    .iter()
    .map(|k| (k.as_str(), Score::default()))
    .collect();
    let mut overall = Score::default();
    let hit = |d: &drift::Drift, t: &(DriftKind, String, String)| d.kind == t.0 && d.earlier == t.1 && d.later == t.2;
    for d in &found {
        let e = per.entry(d.kind.as_str()).or_default();
        e.signals += 1;
        overall.signals += 1;
        if case.truth.iter().any(|t| hit(d, t)) {
            e.true_positive_signals += 1;
            overall.true_positive_signals += 1;
        }
    }
    for t in &case.truth {
        let e = per.entry(t.0.as_str()).or_default();
        e.injected += 1;
        overall.injected += 1;
        if found.iter().any(|d| hit(d, t)) {
            e.detected += 1;
            overall.detected += 1;
        }
    }
    per.values_mut().for_each(Score::finish);
    overall.finish();
    DriftReport {
        versions: case.versions.len(),
        per_kind: per,
        overall,
    }
}

/// The standard evaluation: 8 seeds × (24 honest suppliers + 3 of each injected kind) × 40 certificates, and
/// 8 seeds × 60 drawings × 8 events.
pub fn standard_inject() -> Vec<Option<SignalKind>> {
    let mut v: Vec<Option<SignalKind>> = vec![None; 24];
    for k in SignalKind::ALL {
        v.extend([Some(k), Some(k), Some(k)]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips_day_number() {
        for d in [-1000i64, 0, 20_762, 59_000] {
            assert_eq!(crate::day_number(&iso(d)), Some(d));
        }
    }

    #[test]
    fn generator_is_deterministic() {
        let a = certificates(7, &[None, Some(SignalKind::TableCopy)], 20);
        let b = certificates(7, &[None, Some(SignalKind::TableCopy)], 20);
        assert_eq!(a.certs, b.certs);
        assert_eq!(a.truth, b.truth);
    }
}
