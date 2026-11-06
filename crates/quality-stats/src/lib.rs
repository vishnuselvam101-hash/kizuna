//! Pure statistics for manufacturing quality evidence: statistical process control (SPC), process capability and
//! data-integrity ("was this table tampered with or fabricated?") tests.
//!
//! Design rules (see `docs/decisions/0019-quality-stats-api.md`):
//! - No I/O, no database, no global state, no randomness at run time. Every output is a pure function of the input
//!   (MinHash uses fixed, documented seeds), so results are reproducible and auditable.
//! - Measured values enter as decimal **strings** and are parsed exactly ([`Measurement`], backed by
//!   `rust_decimal`). Anything that depends on the written digits (terminal digits, rounding heaping, values exactly
//!   at a limit) uses the exact decimal. `f64` is used only inside the statistics, with the error bounds documented
//!   in [`special`] and [`describe`].
//! - Statistical tests return a p-value plus an explanation; a test that does not apply to the data (for example
//!   Benford on values that span less than two orders of magnitude) returns [`Applicability::NotApplicable`] with the
//!   reason instead of a misleading number.
//! - A small p-value is a reason for a human to look, never proof of fraud.

pub mod capability;
pub mod conformal;
pub mod decimal;
pub mod describe;
pub mod error;
pub mod integrity;
pub mod multitest;
pub mod spc;
pub mod special;
pub mod synth;

pub use decimal::Measurement;
pub use error::{StatsError, StatsResult};

/// Outcome of a test that may not be meaningful for the given data.
#[derive(Debug, Clone, PartialEq)]
pub enum Applicability<T> {
    /// The test applies; here is its result.
    Applicable(T),
    /// The test's assumptions do not hold for this data; the string says which and why.
    NotApplicable(String),
}

impl<T> Applicability<T> {
    /// The result, if the test applied.
    pub fn applicable(&self) -> Option<&T> {
        match self {
            Applicability::Applicable(t) => Some(t),
            Applicability::NotApplicable(_) => None,
        }
    }
}

/// Common shape of a hypothesis-test result.
#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    /// Test statistic (its meaning is documented on the producing function).
    pub statistic: f64,
    /// Degrees of freedom where the reference distribution has them, else 0.
    pub df: f64,
    /// One-sided or two-sided p-value as documented on the producing function, in [0, 1].
    pub p_value: f64,
    /// Number of values that entered the test.
    pub n: usize,
    /// Plain-language summary for reviewers.
    pub explanation: String,
}
