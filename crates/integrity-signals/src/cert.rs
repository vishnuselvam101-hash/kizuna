//! Certificate integrity signals over a supplier's certificate / inspection-report history.
//!
//! Signals: identical values across different lots/heats, whole-table copies, values in a suspiciously narrow
//! band, values piled up at a spec limit, precision finer than the gauge resolution, impossible chemistry sums,
//! dates in an impossible order, lot numbers out of date order. Each one states its statistic, the threshold it
//! was compared with and the data used, and names benign explanations. None of them is an accusation.

use std::collections::{BTreeMap, BTreeSet};

use kizuna_messages::{Lang, Msg};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{EvidenceRef, Settings, Strength, day_number, fingerprint};

/// One numeric value on a certificate (`qa.measured.*`, `qa.property.*`, `qa.chem.*`).
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub characteristic: String,
    pub value: Decimal,
    pub unit: Option<String>,
    pub claim: String,
    pub location: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateField {
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    pub claim: String,
    pub location: Value,
}

/// One certificate / inspection report (one artifact) of a supplier for a part.
#[derive(Debug, Clone, PartialEq)]
pub struct Certificate {
    /// Unique per certificate (one artifact can hold several, e.g. one row per lot in a CSV).
    pub id: String,
    /// Artifact (document) id the certificate came from.
    pub artifact: String,
    pub document: String,
    pub supplier: String,
    pub part: String,
    pub lot: Option<String>,
    pub heat: Option<String>,
    pub issued: Option<DateField>,
    pub shipped: Option<DateField>,
    pub inspected: Option<DateField>,
    pub readings: Vec<Reading>,
}

/// Spec limits of one characteristic of a part, from drawings/specs (nominal ± tolerance).
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub lower: Option<Decimal>,
    pub upper: Option<Decimal>,
    pub unit: Option<String>,
    pub claims: Vec<String>,
}

/// (part, characteristic) → limit.
pub type Limits = BTreeMap<(String, String), Limit>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    IdenticalValues,
    TableCopy,
    NarrowBand,
    AtLimit,
    ExcessPrecision,
    ChemistrySum,
    DateOrder,
    LotSequence,
    DigitPreference,
}

impl SignalKind {
    pub const ALL: [SignalKind; 9] = [
        SignalKind::IdenticalValues,
        SignalKind::TableCopy,
        SignalKind::NarrowBand,
        SignalKind::AtLimit,
        SignalKind::ExcessPrecision,
        SignalKind::ChemistrySum,
        SignalKind::DateOrder,
        SignalKind::LotSequence,
        SignalKind::DigitPreference,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::IdenticalValues => "identical_values",
            SignalKind::TableCopy => "table_copy",
            SignalKind::NarrowBand => "narrow_band",
            SignalKind::AtLimit => "at_limit",
            SignalKind::ExcessPrecision => "excess_precision",
            SignalKind::ChemistrySum => "chemistry_sum",
            SignalKind::DateOrder => "date_order",
            SignalKind::LotSequence => "lot_sequence",
            SignalKind::DigitPreference => "digit_preference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub supplier: String,
    pub part: Option<String>,
    pub characteristic: Option<String>,
    pub strength: Strength,
    pub why: String,
    pub why_ja: String,
    /// The explanation as a message code (`why`/`why_ja` are its EN/JA renderings).
    pub msg: Msg,
    /// The statistic, the threshold it was compared with, and sample sizes.
    pub statistic: Value,
    pub evidence: Vec<EvidenceRef>,
    /// Artifact ids of the certificates involved.
    pub certificates: Vec<String>,
    pub fingerprint: String,
}

/// Cap on evidence references per signal (the statistic keeps the full counts).
pub const MAX_EVIDENCE: usize = 60;
/// Pairwise comparison is limited to this many certificates per supplier and part (the most recent ones).
pub const MAX_PAIR_GROUP: usize = 2000;

fn norm_id(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase())
        .filter(|v| !v.is_empty())
}

