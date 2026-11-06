//! Monte Carlo stack-up with non-normal links, a Wilson confidence interval on the yield, and Johnson / Pearson
//! distribution fitting.
//!
//! [`crate::stack::analyze`] models the gap G = Σ sᵢ·xᵢ as a normal. That is exact only for normal links; uniform,
//! triangular or skewed links (tool wear, one-sided processes, sorted parts) give a different gap distribution and a
//! different yield. [`monte_carlo`] draws every link from its own [`Distribution`] and counts the assemblies inside
//! the spec. It is pure and deterministic: the same inputs and seed give the same report on every platform that
//! implements IEEE-754 `f64` and `ln`/`exp`/`cos`/`sqrt`/`sinh` identically.
//!
//! * RNG: SplitMix64 (Steele, Lea & Flood 2014; state += 0x9e3779b97f4a7c15, then the splitmix64 finaliser), the
//!   same generator as `quality-stats` synth. Normals by Box–Muller (one of the pair).
//! * Yield CI: Wilson (1927) score interval for a binomial proportion, z = 1.959963984540054 for 95 %. The CI covers
//!   only the Monte Carlo sampling error; it says nothing about whether the link distributions are right.
//! * Johnson system (Johnson 1949) fitted from data by the percentile method of Slifker & Shapiro (1980),
//!   "The Johnson System: Selection and Parameter Estimation", Technometrics 22(2), 239–246: see [`JohnsonFit`].
//! * Pearson system: moments β₁ (skewness²), β₂ (kurtosis) and the type from Pearson's κ criterion (Elderton &
//!   Johnson, "Systems of Frequency Curves", 1969). This is a report-only diagnostic; sampling uses Johnson.

use rust_decimal::prelude::ToPrimitive;

use crate::error::{TolResult, invalid};
use crate::spec::{Bound, Spec};
use crate::stack::{Direction, Link, StackOptions, link_model};
use crate::units::Dimension;

/// z for a two-sided 95 % interval (Φ⁻¹(0.975)).
pub const Z95: f64 = 1.959_963_984_540_054;

/// Largest sample count accepted (every draw of the gap is kept for the quantiles: 8 bytes each).
pub const MAX_SAMPLES: usize = 50_000_000;

/// Deterministic SplitMix64 generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1) with 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller, one of the pair).
    pub fn normal(&mut self) -> f64 {
        let u1 = 1.0 - self.uniform(); // (0, 1]
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Distribution of one link's actual dimension, canonical mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Distribution {
    Normal {
        mean: f64,
        sd: f64,
    },
    /// Flat on [lo, hi].
    Uniform {
        lo: f64,
        hi: f64,
    },
    /// Triangular on [lo, hi] with peak at `mode`.
    Triangular {
        lo: f64,
        mode: f64,
        hi: f64,
    },
    /// X = shift + exp(mu + sigma·Z) (right-skewed), or X = shift − exp(mu + sigma·Z) when `left_skewed`.
    ShiftedLogNormal {
        shift: f64,
        mu: f64,
        sigma: f64,
        left_skewed: bool,
    },
    /// A fitted Johnson distribution (see [`JohnsonFit`]).
    Johnson(JohnsonFit),
}

impl Distribution {
    /// Parameters finite and ordered.
    pub fn validate(&self) -> TolResult<()> {
        let fin = |xs: &[f64]| xs.iter().all(|x| x.is_finite());
        match *self {
            Distribution::Normal { mean, sd } if fin(&[mean, sd]) && sd >= 0.0 => Ok(()),
            Distribution::Normal { .. } => invalid("normal: mean and sd must be finite, sd ≥ 0"),
            Distribution::Uniform { lo, hi } if fin(&[lo, hi]) && lo <= hi => Ok(()),
            Distribution::Uniform { .. } => invalid("uniform: lo ≤ hi, both finite"),
            Distribution::Triangular { lo, mode, hi }
                if fin(&[lo, mode, hi]) && lo <= mode && mode <= hi && lo < hi =>
            {
                Ok(())
            }
            Distribution::Triangular { .. } => invalid("triangular: lo ≤ mode ≤ hi, lo < hi, all finite"),
            Distribution::ShiftedLogNormal { shift, mu, sigma, .. } if fin(&[shift, mu, sigma]) && sigma > 0.0 => {
                Ok(())
            }
            Distribution::ShiftedLogNormal { .. } => invalid("lognormal: shift, mu, sigma finite and sigma > 0"),
            Distribution::Johnson(j) => j.validate(),
        }
    }

