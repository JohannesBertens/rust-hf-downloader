//! Root CLI-surface tests (M6/T1 split of `cli/tests.rs`): the whole-`Cli`
//! properties — version-string truth, no-subcommand-means-TUI, and clap's
//! own debug consistency check over the command tree. Run:
//! `cargo test cli_surface_tests`

use clap::{CommandFactory, Parser};

use super::Cli;

#[test]
fn version_string_matches_cargo_pkg_version() {
    // v1 CLI drifted here (stale hardcoded version) — never again.
    let err = Cli::try_parse_from(["rust-hf-downloader", "--version"]).unwrap_err();
    assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn no_subcommand_means_tui() {
    let cli = Cli::try_parse_from(["rust-hf-downloader"]).unwrap();
    assert!(cli.command.is_none());
}

#[test]
fn cli_command_tree_passes_clap_debug_assert() {
    // clap's internal consistency check: flag conflicts, arg groups,
    // subcommand wiring. Panics on any inconsistency.
    Cli::command().debug_assert();
}
