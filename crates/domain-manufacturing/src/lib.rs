//! Manufacturing domain pack: maps extracted records to claims (versioned rules) and supplies the
//! manufacturing conflict rules. Core crates know nothing about these predicates.

pub mod ai;
pub mod ai_guard;
pub mod mapping;
pub mod rules;

pub use mapping::{ColumnMapping, MAPPING_VERSION, Mapped, Record, apply_column_mapping, map_records};
pub use rules::{RULE_VERSION, all_rules, compare_revisions};