    /// One draw. Call [`Distribution::validate`] first; invalid parameters give NaN or meaningless values.
    pub fn sample(&self, rng: &mut SplitMix64) -> f64 {
        match *self {
            Distribution::Normal { mean, sd } => mean + sd * rng.normal(),
            Distribution::Uniform { lo, hi } => lo + (hi - lo) * rng.uniform(),
            Distribution::Triangular { lo, mode, hi } => {
                // Inverse CDF.
                let u = rng.uniform();
                let f = (mode - lo) / (hi - lo);
                if u < f {
                    lo + (u * (hi - lo) * (mode - lo)).sqrt()
                } else {
                    hi - ((1.0 - u) * (hi - lo) * (hi - mode)).sqrt()
                }
            }
            Distribution::ShiftedLogNormal {
                shift,
                mu,
                sigma,
                left_skewed,
            } => {
                let e = (mu + sigma * rng.normal()).exp();
                if left_skewed { shift - e } else { shift + e }
            }
            Distribution::Johnson(j) => j.transform(rng.normal()),
        }
    }
}

/// Johnson family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JohnsonFamily {
    /// Unbounded: Z = γ + δ·asinh((X − ξ)/λ).
    Su,
    /// Bounded on (ξ, ξ + λ): Z = γ + δ·ln(y/(1 − y)), y = (X − ξ)/λ.
    Sb,
    /// Lognormal: Z = γ + δ·ln((X − ξ)/λ) with λ = ±1 (λ = −1 is the left-skewed mirror image).
    Sl,
    /// Normal: Z = γ + δ·(X − ξ)/λ (the fit sets γ = 0, δ = 1, ξ = mean, λ = σ).
    Normal,
}

/// A Johnson distribution: X = ξ + λ·g((Z − γ)/δ) with Z standard normal and g = sinh (SU), logistic (SB),
/// exp (SL) or identity (normal).
///
/// Fitting ([`JohnsonFit::from_data`]) follows Slifker & Shapiro (1980). Pick z > 0, read the sample quantiles at
/// the probabilities Φ(−3z), Φ(−z), Φ(z), Φ(3z), and form m = x₃z − x_z, n = x₋z − x₋₃z, p = x_z − x₋z.
/// The quantile ratio mn/p² selects the family (> 1: SU, < 1: SB, = 1: SL; SL with m = n = p is the normal) and
/// the parameters follow in closed form (equations in the paper, reproduced in the code below). The fit
/// reproduces those four quantiles exactly. The default z = 0.524 (3z = 1.572: quantiles at 5.8 % / 94.2 %) is
/// the value commonly used with this method; Slifker & Shapiro advise z ≈ 0.5 for moderate samples so that the
/// ±3z quantiles stay inside the data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JohnsonFit {
    pub family: JohnsonFamily,
    pub gamma: f64,
    pub delta: f64,
    pub xi: f64,
    pub lambda: f64,
}

/// |mn/p² − 1| below this is treated as exactly 1 (SL or normal).
const SL_BAND: f64 = 1e-6;

impl JohnsonFit {
    /// Fit with the default z = 0.524.
    pub fn from_data(data: &[f64]) -> TolResult<Self> {
        Self::from_data_with_z(data, 0.524)
    }

    /// Fit with a chosen z (0 < z, and Φ(−3z)·len ≥ 1 so the outer quantiles are inside the data).
    pub fn from_data_with_z(data: &[f64], z: f64) -> TolResult<Self> {
        if !(z.is_finite() && z > 0.0) {
            return invalid("Johnson fit: z must be finite and > 0");
        }
        if data.len() < 10 || data.iter().any(|x| !x.is_finite()) {
            return invalid("Johnson fit: need at least 10 finite values");
        }
        let p3 = crate::normal::cdf(-3.0 * z);
        if p3 * (data.len() as f64) < 1.0 {
            return invalid(format!(
                "Johnson fit: {} values are too few for z = {z} (need ≥ {:.0})",
                data.len(),
                (1.0 / p3).ceil()
            ));
        }
        let mut s = data.to_vec();
        s.sort_by(f64::total_cmp);
        let q = |pr: f64| quantile_sorted(&s, pr);
        let p1 = crate::normal::cdf(-z);
        Self::from_quantiles([q(p3), q(p1), q(1.0 - p1), q(1.0 - p3)], z)
    }

