//! `cli::hf_cache` tests (M6/T1 split of `cli/tests.rs`): the pure
//! plans/hf-cache-sync.md §2.2 selection precedence (positional > include/exclude > preset >
//! whole-repo), the ref/symlink/absolute-path policy helpers, and the
//! clap surface of the `hf-cache` group. Run: `cargo test hf_cache_tests`

use clap::Parser;
use std::path::Path;

use super::args::{HfCacheArgs, HfCacheCommand};
use super::hf_cache::{
    absolute_path, ref_name_for_revision, select_sync_files, symlinks_enabled, SelectionMode,
    SyncSelectionError,
};
use super::resolve::SelectionError;
use super::{Cli, Command};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// Representative repo tree exercising every plans/hf-cache-sync.md §2.3 preset category.
fn sync_tree() -> Vec<&'static str> {
    vec![
        "config.json",
        "model.safetensors",
        "model.safetensors.index.json",
        "pytorch_model.bin",
        "README.md",
        "original/consolidated.safetensors",
        "tokenizer.model",
    ]
}

fn run_select(
    tree: &[&str],
    files: &[&str],
    include: &[&str],
    exclude: &[&str],
    preset: Option<&str>,
) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
    select_sync_files(
        tree,
        &strings(files),
        &strings(include),
        &strings(exclude),
        preset,
    )
}

#[test]
fn sync_selection_positional_files_win_over_every_other_selector() {
    // plans/hf-cache-sync.md §2.2 precedence: positional FILE… beats --include/--exclude and
    // the preset, even when all are given at once.
    let (selected, mode) = run_select(
        &sync_tree(),
        &["config.json", "README.md"],
        &["*.safetensors"],
        &[],
        Some("vllm"),
    )
    .unwrap();
    assert_eq!(selected, strings(&["config.json", "README.md"]));
    assert_eq!(mode, SelectionMode::Files);
}

