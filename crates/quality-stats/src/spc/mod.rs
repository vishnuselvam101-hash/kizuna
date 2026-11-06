//! Statistical process control (Shewhart, EWMA, CUSUM charts and run rules; short-run Q/DNOM charts and Bayesian
//! online change-point detection, MATH v2).
//!
//! References: D. C. Montgomery, *Introduction to Statistical Quality Control* (ch. 6 Shewhart variables charts,
//! ch. 9 CUSUM and EWMA); ASTM E2587 *Standard Practice for Use of Control Charts*; Western Electric *Statistical
//! Quality Control Handbook* (1956); L. S. Nelson, "The Shewhart control chart — tests for special causes",
//! J. Quality Technology 16 (1984).

pub mod bocpd;
pub mod charts;
pub mod constants;
pub mod rules;
pub mod sequential;
pub mod short_run;

pub use bocpd::{BocpdStep, NigPrior, bocpd};
pub use charts::{ChartLimits, IndividualsChart, SubgroupChart, individuals_moving_range, xbar_r, xbar_s};
pub use rules::{RuleSet, Violation, check_rules};
pub use sequential::{CusumChart, EwmaChart, cusum, ewma};
pub use short_run::{QPoint, dnom, dnom_standardized, q_chart, q_chart_known_sigma};
