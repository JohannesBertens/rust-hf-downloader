//! rust-hf-downloader library root.
//!
//! The crate is both a library (this file) and a binary (`src/main.rs`).
//! Exposing the modules here allows the `tests/` integration suite and
//! future tooling to exercise the download/verification/registry logic
//! directly, without going through the TUI or CLI front-ends.
//!
//! The binary entry point in `src/main.rs` simply reuses these modules.

pub mod api;
pub mod cli;
pub mod config;
pub mod download;
pub mod headless;
pub mod http_client;
pub mod models;
pub mod rate_limiter;
pub mod runtime;
pub mod registry;
pub mod ui;
pub mod utils;
pub mod verification;
