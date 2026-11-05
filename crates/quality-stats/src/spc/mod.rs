//! Statistical process control (Shewhart, EWMA, CUSUM charts and run rules).
//!
//! References: D. C. Montgomery, *Introduction to Statistical Quality Control* (ch. 6 Shewhart variables charts,
//! ch. 9 CUSUM and EWMA); ASTM E2587 *Standard Practice for Use of Control Charts*; Western Electric *Statistical
//! Quality Control Handbook* (1956); L. S. Nelson, "The Shewhart control chart — tests for special causes",
//! J. Quality Technology 16 (1984).

pub mod charts;
pub mod constants;
pub mod rules;
pub mod sequential;

pub use charts::{ChartLimits, IndividualsChart, SubgroupChart, individuals_moving_range, xbar_r, xbar_s};
pub use rules::{RuleSet, Violation, check_rules};
pub use sequential::{CusumChart, EwmaChart, cusum, ewma};
