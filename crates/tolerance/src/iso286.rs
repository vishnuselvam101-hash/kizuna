//! ISO 286-1:2010 limits and fits for nominal sizes 0 < D ≤ 500 mm.
//!
//! Sources (published values, reproduced as numbers only):
//! * Standard tolerance grades IT4–IT13: ISO 286-1:2010 Table 1 (identical to ISO 286-1:1988 Table 1, JIS B 0401-1).
//! * Fundamental deviations of shafts d, e, f, g, h, js, k, m, n, p: ISO 286-1:2010 Table 2/3 (JIS B 0401-1 表 2/3).
//! * Holes D–H: EI = −es of the same-letter shaft (general rule, ISO 286-1 §A.3.2.1).
//! * Holes K, M, N (≤ IT8) and P (≤ IT7): special rule ES = −ei + Δ with Δ = ITn − IT(n−1) (ISO 286-1 §A.3.2.2,
//!   Table 3 "Δ values"); for D ≤ 3 mm Δ = 0. Above those grades: K, N → ES = 0, M, P → ES = −ei.
//! * js/JS: ±IT/2; for IT7–IT11 an odd IT value in µm is first replaced by the even value immediately below
//!   (ISO 286-1 Table 2 note), which is what the ISO 286-2 tables print (e.g. js7 for 18–30 mm = ±10 µm).
//!
//! Size ranges are "over a up to and including b" (ISO 286-1 Table 1). The intermediate sub-ranges ISO uses for
//! r–zc and some IT grades above IT13 are not needed for the letters covered here.

use rust_decimal::Decimal;
use std::fmt;

use crate::error::{TolResult, invalid, unsupported};

/// Upper bounds of the 13 main size ranges (mm).
const RANGES: [u16; 13] = [3, 6, 10, 18, 30, 50, 80, 120, 180, 250, 315, 400, 500];

/// IT4 … IT13 in µm (rows) per size range (columns).
const IT: [[u16; 13]; 10] = [
    [3, 4, 4, 5, 6, 7, 8, 10, 12, 14, 16, 18, 20],                     // IT4
    [4, 5, 6, 8, 9, 11, 13, 15, 18, 20, 23, 25, 27],                   // IT5
    [6, 8, 9, 11, 13, 16, 19, 22, 25, 29, 32, 36, 40],                 // IT6
    [10, 12, 15, 18, 21, 25, 30, 35, 40, 46, 52, 57, 63],              // IT7
    [14, 18, 22, 27, 33, 39, 46, 54, 63, 72, 81, 89, 97],              // IT8
    [25, 30, 36, 43, 52, 62, 74, 87, 100, 115, 130, 140, 155],         // IT9
    [40, 48, 58, 70, 84, 100, 120, 140, 160, 185, 210, 230, 250],      // IT10
    [60, 75, 90, 110, 130, 160, 190, 220, 250, 290, 320, 360, 400],    // IT11
    [100, 120, 150, 180, 210, 250, 300, 350, 400, 460, 520, 570, 630], // IT12
    [140, 180, 220, 270, 330, 390, 460, 540, 630, 720, 810, 890, 970], // IT13
];
const IT_MIN: u8 = 4;
const IT_MAX: u8 = 13;

// Shaft fundamental deviations (µm): upper deviation es for d–g, lower deviation ei for k–p.
const ES_D: [i16; 13] = [-20, -30, -40, -50, -65, -80, -100, -120, -145, -170, -190, -210, -230];
const ES_E: [i16; 13] = [-14, -20, -25, -32, -40, -50, -60, -72, -85, -100, -110, -125, -135];
const ES_F: [i16; 13] = [-6, -10, -13, -16, -20, -25, -30, -36, -43, -50, -56, -62, -68];
const ES_G: [i16; 13] = [-2, -4, -5, -6, -7, -9, -10, -12, -14, -15, -17, -18, -20];
/// k for IT4–IT7 (0 for other grades); also the K-hole special-rule term for ≤ IT8 (ISO 286-1 Table 3).
const EI_K: [i16; 13] = [0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4, 5];
const EI_M: [i16; 13] = [2, 4, 6, 7, 8, 9, 11, 13, 15, 17, 20, 21, 23];
const EI_N: [i16; 13] = [4, 8, 10, 12, 15, 17, 20, 23, 27, 31, 34, 37, 40];
const EI_P: [i16; 13] = [6, 12, 15, 18, 22, 26, 32, 37, 43, 50, 56, 62, 68];

/// Hole (internal feature, upper-case letters) or shaft (external feature, lower-case letters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Hole,
    Shaft,
}

/// Fundamental-deviation letters covered by the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Letter {
    D,
    E,
    F,
    G,
    H,
    Js,
    K,
    M,
    N,
    P,
}

/// A tolerance class such as `H7` or `g6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fit {
    pub feature: Feature,
    pub letter: Letter,
    pub grade: u8,
}

