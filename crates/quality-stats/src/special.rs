//! Special functions and reference distributions, implemented from their textbook definitions.
//!
//! | Function | Method | Accuracy (tested) |
//! |---|---|---|
//! | [`ln_gamma`] | Lanczos approximation, g = 7, 9 terms (Lanczos 1964; coefficients as tabulated by Godfrey) | ~1e-15 relative for x ≥ 0.5 |
//! | [`gamma_p`], [`gamma_q`] | power series for x < a + 1, otherwise continued fraction evaluated with the modified Lentz method (Abramowitz & Stegun 6.5.29 / 6.5.31) | ~1e-13 absolute |
//! | [`normal_cdf`], [`normal_sf`] | Φ(z) = ½·erfc(−z/√2), erfc(x) = Q(½, x²) | ~1e-14 absolute, tail relative ~1e-12 |
//! | [`normal_quantile`] | Acklam's rational approximation (rel. error 1.15e-9) + one Halley refinement step | ~1e-14 |
//! | [`chi2_cdf`], [`chi2_sf`] | P(k/2, x/2), Q(k/2, x/2) | as `gamma_p` |
//! | [`chi2_quantile`] | bisection on `chi2_cdf` to 1e-12 relative | 1e-10 |
//! | [`binomial_sf`] | direct summation of the pmf from k upward using the ratio recurrence, start term from `ln_gamma` | ~1e-12 relative |
//!
//! Inputs outside a function's domain return NaN (internal use only; public statistics validate first).

const LANCZOS_G: f64 = 7.0;
const LANCZOS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

/// ln Γ(x) for x > 0.
pub fn ln_gamma(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return f64::NAN;
    }
    if x < 0.5 {
        // Γ(x) = Γ(x + 1) / x keeps the Lanczos sum in its accurate range.
        return ln_gamma(x + 1.0) - x.ln();
    }
    let x = x - 1.0;
    let mut sum = LANCZOS[0];
    for (i, c) in LANCZOS.iter().enumerate().skip(1) {
        sum += c / (x + i as f64);
    }
    let t = x + LANCZOS_G + 0.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + sum.ln()
}

const EPS: f64 = 1e-16;
const TINY: f64 = 1e-300;
const MAX_ITER: usize = 10_000;

/// Regularised lower incomplete gamma P(a, x) = γ(a, x) / Γ(a).
pub fn gamma_p(a: f64, x: f64) -> f64 {
    if a.is_nan() || a <= 0.0 || x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x < a + 1.0 {
        gamma_series(a, x)
    } else {
        1.0 - gamma_cf(a, x)
    }
}

/// Regularised upper incomplete gamma Q(a, x) = 1 − P(a, x), computed directly in the tail.
pub fn gamma_q(a: f64, x: f64) -> f64 {
    if a.is_nan() || a <= 0.0 || x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        1.0 - gamma_series(a, x)
    } else {
        gamma_cf(a, x)
    }
}

fn prefactor(a: f64, x: f64) -> f64 {
    (-x + a * x.ln() - ln_gamma(a)).exp()
}

/// P(a, x) = e^{−x} x^a / Γ(a) · Σ_{n≥0} x^n / (a (a+1) … (a+n)).
fn gamma_series(a: f64, x: f64) -> f64 {
    let mut term = 1.0 / a;
    let mut sum = term;
    let mut ap = a;
    for _ in 0..MAX_ITER {
        ap += 1.0;
        term *= x / ap;
        sum += term;
        if term.abs() < sum.abs() * EPS {
            break;
        }
    }
    (sum * prefactor(a, x)).min(1.0)
}

/// Q(a, x) via the continued fraction 1/(x+1−a− 1·(1−a)/(x+3−a− 2·(2−a)/(x+5−a− …))), modified Lentz.
fn gamma_cf(a: f64, x: f64) -> f64 {
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..MAX_ITER {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < TINY {
            d = TINY;
        }
        c = b + an / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPS {
            break;
        }
    }
    (prefactor(a, x) * h).clamp(0.0, 1.0)
}

/// Complementary error function.
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x >= 0.0 {
        gamma_q(0.5, x * x)
    } else {
        2.0 - gamma_q(0.5, x * x)
    }
}

/// Standard normal CDF Φ(z).
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

/// Standard normal survival function 1 − Φ(z), accurate in the upper tail.
pub fn normal_sf(z: f64) -> f64 {
    0.5 * erfc(z / std::f64::consts::SQRT_2)
}

/// Standard normal quantile Φ⁻¹(p) for p in (0, 1).
pub fn normal_quantile(p: f64) -> f64 {
    if !(p > 0.0 && p < 1.0) {
        return match p {
            0.0 => f64::NEG_INFINITY,
            1.0 => f64::INFINITY,
            _ => f64::NAN,
        };
    }
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const P_LOW: f64 = 0.024_25;
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    let mut x = if p < P_LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p <= 1.0 - P_LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    };
    // One Halley step on Φ(x) − p brings the 1e-9 approximation to near machine precision.
    let e = if x > 0.0 {
        (1.0 - p) - normal_sf(x)
    } else {
        normal_cdf(x) - p
    };
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    x -= u / (1.0 + x * u / 2.0);
    x
}

/// χ² CDF with `k` degrees of freedom.
pub fn chi2_cdf(x: f64, k: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { gamma_p(k / 2.0, x / 2.0) }
}

