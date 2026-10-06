//! CLI long-help snapshot tests (M6/T1 split of `cli/tests.rs`):
//! byte-identical enforcement for the full help surface of every command
//! (H2). Any change to flag order, grouping, or help text of any command
//! fails these snapshots — regenerate deliberately via
//! `cargo insta accept` after reviewing the diff. Run:
//! `cargo test help_snapshot`

use clap::CommandFactory;

use super::Cli;

/// Render the long help of a command selected from the root `Cli`.
fn long_help(
    select: impl for<'c> FnOnce(&'c mut clap::Command) -> Option<&'c mut clap::Command>,
) -> String {
    let mut cmd = Cli::command();
    select(&mut cmd)
        .unwrap_or_else(|| panic!("selected subcommand not found in the Cli tree"))
        .render_long_help()
        .to_string()
}

#[test]
fn snapshot_help_root_long() {
    insta::assert_snapshot!(
        "help-root-long",
        Cli::command().render_long_help().to_string()
    );
}

#[test]
fn snapshot_help_download_long() {
    insta::assert_snapshot!(
        "help-download-long",
        long_help(|c| c.find_subcommand_mut("download"))
    );
}

#[test]
fn snapshot_help_search_long() {
    insta::assert_snapshot!(
        "help-search-long",
        long_help(|c| c.find_subcommand_mut("search"))
    );
}

#[test]
fn snapshot_help_update_long() {
    insta::assert_snapshot!(
        "help-update-long",
        long_help(|c| c.find_subcommand_mut("update"))
    );
}

#[test]
fn snapshot_help_hf_cache_long() {
    insta::assert_snapshot!(
        "help-hf-cache-long",
        long_help(|c| c.find_subcommand_mut("hf-cache"))
    );
}

#[test]
fn snapshot_help_hf_cache_sync_long() {
    let help = long_help(|c| {
        c.find_subcommand_mut("hf-cache")
            .and_then(|hf| hf.find_subcommand_mut("sync"))
    });
    insta::assert_snapshot!("help-hf-cache-sync-long", help);
}

#[test]
fn snapshot_help_hf_cache_path_long() {
    let help = long_help(|c| {
        c.find_subcommand_mut("hf-cache")
            .and_then(|hf| hf.find_subcommand_mut("path"))
    });
    insta::assert_snapshot!("help-hf-cache-path-long", help);
}