/// Two certificates are about different material only when a lot or a heat number is known on both and differs,
/// and they do not share a known lot (a re-issued certificate for the same lot legitimately repeats values).
fn different_material(a: &Certificate, b: &Certificate) -> bool {
    let (la, lb, ha, hb) = (norm_id(&a.lot), norm_id(&b.lot), norm_id(&a.heat), norm_id(&b.heat));
    if la.is_some() && la == lb {
        return false;
    }
    if ha.is_some() && ha == hb {
        return false;
    }
    matches!((&la, &lb), (Some(x), Some(y)) if x != y) || matches!((&ha, &hb), (Some(x), Some(y)) if x != y)
}

fn label(c: &Certificate) -> String {
    norm_id(&c.lot)
        .or_else(|| norm_id(&c.heat))
        .unwrap_or_else(|| c.document.clone())
}

fn reading_ref(c: &Certificate, r: &Reading) -> EvidenceRef {
    EvidenceRef {
        artifact: c.artifact.clone(),
        document: c.document.clone(),
        claim: Some(r.claim.clone()),
        field: Some(r.characteristic.clone()),
        value: Some(match &r.unit {
            Some(u) => format!("{} {u}", r.value.normalize()),
            None => r.value.normalize().to_string(),
        }),
        location: r.location.clone(),
    }
}

fn date_ref(c: &Certificate, field: &str, d: &DateField) -> EvidenceRef {
    EvidenceRef {
        artifact: c.artifact.clone(),
        document: c.document.clone(),
        claim: Some(d.claim.clone()),
        field: Some(field.into()),
        value: Some(d.date.clone()),
        location: d.location.clone(),
    }
}

type CharKey = (String, Option<String>);

/// Per characteristic (and unit): the sorted values on one certificate.
fn table(c: &Certificate) -> BTreeMap<CharKey, Vec<Decimal>> {
    let mut t: BTreeMap<CharKey, Vec<Decimal>> = BTreeMap::new();
    for r in &c.readings {
        t.entry((r.characteristic.clone(), r.unit.clone()))
            .or_default()
            .push(r.value.normalize());
    }
    for v in t.values_mut() {
        v.sort();
    }
    t
}

/// Runs every certificate signal. Output is sorted by fingerprint (deterministic).
pub fn analyze(certs: &[Certificate], limits: &Limits, s: &Settings) -> Vec<Signal> {
    // Canonical order first, so evidence lists (and their caps) never depend on the input order.
    let mut sorted: Vec<&Certificate> = certs.iter().collect();
    sorted
        .sort_by(|a, b| (&a.supplier, &a.part, sort_date(a), &a.id).cmp(&(&b.supplier, &b.part, sort_date(b), &b.id)));
    let certs: Vec<Certificate> = sorted.into_iter().cloned().collect();
    let certs = certs.as_slice();
    let mut groups: BTreeMap<(String, String), Vec<&Certificate>> = BTreeMap::new();
    for c in certs {
        groups.entry((c.supplier.clone(), c.part.clone())).or_default().push(c);
    }
    let mut out = Vec::new();
    for ((supplier, part), mut group) in groups {
        group.sort_by(|a, b| sort_date(a).cmp(&sort_date(b)).then_with(|| a.id.cmp(&b.id)));
        copies(&supplier, &part, &group, s, &mut out);
        distribution(&supplier, &part, &group, limits, s, &mut out);
        lot_sequence(&supplier, &part, &group, s, &mut out);
    }
    precision(certs, s, &mut out);
    digits(certs, s, &mut out);
    for c in certs {
        chemistry(c, s, &mut out);
        dates(c, s, &mut out);
    }
    for g in &mut out {
        g.why = g.msg.en();
        g.why_ja = g.msg.render(Lang::Ja);
    }
    out.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    out
}

fn sort_date(c: &Certificate) -> Option<i64> {
    c.issued
        .as_ref()
        .or(c.shipped.as_ref())
        .and_then(|d| day_number(&d.date))
}