    /// Fit from the four quantiles `[x₋₃z, x₋z, x_z, x₃z]` (Slifker & Shapiro 1980).
    pub fn from_quantiles(x: [f64; 4], z: f64) -> TolResult<Self> {
        let [xm3, xm1, x1, x3] = x;
        if !(x.iter().all(|v| v.is_finite()) && z.is_finite() && z > 0.0) {
            return invalid("Johnson fit: quantiles and z must be finite, z > 0");
        }
        let (m, n, p) = (x3 - x1, xm1 - xm3, x1 - xm1);
        if !(m > 0.0 && n > 0.0 && p > 0.0) {
            return invalid("Johnson fit: quantiles must be strictly increasing (data too discrete or constant)");
        }
        let mid = (x1 + xm1) / 2.0;
        let qr = m * n / (p * p);
        let fit = if (qr - 1.0).abs() <= SL_BAND {
            let (mp, np) = (m / p, n / p);
            if (mp - 1.0).abs() <= SL_BAND && (np - 1.0).abs() <= SL_BAND {
                JohnsonFit {
                    family: JohnsonFamily::Normal,
                    gamma: 0.0,
                    delta: 1.0,
                    xi: mid,
                    lambda: p / (2.0 * z),
                }
            } else {
                // Right-skewed (m > n) directly; left-skewed by mirroring x → −x (swaps m and n).
                let (r, sign) = if m > n { (mp, 1.0) } else { (np, -1.0) };
                let delta = 2.0 * z / r.ln();
                let gamma = delta * ((r - 1.0) / (p * r.sqrt())).ln();
                let xi_m = sign * mid - (p / 2.0) * (r + 1.0) / (r - 1.0);
                JohnsonFit {
                    family: JohnsonFamily::Sl,
                    gamma,
                    delta,
                    xi: sign * xi_m,
                    lambda: sign,
                }
            }
        } else if qr > 1.0 {
            let (mp, np) = (m / p, n / p);
            let delta = 2.0 * z / ((mp + np) / 2.0).acosh();
            let gamma = delta * ((np - mp) / (2.0 * (mp * np - 1.0).sqrt())).asinh();
            let lambda = 2.0 * p * (mp * np - 1.0).sqrt() / ((mp + np - 2.0) * (mp + np + 2.0).sqrt());
            let xi = mid + p * (np - mp) / (2.0 * (mp + np - 2.0));
            JohnsonFit {
                family: JohnsonFamily::Su,
                gamma,
                delta,
                xi,
                lambda,
            }
        } else {
            let (pm, pn) = (p / m, p / n);
            let prod = (1.0 + pm) * (1.0 + pn);
            let d = pm * pn - 1.0;
            let delta = z / (0.5 * prod.sqrt()).acosh();
            let gamma = delta * ((pn - pm) * (prod - 4.0).sqrt() / (2.0 * d)).asinh();
            let lambda = p * ((prod - 2.0).powi(2) - 4.0).sqrt() / d;
            let xi = mid - lambda / 2.0 + p * (pn - pm) / (2.0 * d);
            JohnsonFit {
                family: JohnsonFamily::Sb,
                gamma,
                delta,
                xi,
                lambda,
            }
        };
        fit.validate()?;
        Ok(fit)
    }

    /// X for a standard-normal value z: ξ + λ·g((z − γ)/δ). Increasing in z except for the mirrored SL (λ = −1).
    pub fn transform(&self, z: f64) -> f64 {
        let u = (z - self.gamma) / self.delta;
        let g = match self.family {
            JohnsonFamily::Su => u.sinh(),
            JohnsonFamily::Sb => 1.0 / (1.0 + (-u).exp()),
            JohnsonFamily::Sl => u.exp(),
            JohnsonFamily::Normal => u,
        };
        self.xi + self.lambda * g
    }

    fn validate(&self) -> TolResult<()> {
        let ok = [self.gamma, self.delta, self.xi, self.lambda]
            .iter()
            .all(|v| v.is_finite())
            && self.delta > 0.0
            && match self.family {
                JohnsonFamily::Sl => self.lambda == 1.0 || self.lambda == -1.0,
                _ => self.lambda > 0.0,
            };
        if ok {
            Ok(())
        } else {
            invalid(format!("Johnson parameters out of range: {self:?}"))
        }
    }
}

/// Empirical quantile of sorted data, linear interpolation between order statistics (Hyndman & Fan type 7).
pub fn quantile_sorted(sorted: &[f64], prob: f64) -> f64 {
    match sorted.len() {
        0 => f64::NAN,
        1 => sorted[0],
        len => {
            let h = (len - 1) as f64 * prob.clamp(0.0, 1.0);
            let i = (h.floor() as usize).min(len - 2);
            sorted[i] + (h - i as f64) * (sorted[i + 1] - sorted[i])
        }
    }
}

