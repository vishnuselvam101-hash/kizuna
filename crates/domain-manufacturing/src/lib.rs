//! Manufacturing domain pack: maps extracted records to claims (versioned rules) and supplies the
//! manufacturing conflict rules. Core crates know nothing about these predicates.

pub mod mapping;
pub mod rules;

pub use mapping::{MAPPING_VERSION, Mapped, Record, map_records};
pub use rules::{RULE_VERSION, all_rules, compare_revisions};
