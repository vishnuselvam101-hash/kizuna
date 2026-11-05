//! Errors. Every failure is a value; nothing in this crate panics on caller input.

use thiserror::Error;

/// Result alias for this crate.
pub type TolResult<T> = Result<T, TolError>;

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TolError {
    /// The notation could not be read; the payload says where and why.
    #[error("cannot parse tolerance notation {text:?}: {why}")]
    Parse { text: String, why: String },
    /// Quantities of different dimensions (e.g. a length and a stress) were combined.
    #[error("dimension mismatch: {0}")]
    Dimension(String),
    /// A decimal operation overflowed or a unit conversion would not be exact.
    #[error("arithmetic: {0}")]
    Arithmetic(String),
    /// The input is well-formed but outside what the tables or rules in this crate cover.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The input contradicts itself (lower limit above upper, negative uncertainty, Cpk ≤ 0 …).
    #[error("invalid: {0}")]
    Invalid(String),
}

pub(crate) fn invalid<T>(msg: impl Into<String>) -> TolResult<T> {
    Err(TolError::Invalid(msg.into()))
}

pub(crate) fn unsupported<T>(msg: impl Into<String>) -> TolResult<T> {
    Err(TolError::Unsupported(msg.into()))
}