impl Fit {
    /// Parse `H7`, `g6`, `js6`, `JS7` (case decides hole vs shaft; mixed case such as `Js` is refused).
    pub fn parse(s: &str) -> TolResult<Fit> {
        let split = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
        let (letters, digits) = s.split_at(split);
        let feature = if !letters.is_empty() && letters.chars().all(|c| c.is_ascii_uppercase()) {
            Feature::Hole
        } else if !letters.is_empty() && letters.chars().all(|c| c.is_ascii_lowercase()) {
            Feature::Shaft
        } else {
            return invalid(format!("tolerance class {s:?} needs letters in one case"));
        };
        let letter = match letters.to_ascii_lowercase().as_str() {
            "d" => Letter::D,
            "e" => Letter::E,
            "f" => Letter::F,
            "g" => Letter::G,
            "h" => Letter::H,
            "js" => Letter::Js,
            "k" => Letter::K,
            "m" => Letter::M,
            "n" => Letter::N,
            "p" => Letter::P,
            other => {
                return unsupported(format!(
                    "fundamental deviation {other:?} is not in the tables (d–p only)"
                ));
            }
        };
        let grade: u8 = match digits.parse() {
            Ok(g) if !digits.is_empty() && digits.len() <= 2 => g,
            _ => return invalid(format!("tolerance class {s:?} has no IT grade")),
        };
        Ok(Fit { feature, letter, grade })
    }
}

impl fmt::Display for Fit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let l = match self.letter {
            Letter::D => "d",
            Letter::E => "e",
            Letter::F => "f",
            Letter::G => "g",
            Letter::H => "h",
            Letter::Js => "js",
            Letter::K => "k",
            Letter::M => "m",
            Letter::N => "n",
            Letter::P => "p",
        };
        match self.feature {
            Feature::Hole => write!(f, "{}{}", l.to_ascii_uppercase(), self.grade),
            Feature::Shaft => write!(f, "{l}{}", self.grade),
        }
    }
}

fn range_index(nominal_mm: Decimal) -> TolResult<usize> {
    if nominal_mm <= Decimal::ZERO {
        return invalid(format!("nominal size {nominal_mm} mm must be positive"));
    }
    RANGES
        .iter()
        .position(|&hi| nominal_mm <= Decimal::from(hi))
        .map_or_else(
            || {
                unsupported(format!(
                    "nominal size {nominal_mm} mm is above 500 mm (ISO 286-1 Table 1 extension not tabulated)"
                ))
            },
            Ok,
        )
}

fn it_um(grade: u8, r: usize) -> TolResult<i32> {
    if !(IT_MIN..=IT_MAX).contains(&grade) {
        return unsupported(format!("IT{grade} is outside the tabulated IT{IT_MIN}–IT{IT_MAX}"));
    }
    Ok(i32::from(IT[usize::from(grade - IT_MIN)][r]))
}

/// Standard tolerance ITn for a nominal size, in mm.
pub fn it_grade(nominal_mm: Decimal, grade: u8) -> TolResult<Decimal> {
    Ok(um(it_um(grade, range_index(nominal_mm)?)?))
}

fn um(v: i32) -> Decimal {
    Decimal::new(i64::from(v), 3)
}