/// Pearson curve type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PearsonType {
    /// β₁ = 0, β₂ = 3.
    Normal,
    I,
    II,
    III,
    IV,
    V,
    VI,
    VII,
    /// β₂ < β₁ + 1: no distribution has these moments.
    Impossible,
}

/// Moment diagnostic of a sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PearsonDiagnostic {
    pub n: usize,
    pub mean: f64,
    /// Population (biased) standard deviation √m₂.
    pub sd: f64,
    /// β₁ = m₃²/m₂³ (squared skewness).
    pub beta1: f64,
    /// β₂ = m₄/m₂² (kurtosis, 3 for the normal).
    pub beta2: f64,
    /// κ = β₁(β₂ + 3)² / (4(4β₂ − 3β₁)(2β₂ − 3β₁ − 6)); infinite on the type III line.
    pub kappa: f64,
    pub kind: PearsonType,
}

impl PearsonDiagnostic {
    /// Central moments (biased mᵢ) and the type. "β₁ = 0" and "β₂ = 3" are tested within two large-sample standard
    /// errors (√β₁ within 2√(6/n), β₂ − 3 within 2√(24/n)), so a sample of a normal reads as normal rather than as
    /// a spurious type IV or VI. Report-only: sampling uses [`JohnsonFit`].
    pub fn from_data(data: &[f64]) -> TolResult<Self> {
        if data.len() < 4 || data.iter().any(|x| !x.is_finite()) {
            return invalid("Pearson diagnostic: need at least 4 finite values");
        }
        let nf = data.len() as f64;
        let mean = data.iter().sum::<f64>() / nf;
        let (mut m2, mut m3, mut m4) = (0.0, 0.0, 0.0);
        for &x in data {
            let d = x - mean;
            m2 += d * d;
            m3 += d * d * d;
            m4 += d * d * d * d;
        }
        let (m2, m3, m4) = (m2 / nf, m3 / nf, m4 / nf);
        if m2 <= 0.0 {
            return invalid("Pearson diagnostic: data are constant");
        }
        let beta1 = m3 * m3 / (m2 * m2 * m2);
        let beta2 = m4 / (m2 * m2);
        Ok(PearsonDiagnostic {
            n: data.len(),
            mean,
            sd: m2.sqrt(),
            beta1,
            beta2,
            kappa: pearson_kappa(beta1, beta2),
            kind: pearson_type(beta1, beta2, 2.0 * (6.0 / nf).sqrt(), 2.0 * (24.0 / nf).sqrt()),
        })
    }
}

/// κ = β₁(β₂ + 3)² / (4(4β₂ − 3β₁)(2β₂ − 3β₁ − 6)).
pub fn pearson_kappa(beta1: f64, beta2: f64) -> f64 {
    beta1 * (beta2 + 3.0).powi(2) / (4.0 * (4.0 * beta2 - 3.0 * beta1) * (2.0 * beta2 - 3.0 * beta1 - 6.0))
}

/// Pearson type from (β₁, β₂). `skew_tol` bounds √β₁ for "symmetric"; `kurt_tol` bounds |β₂ − 3| for "normal" and
/// |2β₂ − 3β₁ − 6| for the type III line. Pass 0 for exact theoretical moments. Rules (Elderton & Johnson):
/// symmetric → normal / II (β₂ < 3) / VII (β₂ > 3); otherwise III on the line, κ < 0 → I, 0 < κ < 1 → IV,
/// κ = 1 → V, κ > 1 → VI.
pub fn pearson_type(beta1: f64, beta2: f64, skew_tol: f64, kurt_tol: f64) -> PearsonType {
    if beta2 < beta1 + 1.0 {
        return PearsonType::Impossible;
    }
    if beta1.sqrt() <= skew_tol {
        return if (beta2 - 3.0).abs() <= kurt_tol {
            PearsonType::Normal
        } else if beta2 < 3.0 {
            PearsonType::II
        } else {
            PearsonType::VII
        };
    }
    if (2.0 * beta2 - 3.0 * beta1 - 6.0).abs() <= kurt_tol {
        return PearsonType::III;
    }
    let k = pearson_kappa(beta1, beta2);
    if k < 0.0 {
        PearsonType::I
    } else if (k - 1.0).abs() <= 1e-9 {
        PearsonType::V
    } else if k < 1.0 {
        PearsonType::IV
    } else {
        PearsonType::VI
    }
}

