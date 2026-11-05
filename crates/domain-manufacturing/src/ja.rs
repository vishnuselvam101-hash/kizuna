//! Japanese document normalisation for `mfg-map@3`, mirroring services/extraction `ja.py`: look-alike folding
//! (㎜, ², U+2212 minus, 〜), 和暦 dates with era bounds, prefix units (HRC55, Ra 1.6, ￥1,250), thousands
//! separators, inline tolerances ("120.5㎜ ±0.1") and the Japanese label tokens. Pure functions; anything that
//! cannot be parsed exactly returns `None` and the caller keeps the original text.

use entity_resolution::safe_normalize;

/// Whole-token label translations (same table as `JA_TOKENS` in claims.py): `外径 規格値` reads as
/// `outer diameter nominal`, `外径 測定値` as `outer diameter measured`, `外径 公差` as `outer diameter tolerance`.
pub(crate) const JA_TOKENS: [(&str, &str); 25] = [
    ("外径", "outer diameter"),
    ("内径", "inner diameter"),
    ("全長", "length"),
    ("幅", "width"),
    ("厚さ", "thickness"),
    ("高さ", "height"),
    ("直径", "diameter"),
    ("硬さ", "hardness"),
    ("硬度", "hardness"),
    ("引張強さ", "tensile strength"),
    ("引張強度", "tensile strength"),
    ("降伏点", "yield strength"),
    ("耐力", "yield strength"),
    ("降伏強さ", "yield strength"),
    ("伸び", "elongation"),
    ("表面粗さ", "surface roughness"),
    ("測定値", "measured"),
    ("実測値", "measured"),
    ("公差", "tolerance"),
    ("許容差", "tolerance"),
    ("規格値", "nominal"),
    ("基準値", "nominal"),
    ("基準寸法", "nominal"),
    ("図面寸法", "nominal"),
    ("公称値", "nominal"),
];

pub(crate) fn translate_token(t: &str) -> &str {
    JA_TOKENS.iter().find(|(k, _)| *k == t).map(|(_, v)| *v).unwrap_or(t)
}

/// Folds the look-alikes `safe_normalize` does not cover, then applies it (full-width ASCII → ASCII, upper-case).
pub(crate) fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{339C}' => out.push_str("mm"),
            '\u{339D}' => out.push_str("cm"),
            '\u{338F}' => out.push_str("kg"),
            '\u{00B2}' => out.push('2'),
            '\u{2212}' | '\u{2010}'..='\u{2015}' | '\u{FE63}' => out.push('-'),
            '\u{301C}' => out.push('~'),
            '\u{FFE5}' | '\u{00A5}' => out.push('¥'),
            '\u{FF08}' => out.push('('),
            '\u{FF09}' => out.push(')'),
            _ => out.push(c),
        }
    }
    safe_normalize(&out)
}

fn valid(y: u32, m: u32, d: u32) -> bool {
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return false,
    };
    (1900..=2200).contains(&y) && (1..=days).contains(&d)
}

/// Era → (offset to Gregorian, first day, last day).
type Ymd = (u32, u32, u32);

fn era(name: &str) -> Option<(u32, Ymd, Ymd)> {
    match name {
        "令和" | "R" => Some((2018, (2019, 5, 1), (9999, 12, 31))),
        "平成" | "H" => Some((1988, (1989, 1, 8), (2019, 4, 30))),
        "昭和" | "S" => Some((1925, (1926, 12, 25), (1989, 1, 7))),
        _ => None,
    }
}

fn small(s: &str, max_len: usize) -> Option<u32> {
    let s = s.trim();
    (!s.is_empty() && s.len() <= max_len && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// 和暦 and 年月日 forms: '令和7年4月1日', '令和元年5月1日', 'R7.4.1', 'H31/4/30', '2025年4月1日' → ISO date.
/// Dates outside their era or the calendar are refused. Slash/dot Gregorian forms are left to the caller.
pub(crate) fn parse_ja_date(v: &str) -> Option<String> {
    if v.chars().count() > 40 {
        return None;
    }
    let t: String = fold(v).chars().filter(|c| !c.is_whitespace()).collect();
    let (era_name, rest) = ["令和", "平成", "昭和", "R", "H", "S"]
        .iter()
        .find_map(|e| t.strip_prefix(e).map(|r| (*e, r)))
        .map_or((None, t.as_str()), |(e, r)| (Some(e), r));
    let parts: Vec<&str> = if rest.contains('年') {
        let r = rest.strip_suffix('日').unwrap_or(rest);
        let (y, r) = r.split_once('年')?;
        let (m, d) = r.split_once('月')?;
        vec![y, m, d]
    } else if era_name.is_some_and(|e| e.len() == 1) {
        rest.split(['.', '/', '-']).collect()
    } else {
        return None;
    };
    let [y, m, d] = parts[..] else { return None };
    let (m, d) = (small(m, 2)?, small(d, 2)?);
    match era_name {
        Some(e) => {
            let (base, first, last) = era(e)?;
            let ey = if y == "元" && e.len() > 1 { 1 } else { small(y, 2)? };
            let ymd = (base + ey, m, d);
            (ey >= 1 && valid(ymd.0, m, d) && first <= ymd && ymd <= last)
                .then(|| format!("{:04}-{:02}-{:02}", ymd.0, m, d))
        }
        None => {
            let y = small(y, 4)?;
            valid(y, m, d).then(|| format!("{y:04}-{m:02}-{d:02}"))
        }
    }
}

/// Units written before the number ('HRC55', 'Ra 1.6', '¥1,250') and the diameter sign ('φ25').
/// Returns (number text, unit) with the unit lower-case, or `None` when the value has no such prefix.
pub(crate) fn split_prefix_unit(t: &str) -> Option<(&str, &'static str)> {
    let t = t.trim_start_matches(['φ', 'Φ', '∅', 'Ø', '⌀']).trim();
    for (p, u) in [
        ("HRC", "hrc"),
        ("HRB", "hrb"),
        ("HV", "hv"),
        ("HB", "hb"),
        ("RA", "ra"),
        ("RZ", "rz"),
        ("¥", "jpy"),
    ] {
        if let Some(rest) = t.strip_prefix(p) {
            let rest = rest.trim();
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                return Some((rest, u));
            }
        }
    }
    None
}

/// '1,200' → '1200' (comma only as a thousands separator in groups of three); other text unchanged.
pub(crate) fn strip_thousands(t: &str) -> Option<String> {
    let int_end = t.find('.').unwrap_or(t.len());
    let (int, frac) = t.split_at(int_end);
    let int = int.trim_start_matches(['+', '-']);
    let groups: Vec<&str> = int.split(',').collect();
    if groups.len() < 2
        || groups[0].is_empty()
        || groups[0].len() > 3
        || groups[1..].iter().any(|g| g.len() != 3)
        || !groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()))
        || frac.contains(',')
    {
        return None;
    }
    Some(t.replace(',', ""))
}

