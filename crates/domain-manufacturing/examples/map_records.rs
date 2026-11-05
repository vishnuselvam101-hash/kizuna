//! Evaluation harness: JSON lines `{"kind": ..., "records": [...]}` on stdin → one `Mapped` JSON line per input on
//! stdout; an optional `"synonyms": [[label, canonical], ...]` per line plays the tenant's learnt synonyms. Used by
//! services/extraction/tests/ja_eval.py to score the production mapping on the Japanese corpus.

use std::io::{BufRead, Write};

use domain_manufacturing::LabelSynonyms;
use domain_manufacturing::mapping::{Record, map_records_with};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin");
        let v: serde_json::Value = serde_json::from_str(&line).expect("json line");
        let kind = v["kind"].as_str().unwrap_or("drawing");
        let records: Vec<Record> = serde_json::from_value(v["records"].clone()).expect("records");
        // optional tenant synonyms: [["label", "canonical label"], ...]
        let mut syn = LabelSynonyms::new();
        for pair in v["synonyms"].as_array().into_iter().flatten() {
            if let (Some(l), Some(t)) = (pair[0].as_str(), pair[1].as_str()) {
                syn.insert(l, t);
            }
        }
        let mapped = map_records_with(kind, &records, 8000, &syn);
        writeln!(out, "{}", serde_json::to_string(&mapped).expect("serialise")).expect("stdout");
    }
}
