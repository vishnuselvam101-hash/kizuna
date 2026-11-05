//! KIZUNA Verify API: HTTP layer, persistence and identity. Rules live in the engine crates.

pub mod app;
pub mod artifacts;
pub mod audit;
pub mod auth;
pub mod claims;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod ratelimit;
pub mod security_events;
pub mod storage;
pub mod users;

pub use app::{AppState, router};