/// Upper and lower deviation (mm, signed) of a tolerance class at a nominal size: `(es|ES, ei|EI)`.
pub fn deviations(nominal_mm: Decimal, fit: Fit) -> TolResult<(Decimal, Decimal)> {
    let r = range_index(nominal_mm)?;
    let g = fit.grade;
    let it = it_um(g, r)?;
    if fit.letter == Letter::Js {
        let even = if (7..=11).contains(&g) && it % 2 == 1 {
            it - 1
        } else {
            it
        };
        let half = Decimal::new(i64::from(even) * 5, 4); // even/2 µm in mm
        return Ok((half, -half));
    }
    // Shaft: (upper, lower) in µm.
    let shaft = |letter: Letter| -> (i32, i32) {
        let es = |t: &[i16; 13]| i32::from(t[r]);
        match letter {
            Letter::D => (es(&ES_D), es(&ES_D) - it),
            Letter::E => (es(&ES_E), es(&ES_E) - it),
            Letter::F => (es(&ES_F), es(&ES_F) - it),
            Letter::G => (es(&ES_G), es(&ES_G) - it),
            Letter::H => (0, -it),
            Letter::K => {
                let ei = if (4..=7).contains(&g) { es(&EI_K) } else { 0 };
                (ei + it, ei)
            }
            Letter::M => (es(&EI_M) + it, es(&EI_M)),
            Letter::N => (es(&EI_N) + it, es(&EI_N)),
            Letter::P => (es(&EI_P) + it, es(&EI_P)),
            Letter::Js => (0, 0), // handled above
        }
    };
    let (upper, lower) = match fit.feature {
        Feature::Shaft => shaft(fit.letter),
        Feature::Hole => {
            let delta = || -> TolResult<i32> { if r == 0 { Ok(0) } else { Ok(it - it_um(g - 1, r)?) } };
            let ei_of = |t: &[i16; 13]| i32::from(t[r]);
            let es_hole = match fit.letter {
                Letter::D | Letter::E | Letter::F | Letter::G | Letter::H => {
                    let ei = -shaft(fit.letter).0; // EI = −es
                    ei + it
                }
                Letter::K if g <= 8 => -ei_of(&EI_K) + delta()?,
                Letter::K => 0,
                Letter::M if g <= 8 => -ei_of(&EI_M) + delta()?,
                Letter::M => -ei_of(&EI_M),
                Letter::N if g <= 8 => -ei_of(&EI_N) + delta()?,
                Letter::N => 0,
                Letter::P if g <= 7 => -ei_of(&EI_P) + delta()?,
                Letter::P => -ei_of(&EI_P),
                Letter::Js => 0,
            };
            (es_hole, es_hole - it)
        }
    };
    Ok((um(upper), um(lower)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    fn dev(size: &str, class: &str) -> (i64, i64) {
        let (u, l) = deviations(d(size), Fit::parse(class).expect("class")).expect("deviations");
        let to_um = |x: Decimal| (x * Decimal::from(1000)).trunc().mantissa() as i64;
        (to_um(u), to_um(l))
    }

    #[test]
    fn published_25mm_values() {
        // ISO 286-2 tables (and every machinist's handbook) for 18 < D ≤ 30 mm, in µm.
        assert_eq!(dev("25", "H7"), (21, 0));
        assert_eq!(dev("25", "h7"), (0, -21));
        assert_eq!(dev("25", "g6"), (-7, -20));
        assert_eq!(dev("25", "f7"), (-20, -41));
        assert_eq!(dev("25", "k6"), (15, 2));
        assert_eq!(dev("25", "m6"), (21, 8));
        assert_eq!(dev("25", "n6"), (28, 15));
        assert_eq!(dev("25", "p6"), (35, 22));
        assert_eq!(dev("25", "K7"), (6, -15));
        assert_eq!(dev("25", "K6"), (2, -11));
        assert_eq!(dev("25", "M7"), (0, -21));
        assert_eq!(dev("25", "N7"), (-7, -28));
        assert_eq!(dev("25", "P7"), (-14, -35));
        assert_eq!(dev("25", "F8"), (53, 20));
        assert_eq!(dev("25", "js7"), (10, -10));
        assert_eq!(dev("25", "D10"), (149, 65));
    }

    #[test]
    fn other_sizes_and_boundaries() {
        assert_eq!(dev("50", "H7"), (25, 0)); // 30 < D ≤ 50
        assert_eq!(dev("50", "g6"), (-9, -25));
        assert_eq!(dev("10", "N7"), (-4, -19)); // 6 < D ≤ 10
        assert_eq!(dev("18", "H7"), (18, 0)); // 18 belongs to 10–18
        assert_eq!(dev("18.001", "H7"), (21, 0));
        assert_eq!(dev("3", "K7"), (0, -10)); // D ≤ 3: Δ = 0
        assert_eq!(dev("100", "h9"), (0, -87));
        let (u, l) = deviations(d("25"), Fit::parse("js6").expect("c")).expect("js6");
        assert_eq!((u, l), (d("0.0065"), d("-0.0065")));
    }

    #[test]
    fn out_of_table_is_an_error_not_a_guess() {
        assert!(deviations(d("501"), Fit::parse("H7").expect("c")).is_err());
        assert!(deviations(d("0"), Fit::parse("H7").expect("c")).is_err());
        assert!(deviations(d("25"), Fit::parse("H14").expect("c")).is_err());
        assert!(Fit::parse("x7").is_err());
        assert!(Fit::parse("Js7").is_err());
        assert!(Fit::parse("H").is_err());
    }

    #[test]
    fn it_table_follows_the_standard_tolerance_factor() {
        // ISO 286-1 §A.2: i = 0.45·∛D + 0.001·D (µm), D = geometric mean of the range; IT5..IT13 = 7,10,16,25,40,64,
        // 100,160,250 · i. Published values are rounded, so allow ±25 %; a transcription error would be far larger.
        let k = [7.0, 10.0, 16.0, 25.0, 40.0, 64.0, 100.0, 160.0, 250.0];
        let mut lo = 1.0_f64;
        for (r, &hi) in RANGES.iter().enumerate() {
            let dm = (lo * f64::from(hi)).sqrt();
            let i = 0.45 * dm.cbrt() + 0.001 * dm;
            for (gi, kk) in k.iter().enumerate() {
                let table = f64::from(IT[gi + 1][r]);
                let ratio = table / (kk * i);
                assert!(
                    (0.8..=1.25).contains(&ratio),
                    "IT{} range {r}: {table} vs {}",
                    gi + 5,
                    kk * i
                );
            }
            lo = f64::from(hi);
        }
    }
}