/// Identical values / whole-table copies between certificates of different lots or heats.
fn copies(supplier: &str, part: &str, group: &[&Certificate], s: &Settings, out: &mut Vec<Signal>) {
    let start = group.len().saturating_sub(MAX_PAIR_GROUP);
    let group = &group[start..];
    let tables: Vec<BTreeMap<CharKey, Vec<Decimal>>> = group.iter().map(|c| table(c)).collect();
    // How often each value vector occurs per characteristic, for the coincidence estimate.
    let mut freq: BTreeMap<&CharKey, (usize, BTreeMap<&Vec<Decimal>, usize>)> = BTreeMap::new();
    for t in &tables {
        for (k, v) in t {
            let e = freq.entry(k).or_default();
            e.0 += 1;
            *e.1.entry(v).or_default() += 1;
        }
    }
    let pairs = (group.len() * group.len().saturating_sub(1) / 2).max(1) as f64;
    for i in 0..group.len() {
        for j in i + 1..group.len() {
            if !different_material(group[i], group[j]) {
                continue;
            }
            let (ta, tb) = (&tables[i], &tables[j]);
            let shared: Vec<&CharKey> = ta.keys().filter(|k| tb.contains_key(*k)).collect();
            let same: Vec<&CharKey> = shared.iter().copied().filter(|k| ta[*k] == tb[*k]).collect();
            if same.len() < s.min_identical_values as usize {
                continue;
            }
            // Chance that all these coincide, from the frequency of each value among the *other* certificates of
            // this supplier and part (add-one smoothing: a short history gives a high chance, i.e. no signal).
            let mut log_p = 0.0f64;
            for k in &same {
                let (n, by) = &freq[*k];
                let m = by.get(&ta[*k]).copied().unwrap_or(0).saturating_sub(2);
                log_p += ((m as f64 + 1.0) / ((n - 2) as f64 + 2.0)).ln();
            }
            // Bonferroni: many pairs are compared, so the per-pair chance is multiplied by the number of pairs.
            let chance = (log_p.exp() * pairs).min(1.0);
            let max = f64::from(s.max_coincidence_ppm) / 1e6;
            if chance > max {
                continue;
            }
            let table_copy = same.len() == shared.len() && same.len() >= s.min_table_values as usize;
            let kind = if table_copy {
                SignalKind::TableCopy
            } else {
                SignalKind::IdenticalValues
            };
            let strength = if chance <= max / 1000.0 {
                Strength::Strong
            } else if chance <= max / 10.0 || table_copy {
                Strength::Moderate
            } else {
                Strength::Weak
            };
            let (a, b) = (group[i], group[j]);
            let (la, lb) = (label(a), label(b));
            let chance_txt = format!("{chance:.1e}");
            let (ns, nsh, hist) = (same.len(), shared.len(), group.len());
            let msg = Msg::new(if table_copy {
                "integrity.table_copy"
            } else {
                "integrity.identical_values"
            })
            .with("same", ns)
            .with("shared", nsh)
            .with("a", la.as_str())
            .with("b", lb.as_str())
            .with("certificates", hist)
            .with("chance", chance_txt.as_str())
            .with_msg("benign", Msg::new("integrity.benign"));
            let mut evidence = Vec::new();
            for c in [a, b] {
                for r in c
                    .readings
                    .iter()
                    .filter(|r| same.iter().any(|k| k.0 == r.characteristic && k.1 == r.unit))
                {
                    evidence.push(reading_ref(c, r));
                }
            }
            evidence.truncate(MAX_EVIDENCE);
            let mut ids = [a.id.as_str(), b.id.as_str()];
            ids.sort();
            out.push(Signal {
                kind,
                supplier: supplier.into(),
                part: Some(part.into()),
                characteristic: None,
                strength,
                why: String::new(),
                why_ja: String::new(),
                msg,
                statistic: json!({
                    "identical": same.len(), "shared": shared.len(), "chance_all_pairs": chance, "pairs_compared": pairs,
                    "threshold": { "min_identical_values": s.min_identical_values, "max_coincidence_ppm": s.max_coincidence_ppm, "min_table_values": s.min_table_values },
                    "history_certificates": group.len(),
                    "characteristics": same.iter().map(|k| k.0.clone()).collect::<Vec<_>>(),
                }),
                evidence,
                certificates: ids.iter().map(|x| x.to_string()).collect(),
                fingerprint: fingerprint(&[kind.as_str(), ids[0], ids[1]]),
            });
        }
    }
}