/// Wilson score interval for `successes` out of `n` trials at normal quantile `z` (n ≥ 1).
pub fn wilson_ci(successes: usize, n: usize, z: f64) -> (f64, f64) {
    let nf = n.max(1) as f64;
    let p = successes.min(n) as f64 / nf;
    let z2 = z * z;
    let denom = 1.0 + z2 / nf;
    let centre = (p + z2 / (2.0 * nf)) / denom;
    let half = z / denom * (p * (1.0 - p) / nf + z2 / (4.0 * nf * nf)).sqrt();
    // Clamp rounding so the interval always contains p and stays inside [0, 1].
    ((centre - half).clamp(0.0, p), (centre + half).clamp(p, 1.0))
}

/// Monte Carlo result. Dimensional values in canonical mm.
#[derive(Debug, Clone, PartialEq)]
pub struct McReport {
    pub n: usize,
    pub seed: u64,
    /// Assemblies inside the spec.
    pub passed: usize,
    /// passed / n.
    pub yield_estimate: f64,
    /// Wilson score 95 % interval on the yield (Monte Carlo error only).
    pub yield_ci95: (f64, f64),
    /// (1 − yield) · 10⁶.
    pub ppm_out: f64,
    pub mean: f64,
    /// Sample standard deviation (n − 1; 0 for n = 1).
    pub sd: f64,
    /// Empirical 0.135 % quantile of the gap (−3σ equivalent).
    pub q_lower: f64,
    pub median: f64,
    /// Empirical 99.865 % quantile of the gap (+3σ equivalent).
    pub q_upper: f64,
}

/// Monte Carlo stack-up with [`StackOptions::default`] for links without a distribution.
pub fn monte_carlo(
    links: &[Link],
    dists: &[Option<Distribution>],
    assembly: &Spec,
    n: usize,
    seed: u64,
) -> TolResult<McReport> {
    monte_carlo_with(links, dists, assembly, n, seed, &StackOptions::default())
}

