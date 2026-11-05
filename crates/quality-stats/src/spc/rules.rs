//! Run rules for special causes on a control chart, in units of the plotted statistic's σ.
//!
//! Western Electric (1956): WE1 one point beyond 3σ; WE2 two of three consecutive beyond 2σ on the same side;
//! WE3 four of five beyond 1σ on the same side; WE4 eight consecutive on one side of the centre.
//! Nelson (1984): N1 beyond 3σ; N2 nine in a row on one side; N3 six in a row steadily increasing or decreasing;
//! N4 fourteen in a row alternating up and down; N5 two of three beyond 2σ same side; N6 four of five beyond 1σ
//! same side; N7 fifteen in a row within 1σ (either side; stratification — or values too good to be true);
//! N8 eight in a row beyond 1σ on either side (mixture).
//! A point exactly on the centre line is on neither side; a tie breaks a trend.
//! Each violation is reported at the index of the point that completes the pattern (so a long run reports several).

use crate::error::{StatsError, StatsResult, check_finite};

/// Which rule set to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSet {
    WesternElectric,
    Nelson,
}

/// One rule firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Index of the point that completes the pattern.
    pub index: usize,
    /// Rule label, e.g. "WE2" or "N7".
    pub rule: &'static str,
}

fn count_in_window(z: &[f64], end: usize, len: usize, pred: impl Fn(f64) -> bool) -> Option<usize> {
    (end + 1 >= len).then(|| z[end + 1 - len..=end].iter().filter(|v| pred(**v)).count())
}

fn all_in_window(z: &[f64], end: usize, len: usize, pred: impl Fn(f64) -> bool) -> bool {
    count_in_window(z, end, len, pred) == Some(len)
}

fn k_of_m_beyond(z: &[f64], i: usize, k: usize, m: usize, t: f64) -> bool {
    count_in_window(z, i, m, |v| v > t).is_some_and(|c| c >= k)
        || count_in_window(z, i, m, |v| v < -t).is_some_and(|c| c >= k)
}

fn run_one_side(z: &[f64], i: usize, len: usize) -> bool {
    all_in_window(z, i, len, |v| v > 0.0) || all_in_window(z, i, len, |v| v < 0.0)
}

