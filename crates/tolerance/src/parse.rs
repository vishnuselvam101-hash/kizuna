//! Parser for Japanese and English drawing / certificate notations.
//!
//! Accepted forms (after full-width → ASCII normalisation, so `１０±０．０５` reads like `10±0.05`):
//!
//! | form | example |
//! |---|---|
//! | symmetric | `10±0.05`, `10 +/- 0.05 mm`, `10mm ± 50µm` |
//! | deviations | `φ10 +0.1/−0`, `10 +0.1 -0.05`, `Ø10 0/-0.1` |
//! | ISO 286 class | `10 h7`, `φ25H7`, `25 js6` |
//! | limits | `9.9~10.1`, `9.9〜10.1 mm` |
//! | one-sided | `10 min`, `MAX 0.8`, `≥ 400 MPa`, `400MPa以上`, `0.5未満`, `10以上10.2以下` |
//! | thread | `M8×1.25 6g`, `M8x1.25-6g`, `M10` |
//!
//! Units: mm (default), µm/μm/um, in/inch/`"`, MPa, N/mm², GPa, kPa. Diameter marks φ Φ ø Ø ⌀ are ignored.
//! The parser never panics: every unexpected character is a `TolError::Parse` naming the position.

use rust_decimal::Decimal;
use std::str::FromStr;

use crate::error::{TolError, TolResult};
use crate::iso286::Fit;
use crate::spec::Spec;
use crate::thread::ThreadSpec;
use crate::units::Unit;

/// What a notation describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Spec(Spec),
    Thread(ThreadSpec),
}

/// Fold full-width forms, minus/tilde variants, micro signs and diameter marks into plain ASCII.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let m = match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            c if c.is_whitespace() => ' ',
            '\u{2212}' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{FE63}' => '-',
            '\u{301C}' | '\u{223C}' => '~',
            'φ' | 'Φ' | 'ø' | 'Ø' | '⌀' | '∅' => continue,
            'µ' | 'μ' => 'u',
            '²' => '2',
            '×' | '✕' => 'x',
            '≧' => '≥',
            '≦' => '≤',
            '”' | '″' => '"',
            c => c,
        };
        // Full-width ～ (FF5E) maps to '~' above; collapse runs of spaces.
        if m == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(m);
    }
    out.trim_end().to_string()
}

/// Parse a notation into a spec or a thread designation.
pub fn parse(text: &str) -> TolResult<Parsed> {
    let norm = normalize(text);
    let mut p = P {
        s: norm.chars().collect(),
        i: 0,
        text,
        nominal_unit: None,
    };
    let r = p.top()?;
    p.ws();
    if p.i < p.s.len() {
        return p.err(format!("unexpected {:?} at position {}", p.rest(), p.i));
    }
    Ok(r)
}

/// Parse into a spec; a thread becomes its major-diameter spec (6g/6h only).
pub fn parse_spec(text: &str) -> TolResult<Spec> {
    match parse(text)? {
        Parsed::Spec(s) => Ok(s),
        Parsed::Thread(t) => t.major_diameter_spec(),
    }
}

struct P<'a> {
    s: Vec<char>,
    i: usize,
    text: &'a str,
    nominal_unit: Option<Unit>,
}

enum Side {
    Min(bool),
    Max(bool),
}