/// Monte Carlo stack-up. `dists[i]` is the distribution of link i; `None` uses the normal model `analyze()`
/// assumes (process σ / Cpk, else centred with σ = T/(6·`opts.default_cp`)). Each draw takes the links in order.
pub fn monte_carlo_with(
    links: &[Link],
    dists: &[Option<Distribution>],
    assembly: &Spec,
    n: usize,
    seed: u64,
    opts: &StackOptions,
) -> TolResult<McReport> {
    if links.is_empty() {
        return invalid("a stack needs at least one link");
    }
    if dists.len() != links.len() {
        return invalid(format!("{} distributions for {} links", dists.len(), links.len()));
    }
    if n == 0 || n > MAX_SAMPLES {
        return invalid(format!("sample count must be in 1..={MAX_SAMPLES}"));
    }
    if assembly.dimension != Dimension::Length {
        return invalid("assembly spec must be a length");
    }
    if !(opts.default_cp.is_finite() && opts.default_cp > 0.0) {
        return invalid("options: default_cp must be finite and > 0");
    }
    let mut model = Vec::with_capacity(links.len());
    for (l, d) in links.iter().zip(dists) {
        if l.spec.dimension != Dimension::Length {
            return invalid(format!("link {:?} is not a length", l.name));
        }
        let dist = match d {
            Some(d) => {
                d.validate()
                    .map_err(|e| crate::TolError::Invalid(format!("link {:?}: {e}", l.name)))?;
                *d
            }
            None => {
                let (_, _, mean, sd) = link_model(l, opts.default_cp)?;
                Distribution::Normal { mean, sd }
            }
        };
        let sign = if l.direction == Direction::Plus { 1.0 } else { -1.0 };
        model.push((sign, dist));
    }
    let limit = |b: Option<Bound>| b.map(|b| (b.value.to_f64().unwrap_or(f64::NAN), b.inclusive));
    let (lsl, usl) = (limit(assembly.lower), limit(assembly.upper));
    if lsl.is_some_and(|l| l.0.is_nan()) || usl.is_some_and(|u| u.0.is_nan()) {
        return invalid("assembly limit is not representable as f64");
    }
    let inside = |g: f64| {
        lsl.is_none_or(|(l, inc)| if inc { g >= l } else { g > l })
            && usl.is_none_or(|(u, inc)| if inc { g <= u } else { g < u })
    };

    let mut rng = SplitMix64::new(seed);
    let mut gaps = Vec::with_capacity(n);
    let mut passed = 0usize;
    for _ in 0..n {
        let g: f64 = model.iter().map(|(s, d)| s * d.sample(&mut rng)).sum();
        if inside(g) {
            passed += 1;
        }
        gaps.push(g);
    }
    let nf = n as f64;
    let mean = gaps.iter().sum::<f64>() / nf;
    let sd = if n > 1 {
        (gaps.iter().map(|g| (g - mean) * (g - mean)).sum::<f64>() / (nf - 1.0)).sqrt()
    } else {
        0.0
    };
    gaps.sort_by(f64::total_cmp);
    let yield_estimate = passed as f64 / nf;
    Ok(McReport {
        n,
        seed,
        passed,
        yield_estimate,
        yield_ci95: wilson_ci(passed, n, Z95),
        ppm_out: (1.0 - yield_estimate) * 1e6,
        mean,
        sd,
        q_lower: quantile_sorted(&gaps, 0.001_35),
        median: quantile_sorted(&gaps, 0.5),
        q_upper: quantile_sorted(&gaps, 0.998_65),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Unit;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    fn link(nom: &str, tol: &str, dir: Direction) -> Link {
        Link {
            name: format!("{nom}±{tol}"),
            direction: dir,
            spec: Spec::symmetric(d(nom), d(tol), Unit::Mm).expect("spec"),
            process: None,
        }
    }

    #[test]
    fn splitmix_matches_reference_stream() {
        // First outputs of SplitMix64 seeded with 0 (Vigna's reference splitmix64.c).
        let mut r = SplitMix64::new(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn two_uniform_links_give_exact_triangular_yield() {
        // U(9.9,10.1) + U(9.9,10.1): triangular on [19.8, 20.2]; P(|G − 20| ≤ 0.16) = 1 − 2·0.04²/(2·0.2²) = 0.96.
        let links = [link("10", "0.1", Direction::Plus), link("10", "0.1", Direction::Plus)];
        let u = Some(Distribution::Uniform { lo: 9.9, hi: 10.1 });
        let gap = Spec::symmetric(d("20"), d("0.16"), Unit::Mm).expect("gap");
        let r = monte_carlo(&links, &[u, u], &gap, 200_000, 42).expect("mc");
        assert!(r.yield_ci95.0 <= 0.96 && 0.96 <= r.yield_ci95.1, "{r:?}");
        assert!((r.yield_estimate - 0.96).abs() < 0.002);
        // Mean 20, sd = 0.2/√12 · √2 = 0.0816497.
        assert!(
            (r.mean - 20.0).abs() < 1e-3 && (r.sd - 0.081_649_658).abs() < 1e-3,
            "{r:?}"
        );
        assert!(r.q_lower >= 19.8 && r.q_upper <= 20.2);
        // Minus direction: 10 − U gives the mirrored gap.
        let links = [link("10", "0.1", Direction::Plus), link("10", "0.1", Direction::Minus)];
        let gap = Spec::symmetric(d("0"), d("0.16"), Unit::Mm).expect("gap");
        let r = monte_carlo(&links, &[u, u], &gap, 200_000, 43).expect("mc");
        assert!(r.yield_ci95.0 <= 0.96 && 0.96 <= r.yield_ci95.1, "{r:?}");
    }

    #[test]
    fn triangular_sampler_has_the_right_mean_and_bounds() {
        let t = Distribution::Triangular {
            lo: 1.0,
            mode: 1.2,
            hi: 2.0,
        };
        let mut rng = SplitMix64::new(9);
        let xs: Vec<f64> = (0..200_000).map(|_| t.sample(&mut rng)).collect();
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        // Mean (lo + mode + hi)/3 = 1.4; sd = √((1 + 1.44 + 4 − 1.2 − 2 − 2.4)/18) = √(0.84/18) = 0.216; SE 4.8e-4.
        assert!((mean - 1.4).abs() < 0.003, "{mean}");
        assert!(xs.iter().all(|&x| (1.0..=2.0).contains(&x)));
    }

    #[test]
    fn fallback_is_analyze_normal_and_inputs_are_validated() {
        let links = [link("10", "0.1", Direction::Plus)];
        let gap = Spec::symmetric(d("10"), d("0.1"), Unit::Mm).expect("gap");
        // σ = 0.1/3 → yield = 2Φ(3) − 1 = 0.9973.
        let r = monte_carlo(&links, &[None], &gap, 100_000, 1).expect("mc");
        // A 95 % CI misses by construction 1 time in 20; check against the z = 4.4 interval (miss ≈ 1e-5).
        let (lo, hi) = wilson_ci(r.passed, r.n, 4.4);
        assert!(lo <= 0.997_300_2 && 0.997_300_2 <= hi, "{r:?}");
        assert!(monte_carlo(&links, &[], &gap, 10, 1).is_err());
        assert!(monte_carlo(&links, &[None], &gap, 0, 1).is_err());
        assert!(monte_carlo(&[], &[], &gap, 10, 1).is_err());
        for bad in [
            Distribution::Uniform { lo: 2.0, hi: 1.0 },
            Distribution::Normal {
                mean: f64::NAN,
                sd: 1.0,
            },
            Distribution::Normal { mean: 0.0, sd: -1.0 },
            Distribution::Triangular {
                lo: 0.0,
                mode: 2.0,
                hi: 1.0,
            },
            Distribution::ShiftedLogNormal {
                shift: 0.0,
                mu: 0.0,
                sigma: 0.0,
                left_skewed: false,
            },
            Distribution::Johnson(JohnsonFit {
                family: JohnsonFamily::Su,
                gamma: 0.0,
                delta: -1.0,
                xi: 0.0,
                lambda: 1.0,
            }),
        ] {
            assert!(monte_carlo(&links, &[Some(bad)], &gap, 10, 1).is_err(), "{bad:?}");
        }
        // n = 1 works.
        let one = monte_carlo(&links, &[None], &gap, 1, 1).expect("n = 1");
        assert_eq!(one.sd, 0.0);
    }

    #[test]
    fn wilson_hand_values() {
        // 0 of 10 at z = 1.96: upper = z²/(n + z²) = 3.8414588/13.8414588 = 0.277532.
        let (lo, hi) = wilson_ci(0, 10, Z95);
        assert_eq!(lo, 0.0);
        assert!((hi - 0.277_532_3).abs() < 1e-6, "{hi}");
        // 50 of 100: centre 0.5, half = 1.96/(1+0.0384146)·√(0.0025 + 0.0000960365) = 0.0961685.
        let (lo, hi) = wilson_ci(50, 100, Z95);
        assert!(
            (lo - 0.403_831_5).abs() < 1e-6 && (hi - 0.596_168_5).abs() < 1e-6,
            "{lo} {hi}"
        );
    }

    fn exact_quantiles(j: &JohnsonFit, z: f64) -> [f64; 4] {
        [
            j.transform(-3.0 * z),
            j.transform(-z),
            j.transform(z),
            j.transform(3.0 * z),
        ]
    }

    fn close(a: &JohnsonFit, b: &JohnsonFit, tol: f64) -> bool {
        a.family == b.family
            && (a.gamma - b.gamma).abs() <= tol * (1.0 + b.gamma.abs())
            && (a.delta - b.delta).abs() <= tol * b.delta
            && (a.xi - b.xi).abs() <= tol * (1.0 + b.xi.abs())
            && (a.lambda - b.lambda).abs() <= tol * b.lambda.abs()
    }

    #[test]
    fn johnson_exact_quantiles_recover_parameters() {
        let z = 0.524;
        let cases = [
            JohnsonFit {
                family: JohnsonFamily::Su,
                gamma: -1.2,
                delta: 1.5,
                xi: 10.0,
                lambda: 0.05,
            },
            JohnsonFit {
                family: JohnsonFamily::Su,
                gamma: 0.7,
                delta: 2.5,
                xi: -3.0,
                lambda: 2.0,
            },
            JohnsonFit {
                family: JohnsonFamily::Sb,
                gamma: 0.8,
                delta: 1.1,
                xi: 9.9,
                lambda: 0.3,
            },
            JohnsonFit {
                family: JohnsonFamily::Sb,
                gamma: -0.4,
                delta: 0.7,
                xi: 0.0,
                lambda: 1.0,
            },
            JohnsonFit {
                family: JohnsonFamily::Sl,
                gamma: 2.0,
                delta: 1.6,
                xi: 20.0,
                lambda: 1.0,
            },
            JohnsonFit {
                family: JohnsonFamily::Sl,
                gamma: 2.0,
                delta: 1.6,
                xi: 20.0,
                lambda: -1.0,
            },
            JohnsonFit {
                family: JohnsonFamily::Normal,
                gamma: 0.0,
                delta: 1.0,
                xi: 5.0,
                lambda: 0.02,
            },
        ];
        for truth in cases {
            let mut q = exact_quantiles(&truth, z);
            if truth.lambda < 0.0 {
                // The mirrored SL is decreasing in z: its Φ(kz) quantile is transform(−kz).
                q.reverse();
            }
            let fit = JohnsonFit::from_quantiles(q, z).expect("fit");
            assert!(close(&fit, &truth, 1e-6), "truth {truth:?}\nfit   {fit:?}");
        }
        assert!(JohnsonFit::from_quantiles([1.0, 1.0, 2.0, 3.0], z).is_err());
        assert!(JohnsonFit::from_data(&[1.0; 50]).is_err());
        assert!(JohnsonFit::from_data(&[1.0, 2.0, 3.0]).is_err());
    }

    #[test]
    fn johnson_fit_from_samples_recovers_su_and_sb() {
        let z = 0.524;
        let cases = [
            JohnsonFit {
                family: JohnsonFamily::Su,
                gamma: -1.2,
                delta: 1.5,
                xi: 10.0,
                lambda: 0.05,
            },
            JohnsonFit {
                family: JohnsonFamily::Sb,
                gamma: 0.8,
                delta: 1.1,
                xi: 9.9,
                lambda: 0.3,
            },
        ];
        for (k, truth) in cases.into_iter().enumerate() {
            let mut rng = SplitMix64::new(100 + k as u64);
            let xs: Vec<f64> = (0..200_000).map(|_| truth.transform(rng.normal())).collect();
            let fit = JohnsonFit::from_data_with_z(&xs, z).expect("fit");
            assert_eq!(fit.family, truth.family, "{fit:?}");
            // Compare the fitted and true quantile functions over ±3σ rather than raw parameters (γ, δ, λ trade
            // off against each other); the sample-quantile error at n = 2e5 is ~0.3 % of the spread.
            let spread = truth.transform(3.0) - truth.transform(-3.0);
            for zz in [-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0] {
                let err = (fit.transform(zz) - truth.transform(zz)).abs() / spread;
                assert!(err < 0.01, "family {:?} z {zz}: rel err {err}\n{fit:?}", truth.family);
            }
            assert!((fit.delta - truth.delta).abs() < 0.1 * truth.delta, "{fit:?}");
            assert!(
                (fit.gamma - truth.gamma).abs() < 0.1 + 0.1 * truth.gamma.abs(),
                "{fit:?}"
            );
        }
    }

    #[test]
    fn pearson_types_of_known_distributions() {
        let mut rng = SplitMix64::new(5);
        let normal: Vec<f64> = (0..20_000).map(|_| rng.normal()).collect();
        let uniform: Vec<f64> = (0..20_000).map(|_| rng.uniform()).collect();
        let n = PearsonDiagnostic::from_data(&normal).expect("n");
        let u = PearsonDiagnostic::from_data(&uniform).expect("u");
        assert_eq!(n.kind, PearsonType::Normal, "{n:?}");
        assert_eq!(u.kind, PearsonType::II, "{u:?}");
        assert!((u.beta2 - 1.8).abs() < 0.05, "{u:?}");
        // Exact theoretical moments: exponential (β₁ 4, β₂ 9) is on the type III line; Student t₅ (0, 9) is VII;
        // uniform (0, 1.8) is II; beta-like (0.5, 2.5) gives κ < 0 → I; β₂ < β₁ + 1 is impossible.
        assert_eq!(pearson_type(4.0, 9.0, 0.0, 0.0), PearsonType::III);
        assert_eq!(pearson_type(0.0, 9.0, 0.0, 0.0), PearsonType::VII);
        assert_eq!(pearson_type(0.0, 1.8, 0.0, 0.0), PearsonType::II);
        assert_eq!(pearson_type(0.0, 3.0, 0.0, 0.0), PearsonType::Normal);
        assert_eq!(pearson_type(0.5, 2.5, 0.0, 0.0), PearsonType::I);
        assert_eq!(pearson_type(1.0, 1.5, 0.0, 0.0), PearsonType::Impossible);
        // κ for (0.5, 4.5): 0.5·56.25 / (4·(18 − 1.5)·(9 − 1.5 − 6)) = 28.125/99 = 0.284 → IV.
        assert_eq!(pearson_type(0.5, 4.5, 0.0, 0.0), PearsonType::IV);
        // Lognormal σ = 0.5: β₁ = (w + 2)²(w − 1), β₂ = w⁴ + 2w³ + 3w² − 3, w = e^{0.25}: κ > 1 → VI.
        let w = 0.25_f64.exp();
        let (b1, b2) = (
            (w + 2.0).powi(2) * (w - 1.0),
            w.powi(4) + 2.0 * w.powi(3) + 3.0 * w * w - 3.0,
        );
        assert_eq!(
            pearson_type(b1, b2, 0.0, 0.0),
            PearsonType::VI,
            "κ = {}",
            pearson_kappa(b1, b2)
        );
    }
}