/// Apply `set` to `points` given the chart centre and the σ of the plotted statistic.
/// Output is sorted by (index, rule) and deterministic.
pub fn check_rules(points: &[f64], center: f64, sigma: f64, set: RuleSet) -> StatsResult<Vec<Violation>> {
    check_finite(points, "points")?;
    if !center.is_finite() || !sigma.is_finite() {
        return Err(StatsError::NonFinite("center/sigma"));
    }
    if sigma <= 0.0 {
        return Err(StatsError::ZeroVariance("rules need sigma > 0"));
    }
    let z: Vec<f64> = points.iter().map(|p| (p - center) / sigma).collect();
    let d: Vec<f64> = points.windows(2).map(|w| w[1] - w[0]).collect();
    let mut out = vec![];
    for i in 0..z.len() {
        let mut fire = |rule| out.push(Violation { index: i, rule });
        let beyond3 = z[i].abs() > 3.0;
        let two_of_three = k_of_m_beyond(&z, i, 2, 3, 2.0);
        let four_of_five = k_of_m_beyond(&z, i, 4, 5, 1.0);
        match set {
            RuleSet::WesternElectric => {
                if beyond3 {
                    fire("WE1");
                }
                if two_of_three {
                    fire("WE2");
                }
                if four_of_five {
                    fire("WE3");
                }
                if run_one_side(&z, i, 8) {
                    fire("WE4");
                }
            }
            RuleSet::Nelson => {
                if beyond3 {
                    fire("N1");
                }
                if run_one_side(&z, i, 9) {
                    fire("N2");
                }
                // Six points ⇒ five consecutive differences of the same strict sign.
                if i >= 5 && (all_in_window(&d, i - 1, 5, |v| v > 0.0) || all_in_window(&d, i - 1, 5, |v| v < 0.0)) {
                    fire("N3");
                }
                // Fourteen points ⇒ thirteen differences alternating in strict sign.
                if i >= 13 && d[i - 13..i].windows(2).all(|w| w[0] * w[1] < 0.0) {
                    fire("N4");
                }
                if two_of_three {
                    fire("N5");
                }
                if four_of_five {
                    fire("N6");
                }
                if all_in_window(&z, i, 15, |v| v.abs() < 1.0) {
                    fire("N7");
                }
                if all_in_window(&z, i, 8, |v| v.abs() > 1.0) {
                    fire("N8");
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(points: &[f64], set: RuleSet) -> Vec<(usize, &'static str)> {
        check_rules(points, 0.0, 1.0, set)
            .unwrap()
            .into_iter()
            .map(|v| (v.index, v.rule))
            .collect()
    }

    #[test]
    fn each_rule_fires_on_its_pattern() {
        assert_eq!(rules(&[0.1, 3.5, -0.1], RuleSet::WesternElectric), vec![(1, "WE1")]);
        assert_eq!(rules(&[2.5, -0.1, 2.1], RuleSet::WesternElectric), vec![(2, "WE2")]);
        assert_eq!(
            rules(&[1.5, 1.2, -0.3, 1.1, 1.9], RuleSet::WesternElectric),
            vec![(4, "WE3")]
        );
        assert_eq!(rules(&[0.5; 8], RuleSet::WesternElectric), vec![(7, "WE4")]);
        assert_eq!(rules(&[-0.5; 9], RuleSet::Nelson), vec![(8, "N2")]);
        let trend = [-0.9, -0.6, -0.3, 0.05, -0.02, 0.0];
        assert!(rules(&trend, RuleSet::Nelson).is_empty());
        let trend = [-0.9, -0.6, -0.3, -0.1, 0.2, 0.5];
        assert_eq!(rules(&trend, RuleSet::Nelson), vec![(5, "N3")]);
        let alt: Vec<f64> = (0..14).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        assert_eq!(rules(&alt, RuleSet::Nelson), vec![(13, "N4")]);
        let hug: Vec<f64> = (0..15).map(|i| if i % 3 == 0 { 0.2 } else { -0.1 }).collect();
        assert_eq!(rules(&hug, RuleSet::Nelson), vec![(14, "N7")]);
        let mix = [1.5, -1.5, 1.2, -1.2, 1.4, -1.6, 1.3, -1.1];
        assert!(rules(&mix, RuleSet::Nelson).contains(&(7, "N8")));
    }

    #[test]
    fn center_point_breaks_a_run_and_bad_sigma_errors() {
        let mut v = vec![0.5; 8];
        v[4] = 0.0;
        assert!(rules(&v, RuleSet::WesternElectric).is_empty());
        assert!(check_rules(&[1.0], 0.0, 0.0, RuleSet::Nelson).is_err());
        assert!(check_rules(&[f64::NAN], 0.0, 1.0, RuleSet::Nelson).is_err());
        assert!(check_rules(&[], 0.0, 1.0, RuleSet::Nelson).unwrap().is_empty());
    }

    proptest::proptest! {
        #[test]
        fn scale_and_shift_invariant(xs in proptest::collection::vec(-4f64..4.0, 0..60), a in 0.1f64..10.0, b in -5f64..5.0) {
            let base = check_rules(&xs, 0.0, 1.0, RuleSet::Nelson).unwrap();
            let t: Vec<f64> = xs.iter().map(|x| a * x + b).collect();
            let moved = check_rules(&t, b, a, RuleSet::Nelson).unwrap();
            // Equal up to boundary rounding: compare only away from exact thresholds.
            let near = xs.iter().any(|x| [0.0f64, 1.0, 2.0, 3.0].iter().any(|t| (x.abs() - t).abs() < 1e-9));
            if !near { proptest::prop_assert_eq!(base, moved); }
        }
    }
}
