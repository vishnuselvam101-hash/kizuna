//! One-dimensional tolerance stack-up of a dimension loop.
//!
//! The loop's closing dimension (gap) is G = Σ sᵢ·xᵢ with sᵢ = +1 / −1. Three analyses:
//! * **Worst case** (exact decimal): G ∈ [Σ sᵢ·(sᵢ>0 ? Lᵢ : Uᵢ), Σ sᵢ·(sᵢ>0 ? Uᵢ : Lᵢ)].
//! * **RSS**: half-width √Σtᵢ² about Σ sᵢ·midᵢ, tᵢ = half the tolerance of link i (assumes each link is a centred
//!   normal with Cp = 1, i.e. ±tᵢ = ±3σᵢ).
//! * **Modified RSS** (Bender 1962/1968): k·√Σtᵢ², k = 1.5 by default, capped at the worst case (a statistical
//!   width larger than the worst case is meaningless; the cap is reported).
//!
//! With process data per link (σ, or Cpk with an optional mean) the assembly is modelled as a normal with
//! μ = Σ sᵢ·μᵢ, σ² = Σσᵢ²; predicted yield = P(LSL ≤ G ≤ USL) (normal approximation, valid for independent
//! links; Cpk → σ via σ = min(USL − μ, μ − LSL)/(3·Cpk)). Statistics are f64; limits stay exact.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::error::{TolError, TolResult, invalid};
use crate::normal;
use crate::spec::Spec;
use crate::units::Dimension;

/// Sense of a link in the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Plus,
    Minus,
}

/// Supplier process knowledge for one link. `mean` is canonical (mm); `None` = centred at the mid-tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Process {
    Sigma { mean: Option<Decimal>, sigma: f64 },
    Cpk { mean: Option<Decimal>, cpk: f64 },
}

/// One dimension in the loop. The spec must be two-sided and a length.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub name: String,
    pub direction: Direction,
    pub spec: Spec,
    pub process: Option<Process>,
}

/// Analysis settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackOptions {
    /// Bender factor for modified RSS (1.5 in Bender's paper).
    pub bender_factor: f64,
    /// Cp assumed for a link without process data (1.0 → σ = T/6).
    pub default_cp: f64,
    /// Yield the assembly must reach; below it a `YieldBelowTarget` finding is raised.
    pub min_yield: f64,
}

impl Default for StackOptions {
    fn default() -> Self {
        StackOptions {
            bender_factor: 1.5,
            default_cp: 1.0,
            min_yield: 0.9973,
        }
    }
}

/// Share of the assembly variance contributed by one link.
#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    pub name: String,
    pub variance_share: f64,
}

/// Something the reviewer must see.
#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// The worst-case range is not inside the assembly spec (exact decimal).
    WorstCaseOutside { low: Decimal, high: Decimal },
    /// The RSS range is not inside the assembly spec.
    RssOutside { low: f64, high: f64 },
    /// The modified-RSS width was capped at the worst case.
    MrssCappedAtWorstCase,
    /// Predicted yield is below the target: the stack cannot meet the assembly spec with these suppliers.
    YieldBelowTarget {
        predicted: f64,
        target: f64,
        top_contributor: String,
    },
}

/// Full result. All dimensional values are canonical (mm).
#[derive(Debug, Clone, PartialEq)]
pub struct StackReport {
    pub nominal: Decimal,
    pub worst_case: (Decimal, Decimal),
    pub rss: (f64, f64),
    pub mrss: (f64, f64),
    pub mean: f64,
    pub sigma: f64,
    pub predicted_yield: f64,
    /// Expected nonconforming assemblies per million.
    pub ppm_out: f64,
    pub contributions: Vec<Contribution>,
    pub findings: Vec<Finding>,
}

fn f(x: Decimal) -> f64 {
    x.to_f64().unwrap_or(f64::NAN)
}

fn arith(msg: &str) -> TolError {
    TolError::Arithmetic(msg.into())
}