fn limit_for<'l>(limits: &'l Limits, part: &str, ch: &str, unit: &Option<String>) -> Option<&'l Limit> {
    limits
        .get(&(part.to_string(), ch.to_string()))
        .filter(|l| l.unit == *unit)
}

/// Narrow band and at-limit, per characteristic with known spec limits.
fn distribution(
    supplier: &str,
    part: &str,
    group: &[&Certificate],
    limits: &Limits,
    s: &Settings,
    out: &mut Vec<Signal>,
) {
    let mut by: BTreeMap<CharKey, Vec<(&Certificate, &Reading)>> = BTreeMap::new();
    for c in group {
        for r in &c.readings {
            by.entry((r.characteristic.clone(), r.unit.clone()))
                .or_default()
                .push((c, r));
        }
    }
    for ((ch, unit), rs) in by {
        let Some(lim) = limit_for(limits, part, &ch, &unit) else {
            continue;
        };
        let n = rs.len();
        if n < s.min_samples as usize {
            continue;
        }
        let res = s.resolution_for(&ch, unit.as_deref());
        let vals: Vec<Decimal> = rs.iter().map(|(_, r)| r.value).collect();
        let (Some(min), Some(max)) = (vals.iter().min().copied(), vals.iter().max().copied()) else {
            continue;
        };
        let certs: Vec<String> = {
            let set: BTreeSet<&str> = rs.iter().map(|(c, _)| c.id.as_str()).collect();
            set.into_iter().map(str::to_string).collect()
        };
        let evidence: Vec<EvidenceRef> = rs.iter().take(MAX_EVIDENCE).map(|(c, r)| reading_ref(c, r)).collect();
        let unit_txt = unit.clone().unwrap_or_default();
        // Narrow band: needs both limits, and a resolution fine enough to see a spread (≥ 20 steps per tolerance).
        if let (Some(lo), Some(hi)) = (lim.lower, lim.upper) {
            let width = hi - lo;
            let resolvable = res.is_none_or(|r| width >= r * Decimal::from(20));
            let spread = max - min;
            if width > Decimal::ZERO
                && resolvable
                && spread * Decimal::from(100) <= width * Decimal::from(s.narrow_band_pct)
            {
                let pct = (spread / width * Decimal::from(100)).round_dp(2);
                let strength = if spread * Decimal::from(400) <= width * Decimal::from(s.narrow_band_pct) {
                    Strength::Moderate
                } else {
                    Strength::Weak
                };
                out.push(Signal {
                    kind: SignalKind::NarrowBand,
                    supplier: supplier.into(),
                    part: Some(part.into()),
                    characteristic: Some(ch.clone()),
                    strength,
                    why: String::new(),
                    why_ja: String::new(),
                    msg: Msg::new("integrity.narrow_band")
                        .with("n", n)
                        .with("characteristic", ch.as_str())
                        .with("min", min.to_string())
                        .with("max", max.to_string())
                        .with("unit", unit_txt.as_str())
                        .with("pct", pct.to_string())
                        .with("lower", lo.to_string())
                        .with("upper", hi.to_string())
                        .with("threshold", s.narrow_band_pct)
                        .with_msg("benign", Msg::new("integrity.benign")),
                    statistic: json!({
                        "n": n, "spread": spread.normalize().to_string(), "tolerance_width": width.normalize().to_string(),
                        "spread_pct_of_tolerance": pct.to_string(), "threshold_pct": s.narrow_band_pct,
                        "lower": lo.to_string(), "upper": hi.to_string(), "limit_claims": lim.claims,
                    }),
                    evidence: evidence.clone(),
                    certificates: certs.clone(),
                    fingerprint: fingerprint(&["narrow_band", supplier, part, &ch, &unit_txt]),
                });
            }
        }
        // At limit: share of values at a limit or within `limit_steps` resolution steps inside it.
        let band = res.unwrap_or(Decimal::ZERO) * Decimal::from(s.limit_steps);
        let width = match (lim.lower, lim.upper) {
            (Some(lo), Some(hi)) => Some(hi - lo),
            _ => None,
        };
        if width.is_some_and(|w| band * Decimal::from(10) >= w) {
            continue; // the "near the limit" zone would cover a tenth of the tolerance: not meaningful
        }
        let near = |v: Decimal| {
            let inside = lim.lower.is_none_or(|lo| v >= lo) && lim.upper.is_none_or(|hi| v <= hi);
            inside && (lim.lower.is_some_and(|lo| v - lo <= band) || lim.upper.is_some_and(|hi| hi - v <= band))
        };
        let k = vals.iter().filter(|v| near(**v)).count();
        let exact = vals
            .iter()
            .filter(|v| Some(**v) == lim.lower || Some(**v) == lim.upper)
            .count();
        if k >= 3 && k * 100 >= s.at_limit_pct as usize * n {
            let strength = if k * 100 >= 2 * s.at_limit_pct as usize * n {
                Strength::Strong
            } else if k * 200 >= 3 * s.at_limit_pct as usize * n {
                Strength::Moderate
            } else {
                Strength::Weak
            };
            let lims = format!(
                "{}–{}",
                lim.lower.map(|d| d.to_string()).unwrap_or_default(),
                lim.upper.map(|d| d.to_string()).unwrap_or_default()
            );
            out.push(Signal {
                kind: SignalKind::AtLimit,
                supplier: supplier.into(),
                part: Some(part.into()),
                characteristic: Some(ch.clone()),
                strength,
                why: String::new(),
                why_ja: String::new(),
                msg: Msg::new("integrity.at_limit")
                    .with("k", k)
                    .with("n", n)
                    .with("characteristic", ch.as_str())
                    .with("limits", lims.as_str())
                    .with("unit", unit_txt.as_str())
                    .with("steps", s.limit_steps)
                    .with("exact", exact)
                    .with("threshold", s.at_limit_pct)
                    .with_msg("benign", Msg::new("integrity.benign")),
                statistic: json!({
                    "n": n, "near_limit": k, "exactly_at_limit": exact, "threshold_pct": s.at_limit_pct,
                    "limit_steps": s.limit_steps, "resolution": res.map(|r| r.to_string()),
                    "lower": lim.lower.map(|d| d.to_string()), "upper": lim.upper.map(|d| d.to_string()), "limit_claims": lim.claims,
                }),
                evidence: rs
                    .iter()
                    .filter(|(_, r)| near(r.value))
                    .take(MAX_EVIDENCE)
                    .map(|(c, r)| reading_ref(c, r))
                    .collect(),
                certificates: certs.clone(),
                fingerprint: fingerprint(&["at_limit", supplier, part, &ch, &unit_txt]),
            });
        }
    }
}

