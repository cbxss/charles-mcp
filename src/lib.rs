//! charles-mcp — control Charles Proxy 5 and inspect captured HTTP(S) traffic.
//!
//! The binary (`main.rs`) is a thin wrapper that serves [`server::CharlesServer`]
//! over stdio; the logic lives in these modules so it can be unit/integration
//! tested without a running Charles.

pub mod config;
pub mod error;
pub mod format;
pub mod server;
pub mod session;
pub mod store;
pub mod tools;
pub mod web;