impl P<'_> {
    fn err<T>(&self, why: impl Into<String>) -> TolResult<T> {
        Err(TolError::Parse {
            text: self.text.to_string(),
            why: why.into(),
        })
    }

    fn rest(&self) -> String {
        self.s.get(self.i..).map(|r| r.iter().collect()).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }

    /// Consume `word` (ASCII case-insensitive) if it is next; ASCII-letter words must end at a non-letter.
    fn eat(&mut self, word: &str) -> bool {
        let w: Vec<char> = word.chars().collect();
        let Some(slice) = self.s.get(self.i..self.i + w.len()) else {
            return false;
        };
        if !slice.iter().zip(&w).all(|(a, b)| a.to_ascii_lowercase() == *b) {
            return false;
        }
        let ends_word = w.last().is_some_and(|c| c.is_ascii_alphabetic());
        if ends_word && self.s.get(self.i + w.len()).is_some_and(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        self.i += w.len();
        true
    }

    fn number(&mut self, signed: bool) -> TolResult<Decimal> {
        let start = self.i;
        if signed && matches!(self.peek(), Some('+' | '-')) {
            self.i += 1;
        }
        let digits_from = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        let body: String = self
            .s
            .get(digits_from..self.i)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        let (int, frac) = body.split_once('.').unwrap_or((&body, ""));
        if !body.chars().any(|c| c.is_ascii_digit()) || frac.contains('.') || int.is_empty() && frac.is_empty() {
            self.i = start;
            return self.err(format!("expected a number at position {start}"));
        }
        let lit: String = self
            .s
            .get(start..self.i)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        Decimal::from_str(lit.trim_start_matches('+')).or_else(|_| self.err(format!("number {lit:?} is out of range")))
    }

    fn unit(&mut self) -> Option<Unit> {
        let save = self.i;
        self.ws();
        for (w, u) in [
            ("n/mm2", Unit::NPerMm2),
            ("mpa", Unit::MPa),
            ("gpa", Unit::GPa),
            ("kpa", Unit::KPa),
            ("mm", Unit::Mm),
            ("um", Unit::Um),
            ("inch", Unit::Inch),
            ("in", Unit::Inch),
            ("\"", Unit::Inch),
        ] {
            if self.eat(w) {
                return Some(u);
            }
        }
        self.i = save;
        None
    }

    fn side_suffix(&mut self) -> Option<Side> {
        self.ws();
        if self.eat("以上") || self.eat("min.") || self.eat("min") {
            Some(Side::Min(true))
        } else if self.eat("以下") || self.eat("max.") || self.eat("max") {
            Some(Side::Max(true))
        } else if self.eat("超") {
            Some(Side::Min(false))
        } else if self.eat("未満") {
            Some(Side::Max(false))
        } else {
            None
        }
    }

    fn top(&mut self) -> TolResult<Parsed> {
        self.ws();
        if self.peek() == Some('M') && self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
            return self.thread().map(Parsed::Thread);
        }
        // Prefix comparison: ≥ 400 MPa, MIN 10, < 0.5.
        let prefix = if self.eat("≥") || self.eat(">=") || self.eat("min") {
            Some(Side::Min(true))
        } else if self.eat("≤") || self.eat("<=") || self.eat("max") {
            Some(Side::Max(true))
        } else if self.eat(">") {
            Some(Side::Min(false))
        } else if self.eat("<") {
            Some(Side::Max(false))
        } else {
            None
        };
        if let Some(side) = prefix {
            self.ws();
            let v = self.number(true)?;
            let u = self.unit().unwrap_or(Unit::Mm);
            return one_sided(side, v, u).map(Parsed::Spec);
        }
        let n = self.number(true)?;
        let nu = self.unit();
        self.nominal_unit = nu;
        if let Some(side) = self.side_suffix() {
            let u = nu.unwrap_or(Unit::Mm);
            self.ws();
            if self.i >= self.s.len() {
                return one_sided(side, n, u).map(Parsed::Spec);
            }
            // 10以上10.2以下
            let n2 = self.number(true)?;
            let u2 = self.unit().unwrap_or(u);
            let side2 = match self.side_suffix() {
                Some(s) => s,
                None => return self.err("second limit needs 以上/以下/未満/超/min/max"),
            };
            return two_one_sided((side, n, u), (side2, n2, u2)).map(Parsed::Spec);
        }
        self.ws();
        let unit = nu.unwrap_or(Unit::Mm);
        match self.peek() {
            None => self.err("a nominal value alone has no tolerance or limit"),
            Some('±') => {
                self.i += 1;
                self.sym(n, unit)
            }
            Some('+') if self.eat("+/-") || self.eat("+-") => self.sym(n, unit),
            Some('~') => {
                self.i += 1;
                self.ws();
                let hi = self.number(true)?;
                let hu = self.unit();
                let u = match (nu, hu) {
                    (Some(a), Some(b)) if a != b => return self.err("limits written in two different units"),
                    (a, b) => a.or(b).unwrap_or(Unit::Mm),
                };
                Spec::limits(n, hi, u).map(Parsed::Spec)
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                    self.i += 1;
                }
                let class: String = self
                    .s
                    .get(start..self.i)
                    .map(|r| r.iter().collect())
                    .unwrap_or_default();
                if unit != Unit::Mm {
                    return self.err("ISO 286 classes apply to millimetre sizes only");
                }
                let fit = Fit::parse(&class).map_err(|e| TolError::Parse {
                    text: self.text.to_string(),
                    why: e.to_string(),
                })?;
                Spec::from_fit(n, fit).map(Parsed::Spec)
            }
            Some(c) if c == '+' || c == '-' || c.is_ascii_digit() => {
                let a = self.number(true)?;
                let au = self.unit();
                self.ws();
                if self.peek() == Some('/') {
                    self.i += 1;
                    self.ws();
                }
                let b = match self.number(true) {
                    Ok(b) => b,
                    Err(_) => return self.err("deviations need two values, e.g. +0.1/-0"),
                };
                let bu = self.unit();
                let du = match (au, bu) {
                    (Some(x), Some(y)) if x != y => return self.err("deviations written in two different units"),
                    (x, y) => x.or(y).unwrap_or(unit),
                };
                let nu = if nu.is_some() { unit } else { du };
                Spec::with_deviations(n, nu, a.max(b), a.min(b), du).map(Parsed::Spec)
            }
            Some(c) => self.err(format!("unexpected {c:?} after the nominal value")),
        }
    }

    fn sym(&mut self, n: Decimal, unit: Unit) -> TolResult<Parsed> {
        self.ws();
        let t = self.number(false)?;
        let tu = self.unit();
        // A unit written only after the tolerance applies to the nominal too ("0.5 ± 0.002 in").
        let nu = if self.nominal_unit.is_some() {
            unit
        } else {
            tu.unwrap_or(unit)
        };
        Spec::with_deviations(n, nu, t, -t, tu.unwrap_or(nu)).map(Parsed::Spec)
    }

    fn thread(&mut self) -> TolResult<ThreadSpec> {
        let d = self.number(false)?;
        self.ws();
        let pitch = if matches!(self.peek(), Some('x' | 'X' | '*')) {
            self.i += 1;
            self.ws();
            Some(self.number(false)?)
        } else {
            None
        };
        self.ws();
        if self.peek() == Some('-') {
            self.i += 1;
            self.ws();
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.i += 1;
        }
        let class: String = self
            .s
            .get(start..self.i)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        let ok = class.is_empty()
            || (class.len().is_multiple_of(2)
                && class
                    .as_bytes()
                    .chunks(2)
                    .all(|c| c[0].is_ascii_digit() && c[1].is_ascii_alphabetic()));
        if !ok {
            return self.err(format!("thread tolerance class {class:?} is not like 6g or 6H"));
        }
        ThreadSpec::new(d, pitch, (!class.is_empty()).then_some(class))
    }
}