/// Precision finer than the configured gauge / method resolution, per supplier and characteristic.
type SupplierChar = (String, String, Option<String>);

fn precision(certs: &[Certificate], s: &Settings, out: &mut Vec<Signal>) {
    let mut by: BTreeMap<SupplierChar, Vec<(&Certificate, &Reading)>> = BTreeMap::new();
    for c in certs {
        for r in &c.readings {
            by.entry((c.supplier.clone(), r.characteristic.clone(), r.unit.clone()))
                .or_default()
                .push((c, r));
        }
    }
    for ((supplier, ch, unit), rs) in by {
        let Some(res) = s.resolution_for(&ch, unit.as_deref()) else {
            continue; // unknown resolution: never guessed
        };
        let n = rs.len();
        let finer: Vec<&(&Certificate, &Reading)> = rs
            .iter()
            .filter(|(_, r)| r.value.checked_rem(res).is_some_and(|m| !m.is_zero()))
            .collect();
        let k = finer.len();
        if k < s.min_excess_values as usize || k * 100 < s.excess_pct as usize * n {
            continue;
        }
        let strength = if k == n && n >= 10 {
            Strength::Strong
        } else if k * 4 >= n * 3 {
            Strength::Moderate
        } else {
            Strength::Weak
        };
        let unit_txt = unit.clone().unwrap_or_default();
        let max_dp = finer
            .iter()
            .map(|(_, r)| r.value.normalize().scale())
            .max()
            .unwrap_or(0);
        let certs_set: BTreeSet<&str> = finer.iter().map(|(c, _)| c.id.as_str()).collect();
        out.push(Signal {
            kind: SignalKind::ExcessPrecision,
            supplier: supplier.clone(),
            part: None,
            characteristic: Some(ch.clone()),
            strength,
            why: String::new(),
            why_ja: String::new(),
            msg: Msg::new("integrity.excess_precision")
                .with("k", k)
                .with("n", n)
                .with("characteristic", ch.as_str())
                .with("max_dp", max_dp)
                .with("resolution", res.to_string())
                .with("unit", unit_txt.as_str())
                .with_msg("benign", Msg::new("integrity.benign")),
            statistic: json!({ "n": n, "finer_than_resolution": k, "resolution": res.to_string(), "unit": unit, "max_decimal_places": max_dp, "threshold_pct": s.excess_pct, "min_values": s.min_excess_values }),
            evidence: finer.iter().take(MAX_EVIDENCE).map(|(c, r)| reading_ref(c, r)).collect(),
            certificates: certs_set.into_iter().map(str::to_string).collect(),
            fingerprint: fingerprint(&["excess_precision", &supplier, &ch, &unit_txt]),
        });
    }
}

