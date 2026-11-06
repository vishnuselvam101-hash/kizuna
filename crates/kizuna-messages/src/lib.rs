//! Server-generated text as stable message codes with typed parameters.
//!
//! Engines describe what they found as a [`Msg`] (`code` + `params`) instead of a finished English sentence.
//! The API stores and returns the `Msg` next to a plain English fallback string (for API clients), the web app
//! renders it from the same catalogue in the reader's language, and server-rendered artefacts (letters, reports)
//! render it in the request language. The catalogue lives in `web/src/i18n-server.json` so both sides read one
//! file; `tests/catalog.rs` fails when a code lacks an English or Japanese entry.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The shared EN/JA catalogue (code → templates with `{name}` placeholders).
pub const CATALOG_JSON: &str = include_str!("../../../web/src/i18n-server.json");

/// Output language of server-rendered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    /// `"ja"` / `"en"`; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ja" => Some(Self::Ja),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// First supported language of an `Accept-Language` header, in the order given (weights are ignored: the
    /// web app sends exactly its UI language).
    pub fn from_accept_language(h: &str) -> Option<Self> {
        h.split(',').find_map(|part| {
            let tag = part.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            match tag.split('-').next().unwrap_or("") {
                "ja" => Some(Self::Ja),
                "en" => Some(Self::En),
                _ => None,
            }
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }

    fn list_separator(self) -> &'static str {
        match self {
            Self::Ja => "、",
            Self::En => ", ",
        }
    }
}

/// One catalogue entry.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub en: String,
    pub ja: String,
}

impl Entry {
    pub fn get(&self, lang: Lang) -> &str {
        match lang {
            Lang::Ja => &self.ja,
            Lang::En => &self.en,
        }
    }
}

/// The parsed catalogue. A malformed catalogue is a build defect caught by the crate's tests.
pub fn catalog() -> &'static BTreeMap<String, Entry> {
    static C: OnceLock<BTreeMap<String, Entry>> = OnceLock::new();
    C.get_or_init(|| serde_json::from_str(CATALOG_JSON).expect("web/src/i18n-server.json is valid"))
}

/// A message: a stable code plus typed parameters. A parameter may itself be a message (`{code, params}`),
/// a list (rendered joined), a string/number (shown as recorded) or null (shown as "—").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Msg {
    pub code: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

impl Msg {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            params: Map::new(),
        }
    }

    /// Adds a parameter (string, number, list, null or JSON value).
    #[must_use]
    pub fn with(mut self, name: &str, v: impl Into<Value>) -> Self {
        self.params.insert(name.to_string(), v.into());
        self
    }

    /// Adds a parameter that is itself a message (rendered in the same language).
    #[must_use]
    pub fn with_msg(mut self, name: &str, m: Msg) -> Self {
        self.params.insert(name.to_string(), m.to_value());
        self
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Reads a message back from stored JSON; `None` when the value is not a message.
    pub fn from_value(v: &Value) -> Option<Self> {
        v.get("code")?.as_str()?;
        serde_json::from_value(v.clone()).ok()
    }

    /// The text in `lang`. An unknown code renders as the code itself (never empty, never panics).
    pub fn render(&self, lang: Lang) -> String {
        let Some(e) = catalog().get(&self.code) else {
            return self.code.clone();
        };
        fill(e.get(lang), &self.params, lang)
    }

    /// The English fallback kept for API clients.
    pub fn en(&self) -> String {
        self.render(Lang::En)
    }
}

/// Renders a stored message value in `lang`, or `fallback` when the value is not a message.
pub fn render_value(v: &Value, lang: Lang, fallback: &str) -> String {
    Msg::from_value(v).map_or_else(|| fallback.to_string(), |m| m.render(lang))
}

fn param_text(v: &Value, lang: Lang) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => s.clone(),
        Value::Array(xs) => xs
            .iter()
            .map(|x| param_text(x, lang))
            .collect::<Vec<_>>()
            .join(lang.list_separator()),
        Value::Object(_) => render_value(v, lang, &v.to_string()),
        other => other.to_string(),
    }
}

fn fill(template: &str, params: &Map<String, Value>, lang: Lang) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        match tail.find('}') {
            Some(j) if tail[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && j > 0 => {
                let name = &tail[..j];
                match params.get(name) {
                    Some(v) => out.push_str(&param_text(v, lang)),
                    None => out.push_str(&rest[i..i + j + 2]),
                }
                rest = &tail[j + 1..];
            }
            _ => {
                out.push('{');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Placeholder names of a template (for the catalogue consistency test).
pub fn placeholders(template: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let tail = &rest[i + 1..];
        match tail.find('}') {
            Some(j) if j > 0 && tail[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                v.push(tail[..j].to_string());
                rest = &tail[j + 1..];
            }
            _ => rest = tail,
        }
    }
    v.sort();
    v.dedup();
    v
}

/// The first run of three or more plain English words in `s` (identifiers such as `value_mismatch`, `qa.result`,
/// `SHA-256` or `EN 10204` do not count). Used by tests to keep English sentences out of Japanese text.
pub fn english_run(s: &str) -> Option<String> {
    let mut run: Vec<&str> = Vec::new();
    for w in s
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '\'' | '_' | '{' | '}' | '.' | '-')))
        .map(|w| w.trim_end_matches('.'))
    {
        if w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') {
            run.push(w);
            if run.len() >= 3 {
                return Some(run.join(" "));
            }
        } else if !w.is_empty() {
            run.clear();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fill_substitutes_nested_lists_and_keeps_unknown_placeholders() {
        let mut p = Map::new();
        p.insert("a".into(), json!(["x", "y"]));
        p.insert("n".into(), json!(3));
        assert_eq!(fill("{a} / {n} / {zz} / {", &p, Lang::En), "x, y / 3 / {zz} / {");
        assert_eq!(fill("{a}", &p, Lang::Ja), "x、y");
    }

    #[test]
    fn unknown_code_renders_as_code_and_round_trips() {
        let m = Msg::new("no.such.code").with("x", 1);
        assert_eq!(m.render(Lang::Ja), "no.such.code");
        assert_eq!(Msg::from_value(&m.to_value()), Some(m));
        assert_eq!(render_value(&json!("plain"), Lang::Ja, "fb"), "fb");
    }

    #[test]
    fn english_run_ignores_identifiers() {
        assert_eq!(english_run("値 {a} が EN 10204 3.1 の value_mismatch です"), None);
        assert_eq!(
            english_run("同じ same predicate and revision"),
            Some("same predicate and".into())
        );
    }

    #[test]
    fn accept_language_picks_first_supported() {
        assert_eq!(Lang::from_accept_language("ja-JP,en;q=0.5"), Some(Lang::Ja));
        assert_eq!(Lang::from_accept_language("fr, en-GB"), Some(Lang::En));
        assert_eq!(Lang::from_accept_language("de"), None);
    }
}