#[test]
fn sync_selection_positional_missing_and_dedup() {
    let tree = sync_tree();
    let err = run_select(&tree, &["nope.json"], &[], &[], None).unwrap_err();
    match err {
        SyncSelectionError::MissingPositional { path, available } => {
            assert_eq!(path, "nope.json");
            assert_eq!(available, strings(&tree));
        }
        other => panic!("expected MissingPositional, got {:?}", other),
    }
    // Repeated positional files collapse (order preserved).
    let (selected, _) = run_select(
        &tree,
        &["config.json", "config.json", "README.md"],
        &[],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(selected, strings(&["config.json", "README.md"]));
}

#[test]
fn sync_selection_include_exclude_glob_mode() {
    let tree = sync_tree();
    let (selected, mode) = run_select(
        &tree,
        &[],
        &["*.json"],
        &["model.safetensors.index.json"],
        None,
    )
    .unwrap();
    assert_eq!(selected, strings(&["config.json"]));
    assert_eq!(mode, SelectionMode::Patterns);

    // Exclude alone (no include, no preset) filters the whole repo —
    // mode stays WholeRepo with the universal exclude on top (plans/hf-cache-sync.md §2.2).
    let (selected, mode) =
        run_select(&tree, &[], &[], &["*.bin", "*.md", "original/**"], None).unwrap();
    assert_eq!(
        selected,
        strings(&[
            "config.json",
            "model.safetensors",
            "model.safetensors.index.json",
            "tokenizer.model",
        ])
    );
    assert_eq!(mode, SelectionMode::WholeRepo);
}

#[test]
fn sync_selection_vllm_preset_applies_allow_and_ignore_tables() {
    // plans/hf-cache-sync.md §2.3 via patterns::VLLM_ALLOW/VLLM_IGNORE: safetensors (incl.
    // subfolders, `*` crosses `/`), config/tokenizer files in; fallback
    // weight formats, docs, and original/ out.
    let (selected, mode) = run_select(&sync_tree(), &[], &[], &[], Some("vllm")).unwrap();
    assert_eq!(
        selected,
        strings(&[
            "config.json",
            "model.safetensors",
            "model.safetensors.index.json",
            "tokenizer.model",
        ])
    );
    assert_eq!(mode, SelectionMode::Preset);

    // plans/hf-cache-sync.md §2.2 precedence: --include beats the preset; --exclude applies
    // ON TOP of the preset — the preset selection minus the excluded
    // file, still reported as Preset mode.
    let (selected, mode) =
        run_select(&sync_tree(), &[], &[], &["tokenizer.model"], Some("vllm")).unwrap();
    assert_eq!(mode, SelectionMode::Preset);
    assert_eq!(
        selected,
        strings(&[
            "config.json",
            "model.safetensors",
            "model.safetensors.index.json",
        ])
    );
}

#[test]
fn sync_selection_whole_repo_is_the_last_resort() {
    let tree = sync_tree();
    let (selected, mode) = run_select(&tree, &[], &[], &[], None).unwrap();
    assert_eq!(selected, strings(&tree));
    assert_eq!(mode, SelectionMode::WholeRepo); // caller prints the tip
}

#[test]
fn sync_selection_empty_after_filtering_is_a_usage_error() {
    let tree = sync_tree();
    // Include matching nothing.
    let err = run_select(&tree, &[], &["*.nonexistent"], &[], None).unwrap_err();
    match &err {
        SyncSelectionError::EmptySelection { available } => {
            assert_eq!(*available, strings(&tree));
        }
        other => panic!("expected EmptySelection, got {:?}", other),
    }
    // --exclude applies on top of positional files (plans/hf-cache-sync.md §2.2): dropping the
    // only positional file empties the selection.
    let err = run_select(&tree, &["README.md"], &[], &["*.md"], None).unwrap_err();
    assert_eq!(err.code(), "empty_selection");
    assert_eq!(
        err.message(),
        "selection matched no files in the repository"
    );
    // Unknown preset (unreachable via clap's parse_preset, direct callers).
    let err = run_select(&tree, &[], &[], &[], Some("transformers")).unwrap_err();
    assert_eq!(err.code(), "unknown_preset");
}

// --- hf-cache parsing -----------------------------------------------------

#[test]
fn parses_hf_cache_sync_positional_files_and_flags() {
    let cli = Cli::try_parse_from([
        "rhd",
        "hf-cache",
        "sync",
        "a/b",
        "config.json",
        "tokenizer.model",
        "--revision",
        "2.0bpw",
        "--include",
        "*.safetensors",
        "--exclude",
        "original/**",
        "--cache-dir",
        "/tmp/hub",
        "--no-symlinks",
        "--force",
        "--dry-run",
        "--no-verify",
        "--json",
        "--quiet",
        "--rate-limit-mbps",
        "7.25",
    ])
    .unwrap();
    let Some(Command::HfCache(HfCacheArgs {
        command: HfCacheCommand::Sync(args),
    })) = cli.command
    else {
        panic!("expected hf-cache sync");
    };
    assert_eq!(args.model_id, "a/b");
    assert_eq!(args.files, vec!["config.json", "tokenizer.model"]);
    assert_eq!(args.revision.as_deref(), Some("2.0bpw"));
    assert_eq!(args.include, vec!["*.safetensors"]);
    assert_eq!(args.exclude, vec!["original/**"]);
    assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
    assert!(args.no_symlinks);
    assert!(args.force);
    assert!(args.dry_run);
    assert!(args.run_output.no_verify);
    assert!(args.run_output.json);
    assert!(args.run_output.quiet);
    assert_eq!(args.rate_limits.rate_limit_mbps, Some(7.25));
    assert!(!args.rate_limits.rate_limit);
    assert!(!args.rate_limits.no_rate_limit);
}

#[test]
fn parses_hf_cache_sync_for_vllm_and_sha_revision() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let cli = Cli::try_parse_from([
        "rhd",
        "hf-cache",
        "sync",
        "Qwen/Qwen2.5-7B-Instruct",
        "--for",
        "vllm",
        "--revision",
        sha,
    ])
    .unwrap();
    let Some(Command::HfCache(HfCacheArgs {
        command: HfCacheCommand::Sync(args),
    })) = cli.command
    else {
        panic!("expected hf-cache sync");
    };
    assert_eq!(args.model_id, "Qwen/Qwen2.5-7B-Instruct");
    assert!(args.files.is_empty());
    assert_eq!(args.for_preset.as_deref(), Some("vllm"));
    assert_eq!(args.revision.as_deref(), Some(sha));
}