/// Terminal-digit preference (χ² of last digits vs uniform, `quality-stats`), per supplier and characteristic with
/// a known resolution. Values are first written at the resolution's number of decimals, because stored values are
/// normalised ("120.10" → "120.1") and a lost trailing zero would itself look like a digit preference. Values finer
/// than the resolution are left out (that is the excess-precision signal). quality-stats guards n ≥ 50 and a spread
/// of ≥ 5 last-digit units.
fn digits(certs: &[Certificate], s: &Settings, out: &mut Vec<Signal>) {
    use quality_stats::Measurement;
    use quality_stats::integrity::{rounding_heaping, terminal_digit_preference};
    let mut by: BTreeMap<SupplierChar, Vec<(&Certificate, &Reading)>> = BTreeMap::new();
    for c in certs {
        for r in &c.readings {
            by.entry((c.supplier.clone(), r.characteristic.clone(), r.unit.clone()))
                .or_default()
                .push((c, r));
        }
    }
    let max_p = f64::from(s.digit_max_p_ppm) / 1e6;
    for ((supplier, ch, unit), rs) in by {
        let Some(res) = s.resolution_for(&ch, unit.as_deref()) else {
            continue;
        };
        let dp = res.normalize().scale();
        let ms: Vec<Measurement> = rs
            .iter()
            .filter(|(_, r)| r.value.checked_rem(res).is_some_and(|m| m.is_zero()))
            .map(|(_, r)| {
                let mut v = r.value;
                v.rescale(dp);
                Measurement::from_decimal(v)
            })
            .collect();
        let Some(t) = terminal_digit_preference(&ms).applicable().cloned() else {
            continue;
        };
        if t.p_value > max_p {
            continue;
        }
        let heap = rounding_heaping(&ms).applicable().cloned();
        let strength = if t.p_value <= max_p / 1e4 {
            Strength::Strong
        } else if t.p_value <= max_p / 100.0 {
            Strength::Moderate
        } else {
            Strength::Weak
        };
        let unit_txt = unit.clone().unwrap_or_default();
        let p_txt = format!("{:.1e}", t.p_value);
        let certs_set: BTreeSet<&str> = rs.iter().map(|(c, _)| c.id.as_str()).collect();
        out.push(Signal {
            kind: SignalKind::DigitPreference,
            supplier: supplier.clone(),
            part: None,
            characteristic: Some(ch.clone()),
            strength,
            why: String::new(),
            why_ja: String::new(),
            msg: Msg::new("integrity.digit_preference")
                .with("n", t.n)
                .with("characteristic", ch.as_str())
                .with("chi2", format!("{:.1}", t.statistic))
                .with("p", p_txt.as_str())
                .with("threshold", format!("{max_p:.0e}"))
                .with_msg("benign", Msg::new("integrity.benign")),
            statistic: json!({
                "n": t.n, "chi2": t.statistic, "df": t.df, "p_value": t.p_value, "threshold_p": max_p,
                "detail": t.explanation, "resolution": res.to_string(), "unit": unit,
                "heaping_0_5": heap.map(|h| json!({ "index": h.statistic, "p_value": h.p_value })),
                "method": "quality-stats terminal_digit_preference",
            }),
            evidence: rs.iter().take(MAX_EVIDENCE).map(|(c, r)| reading_ref(c, r)).collect(),
            certificates: certs_set.into_iter().map(str::to_string).collect(),
            fingerprint: fingerprint(&["digit_preference", &supplier, &ch, &unit_txt]),
        });
    }
}

