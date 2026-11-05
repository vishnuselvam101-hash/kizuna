//! Dimensional tolerance engine for KIZUNA Verify (pure; no I/O, no database).
//!
//! * [`spec`] — nominal ± tolerances, limits, one-sided limits; exact decimals in canonical units ([`units`]).
//! * [`iso286`] — ISO 286-1 fits (IT4–IT13, deviations d–p / D–P, 0–500 mm); [`thread`] — ISO metric threads.
//! * [`parse`] — JA/EN drawing and certificate notations, full-width aware, never panics.
//! * [`conformance`] — ISO 14253-1 pass / fail / indeterminate with expanded uncertainty U.
//! * [`stack`] — 1-D stack-up: worst case, RSS, modified RSS (Bender), predicted yield from supplier capability.
//!
//! Design decisions: `docs/decisions/0019-tolerance-engine.md`.

pub mod conformance;
pub mod error;
pub mod iso286;
pub mod normal;
pub mod parse;
pub mod spec;
pub mod stack;
pub mod thread;
pub mod units;

pub use conformance::{Outcome, Verdict, judge};
pub use error::{TolError, TolResult};
pub use iso286::{Feature, Fit, Letter};
pub use parse::{Parsed, parse, parse_spec};
pub use spec::{Bound, Spec};
pub use stack::{Direction, Finding, Link, Process, StackOptions, StackReport, analyze};
pub use thread::ThreadSpec;
pub use units::{Dimension, Quantity, Unit};
