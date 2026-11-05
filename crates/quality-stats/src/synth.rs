//! Deterministic synthetic data for tests and calibration (not used by the statistics themselves).
//!
//! [`Rng`] is SplitMix64 (Steele, Lea & Flood 2014): tiny, seedable, identical output on every platform. Normal
//! variates use the Box–Muller transform. Values are rendered to decimal strings with Rust's correctly rounded
//! `{:.N}` formatting and parsed back through [`Measurement::parse`], exercising the same path as real input.

use crate::decimal::Measurement;

/// SplitMix64 pseudo-random generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        crate::integrity::minhash::mix64(self.0)
    }

    /// Uniform in [0, 1) with 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform integer in 0..n (n ≥ 1; modulo bias < n/2^64).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// Standard normal (Box–Muller, one of the pair).
    pub fn normal(&mut self) -> f64 {
        let u1 = 1.0 - self.uniform(); // (0, 1]
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Render `x` with `decimals` places and parse it exactly.
pub fn record(x: f64, decimals: usize) -> Measurement {
    let s = format!("{x:.decimals$}");
    // `{:.N}` of a finite f64 is always a plain decimal; "-0.00" parses to zero.
    Measurement::parse(&s).unwrap_or_else(|_| Measurement::from_decimal(rust_decimal::Decimal::ZERO))
}

/// n honest measurements N(mean, sd²) recorded at `decimals`.
pub fn honest(rng: &mut Rng, n: usize, mean: f64, sd: f64, decimals: usize) -> Vec<Measurement> {
    (0..n).map(|_| record(mean + sd * rng.normal(), decimals)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_plausible() {
        let a: Vec<u64> = (0..3)
            .map({
                let mut r = Rng::new(1);
                move |_| r.next_u64()
            })
            .collect();
        let b: Vec<u64> = (0..3)
            .map({
                let mut r = Rng::new(1);
                move |_| r.next_u64()
            })
            .collect();
        assert_eq!(a, b);
        let mut r = Rng::new(42);
        let xs: Vec<f64> = (0..20_000).map(|_| r.normal()).collect();
        let s = crate::describe::summary(&xs).unwrap();
        assert!(s.mean.abs() < 0.03 && (s.sd() - 1.0).abs() < 0.03);
        assert_eq!(record(1.005, 2).scale(), 2);
    }
}
