//! Multiple-testing control for integrity signals (MATH v2 (a)).
//!
//! A certificate is checked by several tests (terminal digits, heaping, variance per characteristic, …) and a
//! batch holds many certificates. Flagging every test at p < α inflates false alarms (≈ 1 − (1 − α)^m per
//! certificate). This module gives three standard remedies, each a pure function:
//!
//! * **Combination per certificate**: Fisher (−2 Σ ln pᵢ ~ χ²_{2m}; Fisher 1932) and weighted Stouffer
//!   (Σ wᵢ Φ⁻¹(1 − pᵢ) / √Σwᵢ²; Stouffer et al. 1949, Whitlock 2005). Both assume independent tests under H0.
//! * **Calibration per supplier**: a conformal (empirical) p-value of a certificate's combined score against the
//!   same supplier's reference certificates that reviewers already cleared: p = (1 + #{ref ≥ score}) / (n + 1)
//!   (Vovk et al. 2005; Bates et al. 2023). If the new certificate is exchangeable with the references (same
//!   honest process), P(p ≤ α) ≤ α exactly — whatever the supplier's harmless quirks (a gauge that only shows 0/5
//!   last digits, a conservatively stated σ). The guarantee is only as good as the references: a manipulated
//!   certificate that was cleared by mistake makes the calibration more permissive.
//! * **Batch control**: Benjamini–Hochberg step-up (1995) controls the false discovery rate at q for independent
//!   or positively dependent p-values; Benjamini–Yekutieli (2001) for arbitrary dependence (more conservative).
//!
//! Output is a ranking for review. Nothing here rejects a certificate or overrides a reviewer's decision.

use std::collections::BTreeMap;

use crate::TestResult;
use crate::error::{StatsError, StatsResult, check_finite, need};
use crate::special::{chi2_sf, normal_quantile, normal_sf};

/// Smallest p-value used inside logarithms and quantiles (an exact 0 from a test means "beyond f64").
const P_FLOOR: f64 = 1e-300;

fn check_p(p: &[f64]) -> StatsResult<()> {
    check_finite(p, "p-values")?;
    if p.iter().all(|x| (0.0..=1.0).contains(x)) {
        Ok(())
    } else {
        Err(StatsError::InvalidParameter("p-values must lie in [0, 1]"))
    }
}

/// Fisher's combination: statistic −2 Σ ln pᵢ, df 2m, p = χ²_{2m} survival. Needs m ≥ 1.
pub fn fisher_combine(p: &[f64]) -> StatsResult<TestResult> {
    need(p.len(), 1)?;
    check_p(p)?;
    let stat: f64 = p.iter().map(|x| -2.0 * x.max(P_FLOOR).ln()).sum();
    let df = 2.0 * p.len() as f64;
    let pv = chi2_sf(stat, df);
    Ok(TestResult {
        statistic: stat,
        df,
        p_value: pv,
        n: p.len(),
        explanation: format!(
            "Fisher combination of {} p-values: χ²({df}) = {stat:.3}, p = {pv:.3e}",
            p.len()
        ),
    })
}

/// Weighted Stouffer combination: Z = Σ wᵢ zᵢ / √Σwᵢ² with zᵢ = Φ⁻¹(1 − pᵢ); p = 1 − Φ(Z). `weights` = None means
/// equal weights; weights must be ≥ 0 with a positive sum of squares.
pub fn stouffer_combine(p: &[f64], weights: Option<&[f64]>) -> StatsResult<TestResult> {
    need(p.len(), 1)?;
    check_p(p)?;
    let w: Vec<f64> = match weights {
        Some(w) => {
            if w.len() != p.len() {
                return Err(StatsError::InvalidParameter("one weight per p-value"));
            }
            check_finite(w, "weights")?;
            if w.iter().any(|x| *x < 0.0) {
                return Err(StatsError::InvalidParameter("weights must be ≥ 0"));
            }
            w.to_vec()
        }
        None => vec![1.0; p.len()],
    };
    let norm = w.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm <= 0.0 {
        return Err(StatsError::InvalidParameter("weights are all zero"));
    }
    let z: f64 = p
        .iter()
        .zip(&w)
        .map(|(x, wi)| wi * -normal_quantile(x.clamp(P_FLOOR, 1.0 - 1e-16)))
        .sum::<f64>()
        / norm;
    let pv = normal_sf(z);
    Ok(TestResult {
        statistic: z,
        df: 0.0,
        p_value: pv,
        n: p.len(),
        explanation: format!("Stouffer combination of {} p-values: Z = {z:.3}, p = {pv:.3e}", p.len()),
    })
}