/// Analyse a dimension loop against the assembly (gap) spec.
pub fn analyze(links: &[Link], assembly: &Spec, opts: &StackOptions) -> TolResult<StackReport> {
    if links.is_empty() {
        return invalid("a stack needs at least one link");
    }
    if assembly.dimension != Dimension::Length {
        return invalid("assembly spec must be a length");
    }
    if !(opts.bender_factor >= 1.0 && opts.default_cp > 0.0 && opts.min_yield > 0.0 && opts.min_yield < 1.0) {
        return invalid("options: bender_factor ≥ 1, default_cp > 0, 0 < min_yield < 1");
    }
    let (mut nom, mut wc_lo, mut wc_hi) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    let (mut mid, mut sum_t2, mut mu, mut var) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    let mut variances = Vec::with_capacity(links.len());
    for l in links {
        let (Some(lo), Some(hi)) = (l.spec.lower, l.spec.upper) else {
            return invalid(format!("link {:?} needs two limits", l.name));
        };
        if l.spec.dimension != Dimension::Length {
            return invalid(format!("link {:?} is not a length", l.name));
        }
        let (lo, hi) = (lo.value, hi.value);
        let n = l.spec.nominal.unwrap_or((lo + hi) / Decimal::TWO);
        let m = (lo + hi) / Decimal::TWO;
        let t = f(hi - lo) / 2.0;
        let (mean_i, sigma_i) = match l.process {
            None => (f(m), t / (3.0 * opts.default_cp)),
            Some(Process::Sigma { mean, sigma }) => {
                if !(sigma.is_finite() && sigma >= 0.0) {
                    return invalid(format!("link {:?}: σ must be finite and ≥ 0", l.name));
                }
                (f(mean.unwrap_or(m)), sigma)
            }
            Some(Process::Cpk { mean, cpk }) => {
                if !(cpk.is_finite() && cpk > 0.0) {
                    return invalid(format!(
                        "link {:?}: Cpk must be > 0 (a process centred outside its limits has no σ from Cpk)",
                        l.name
                    ));
                }
                let mu_i = mean.unwrap_or(m);
                let dist = (hi - mu_i).min(mu_i - lo);
                if dist <= Decimal::ZERO {
                    return invalid(format!("link {:?}: mean {mu_i} is not inside its limits", l.name));
                }
                (f(mu_i), f(dist) / (3.0 * cpk))
            }
        };
        let s = match l.direction {
            Direction::Plus => 1.0,
            Direction::Minus => -1.0,
        };
        let add = |a: Decimal, b: Decimal| a.checked_add(b).ok_or_else(|| arith("stack sum overflows"));
        match l.direction {
            Direction::Plus => {
                nom = add(nom, n)?;
                wc_lo = add(wc_lo, lo)?;
                wc_hi = add(wc_hi, hi)?;
            }
            Direction::Minus => {
                nom = add(nom, -n)?;
                wc_lo = add(wc_lo, -hi)?;
                wc_hi = add(wc_hi, -lo)?;
            }
        }
        mid += s * f(m);
        sum_t2 += t * t;
        mu += s * mean_i;
        var += sigma_i * sigma_i;
        variances.push((l.name.clone(), sigma_i * sigma_i));
    }
    let wc_half = f(wc_hi - wc_lo) / 2.0;
    let rss_half = sum_t2.sqrt();
    let mut findings = Vec::new();
    let mut mrss_half = opts.bender_factor * rss_half;
    if mrss_half > wc_half {
        mrss_half = wc_half;
        findings.push(Finding::MrssCappedAtWorstCase);
    }
    let sigma = var.sqrt();

    let lsl = assembly.lower.map(|b| f(b.value));
    let usl = assembly.upper.map(|b| f(b.value));
    let low_tail = lsl.map_or(0.0, |l| tail_below(l, mu, sigma));
    let high_tail = usl.map_or(0.0, |u| tail_above(u, mu, sigma));
    let predicted_yield = (1.0 - low_tail - high_tail).max(0.0);

    let wc_inside = assembly.lower.is_none_or(|b| wc_lo >= b.value) && assembly.upper.is_none_or(|b| wc_hi <= b.value);
    if !wc_inside {
        findings.push(Finding::WorstCaseOutside {
            low: wc_lo,
            high: wc_hi,
        });
    }
    let rss = (mid - rss_half, mid + rss_half);
    if lsl.is_some_and(|l| rss.0 < l) || usl.is_some_and(|u| rss.1 > u) {
        findings.push(Finding::RssOutside {
            low: rss.0,
            high: rss.1,
        });
    }
    let mut contributions: Vec<Contribution> = variances
        .into_iter()
        .map(|(name, v)| Contribution {
            name,
            variance_share: if var > 0.0 { v / var } else { 0.0 },
        })
        .collect();
    contributions.sort_by(|a, b| b.variance_share.total_cmp(&a.variance_share));
    if predicted_yield < opts.min_yield {
        findings.push(Finding::YieldBelowTarget {
            predicted: predicted_yield,
            target: opts.min_yield,
            top_contributor: contributions.first().map(|c| c.name.clone()).unwrap_or_default(),
        });
    }
    Ok(StackReport {
        nominal: nom,
        worst_case: (wc_lo, wc_hi),
        rss,
        mrss: (mid - mrss_half, mid + mrss_half),
        mean: mu,
        sigma,
        predicted_yield,
        ppm_out: (low_tail + high_tail) * 1e6,
        contributions,
        findings,
    })
}