fn one_sided(side: Side, v: Decimal, u: Unit) -> TolResult<Spec> {
    match side {
        Side::Min(inc) => Spec::at_least(v, inc, u),
        Side::Max(inc) => Spec::at_most(v, inc, u),
    }
}

fn two_one_sided(a: (Side, Decimal, Unit), b: (Side, Decimal, Unit)) -> TolResult<Spec> {
    let (lo, hi) = match (a, b) {
        ((Side::Min(li), lv, lu), (Side::Max(hi), hv, hu)) | ((Side::Max(hi), hv, hu), (Side::Min(li), lv, lu)) => {
            (Spec::at_least(lv, li, lu)?, Spec::at_most(hv, hi, hu)?)
        }
        _ => return Err(TolError::Invalid("two limits on the same side".into())),
    };
    if lo.dimension != hi.dimension {
        return Err(TolError::Dimension("limits of different dimensions".into()));
    }
    let mut s = Spec { upper: hi.upper, ..lo };
    if let (Some(l), Some(h)) = (s.lower, s.upper)
        && l.value > h.value
    {
        return Err(TolError::Invalid("lower limit above upper limit".into()));
    }
    s.nominal = None;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Bound;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    fn lim(text: &str) -> (Option<Bound>, Option<Bound>, Unit) {
        let s = parse_spec(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        (s.lower, s.upper, s.display_unit)
    }

    fn b(v: &str) -> Option<Bound> {
        Some(Bound::inclusive(d(v)))
    }

    #[test]
    fn symmetric_forms() {
        for t in [
            "10±0.05",
            "10 ± 0.05",
            "１０±０．０５",
            "10 +/- 0.05",
            "10+-0.05 mm",
            "10mm ± 50um",
            "10 mm ±50μm",
            "10 ± 0.05 mm",
        ] {
            assert_eq!(lim(t), (b("9.95"), b("10.05"), Unit::Mm), "{t}");
        }
    }

    #[test]
    fn deviation_forms() {
        for t in [
            "φ10 +0.1/−0",
            "Φ10 +0.1/-0",
            "ø10 +0.1 -0",
            "⌀10 0/+0.1",
            "φ１０　＋０．１／－０",
            "10 -0 +0.1",
        ] {
            assert_eq!(lim(t), (b("10"), b("10.1"), Unit::Mm), "{t}");
        }
        assert_eq!(lim("10 +0.1/-0.05"), (b("9.95"), b("10.1"), Unit::Mm));
        assert_eq!(lim("10 -0.1/-0.3"), (b("9.7"), b("9.9"), Unit::Mm));
        assert_eq!(lim("0.5 +0.002/-0 in"), (b("12.7"), b("12.7508"), Unit::Inch));
        assert_eq!(lim("10 mm +50/-0 um"), (b("10"), b("10.05"), Unit::Mm));
    }

    #[test]
    fn fits_limits_and_one_sided() {
        assert_eq!(lim("25 h7"), (b("24.979"), b("25"), Unit::Mm));
        assert_eq!(lim("φ25H7"), (b("25"), b("25.021"), Unit::Mm));
        assert_eq!(lim("２５ｇ６"), (b("24.980"), b("24.993"), Unit::Mm));
        assert_eq!(lim("9.9~10.1"), (b("9.9"), b("10.1"), Unit::Mm));
        assert_eq!(lim("9.9〜10.1mm"), (b("9.9"), b("10.1"), Unit::Mm));
        assert_eq!(lim("400MPa以上"), (b("400"), None, Unit::MPa));
        assert_eq!(lim("≥ 400 N/mm²"), (b("400"), None, Unit::NPerMm2));
        assert_eq!(lim("0.8 max"), (None, b("0.8"), Unit::Mm));
        assert_eq!(lim("MAX 0.8"), (None, b("0.8"), Unit::Mm));
        assert_eq!(lim("10以上10.2以下"), (b("10"), b("10.2"), Unit::Mm));
        let (lo, hi, _) = lim("0.5未満");
        assert_eq!(
            (lo, hi),
            (
                None,
                Some(Bound {
                    value: d("0.5"),
                    inclusive: false
                })
            )
        );
        assert_eq!(lim("0.5 ± 0.002 in"), (b("12.6492"), b("12.7508"), Unit::Inch));
    }

    #[test]
    fn threads() {
        for t in ["M8×1.25 6g", "M8x1.25-6g", "Ｍ８×１．２５　６ｇ", "M8 x 1.25 6g"] {
            match parse(t) {
                Ok(Parsed::Thread(th)) => {
                    assert_eq!(
                        (th.nominal_mm, th.pitch_mm, th.class.as_deref()),
                        (d("8"), d("1.25"), Some("6g")),
                        "{t}"
                    )
                }
                other => panic!("{t}: {other:?}"),
            }
        }
        assert_eq!(lim("M8×1.25 6g"), (b("7.760"), b("7.972"), Unit::Mm));
        assert!(matches!(parse("M10"), Ok(Parsed::Thread(t)) if t.pitch_mm == d("1.5")));
    }

    #[test]
    fn rejects_without_guessing() {
        for t in [
            "",
            "10",
            "10 +0.1",
            "abc",
            "10±",
            "10 ± -0.1",
            "10.1.2±0.1",
            "10 x7",
            "10 in h7",
            "10 ~ 9",
            "1,5±0.1",
            "10 mm ~ 11 in",
            "10以上11以上",
            "M8×1.25 6",
            "10±0.1 junk",
            "M",
            "±0.1",
        ] {
            assert!(parse(t).is_err(), "{t:?} should be rejected: {:?}", parse(t));
        }
    }
}
