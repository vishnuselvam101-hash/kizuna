//! Data-integrity statistics: signs that recorded numbers were invented, edited, rounded by hand, clamped to a
//! limit or copied from another certificate.
//!
//! Every test answers "is this pattern unlikely for honestly recorded measurements?" with a p-value or a similarity,
//! and refuses (returns [`crate::Applicability::NotApplicable`]) when its assumptions do not hold. Calibration on
//! synthetic honest vs manipulated data is in `docs/testing/QUALITY_STATS_2026-11-05.md`. A flag is a reason for a
//! reviewer to ask for raw data — never a finding of fraud on its own.

pub mod benford;
pub mod digits;
pub mod limit;
pub mod minhash;
pub mod variance;

pub use benford::benford_first_digit;
pub use digits::{rounding_heaping, terminal_digit_preference};
pub use limit::{LimitSide, exact_at_limit};
pub use minhash::{MinHasher, NearDuplicate, Signature, find_near_duplicates, table_tokens};
pub use variance::variance_too_low;