fn tail_below(l: f64, mu: f64, sigma: f64) -> f64 {
    if sigma > 0.0 {
        normal::cdf((l - mu) / sigma)
    } else if mu < l {
        1.0
    } else {
        0.0
    }
}

fn tail_above(u: f64, mu: f64, sigma: f64) -> f64 {
    if sigma > 0.0 {
        normal::sf((u - mu) / sigma)
    } else if mu > u {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Unit;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    fn link(name: &str, dir: Direction, nom: &str, tol: &str) -> Link {
        Link {
            name: name.into(),
            direction: dir,
            spec: Spec::symmetric(d(nom), d(tol), Unit::Mm).expect("spec"),
            process: None,
        }
    }

    fn chain() -> Vec<Link> {
        // Housing 10 ±0.1 minus two parts 4 ±0.05 and 5 ±0.05 → gap 1.
        vec![
            link("housing", Direction::Plus, "10", "0.1"),
            link("part A", Direction::Minus, "4", "0.05"),
            link("part B", Direction::Minus, "5", "0.05"),
        ]
    }

    #[test]
    fn hand_computed_three_link_loop() {
        let gap = Spec::symmetric(d("1"), d("0.15"), Unit::Mm).expect("gap");
        let r = analyze(&chain(), &gap, &StackOptions::default()).expect("report");
        assert_eq!(r.nominal, d("1"));
        assert_eq!(r.worst_case, (d("0.8"), d("1.2")));
        // √(0.1² + 0.05² + 0.05²) = √0.015 = 0.122474487…
        assert!((r.rss.1 - 1.0 - 0.015_f64.sqrt()).abs() < 1e-12);
        assert!((r.mrss.1 - 1.0 - 0.183_711_730_708_738_36).abs() < 1e-12);
        // σ = √0.015/3; yield = 2Φ(0.15/σ) − 1 = 0.99976143654597 (CPython math.erfc).
        assert!(
            (r.predicted_yield - 0.999_761_436_545_971_3).abs() < 1e-10,
            "{}",
            r.predicted_yield
        );
        assert!(r.findings.contains(&Finding::WorstCaseOutside {
            low: d("0.8"),
            high: d("1.2")
        }));
        assert!(!r.findings.iter().any(|f| matches!(f, Finding::YieldBelowTarget { .. })));
        assert_eq!(r.contributions[0].name, "housing");
        assert!((r.contributions[0].variance_share - 0.01 / 0.015).abs() < 1e-12);
    }

    #[test]
    fn poor_supplier_capability_is_a_finding() {
        let gap = Spec::symmetric(d("1"), d("0.15"), Unit::Mm).expect("gap");
        let mut c = chain();
        c[0].process = Some(Process::Cpk {
            mean: Some(d("10.03")),
            cpk: 0.5,
        }); // σ = 0.07/1.5
        let r = analyze(&c, &gap, &StackOptions::default()).expect("report");
        match r
            .findings
            .iter()
            .find(|f| matches!(f, Finding::YieldBelowTarget { .. }))
        {
            Some(Finding::YieldBelowTarget {
                predicted,
                top_contributor,
                ..
            }) => {
                assert!(*predicted < 0.9973 && top_contributor == "housing", "{predicted}");
            }
            other => panic!("expected YieldBelowTarget, got {other:?} in {:?}", r.findings),
        }
        assert!((r.mean - 1.03).abs() < 1e-12);
    }

    #[test]
    fn single_link_caps_mrss_and_bad_inputs_are_refused() {
        let gap = Spec::at_least(d("0"), true, Unit::Mm).expect("gap");
        let r = analyze(
            &[link("a", Direction::Plus, "1", "0.1")],
            &gap,
            &StackOptions::default(),
        )
        .expect("r");
        assert!(r.findings.contains(&Finding::MrssCappedAtWorstCase));
        assert!((r.mrss.1 - 1.1).abs() < 1e-12);
        assert!(analyze(&[], &gap, &StackOptions::default()).is_err());
        let mut l = link("a", Direction::Plus, "1", "0.1");
        l.process = Some(Process::Cpk { mean: None, cpk: 0.0 });
        assert!(analyze(&[l.clone()], &gap, &StackOptions::default()).is_err());
        l.process = Some(Process::Cpk {
            mean: Some(d("2")),
            cpk: 1.0,
        });
        assert!(analyze(&[l.clone()], &gap, &StackOptions::default()).is_err());
        l.spec = Spec::at_most(d("1"), true, Unit::Mm).expect("s");
        assert!(analyze(&[l], &gap, &StackOptions::default()).is_err());
    }
}
