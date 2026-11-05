//! Error type shared by the crate.

/// Why a statistic could not be computed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StatsError {
    /// Fewer values than the method needs.
    #[error("need at least {need} values, got {got}")]
    TooFewValues { need: usize, got: usize },
    /// The spread is zero where the method divides by it.
    #[error("zero variance: {0}")]
    ZeroVariance(&'static str),
    /// NaN or infinite input or parameter.
    #[error("non-finite value in {0}")]
    NonFinite(&'static str),
    /// A parameter is outside its valid range.
    #[error("invalid parameter: {0}")]
    InvalidParameter(&'static str),
    /// A decimal string could not be parsed exactly.
    #[error("cannot parse decimal {0:?}")]
    Parse(String),
}

/// Result alias.
pub type StatsResult<T> = Result<T, StatsError>;

/// Reject NaN/inf inputs (every public entry point taking `f64` slices calls this).
pub(crate) fn check_finite(xs: &[f64], what: &'static str) -> StatsResult<()> {
    if xs.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(StatsError::NonFinite(what))
    }
}

pub(crate) fn need(n: usize, min: usize) -> StatsResult<()> {
    if n >= min {
        Ok(())
    } else {
        Err(StatsError::TooFewValues { need: min, got: n })
    }
}
