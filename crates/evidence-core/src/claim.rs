//! Claim input validation (model version 1, see docs/data-model/claims.md).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MODEL_VERSION: i16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    #[error("invalid subject_type")]
    SubjectType,
    #[error("invalid subject_key")]
    SubjectKey,
    #[error("invalid predicate")]
    Predicate,
    #[error("claim needs a value")]
    NoValue,
    #[error("invalid decimal")]
    Decimal,
    #[error("invalid unit")]
    Unit,
    #[error("value_text too long")]
    TextTooLong,
    #[error("invalid source_location")]
    Location,
    #[error("invalid scope")]
    Scope,
    #[error("confidence out of range")]
    Confidence,
    #[error("valid_to must be after valid_from")]
    ValidTime,
}

impl ClaimError {
    pub fn code(&self) -> &'static str {
        match self {
            ClaimError::SubjectType => "invalid_subject_type",
            ClaimError::SubjectKey => "invalid_subject_key",
            ClaimError::Predicate => "invalid_predicate",
            ClaimError::NoValue => "claim_needs_value",
            ClaimError::Decimal => "invalid_decimal",
            ClaimError::Unit => "invalid_unit",
            ClaimError::TextTooLong => "value_text_too_long",
            ClaimError::Location => "invalid_source_location",
            ClaimError::Scope => "invalid_scope",
            ClaimError::Confidence => "invalid_confidence",
            ClaimError::ValidTime => "invalid_valid_time",
        }
    }
}

/// Exact decimal as a canonical string (no floats anywhere): `-?digits(.digits)?`, ≤18 integer and ≤12 fraction digits.
pub fn parse_decimal(s: &str) -> Result<String, ClaimError> {
    let t = s.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let ok = !int.is_empty()
        && int.len() <= 18
        && frac.len() <= 12
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
        && !(body.contains('.') && frac.is_empty());
    if !ok {
        return Err(ClaimError::Decimal);
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let mut out = String::new();
    if neg && !(int == "0" && frac.is_empty()) {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    Ok(out)
}

fn is_ident(s: &str, min: usize, max: usize, extra: &[u8]) -> bool {
    (min..=max).contains(&s.len())
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b))
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ClaimInput {
    pub subject_type: String,
    pub subject_key: String,
    pub predicate: String,
    pub value_text: Option<String>,
    pub value_num: Option<String>,
    pub value_unit: Option<String>,
    #[serde(default)]
    pub scope: Option<Value>,
    pub revision: Option<String>,
    #[serde(default)]
    pub source_location: Option<Value>,
    pub confidence_bps: Option<i32>,
}

/// Normalised, validated claim content.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidClaim {
    pub subject_type: String,
    pub subject_key: String,
    pub predicate: String,
    pub value_text: Option<String>,
    pub value_num: Option<String>,
    pub value_unit: Option<String>,
    pub scope: Value,
    pub revision: Option<String>,
    pub source_location: Value,
    pub confidence_bps: i32,
}

/// Keys a claim's source location may carry. Values: small unsigned integers, short strings or short integer
/// arrays (boxes). `original` keeps printed text a value was normalised from (全角, 和暦), `review` =
/// "needs_confirmation" marks a low-confidence OCR value, `ocr_conf_bps` its confidence.
pub const LOCATION_KEYS: [&str; 29] = [
    "page",
    "table",
    "row",
    "col",
    "sheet",
    "cell",
    "record",
    "region",
    "line",
    "section",
    "handle",
    "bbox",
    "bbox_px",
    "variable",
    "metadata",
    "header",
    "item",
    "original",
    "review",
    "ocr_conf_bps",
    "detector",
    "red_px",
    "label_basis",
    "label",
    "mapped_as",
    "subject_basis",
    "subject_label",
    "subject_as",
    "p_correct_bps",
];

/// Location keys the mapper derives itself; values a document carried under these names are dropped.
pub const DERIVED_LOCATION_KEYS: [&str; 9] = [
    "review",
    "original",
    "label_basis",
    "label",
    "mapped_as",
    "subject_basis",
    "subject_label",
    "subject_as",
    "p_correct_bps",
];