/// Benjamini–Hochberg adjusted p-values (q-values): q₍ᵢ₎ = min_{j ≥ i} min(1, m·p₍ⱼ₎/j). Reject H₀ᵢ at FDR q iff
/// adjusted ≤ q. Output is in input order.
pub fn bh_adjust(p: &[f64]) -> StatsResult<Vec<f64>> {
    step_up(p, 1.0)
}

/// Benjamini–Yekutieli adjusted p-values: BH multiplied by c(m) = Σ 1/i (valid under any dependence).
pub fn by_adjust(p: &[f64]) -> StatsResult<Vec<f64>> {
    let c: f64 = (1..=p.len()).map(|i| 1.0 / i as f64).sum();
    step_up(p, c)
}

fn step_up(p: &[f64], c: f64) -> StatsResult<Vec<f64>> {
    check_p(p)?;
    let m = p.len();
    let mut idx: Vec<usize> = (0..m).collect();
    // Stable order: by p, then by index (ties keep input order, so the output is deterministic).
    idx.sort_by(|&a, &b| p[a].total_cmp(&p[b]).then(a.cmp(&b)));
    let mut out = vec![0.0; m];
    let mut running = 1.0_f64;
    for (rank, &i) in idx.iter().enumerate().rev() {
        let v = (c * m as f64 * p[i] / (rank + 1) as f64).min(1.0);
        running = running.min(v);
        out[i] = running;
    }
    Ok(out)
}

/// Which hypotheses BH rejects at false discovery rate `q` in (0, 1).
pub fn benjamini_hochberg(p: &[f64], q: f64) -> StatsResult<Vec<bool>> {
    if !(q > 0.0 && q < 1.0) {
        return Err(StatsError::InvalidParameter("q must be in (0, 1)"));
    }
    Ok(bh_adjust(p)?.into_iter().map(|a| a <= q).collect())
}

/// Conformal p-value of `score` (larger = more suspicious) against reference scores of cleared items:
/// (1 + #{r ≥ score}) / (n + 1). Valid (P(p ≤ α) ≤ α) when the item is exchangeable with the references.
pub fn conformal_p(score: f64, reference: &[f64]) -> StatsResult<f64> {
    check_finite(&[score], "score")?;
    check_finite(reference, "reference scores")?;
    let ge = reference.iter().filter(|r| **r >= score).count();
    Ok((1 + ge) as f64 / (reference.len() + 1) as f64)
}

/// Per-supplier reference scores (e.g. −log10 of the Fisher p of each certificate a reviewer cleared).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SupplierCalibration {
    reference: BTreeMap<String, Vec<f64>>,
    min_reference: usize,
}

/// Calibrated p-value of one certificate.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibratedP {
    pub p_value: f64,
    /// Reference certificates the p-value was computed against.
    pub reference_n: usize,
    /// The smallest p-value this reference size can produce, 1 / (n + 1): with few references even an extreme
    /// certificate cannot reach a small α.
    pub resolution: f64,
}

impl SupplierCalibration {
    /// `min_reference`: below this many cleared certificates a supplier is reported as not calibrated.
    pub fn new(min_reference: usize) -> Self {
        SupplierCalibration {
            reference: BTreeMap::new(),
            min_reference: min_reference.max(1),
        }
    }

    /// Adds the score of a certificate a reviewer cleared. Non-finite scores are refused.
    pub fn add_cleared(&mut self, supplier: &str, score: f64) -> StatsResult<()> {
        check_finite(&[score], "score")?;
        self.reference.entry(supplier.to_string()).or_default().push(score);
        Ok(())
    }