#[test]
fn hf_cache_sync_rejects_unknown_preset_bad_revision_and_flag_conflicts() {
    // --for validates its preset with a clear error.
    let err = Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--for", "transformers"])
        .unwrap_err();
    assert!(err.to_string().contains("available presets: vllm"));
    // parse_revision runs at the parser level.
    assert!(Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--revision", ".."]).is_err());
    // Shared rate-limit flags keep their conflicts.
    assert!(Cli::try_parse_from([
        "rhd",
        "hf-cache",
        "sync",
        "a/b",
        "--rate-limit",
        "--no-rate-limit",
    ])
    .is_err());
    // Model id required.
    assert!(Cli::try_parse_from(["rhd", "hf-cache", "sync"]).is_err());
}

#[test]
fn parses_hf_cache_path_flags() {
    let cli = Cli::try_parse_from([
        "rhd",
        "hf-cache",
        "path",
        "a/b",
        "--revision",
        "main",
        "--cache-dir",
        "/tmp/hub",
        "--token",
        "hf_x",
    ])
    .unwrap();
    let Some(Command::HfCache(HfCacheArgs {
        command: HfCacheCommand::Path(args),
    })) = cli.command
    else {
        panic!("expected hf-cache path");
    };
    assert_eq!(args.model_id, "a/b");
    assert_eq!(args.revision.as_deref(), Some("main"));
    assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
    assert_eq!(args.token.as_deref(), Some("hf_x"));

    // Absent revision → None (main is applied at run time).
    let cli = Cli::try_parse_from(["rhd", "hf-cache", "path", "a/b"]).unwrap();
    let Some(Command::HfCache(HfCacheArgs {
        command: HfCacheCommand::Path(args),
    })) = cli.command
    else {
        panic!()
    };
    assert_eq!(args.revision, None);
}

// --- refs/symlink/path pure helpers ---------------------------------------

#[test]
fn refs_are_written_for_branches_tags_but_not_raw_shas() {
    // Branch/tag names get a refs/<name> entry (R2) — including
    // slash-separated branches and hex-looking short names.
    assert_eq!(ref_name_for_revision("main"), Some("main"));
    assert_eq!(ref_name_for_revision("2.0bpw"), Some("2.0bpw"));
    assert_eq!(ref_name_for_revision("release/v2"), Some("release/v2"));
    assert_eq!(
        ref_name_for_revision("0123456789abcdef"),
        Some("0123456789abcdef")
    );
    // 40-hex commit SHAs (either case) get NO refs/ entry.
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(ref_name_for_revision(sha), None);
    let upper = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
    assert_eq!(ref_name_for_revision(upper), None);
    // 40 chars but not hex: a (weird) branch name — still gets a ref.
    let not_hex = "z".repeat(40);
    assert_eq!(ref_name_for_revision(&not_hex), Some(not_hex.as_str()));
}

#[test]
fn symlink_policy_honors_flag_and_hub_env_var() {
    // Default: symlinks on Unix, hub's degraded no-symlink cache on
    // Windows (relative `/`-separator targets fail to resolve there).
    #[cfg(unix)]
    assert!(symlinks_enabled(false, None));
    #[cfg(windows)]
    assert!(!symlinks_enabled(false, None));
    // --no-symlinks forces the copy fallback (R4).
    assert!(!symlinks_enabled(true, None));
    // HF_HUB_DISABLE_SYMLINKS disables too (E10, hub parity).
    assert!(!symlinks_enabled(false, Some("1")));
    assert!(!symlinks_enabled(false, Some("true")));
    assert!(!symlinks_enabled(false, Some(" YES ")));
    #[cfg(unix)]
    {
        assert!(symlinks_enabled(false, Some("0")));
        assert!(symlinks_enabled(false, Some("")));
    }
    assert!(!symlinks_enabled(true, Some("1")));
}

#[test]
fn absolute_path_anchors_relative_paths_at_cwd() {
    // Platform-neutral: on Unix a leading-/ path is already absolute and
    // passes through verbatim; on Windows it is drive-relative, so only
    // assert absoluteness and the trailing components (same on both).
    let abs = absolute_path(Path::new("/cache/hub"));
    assert!(abs.is_absolute(), "rooted input must come back absolute");
    assert!(abs.ends_with("cache/hub"), "components preserved: {abs:?}");
    let rel = absolute_path(Path::new("hub/models--a--b"));
    assert!(rel.is_absolute());
    assert!(rel.ends_with("hub/models--a--b"));
}