/// Japanese counting/currency units → canonical unit.
pub(crate) fn ja_unit(u: &str) -> Option<&'static str> {
    match u.trim() {
        "個" | "本" | "枚" | "ヶ" | "ケ" | "台" => Some("pcs"),
        "円" | "¥" | "JPY" | "jpy" => Some("jpy"),
        _ => None,
    }
}

/// '120.5 MM ±0.1' → ('120.5 MM', '±0.1'); 'φ25 +0/-0.05' → ('φ25', '+0/-0.05'). Input must be folded.
pub(crate) fn split_inline_tolerance(t: &str) -> Option<(String, String)> {
    if t.chars().count() > 80 {
        return None;
    }
    let pos = t
        .char_indices()
        .find(|(i, c)| (*c == '±' || *c == '+') && t[..*i].trim_end().ends_with(|p: char| p.is_ascii_alphanumeric()))?;
    let (head, tol) = t.split_at(pos.0);
    let head = head.trim();
    head.chars()
        .any(|c| c.is_ascii_digit())
        .then(|| (head.to_string(), tol.trim().to_string()))
}

pub(crate) fn result_word(v: &str) -> Option<&'static str> {
    match v.trim() {
        "○" | "〇" | "◯" | "良" => Some("pass"),
        "×" | "✕" | "否" => Some("fail"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wareki_dates_respect_era_bounds() {
        let ok = [
            ("令和7年4月1日", "2025-04-01"),
            ("令和元年5月1日", "2019-05-01"),
            ("R7.4.1", "2025-04-01"),
            ("H31/4/30", "2019-04-30"),
            ("平成元年1月8日", "1989-01-08"),
            ("２０２５年４月１日", "2025-04-01"),
            ("令和 7 年 12 月 31 日", "2025-12-31"),
        ];
        for (v, iso) in ok {
            assert_eq!(parse_ja_date(v).as_deref(), Some(iso), "{v}");
        }
        for bad in [
            "令和1年4月30日", // before 令和 began
            "平成32年1月1日", // after 平成 ended
            "令和7年2月29日",
            "令和0年5月1日",
            "R7.13.1",
            "令和99999999999999年1月1日",
            "令和7年4月",
            "X7.4.1",
            "",
        ] {
            assert_eq!(parse_ja_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn folding_and_units() {
        assert_eq!(fold("120.5㎜"), "120.5MM");
        assert_eq!(fold("440 N/mm²"), "440 N/MM2");
        assert_eq!(fold("+0.1/−0"), "+0.1/-0");
        assert_eq!(split_prefix_unit("HRC55"), Some(("55", "hrc")));
        assert_eq!(split_prefix_unit("RA 1.6"), Some(("1.6", "ra")));
        assert_eq!(split_prefix_unit("¥1,250"), Some(("1,250", "jpy")));
        assert_eq!(split_prefix_unit("HRC"), None);
        assert_eq!(strip_thousands("12,000").as_deref(), Some("12000"));
        assert_eq!(strip_thousands("1,5"), None);
        assert_eq!(strip_thousands("1,2345"), None);
        assert_eq!(strip_thousands(",123"), None);
        assert_eq!(ja_unit("個"), Some("pcs"));
        assert_eq!(
            split_inline_tolerance("120.5 MM ±0.1"),
            Some(("120.5 MM".into(), "±0.1".into()))
        );
        assert_eq!(split_inline_tolerance("+0.1/-0"), None);
        assert_eq!(result_word("○"), Some("pass"));
    }
}