pub fn validate(c: &ClaimInput) -> Result<ValidClaim, ClaimError> {
    if !is_ident(&c.subject_type, 2, 40, b"_") {
        return Err(ClaimError::SubjectType);
    }
    let key = c.subject_key.trim();
    // Zero-width and bidi/format characters would make visually identical keys distinct (bughunt).
    let invisible = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' | '\u{00AD}' | '\u{180E}');
    if key.is_empty() || key.chars().count() > 200 || key.chars().any(|c| c.is_control() || invisible(c)) {
        return Err(ClaimError::SubjectKey);
    }
    if !is_ident(&c.predicate, 2, 81, b"_.") {
        return Err(ClaimError::Predicate);
    }
    let value_num = c.value_num.as_deref().map(parse_decimal).transpose()?;
    let value_text = c
        .value_text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if value_text.as_ref().is_some_and(|t| t.chars().count() > 2000) {
        return Err(ClaimError::TextTooLong);
    }
    if value_num.is_none() && value_text.is_none() {
        return Err(ClaimError::NoValue);
    }
    let unit = c.value_unit.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if unit.is_some_and(|u| u.chars().count() > 32 || u.chars().any(char::is_control)) {
        return Err(ClaimError::Unit);
    }
    let scope = c.scope.clone().unwrap_or_else(|| Value::Object(Default::default()));
    if !scope.is_object() || scope.to_string().len() > 4096 {
        return Err(ClaimError::Scope);
    }
    let loc = c
        .source_location
        .clone()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let Some(obj) = loc.as_object() else {
        return Err(ClaimError::Location);
    };
    for (k, v) in obj {
        let small = |v: &Value| v.as_u64().is_some_and(|n| n < 1_000_000);
        let ok_val = small(v)
            || v.as_str().is_some_and(|s| s.len() <= 120)
            || v.as_array().is_some_and(|a| {
                a.len() <= 8
                    && a.iter()
                        .all(|x| x.as_i64().is_some_and(|n| (0..1_000_000).contains(&n)))
            });
        if !LOCATION_KEYS.contains(&k.as_str()) || !ok_val {
            return Err(ClaimError::Location);
        }
    }
    let confidence_bps = c.confidence_bps.unwrap_or(10_000);
    if !(0..=10_000).contains(&confidence_bps) {
        return Err(ClaimError::Confidence);
    }
    Ok(ValidClaim {
        subject_type: c.subject_type.clone(),
        subject_key: key.to_owned(),
        predicate: c.predicate.clone(),
        value_text,
        value_num,
        value_unit: unit.map(str::to_owned),
        scope,
        revision: c
            .revision
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.chars().count() <= 40)
            .map(str::to_owned),
        source_location: loc,
        confidence_bps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn revision_limit_counts_characters_like_the_database() {
        // DB: CHECK (length(revision) <= 40) counts characters; a 14-char Japanese revision is 42 bytes.
        let rev = "改訂第十四版設計変更通知対応";
        assert!(rev.chars().count() <= 40 && rev.len() > 40);
        let c = ClaimInput {
            subject_type: "part".into(),
            subject_key: "BRK-100".into(),
            predicate: "mfg.material".into(),
            value_text: Some("SUS304".into()),
            revision: Some(rev.into()),
            ..Default::default()
        };
        assert_eq!(validate(&c).unwrap().revision.as_deref(), Some(rev));
    }

    #[test]
    fn decimals_are_exact_and_canonical() {
        assert_eq!(parse_decimal("0012.5000").unwrap(), "12.5");
        assert_eq!(parse_decimal("-0.0").unwrap(), "0");
        assert_eq!(parse_decimal("+3").unwrap(), "3");
        for bad in ["", "1e5", "1.", ".5", "NaN", "1,5", "--1", &"9".repeat(19)] {
            assert!(parse_decimal(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn validates_claim_shape() {
        let mut c = ClaimInput {
            subject_type: "part".into(),
            subject_key: "BRK-100".into(),
            predicate: "mfg.dimension.length".into(),
            value_num: Some("120.50".into()),
            value_unit: Some("mm".into()),
            source_location: Some(json!({"page": 2, "table": 1, "cell": "C7"})),
            ..Default::default()
        };
        let v = validate(&c).unwrap();
        assert_eq!(v.value_num.as_deref(), Some("120.5"));
        assert_eq!(v.confidence_bps, 10_000);
        c.source_location = Some(json!({"path": "/etc/passwd"}));
        assert_eq!(validate(&c), Err(ClaimError::Location));
        c.source_location = None;
        c.predicate = "Bad Predicate".into();
        assert_eq!(validate(&c), Err(ClaimError::Predicate));
        c.predicate = "mfg.material".into();
        c.value_num = None;
        assert_eq!(validate(&c), Err(ClaimError::NoValue));
    }

    proptest! {
        #[test]
        fn decimal_canonical_is_idempotent(i in 0u64..1_000_000_000, f in 0u32..1_000_000, neg in any::<bool>()) {
            let s = format!("{}{}.{:06}", if neg { "-" } else { "" }, i, f);
            let c = parse_decimal(&s).unwrap();
            prop_assert_eq!(parse_decimal(&c).unwrap(), c);
        }
    }
}

#[cfg(test)]
mod fuzz {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn fuzz_validate_never_panics(st in "\\PC{0,50}", sk in "\\PC{0,50}", p in "\\PC{0,50}", n in proptest::option::of("\\PC{0,30}"),
                                      t in proptest::option::of("\\PC{0,30}"), conf in proptest::option::of(any::<i32>())) {
            let c = ClaimInput { subject_type: st, subject_key: sk, predicate: p, value_num: n, value_text: t, confidence_bps: conf, ..Default::default() };
            let _ = validate(&c);
        }
    }
}
