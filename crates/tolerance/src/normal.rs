//! Standard normal distribution (f64). Only the stack-up's statistical predictions use floating point.
//!
//! erfc is computed with its Maclaurin series of erf for x < 1.5 and with the classical continued fraction
//! erfc(x) = e^{−x²}/√π · 1/(x + ½/(x + 1/(x + 3⁄2/(x + …)))) (Abramowitz & Stegun 7.1.14), 120 terms, for x ≥ 1.5.
//! Checked against reference values in the tests to ≤ 1e-11 relative error.

const FRAC_1_SQRT_PI: f64 = 0.564_189_583_547_756_3;

fn erfc_pos(x: f64) -> f64 {
    if x < 1.5 {
        let (mut sum, mut term, mut n) = (0.0_f64, x, 0_u32);
        loop {
            let add = term / f64::from(2 * n + 1);
            sum += add;
            n += 1;
            term *= -x * x / f64::from(n);
            if add.abs() < 1e-17 * sum.abs() || n > 200 {
                break;
            }
        }
        1.0 - 2.0 * FRAC_1_SQRT_PI * sum
    } else {
        let mut t = x;
        for k in (1..=120).rev() {
            t = x + (f64::from(k) / 2.0) / t;
        }
        (-x * x).exp() * FRAC_1_SQRT_PI / t
    }
}

/// erfc(x) for any real x.
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        f64::NAN
    } else if x >= 0.0 {
        erfc_pos(x)
    } else {
        2.0 - erfc_pos(-x)
    }
}

/// Φ(z).
pub fn cdf(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

/// 1 − Φ(z), accurate in the upper tail.
pub fn sf(z: f64) -> f64 {
    0.5 * erfc(z / std::f64::consts::SQRT_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_values() {
        // Reference values: CPython math.erfc (correctly rounded libm) / standard normal tables.
        let cases = [
            (0.0, 0.5),
            (1.0, 0.841_344_746_068_542_9),
            (-1.0, 0.158_655_253_931_457_07),
            (-3.0, 0.001_349_898_031_630_095_7),
            (-6.0, 9.865_876_450_377_014e-10),
            (2.5, 0.993_790_334_674_223_8),
        ];
        for (z, p) in cases {
            assert!((cdf(z) - p).abs() <= 1e-11 * p, "Φ({z}) = {} ≠ {p}", cdf(z));
        }
        assert!((sf(6.0) - 9.865_876_450_377_014e-10).abs() < 1e-20);
        assert_eq!(cdf(f64::INFINITY), 1.0);
        assert_eq!(cdf(f64::NEG_INFINITY), 0.0);
    }
}
