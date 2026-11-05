//! KIZUNA Verify API: HTTP layer, persistence and identity. Rules live in the engine crates.

pub mod app;
pub mod artifacts;
pub mod audit;
pub mod auth;
pub mod claims;
pub mod clock;
pub mod config;
pub mod crypto;
pub mod db;
pub mod decisions;
pub mod error;
pub mod extraction;
pub mod incidents;
pub mod insights;
pub mod keyring;
pub mod ratelimit;
pub mod seccomp;
pub mod security_events;
pub mod storage;
pub mod users;
pub mod verification;
pub mod webauthn;

pub use app::{AppState, router};
