//! Manufacturing domain pack: maps extracted records to claims (versioned rules) and supplies the
//! manufacturing conflict rules. Core crates know nothing about these predicates.

pub mod ai;
pub mod ai_guard;
pub mod calibration;
pub mod capa;
pub mod debt;
pub mod infer;
mod ja;
pub mod mapping;
pub mod reglist;
pub mod resupply;
pub mod rules;
pub mod three_way;

pub use infer::{LabelSynonyms, SYNONYM_TARGETS};
pub use mapping::{
    ColumnMapping, MAPPING_VERSION, Mapped, Record, apply_column_mapping, map_records, map_records_with,
};
pub use rules::{RULE_VERSION, all_rules, compare_revisions};