    pub fn reference_len(&self, supplier: &str) -> usize {
        self.reference.get(supplier).map_or(0, Vec::len)
    }

    /// Conformal p-value against the supplier's own cleared certificates; None if the supplier has fewer than
    /// `min_reference` (the caller then falls back to the uncalibrated combined p-value and says so).
    pub fn calibrated_p(&self, supplier: &str, score: f64) -> StatsResult<Option<CalibratedP>> {
        let Some(r) = self.reference.get(supplier).filter(|r| r.len() >= self.min_reference) else {
            return Ok(None);
        };
        Ok(Some(CalibratedP {
            p_value: conformal_p(score, r)?,
            reference_n: r.len(),
            resolution: 1.0 / (r.len() + 1) as f64,
        }))
    }
}

/// Score of one test for calibration: −log10 p (larger = more suspicious); `None` (test not applicable) scores 0.
pub fn p_score(p: Option<f64>) -> f64 {
    p.map_or(0.0, |p| -p.clamp(P_FLOOR, 1.0).log10())
}

/// Two-stage, fully conformal per-supplier p-value for a certificate with several tests.
///
/// Stage 1 calibrates every test separately: each of the n + 1 certificates (n cleared references plus the new
/// one) gets, per test, the conformal p-value of its score among the *other* n certificates. Stage 2 combines a
/// certificate's per-test p-values with Fisher's statistic and returns the conformal p-value of the new
/// certificate's statistic among all n + 1. Every step is a symmetric function of the n + 1 certificates, so if the
/// new certificate is exchangeable with the references, P(p ≤ α) ≤ α exactly. Unlike calibrating only the combined
/// score, a test that always fires for this supplier (e.g. digit preference from a 0.005 gauge) is neutralised
/// instead of drowning the other tests. Scores are per-test [`p_score`] values; rows must all have the same length.
/// O(n²·m).
pub fn conformal_combined_p(new: &[f64], reference: &[Vec<f64>]) -> StatsResult<f64> {
    let m = new.len();
    need(m, 1)?;
    check_finite(new, "scores")?;
    for r in reference {
        if r.len() != m {
            return Err(StatsError::InvalidParameter("every certificate needs the same tests"));
        }
        check_finite(r, "reference scores")?;
    }
    let rows: Vec<&[f64]> = reference
        .iter()
        .map(Vec::as_slice)
        .chain(std::iter::once(new))
        .collect();
    let n1 = rows.len();
    let fisher: Vec<f64> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            (0..m)
                .map(|j| {
                    let ge = rows
                        .iter()
                        .enumerate()
                        .filter(|(k, other)| *k != i && other[j] >= row[j])
                        .count();
                    -2.0 * ((1 + ge) as f64 / n1 as f64).ln()
                })
                .sum()
        })
        .collect();
    let mine = fisher[n1 - 1];
    let ge = fisher.iter().filter(|f| **f >= mine).count(); // includes itself
    Ok(ge as f64 / n1 as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::special::{normal_quantile, normal_sf};
    use crate::synth::Rng;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b}");
    }

    #[test]
    fn combined_conformal_neutralises_an_always_firing_test() {
        // Test 0 always fires for this supplier (score 30 ± noise); test 1 is ordinary. A certificate extreme only
        // on test 1 gets the smallest possible p; one extreme only on the always-firing test's usual level does not.
        let mut r = Rng::new(9);
        let refs: Vec<Vec<f64>> = (0..99).map(|_| vec![30.0 + r.uniform(), r.uniform()]).collect();
        let p = conformal_combined_p(&[30.5, 5.0], &refs).unwrap();
        assert!(p <= 0.02, "{p}");
        let q = conformal_combined_p(&[30.5, 0.5], &refs).unwrap();
        assert!(q > 0.2, "{q}");
        assert!(conformal_combined_p(&[1.0], &[vec![1.0, 2.0]]).is_err());
        assert!(conformal_combined_p(&[], &[]).is_err());
        assert_eq!(p_score(None), 0.0);
        assert!((p_score(Some(0.01)) - 2.0).abs() < 1e-12);
    }

    proptest::proptest! {
        #[test]
        fn combined_conformal_rank_identity(seed in 0u64..5_000, n in 1usize..40, m in 1usize..5) {
            // Exactly exchangeable: across the n + 1 leave-one-out roles, the share with p ≤ 1/(n+1) is at most 1
            // (ties can only make p larger), and every p is in [1/(n+1), 1].
            let mut r = Rng::new(seed);
            let all: Vec<Vec<f64>> = (0..=n).map(|_| (0..m).map(|_| r.uniform()).collect()).collect();
            let mut low = 0;
            for i in 0..=n {
                let refs: Vec<Vec<f64>> = all.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, v)| v.clone()).collect();
                let p = conformal_combined_p(&all[i], &refs).unwrap();
                proptest::prop_assert!(p >= 1.0 / (n + 1) as f64 - 1e-15 && p <= 1.0);
                low += usize::from(p <= 1.0 / (n + 1) as f64 + 1e-15);
            }
            proptest::prop_assert!(low <= 1);
        }
    }

    #[test]
    fn bh_hand_computed() {
        // m = 5, p = .01 .04 .03 .20 .50 → sorted .01 .03 .04 .20 .50; m·p/i = .05 .075 .0667 .25 .5;
        // step-up minima from the top: .05 .0667 .0667 .25 .5.
        let p = [0.01, 0.04, 0.03, 0.20, 0.50];
        let a = bh_adjust(&p).unwrap();
        for (x, y) in a.iter().zip([0.05, 0.2 / 3.0, 0.2 / 3.0, 0.25, 0.5]) {
            close(*x, y, 1e-12);
        }
        assert_eq!(benjamini_hochberg(&p, 0.07).unwrap(), [true, true, true, false, false]);
        let by = by_adjust(&p).unwrap();
        close(by[0], (0.05_f64 * (1.0 + 0.5 + 1.0 / 3.0 + 0.25 + 0.2)).min(1.0), 1e-12);
        assert!(bh_adjust(&[]).unwrap().is_empty());
        assert!(benjamini_hochberg(&p, 0.0).is_err());
        assert!(bh_adjust(&[1.2]).is_err());
    }

    #[test]
    fn fisher_and_stouffer_hand_computed() {
        // Fisher, one p: −2 ln p ~ χ²₂ whose survival is exp(−x/2) = p, so the combined p is p itself.
        close(fisher_combine(&[0.03]).unwrap().p_value, 0.03, 1e-12);
        // Two p = 0.05: χ² = −4 ln 0.05 = 11.983, df 4, sf = e^{-x/2}(1 + x/2) = 0.0025·(1 + 5.991) = 0.017479.
        close(
            fisher_combine(&[0.05, 0.05]).unwrap().p_value,
            0.0025 * (1.0 - 2.0 * 0.05f64.ln()),
            1e-12,
        );
        // Stouffer, two p = 0.05: Z = 2·1.6449/√2 = 2.3262, p = 0.0100.
        close(
            stouffer_combine(&[0.05, 0.05], None).unwrap().p_value,
            normal_sf(2.0 * normal_quantile(0.95) / 2f64.sqrt()),
            1e-12,
        );
        // A zero weight ignores that test.
        close(
            stouffer_combine(&[0.05, 0.9], Some(&[1.0, 0.0])).unwrap().p_value,
            0.05,
            1e-9,
        );
        assert!(fisher_combine(&[]).is_err());
        assert!(stouffer_combine(&[0.5], Some(&[0.0])).is_err());
        assert!(stouffer_combine(&[0.5, 0.5], Some(&[1.0])).is_err());
        // p = 0 does not produce NaN.
        assert!(fisher_combine(&[0.0, 0.5]).unwrap().p_value < 1e-100);
        assert!(stouffer_combine(&[0.0, 1.0], None).unwrap().p_value.is_finite());
    }

    #[test]
    fn supplier_calibration_needs_references() {
        let mut c = SupplierCalibration::new(3);
        c.add_cleared("A", 1.0).unwrap();
        c.add_cleared("A", 2.0).unwrap();
        assert_eq!(c.calibrated_p("A", 5.0).unwrap(), None);
        c.add_cleared("A", 3.0).unwrap();
        let p = c.calibrated_p("A", 5.0).unwrap().unwrap();
        close(p.p_value, 0.25, 1e-15);
        close(c.calibrated_p("A", 2.0).unwrap().unwrap().p_value, 0.75, 1e-15);
        assert_eq!(c.calibrated_p("B", 5.0).unwrap(), None);
        assert!(c.add_cleared("A", f64::NAN).is_err());
    }

    #[test]
    fn combinations_are_uniform_under_the_null() {
        // 20 000 certificates of 6 independent uniform p-values: the share of combined p ≤ α is ≈ α.
        let mut r = Rng::new(7);
        let (mut f, mut s) = (0usize, 0usize);
        let n = 20_000;
        for _ in 0..n {
            let p: Vec<f64> = (0..6).map(|_| r.uniform()).collect();
            f += usize::from(fisher_combine(&p).unwrap().p_value <= 0.05);
            s += usize::from(stouffer_combine(&p, None).unwrap().p_value <= 0.05);
        }
        for k in [f, s] {
            let rate = k as f64 / n as f64;
            assert!((rate - 0.05).abs() < 0.006, "{rate}");
        }
    }

    proptest::proptest! {
        #[test]
        fn bh_properties(p in proptest::collection::vec(0f64..=1.0, 1..60), q in 0.001f64..0.5) {
            let a = bh_adjust(&p).unwrap();
            let by = by_adjust(&p).unwrap();
            for i in 0..p.len() {
                // Adjusted p in [p, 1], BY ≥ BH, monotone in the raw p.
                proptest::prop_assert!(a[i] >= p[i] - 1e-15 && a[i] <= 1.0);
                proptest::prop_assert!(by[i] >= a[i] - 1e-15);
                for j in 0..p.len() {
                    if p[i] < p[j] { proptest::prop_assert!(a[i] <= a[j] + 1e-15); }
                }
            }
            // Rejections at a larger q are a superset; BH rejects at least what Bonferroni rejects.
            let r1 = benjamini_hochberg(&p, q).unwrap();
            let r2 = benjamini_hochberg(&p, (q * 2.0).min(0.99)).unwrap();
            for i in 0..p.len() {
                proptest::prop_assert!(!r1[i] || r2[i]);
                if p[i] * p.len() as f64 <= q { proptest::prop_assert!(r1[i]); }
            }
            // Deterministic.
            proptest::prop_assert_eq!(a, bh_adjust(&p).unwrap());
        }

        #[test]
        fn combined_p_in_unit_interval_and_monotone(p in proptest::collection::vec(1e-12f64..=1.0, 1..20), k in 0usize..20, f in 0.01f64..1.0) {
            let a = fisher_combine(&p).unwrap().p_value;
            let s = stouffer_combine(&p, None).unwrap().p_value;
            proptest::prop_assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&s));
            // Making one p-value smaller never makes the combination less significant.
            let mut q = p.clone();
            let i = k % q.len();
            q[i] *= f;
            proptest::prop_assert!(fisher_combine(&q).unwrap().p_value <= a + 1e-12);
            proptest::prop_assert!(stouffer_combine(&q, None).unwrap().p_value <= s + 1e-12);
        }

        #[test]
        fn conformal_p_is_valid(seed in 0u64..10_000, n in 1usize..200) {
            // Super-uniform: P(p ≤ α) ≤ α with an exchangeable item. Checked as: p ≥ 1/(n+1), p ≤ 1, and the
            // rank identity (exactly k of n+1 exchangeable scores get p ≤ k/(n+1) when there are no ties).
            let mut r = Rng::new(seed);
            let all: Vec<f64> = (0..=n).map(|_| r.uniform()).collect();
            let mut le = 0;
            for i in 0..=n {
                let refs: Vec<f64> = all.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, x)| *x).collect();
                let p = conformal_p(all[i], &refs).unwrap();
                proptest::prop_assert!(p >= 1.0 / (n + 1) as f64 - 1e-15 && p <= 1.0);
                le += usize::from(p <= 1.0 / (n + 1) as f64 + 1e-15);
            }
            proptest::prop_assert_eq!(le, 1);
        }
    }
}