/// Composition in mass-% that cannot be physical: an element below 0 or above 100 %, or a sum above 100 %.
fn chemistry(c: &Certificate, s: &Settings, out: &mut Vec<Signal>) {
    let chem: Vec<&Reading> = c
        .readings
        .iter()
        .filter(|r| r.characteristic.starts_with("qa.chem.") && r.unit.as_deref() == Some("%"))
        .collect();
    if chem.len() < 3 {
        return;
    }
    let sum: Decimal = chem.iter().map(|r| r.value).sum();
    let limit = Decimal::from(100) + Decimal::new(i64::from(s.chem_sum_tolerance_milli), 3);
    let bad_element = chem
        .iter()
        .any(|r| r.value < Decimal::ZERO || r.value > Decimal::from(100));
    if sum <= limit && !bad_element {
        return;
    }
    let sum_t = sum.normalize();
    out.push(Signal {
        kind: SignalKind::ChemistrySum,
        supplier: c.supplier.clone(),
        part: Some(c.part.clone()),
        characteristic: None,
        strength: Strength::Strong,
        why: String::new(),
        why_ja: String::new(),
        msg: Msg::new(if bad_element { "integrity.chemistry_sum_element" } else { "integrity.chemistry_sum" })
            .with("elements", chem.len())
            .with("document", c.document.as_str())
            .with("sum", sum_t.to_string())
            .with_msg("benign", Msg::new("integrity.benign")),
        statistic: json!({ "sum_pct": sum_t.to_string(), "elements": chem.len(), "limit_pct": limit.to_string(), "element_out_of_range": bad_element }),
        evidence: chem.iter().map(|r| reading_ref(c, r)).collect(),
        certificates: vec![c.id.clone()],
        fingerprint: fingerprint(&["chemistry_sum", &c.id]),
    });
}

/// Dates in an impossible order: certificate issued after shipment, inspection after shipment, or the certificate
/// issued before the inspection it reports.
fn dates(c: &Certificate, s: &Settings, out: &mut Vec<Signal>) {
    type Check<'c> = (
        &'static str,
        &'c Option<DateField>,
        &'static str,
        &'c Option<DateField>,
        &'static str,
    );
    let checks: [Check; 3] = [
        (
            "issued_after_shipment",
            &c.issued,
            "certificate_date",
            &c.shipped,
            "integrity.date.issued_after_shipment",
        ),
        (
            "inspected_after_shipment",
            &c.inspected,
            "inspection_date",
            &c.shipped,
            "integrity.date.inspected_after_shipment",
        ),
        (
            "issued_before_inspection",
            &c.inspected,
            "inspection_date",
            &c.issued,
            "integrity.date.issued_before_inspection",
        ),
    ];
    for (check, later, later_name, earlier, what) in checks {
        let (Some(l), Some(e)) = (later, earlier) else {
            continue;
        };
        let (Some(dl), Some(de)) = (day_number(&l.date), day_number(&e.date)) else {
            continue;
        };
        let gap = dl - de;
        if gap <= i64::from(s.date_grace_days) {
            continue;
        }
        let strength = if gap >= 30 {
            Strength::Strong
        } else if gap >= 7 {
            Strength::Moderate
        } else {
            Strength::Weak
        };
        let earlier_name = if check == "issued_before_inspection" {
            "certificate_date"
        } else {
            "shipment_date"
        };
        out.push(Signal {
            kind: SignalKind::DateOrder,
            supplier: c.supplier.clone(),
            part: Some(c.part.clone()),
            characteristic: Some(check.into()),
            strength,
            why: String::new(),
            why_ja: String::new(),
            msg: Msg::new("integrity.date_order")
                .with("document", c.document.as_str())
                .with_msg("what", Msg::new(what))
                .with("later", l.date.as_str())
                .with("earlier", e.date.as_str())
                .with("days", gap)
                .with_msg("benign", Msg::new("integrity.benign")),
            statistic: json!({ "check": check, "days": gap, "grace_days": s.date_grace_days }),
            evidence: vec![date_ref(c, later_name, l), date_ref(c, earlier_name, e)],
            certificates: vec![c.id.clone()],
            fingerprint: fingerprint(&["date_order", check, &c.id]),
        });
    }
}