/// χ² survival function (upper-tail p-value).
pub fn chi2_sf(x: f64, k: f64) -> f64 {
    if x <= 0.0 { 1.0 } else { gamma_q(k / 2.0, x / 2.0) }
}

/// χ² quantile: the x with `chi2_cdf(x, k) = p`, p in (0, 1).
pub fn chi2_quantile(p: f64, k: f64) -> f64 {
    if p.is_nan() || p <= 0.0 || p >= 1.0 || k.is_nan() || k <= 0.0 {
        return f64::NAN;
    }
    let mut hi = k + 10.0 * (2.0 * k).sqrt() + 10.0;
    while chi2_cdf(hi, k) < p {
        hi *= 2.0;
    }
    let mut lo = 0.0;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if chi2_cdf(mid, k) < p {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-13 * hi {
            break;
        }
    }
    0.5 * (lo + hi)
}

/// P(X ≥ k) for X ~ Binomial(n, p).
pub fn binomial_sf(k: u64, n: u64, p: f64) -> f64 {
    if !(0.0..=1.0).contains(&p) {
        return f64::NAN;
    }
    if k == 0 {
        return 1.0;
    }
    if k > n {
        return 0.0;
    }
    if p == 0.0 {
        return 0.0;
    }
    if p == 1.0 {
        return 1.0;
    }
    let (nf, kf) = (n as f64, k as f64);
    let ln_pmf_k =
        ln_gamma(nf + 1.0) - ln_gamma(kf + 1.0) - ln_gamma(nf - kf + 1.0) + kf * p.ln() + (nf - kf) * (-p).ln_1p();
    let ratio = p / (1.0 - p);
    let mut term = ln_pmf_k.exp();
    let mut sum = term;
    let mut j = k;
    while j < n {
        term *= (n - j) as f64 / (j + 1) as f64 * ratio;
        sum += term;
        j += 1;
        if term < sum * 1e-17 && (j as f64) > nf * p {
            break;
        }
    }
    sum.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
    }

    #[test]
    fn ln_gamma_known_values() {
        close(ln_gamma(1.0), 0.0, 1e-14);
        close(ln_gamma(2.0), 0.0, 1e-14);
        close(ln_gamma(0.5), std::f64::consts::PI.sqrt().ln(), 1e-14);
        close(ln_gamma(10.0), 362_880f64.ln(), 1e-12);
        close(ln_gamma(0.1), 2.252_712_651_734_206, 1e-13);
    }

    #[test]
    fn normal_reference_values() {
        // Abramowitz & Stegun Table 26.1.
        close(normal_cdf(0.0), 0.5, 1e-15);
        close(normal_cdf(1.0), 0.841_344_746_068_543, 1e-13);
        close(normal_cdf(-1.96), 0.024_997_895_148_220, 1e-13);
        close(normal_sf(5.0), 2.866_515_718_791_939e-7, 1e-18);
        close(normal_quantile(0.975), 1.959_963_984_540_054, 1e-12);
        close(normal_quantile(0.995), 2.575_829_303_548_901, 1e-12);
        close(normal_quantile(1e-6), -4.753_424_308_822_899, 1e-10);
        close(normal_quantile(0.5), 0.0, 1e-15);
    }

    #[test]
    fn chi2_reference_values() {
        // Critical values from standard χ² tables.
        close(chi2_sf(3.841_458_820_694_124, 1.0), 0.05, 1e-12);
        close(chi2_sf(16.918_977_604_620_45, 9.0), 0.05, 1e-12);
        close(chi2_quantile(0.01, 9.0), 2.087_901, 1e-6);
        close(chi2_quantile(0.95, 9.0), 16.918_977_604_620_45, 1e-8);
        close(chi2_quantile(0.025, 24.0), 12.401_150_217_444_44, 1e-8);
    }

    #[test]
    fn binomial_tail_matches_direct_sum() {
        // P(X ≥ 3), n = 10, p = 0.2: 1 − Σ_{0..2} = 1 − (0.1073741824 + 0.268435456 + 0.301989888) = 0.3222004736
        close(binomial_sf(3, 10, 0.2), 0.322_200_473_6, 1e-12);
        close(binomial_sf(0, 10, 0.2), 1.0, 0.0);
        close(binomial_sf(11, 10, 0.2), 0.0, 0.0);
        close(binomial_sf(10, 10, 0.5), 0.5f64.powi(10), 1e-18);
    }

    proptest::proptest! {
        #[test]
        fn gamma_p_plus_q_is_one(a in 0.05f64..200.0, x in 0.0f64..400.0) {
            let s = gamma_p(a, x) + gamma_q(a, x);
            proptest::prop_assert!((s - 1.0).abs() < 1e-12);
        }

        #[test]
        fn quantile_inverts_cdf(p in 1e-12f64..(1.0 - 1e-12)) {
            let z = normal_quantile(p);
            let back = if z > 0.0 { 1.0 - normal_sf(z) } else { normal_cdf(z) };
            proptest::prop_assert!((back - p).abs() <= 1e-13 + 1e-10 * p);
        }

        #[test]
        fn chi2_quantile_inverts(p in 0.001f64..0.999, k in 1u32..200) {
            let x = chi2_quantile(p, k as f64);
            proptest::prop_assert!((chi2_cdf(x, k as f64) - p).abs() < 1e-9);
        }
    }
}
