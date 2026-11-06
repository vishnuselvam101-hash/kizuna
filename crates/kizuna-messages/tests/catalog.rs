//! Catalogue completeness: every message code the server can emit has an English and a natural Japanese entry,
//! and every API error code has `err.<code>` in both web dictionaries.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use kizuna_messages::{catalog, english_run, placeholders};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("target" | "tests" | "benches" | "fuzz")
            ) {
                rust_sources(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Quoted strings ("…", or SQL `'code', '…'`) in `src` that look like message codes of a catalogue namespace.
fn code_literals(src: &str, namespaces: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // SQL literals: jsonb_build_object('code', '<code>').
    for part in src.split("'code', '").skip(1) {
        if let Some(code) = part.split('\'').next() {
            out.insert(code.to_string());
        }
    }
    {
        for (i, part) in src.split('"').enumerate() {
            if i % 2 == 1
                && part.contains('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
                && namespaces.contains(part.split('.').next().unwrap_or(""))
            {
                out.insert(part.to_string());
            }
        }
    }
    out
}

#[test]
fn every_entry_has_english_and_natural_japanese() {
    let c = catalog();
    assert!(c.len() > 100, "catalogue looks truncated: {}", c.len());
    for (code, e) in c {
        assert!(
            !e.en.trim().is_empty() && !e.ja.trim().is_empty(),
            "{code}: empty EN or JA"
        );
        let (pe, pj) = (placeholders(&e.en), placeholders(&e.ja));
        for p in &pe {
            assert!(pj.contains(p), "{code}: JA drops placeholder {{{p}}} used in EN");
        }
        let literal: String = e.ja.split('{').map(|s| s.split_once('}').map_or(s, |x| x.1)).collect();
        if code != "text.verbatim" {
            assert!(
                literal
                    .chars()
                    .any(|ch| ('\u{3040}'..='\u{30ff}').contains(&ch) || ('\u{4e00}'..='\u{9fff}').contains(&ch)),
                "{code}: JA has no Japanese text: {}",
                e.ja
            );
        }
        assert_eq!(english_run(&literal), None, "{code}: English words in JA: {}", e.ja);
    }
}

#[test]
fn every_code_used_by_the_server_is_in_the_catalogue_and_vice_versa() {
    let c = catalog();
    let namespaces: BTreeSet<String> = c
        .keys()
        .map(|k| k.split('.').next().unwrap_or("").to_string())
        .collect();
    let mut files = Vec::new();
    rust_sources(&root().join("crates"), &mut files);
    let (mut used, mut direct) = (BTreeSet::new(), BTreeSet::new());
    for f in &files {
        // kizuna-verify-pack is the offline verifier with its own bilingual `Msg::new(en, ja)`.
        if f.ends_with("kizuna-messages/src/lib.rs") || f.components().any(|c| c.as_os_str() == "kizuna-verify-pack") {
            continue;
        }
        let src = fs::read_to_string(f).unwrap_or_default();
        // Production code only: unit-test modules may use made-up codes.
        let prod = src.split("#[cfg(test)]").next().unwrap_or("");
        used.extend(code_literals(prod, &namespaces));
        // Direct `Msg::new("code")` calls must name a catalogue entry (other literals of a namespace, such as
        // audit action names like "decision.recorded", only count as uses).
        for part in prod.split("Msg::new(\"").skip(1) {
            if let Some(code) = part.split('"').next() {
                direct.insert(code.to_string());
            }
        }
    }
    let missing: Vec<&String> = direct.iter().filter(|u| !c.contains_key(*u)).collect();
    assert!(
        missing.is_empty(),
        "codes used in Rust but missing from web/src/i18n-server.json: {missing:?}"
    );
    let unused: Vec<&String> = c.keys().filter(|k| !used.contains(*k)).collect();
    assert!(
        unused.is_empty(),
        "catalogue codes no server code emits (remove or use them): {unused:?}"
    );
}

#[test]
fn every_api_error_code_has_en_and_ja_web_text() {
    let mut files = Vec::new();
    rust_sources(&root().join("crates/kizuna-api/src"), &mut files);
    let mut codes = BTreeSet::new();
    for f in &files {
        let src = fs::read_to_string(f).unwrap_or_default();
        for part in src.split("ApiError::").skip(1) {
            let Some((variant, rest)) = part.split_once('(') else {
                continue;
            };
            if !matches!(variant, "BadRequest" | "Unauthorized" | "Forbidden" | "Conflict") || !rest.starts_with('"') {
                continue;
            }
            if let Some(code) = rest[1..].split('"').next() {
                codes.insert(code.to_string());
            }
        }
    }
    // Fixed codes of `IntoResponse` and conflict codes passed through variables.
    for c in [
        "not_found",
        "payload_too_large",
        "unsupported_media_type",
        "rate_limited",
        "database_unavailable",
        "storage_full",
        "internal_error",
        "account_disabled_for_another_reason",
        "release_via_artifact_review",
        "not_reversible",
    ] {
        codes.insert(c.into());
    }
    assert!(codes.len() > 100, "error-code scan looks broken: {}", codes.len());
    let web: String = fs::read_dir(root().join("web/src"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("i18n") && !n.ends_with(".json"))
        })
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect();
    let missing: Vec<&String> = codes
        .iter()
        .filter(|c| web.matches(&format!("\"err.{c}\":")).count() < 2)
        .collect();
    assert!(
        missing.is_empty(),
        "API error codes without EN+JA err.<code> text in web/src/i18n*: {missing:?}"
    );
}