/// "ABC-0123" → ("ABC-", 123).
fn split_lot(lot: &str) -> Option<(String, u64)> {
    let digits = lot.chars().rev().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 18 {
        return None;
    }
    let cut = lot.len() - digits;
    Some((lot[..cut].to_string(), lot[cut..].parse().ok()?))
}

/// Lot numbers that run against the certificate dates: a certificate whose lot number is higher than the lots
/// of most certificates dated at least `lot_min_days` later (or lower than most dated that much earlier).
fn lot_sequence(supplier: &str, part: &str, group: &[&Certificate], s: &Settings, out: &mut Vec<Signal>) {
    let mut by_prefix: BTreeMap<String, Vec<(&Certificate, u64, i64)>> = BTreeMap::new();
    for c in group {
        let (Some(lot), Some(day)) = (norm_id(&c.lot), sort_date(c)) else {
            continue;
        };
        if let Some((p, n)) = split_lot(&lot) {
            by_prefix.entry(p).or_default().push((c, n, day));
        }
    }
    let gap = i64::from(s.lot_min_days);
    for (prefix, items) in by_prefix {
        if items.len() < 4 {
            continue;
        }
        for (c, n, d) in &items {
            let comparable: Vec<&(&Certificate, u64, i64)> =
                items.iter().filter(|(_, _, d2)| (d2 - d).abs() >= gap).collect();
            if comparable.len() < 3 {
                continue;
            }
            let inv: Vec<&&(&Certificate, u64, i64)> = comparable
                .iter()
                .filter(|(_, n2, d2)| (d2 > d && n2 < n) || (d2 < d && n2 > n))
                .collect();
            let k = inv.len();
            if k < 2 || k * 2 <= comparable.len() {
                continue;
            }
            let strength = if k * 4 >= comparable.len() * 3 {
                Strength::Moderate
            } else {
                Strength::Weak
            };
            let lot = norm_id(&c.lot).unwrap_or_default();
            let mut evidence = vec![EvidenceRef {
                artifact: c.artifact.clone(),
                document: c.document.clone(),
                claim: None,
                field: Some("lot".into()),
                value: Some(lot.clone()),
                location: json!({}),
            }];
            for (o, _, _) in inv.iter().take(MAX_EVIDENCE - 1) {
                evidence.push(EvidenceRef {
                    artifact: o.artifact.clone(),
                    document: o.document.clone(),
                    claim: None,
                    field: Some("lot".into()),
                    value: norm_id(&o.lot),
                    location: json!({}),
                });
            }
            out.push(Signal {
                kind: SignalKind::LotSequence,
                supplier: supplier.into(),
                part: Some(part.into()),
                characteristic: None,
                strength,
                why: String::new(),
                why_ja: String::new(),
                msg: Msg::new("integrity.lot_sequence")
                    .with("lot", lot.as_str())
                    .with("document", c.document.as_str())
                    .with("k", k)
                    .with("comparable", comparable.len())
                    .with("days", gap)
                    .with_msg("benign", Msg::new("integrity.benign")),
                statistic: json!({ "lot_prefix": prefix, "lot_number": n, "inversions": k, "comparable": comparable.len(), "min_days_apart": gap }),
                evidence,
                certificates: std::iter::once(c.id.clone())
                    .chain(inv.iter().map(|(o, _, _)| o.id.clone()))
                    .collect(),
                fingerprint: fingerprint(&["lot_sequence", &c.id]),
            });
        }
    }
}
