//! One-shot CLI download mode: `rust-hf-downloader download <MODEL_ID> …`
//!
//! Non-interactive frontend over the shared [`crate::engine`] pipeline,
//! designed for scripts and AI-agent skills:
//!
//! - human progress on **stderr** (single-line rewrites when interactive),
//!   the summary on **stdout**;
//! - `--json` emits stable NDJSON events on **stdout** (progress included,
//!   throttled; the `error` event is always the last line on failure);
//! - deterministic exit codes (see [`EXIT_USAGE`] etc.);
//! - ambiguous selections fail fast with the full structured file list so an
//!   agent can re-invoke with an explicit selector in one round-trip.
//!
//! `hf-cache sync` (plans/hf-cache-sync.md §2/§5.2) reuses the same engine
//! to populate the real HuggingFace hub cache, publishing staged downloads
//! atomically through [`crate::hf_cache`]; `hf-cache path` is the pure
//! path-math scripting helper.
//!
//! The TUI remains the default when the binary is started without a
//! subcommand (see `main.rs`).

use clap::{Parser, Subcommand};

mod args;
mod download_cmd;
mod events;
mod hf_cache_cmd;
// W1.4 oracle window: `report` is `pub(crate)` only so `fmt`'s differential
// tests can call the live legacy helpers; reverted in W1.4b.
pub(crate) mod report;
mod resolve;
mod search_cmd;
mod update_cmd;

#[cfg(test)]
mod tests;

use args::{DownloadArgs, HfCacheArgs, SearchArgs, UpdateArgs};
use download_cmd::run_download;
use hf_cache_cmd::run_hf_cache;
use search_cmd::run_search;
use update_cmd::run_update;

#[derive(Parser, Debug)]
#[command(
    name = "rust-hf-downloader",
    version, // from CARGO_PKG_VERSION; pinned by a unit test
    about = "TUI and CLI for downloading HuggingFace models",
    long_about = "TUI and CLI for downloading HuggingFace models.\n\nRun without a subcommand to start the interactive TUI."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// One-shot model download (non-interactive, script/AI-friendly)
    #[command(alias = "dl")]
    Download(DownloadArgs),

    /// Populate the HuggingFace hub cache for offline serving (vLLM,
    /// transformers)
    HfCache(HfCacheArgs),

    /// Search HuggingFace models (query-only; prints a table or a JSON array)
    Search(SearchArgs),

    /// Update the installed binary to the newest release
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
}

// ---------------------------------------------------------------------------
// Exit codes (see plans/add-cli.md §2.4)
// ---------------------------------------------------------------------------

/// All requested files present on disk (downloaded or already existed);
/// verification passed or skipped.
pub const EXIT_OK: i32 = 0;
/// Download failed after retries, or any hash mismatch.
pub const EXIT_FAILURE: i32 = 1;
/// Authentication required (gated repo / bad token).
pub const EXIT_AUTH: i32 = 2;
/// Usage error or resolution ambiguity (`EX_USAGE` convention). clap's own
/// usage errors are routed here too, so `2` stays reserved for auth.
pub const EXIT_USAGE: i32 = 64;
/// `update --check` found a newer release (nothing was installed).
pub const EXIT_UPDATE_AVAILABLE: i32 = 70;
/// `update` downloaded an asset whose SHA256 did not match the manifest.
pub const EXIT_CHECKSUM: i32 = 71;
/// Interrupted by SIGINT.
pub const EXIT_INTERRUPTED: i32 = 130;

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Run a parsed CLI command; returns the process exit code.
pub async fn run(command: Command) -> i32 {
    match command {
        Command::Download(args) => run_download(args).await,
        Command::HfCache(args) => run_hf_cache(args).await,
        Command::Search(args) => run_search(args).await,
        Command::Update(args) => run_update(args).await,
    }
}
