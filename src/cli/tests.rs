use clap::{CommandFactory, Parser};
use std::path::Path;

use crate::fmt::{bar_cli, eta_cli, truncate_path_cli};
use crate::models::{ModelMetadata, QuantizationGroup};

use super::args::{
    apply_rate_limit_overrides, merge_token, parse_rate_limit_mbps, parse_revision, valid_model_id,
    DownloadArgs, HfCacheArgs, HfCacheCommand, ModelDto, RateLimitArgs, RunOutputArgs,
};
use super::events::{Event, FileDto, FileStatus, OverallProgress, Summary};
use super::hf_cache::{
    absolute_path, ref_name_for_revision, select_sync_files, symlinks_enabled, tree_file_dtos,
    SelectionMode, SyncSelectionError,
};
use super::report::{
    format_file_progress, format_overall_progress, verification_heartbeat_line, ProgressMode,
    Reporter,
};
use super::resolve::{
    parse_selector, resolve_files, FileSpec, ResolveError, SelectionError, Selector,
};
use super::search_cmd::effective_search_params;
use super::{Cli, Command};

fn file_spec(filename: &str, size: u64) -> FileSpec {
    FileSpec {
        filename: filename.to_string(),
        size_bytes: size,
        sha256: None,
    }
}

fn metadata_with(files: &[(&str, Option<u64>)]) -> ModelMetadata {
    ModelMetadata {
        model_id: "a/b".to_string(),
        library_name: None,
        pipeline_tag: None,
        card_data: None,
        siblings: files
            .iter()
            .map(|(name, size)| crate::models::RepoFile {
                rfilename: name.to_string(),
                size: *size,
                oid: None,
                lfs: None,
            })
            .collect(),
        tags: Vec::new(),
        sha: None,
    }
}

// --- resolve_files -----------------------------------------------------

#[test]
fn resolve_default_single_file_repo() {
    let metadata = metadata_with(&[("only.gguf", Some(10))]);
    let files = resolve_files(&metadata, &[], &Selector::Default).unwrap();
    assert_eq!(files, vec![file_spec("only.gguf", 10)]);
}

#[test]
fn resolve_default_multi_file_repo_is_ambiguous_with_list() {
    let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
    let err = resolve_files(&metadata, &[], &Selector::Default).unwrap_err();
    match err {
        ResolveError::Ambiguous { available } => {
            assert_eq!(available.len(), 2);
            assert_eq!(available[1].filename, "b.gguf");
        }
        other => panic!("expected Ambiguous, got {:?}", other),
    }
}

#[test]
fn resolve_quant_case_insensitive_from_groups() {
    let metadata = metadata_with(&[]);
    let quants = vec![QuantizationGroup {
        quant_type: "Q4_K_M".to_string(),
        files: vec![
            crate::models::QuantizationInfo {
                quant_type: "Q4_K_M".to_string(),
                filename: "m-00001-of-00002.gguf".to_string(),
                size: 1,
                sha256: Some("dead".to_string()),
            },
            crate::models::QuantizationInfo {
                quant_type: "Q4_K_M".to_string(),
                filename: "m-00002-of-00002.gguf".to_string(),
                size: 2,
                sha256: None,
            },
        ],
        total_size: 3,
    }];
    let files = resolve_files(&metadata, &quants, &Selector::Quant("q4_k_m".to_string())).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].sha256.as_deref(), Some("dead"));
    // all parts of the quantization are selected
    assert_eq!(files[1].filename, "m-00002-of-00002.gguf");
}

#[test]
fn resolve_quant_miss_lists_available() {
    let metadata = metadata_with(&[("a.gguf", Some(10))]);
    let err = resolve_files(&metadata, &[], &Selector::Quant("Q8_0".to_string())).unwrap_err();
    assert!(matches!(err, ResolveError::NoFilesMatch { .. }));
    assert_eq!(err.code(), "no_files_match");
}

#[test]
fn file_spec_from_repo_file_pins_the_sibling_mapping() {
    // W4.9 collapsed the twin sibling-mapping loops (resolve_files'
    // `available` and hf-cache selection's `tree_file_dtos`) into one
    // `From<&RepoFile> for FileSpec`. Pin the mapped values — including
    // size:None → 0 and lfs:None → sha256:None — and that both consumers
    // agree on every non-directory fixture.
    let lfs = |oid: &str| crate::models::LfsInfo {
        oid: oid.to_string(),
        size: 123,
        pointer_size: 132,
    };
    let repo_file = |name: &str, size: Option<u64>, lfs: Option<crate::models::LfsInfo>| {
        crate::models::RepoFile {
            rfilename: name.to_string(),
            size,
            oid: None,
            lfs,
        }
    };
    let with_lfs = repo_file("model.Q4_K_M.gguf", Some(4_947), Some(lfs("cafebabe")));
    let bare = repo_file("plain.bin", None, None);
    let sized_no_lfs = repo_file("non-lfs.safetensors", Some(42), None);

    assert_eq!(
        FileSpec::from(&with_lfs),
        FileSpec {
            filename: "model.Q4_K_M.gguf".to_string(),
            size_bytes: 4_947,
            sha256: Some("cafebabe".to_string()),
        }
    );
    assert_eq!(
        FileSpec::from(&bare),
        FileSpec {
            filename: "plain.bin".to_string(),
            size_bytes: 0,
            sha256: None,
        }
    );
    assert_eq!(
        FileSpec::from(&sized_no_lfs),
        FileSpec {
            filename: "non-lfs.safetensors".to_string(),
            size_bytes: 42,
            sha256: None,
        }
    );

    // The hf-cache selection payload (tree_file_dtos) is the same mapping
    // on the wire (FileDto::from(&FileSpec::from(f))), directory markers
    // filtered.
    let metadata = ModelMetadata {
        model_id: "a/b".to_string(),
        library_name: None,
        pipeline_tag: None,
        card_data: None,
        siblings: vec![
            with_lfs,
            bare,
            sized_no_lfs,
            repo_file("subdir/", Some(1), None),
        ],
        tags: Vec::new(),
        sha: None,
    };
    assert_eq!(
        tree_file_dtos(&metadata),
        vec![
            FileDto {
                filename: "model.Q4_K_M.gguf".to_string(),
                size_bytes: 4_947,
                sha256: Some("cafebabe".to_string()),
            },
            FileDto {
                filename: "plain.bin".to_string(),
                size_bytes: 0,
                sha256: None,
            },
            FileDto {
                filename: "non-lfs.safetensors".to_string(),
                size_bytes: 42,
                sha256: None,
            },
        ]
    );
}

#[test]
fn resolve_quant_mmproj_selects_all_projector_groups() {
    // Issue #25: `--quant mmproj` spans every MMPROJ* group; exact names
    // (MMPROJ-Q8_0) still match directly and never pull weight files in.
    let metadata = metadata_with(&[
        ("model.Q8_0.gguf", Some(10)),
        ("model.mmproj-Q8_0.gguf", Some(2)),
        ("mmproj-F32.gguf", Some(3)),
    ]);
    let quants = crate::api::classify_quantizations(&metadata.siblings);

    let files = resolve_files(&metadata, &quants, &Selector::Quant("mmproj".to_string())).unwrap();
    assert_eq!(
        files,
        vec![
            file_spec("mmproj-F32.gguf", 3),
            file_spec("model.mmproj-Q8_0.gguf", 2)
        ]
    );

    let files = resolve_files(
        &metadata,
        &quants,
        &Selector::Quant("MMPROJ-Q8_0".to_string()),
    )
    .unwrap();
    assert_eq!(files, vec![file_spec("model.mmproj-Q8_0.gguf", 2)]);
}

#[test]
fn resolve_files_exact_and_missing() {
    let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
    let files =
        resolve_files(&metadata, &[], &Selector::Files(vec!["b.gguf".to_string()])).unwrap();
    assert_eq!(files, vec![file_spec("b.gguf", 20)]);

    let err = resolve_files(
        &metadata,
        &[],
        &Selector::Files(vec!["nope.gguf".to_string()]),
    )
    .unwrap_err();
    assert_eq!(err.code(), "no_files_match");
    assert_eq!(err.available().len(), 2);
}

#[test]
fn resolve_files_dedups_repeatable_selectors() {
    let metadata = metadata_with(&[("a.gguf", Some(10))]);
    let files = resolve_files(
        &metadata,
        &[],
        &Selector::Files(vec!["a.gguf".to_string(), "a.gguf".to_string()]),
    )
    .unwrap();
    assert_eq!(files.len(), 1);
}

#[test]
fn resolve_all_skips_directories_and_unsized() {
    let metadata = metadata_with(&[
        ("a.gguf", Some(10)),
        ("subdir/", None), // directory marker
        ("broken", None),  // no size → skipped
    ]);
    let files = resolve_files(&metadata, &[], &Selector::All).unwrap();
    assert_eq!(files, vec![file_spec("a.gguf", 10)]);
}

// --- selectors / args ---------------------------------------------------

fn download_args(quant: Option<&str>, file: &[&str], all: bool) -> DownloadArgs {
    DownloadArgs {
        model_id: "a/b".to_string(),
        quant: quant.map(String::from),
        file: file.iter().map(|f| f.to_string()).collect(),
        all,
        run_output: RunOutputArgs {
            progress: ProgressMode::Auto,
            token: None,
            no_verify: false,
            json: false,
            quiet: false,
        },
        rate_limits: RateLimitArgs {
            rate_limit: false,
            no_rate_limit: false,
            rate_limit_mbps: None,
        },
        output: None,
        revision: None,
    }
}

#[test]
fn selector_default_when_none_given() {
    assert_eq!(
        parse_selector(&download_args(None, &[], false)).unwrap(),
        Selector::Default
    );
}

#[test]
fn selector_conflicts_rejected() {
    assert!(parse_selector(&download_args(Some("Q4"), &[], true)).is_err());
    assert!(parse_selector(&download_args(None, &["a.gguf"], true)).is_err());
    assert!(parse_selector(&download_args(Some("Q4"), &["a.gguf"], false)).is_err());
}

#[test]
fn model_id_validation() {
    assert!(valid_model_id("a/b"));
    assert!(!valid_model_id("a"));
    assert!(!valid_model_id("a/b/c"));
    assert!(!valid_model_id("/b"));
    assert!(!valid_model_id("a/"));
    assert!(!valid_model_id(""));
}

#[test]
fn token_precedence_flag_env_then_file() {
    let f = Some("flag".to_string());
    let e = Some("env".to_string());
    let c = Some("cfg".to_string());
    assert_eq!(
        merge_token(f.clone(), e.clone(), c.clone()).as_deref(),
        Some("flag")
    );
    assert_eq!(
        merge_token(None, e.clone(), c.clone()).as_deref(),
        Some("env")
    );
    assert_eq!(merge_token(None, None, c.clone()).as_deref(), Some("cfg"));
    assert_eq!(merge_token(None, None, None), None);
    // empty strings are treated as absent
    assert_eq!(
        merge_token(Some(String::new()), e, c).as_deref(),
        Some("env")
    );
}

// --- Token-precedence matrix (Runner gate: W3.7+W4.2, rebound to the
// PRODUCTION resolvers by the T1 test-hardening pass) -----------------
//
// The token precedence `--token` flag > `$HF_TOKEN` env > config file is
// consumed by exactly TWO production functions (post-W4.2 Runner):
//
// | call sites                                    | production fn             |
// |-----------------------------------------------|---------------------------|
// | `run_download` (download_cmd.rs),             | `run::load_run_config`    |
// | `run_hf_cache_sync` (hf_cache/sync.rs)        | (full bootstrap: load →   |
// |                                               | output/rate overrides →   |
// |                                               | merge → writeback →       |
// |                                               | apply_options →           |
// |                                               | no-verify store)          |
// | `run_hf_cache_path` (hf_cache/path.rs ~51),   | `run::resolve_run_token`  |
// | `run_search` (search_cmd.rs ~80)              | over config-loaded        |
// |                                               | options                   |
//
// The two tests below drive THOSE functions over the full 27-cell grid
// {flag set/empty/unset} × {env set/empty/unset} × {config file with
// token / file without token / no file} and assert the literal expected
// value per cell (the expected-value table IS the falsifiable oracle —
// no hand-copied composition is involved anywhere). They replace the
// pre-T1 tests, which transcribed each site's composition by hand and
// therefore could not fail on drift in the production call sites
// themselves; the old cross-site "all four compositions agree" test was
// unfalsifiable in the same way (it compared the hand copies to each
// other) and is deleted — both families here compare against literals.
//
// §8.8 subtlety this matrix pins: `AppOptions::default()` itself reads
// `HF_TOKEN`, so the "no config file" column carries the env token in
// `options.hf_token` BEFORE `merge_token` runs. That dual read is
// unobservable through the resolved token (the env axis wins over the
// file axis either way), which is exactly why it is declared safe to keep
// (removing it is a behavior change requiring sign-off, not this pass).
/// Restore one env var on drop (matrix cells mutate the ambient env; tests
/// that do so share `paths::ENV_MUTEX`).
struct VarGuard {
    key: &'static str,
    saved: Option<std::ffi::OsString>,
}

impl VarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let saved = std::env::var_os(key);
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        Self { key, saved }
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// The config-file axis of the matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConfigFile {
    /// config.toml exists and carries `hf_token = "cfg"`.
    WithToken,
    /// config.toml exists without an `hf_token` key (serde → None).
    WithoutToken,
    /// No config.toml at all → `AppOptions::default()` (the §8.8 env read).
    NoFile,
}

/// Expected resolved token for one matrix cell. Pure function of the
/// documented precedence: first non-empty of flag > env > file, where the
/// file axis is "cfg" / None / non-empty-env respectively (§8.8 default
/// read), and empty strings count as absent at every level.
fn nonempty_token(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.is_empty())
}

fn expected_token(flag: Option<&str>, env: Option<&str>, config: ConfigFile) -> Option<String> {
    let file = match config {
        ConfigFile::WithToken => Some("cfg"),
        ConfigFile::WithoutToken => None,
        ConfigFile::NoFile => nonempty_token(env),
    };
    nonempty_token(flag)
        .or(nonempty_token(env))
        .or(file)
        .map(|s| s.to_string())
}

/// Install one matrix cell's ambient state: the temp config dir override,
/// `HF_TOKEN`, and (unless `NoFile`) a config fixture inside it. Order
/// matters: `save_config` resolves through the ambient env, so the
/// override must be in place BEFORE the fixture write. The returned
/// guards restore the env when dropped; the caller cleans up the temp dir.
fn install_matrix_cell(
    env: Option<&str>,
    config: ConfigFile,
    tag: &str,
) -> (std::path::PathBuf, VarGuard, VarGuard) {
    // `{env:?}` renders `Some("env")`; the Debug quotes are invalid in
    // Windows filenames (CreateFile error 123), so strip them.
    let cell = format!("{env:?}-{config:?}").replace('"', "");
    let tmp = std::env::temp_dir().join(format!("rhd-token-{tag}-{}-{cell}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp config dir");
    let g1 = VarGuard::set(crate::paths::ENV_CONFIG_DIR, Some(tmp.to_str().unwrap()));
    let g2 = VarGuard::set("HF_TOKEN", env);
    if config != ConfigFile::NoFile {
        let options = crate::models::AppOptions {
            hf_token: match config {
                ConfigFile::WithToken => Some("cfg".to_string()),
                _ => None,
            },
            ..Default::default()
        };
        crate::config::save_config(&options).expect("write config fixture");
    }
    (tmp, g1, g2)
}

/// Iterate the full 27-cell matrix: {flag: "flag"/""/unset} ×
/// {env: "env"/""/unset} × {config: token file / tokenless file / no file}.
fn token_matrix_cells(
) -> impl Iterator<Item = (Option<&'static str>, Option<&'static str>, ConfigFile)> {
    [
        (Some("flag"), Some("env"), ConfigFile::WithToken),
        (Some("flag"), Some("env"), ConfigFile::WithoutToken),
        (Some("flag"), Some("env"), ConfigFile::NoFile),
        (Some("flag"), Some(""), ConfigFile::WithToken),
        (Some("flag"), Some(""), ConfigFile::WithoutToken),
        (Some("flag"), Some(""), ConfigFile::NoFile),
        (Some("flag"), None, ConfigFile::WithToken),
        (Some("flag"), None, ConfigFile::WithoutToken),
        (Some("flag"), None, ConfigFile::NoFile),
        (Some(""), Some("env"), ConfigFile::WithToken),
        (Some(""), Some("env"), ConfigFile::WithoutToken),
        (Some(""), Some("env"), ConfigFile::NoFile),
        (Some(""), Some(""), ConfigFile::WithToken),
        (Some(""), Some(""), ConfigFile::WithoutToken),
        (Some(""), Some(""), ConfigFile::NoFile),
        (Some(""), None, ConfigFile::WithToken),
        (Some(""), None, ConfigFile::WithoutToken),
        (Some(""), None, ConfigFile::NoFile),
        (None, Some("env"), ConfigFile::WithToken),
        (None, Some("env"), ConfigFile::WithoutToken),
        (None, Some("env"), ConfigFile::NoFile),
        (None, Some(""), ConfigFile::WithToken),
        (None, Some(""), ConfigFile::WithoutToken),
        (None, Some(""), ConfigFile::NoFile),
        (None, None, ConfigFile::WithToken),
        (None, None, ConfigFile::WithoutToken),
        (None, None, ConfigFile::NoFile),
    ]
    .into_iter()
}

/// Family 1 — the query-only sites' PRODUCTION composition, run on every
/// cell: `hf_cache/path.rs` is literally
/// `resolve_run_token(args.token, &crate::config::load_config())`, and
/// `search_cmd.rs` loads the same config then calls the same resolver.
/// No hand-copied statements: this drives the real functions the real
/// callers use.
#[test]
fn token_matrix_resolve_run_token_matches_precedence_table() {
    for (flag, env, config) in token_matrix_cells() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (tmp, _g1, _g2) = install_matrix_cell(env, config, "resolve");

        let options = crate::config::load_config();
        let got = super::run::resolve_run_token(flag.map(|s| s.to_string()), &options);
        let want = expected_token(flag, env, config);
        assert_eq!(
            got, want,
            "resolve_run_token cell (flag={flag:?}, env={env:?}, config={config:?})"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Family 2 — the engine-driving sites' PRODUCTION bootstrap, run on
/// every cell: `run::load_run_config` is exactly what `run_download`
/// (with no output/rate/no-verify flags) and `run_hf_cache_sync` call,
/// and it returns the token pair (options.hf_token writeback, resolved
/// token) both sites consume. Assertions are literal per cell, so the
/// writeback (`options.hf_token == token`) is pinned too.
///
/// Full-bootstrap side effects are real but contained: `apply_options`
/// needs a tokio runtime (hence `#[tokio::test]`) and writes the shared
/// engine atomics — the pre-test snapshot is restored afterwards, and
/// every other atomics-mutating unit test shares `ENV_MUTEX` (see
/// `config::EngineGlobalsSnapshot`), so nothing leaks. The spawned
/// rate-limiter update task targets values the snapshot restores; no
/// in-process unit test reads the limiter.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn token_matrix_load_run_config_returns_written_back_pair() {
    let _env_lock = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let globals = crate::config::EngineGlobalsSnapshot::capture();

    for (flag, env, config) in token_matrix_cells() {
        let (tmp, _g1, _g2) = install_matrix_cell(env, config, "loadrun");

        let (options, token, _client) = super::run::load_run_config(
            flag.map(|s| s.to_string()),
            None, // no --output (neither token- nor engine-relevant here)
            false,
            false,
            None,
            false,
        )
        .expect("matrix tokens are all header-representable");
        let want = expected_token(flag, env, config);
        assert_eq!(
            token, want,
            "load_run_config token cell (flag={flag:?}, env={env:?}, config={config:?})"
        );
        assert_eq!(
            options.hf_token, want,
            "load_run_config writeback cell (flag={flag:?}, env={env:?}, config={config:?})"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Spot cell: `--no-verify` runs the post-merge engine store but must
    // not touch the token pair (the historically documented elision, now
    // actually executed and pinned).
    let (tmp, _g1, _g2) = install_matrix_cell(None, ConfigFile::WithToken, "noverify");
    let (options, token, _client) =
        super::run::load_run_config(None, None, false, false, None, true)
            .expect("matrix tokens are all header-representable");
    assert_eq!(token.as_deref(), Some("cfg"));
    assert_eq!(options.hf_token.as_deref(), Some("cfg"));
    let _ = std::fs::remove_dir_all(&tmp);

    globals.restore();
}

// --- CLI parsing --------------------------------------------------------

// --- Rate-limit overrides (issue #26) ---------------------------------

#[test]
fn rate_limit_overrides() {
    let mut options = crate::models::AppOptions::default();
    assert!(!options.download_rate_limit_enabled); // default: off
    assert_eq!(options.download_rate_limit_mbps, 50.0);

    // --rate-limit-mbps implies enable and sets the rate
    apply_rate_limit_overrides(&mut options, false, false, Some(12.5));
    assert!(options.download_rate_limit_enabled);
    assert_eq!(options.download_rate_limit_mbps, 12.5);

    // --no-rate-limit disables again (rate stays but is unused)
    apply_rate_limit_overrides(&mut options, false, true, None);
    assert!(!options.download_rate_limit_enabled);

    // config-enabled survives when no flags are passed
    options.download_rate_limit_enabled = true;
    options.download_rate_limit_mbps = 42.0;
    apply_rate_limit_overrides(&mut options, false, false, None);
    assert!(options.download_rate_limit_enabled);
    assert_eq!(options.download_rate_limit_mbps, 42.0);

    // --rate-limit alone enables with the configured rate
    options.download_rate_limit_enabled = false;
    apply_rate_limit_overrides(&mut options, true, false, None);
    assert!(options.download_rate_limit_enabled);
    assert_eq!(options.download_rate_limit_mbps, 42.0);
}

#[test]
fn rate_limit_override_matrix_full_cross_product() {
    // Full precedence matrix for the two engine-driving sites that share
    // this helper (`run_download` and `run_hf_cache_sync` both call
    // `apply_rate_limit_overrides(&mut options, args.rate_limit,
    // args.no_rate_limit, args.rate_limit_mbps)` with identical argument
    // order — verified by reading both sites; `hf-cache path` and `search`
    // expose no rate-limit flags and never call it).
    //
    // Precedence, as one rule: `--no-rate-limit` forces disable; otherwise
    // `--rate-limit` OR `--rate-limit-mbps` forces enable; otherwise the
    // config-file value survives. The mbps assignment is a SEPARATE
    // unconditional step: `--rate-limit-mbps` always stores the rate, even
    // alongside `--no-rate-limit` (disable wins for `enabled`; the stored
    // rate is dead while disabled).
    //
    // Note: clap's `conflicts_with` makes the {no_rate_limit: true,
    // rate_limit: true / mbps: Some} combinations unreachable from the CLI;
    // they are still pinned here because the helper is shared and its
    // behavior on those cells must not drift.
    for config_enabled in [false, true] {
        for config_mbps in [42.0, 50.0] {
            for rate_limit in [false, true] {
                for no_rate_limit in [false, true] {
                    for mbps in [None, Some(12.5)] {
                        let mut options = crate::models::AppOptions {
                            download_rate_limit_enabled: config_enabled,
                            download_rate_limit_mbps: config_mbps,
                            ..Default::default()
                        };
                        apply_rate_limit_overrides(&mut options, rate_limit, no_rate_limit, mbps);
                        let want_enabled =
                            !no_rate_limit && (rate_limit || mbps.is_some() || config_enabled);
                        assert_eq!(
                            options.download_rate_limit_enabled, want_enabled,
                            "enabled: config={config_enabled} rate_limit={rate_limit} \
                             no_rate_limit={no_rate_limit} mbps={mbps:?}"
                        );
                        let want_mbps = if mbps.is_some() { 12.5 } else { config_mbps };
                        assert_eq!(
                            options.download_rate_limit_mbps, want_mbps,
                            "mbps: config={config_mbps} rate_limit={rate_limit} \
                             no_rate_limit={no_rate_limit} mbps={mbps:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rate_limit_mbps_parser_rejects_bad_values() {
    assert_eq!(parse_rate_limit_mbps("10").ok(), Some(10.0));
    assert_eq!(parse_rate_limit_mbps("10.5").ok(), Some(10.5));
    assert!(parse_rate_limit_mbps("0").is_err());
    assert!(parse_rate_limit_mbps("-3").is_err());
    assert!(parse_rate_limit_mbps("abc").is_err());
    assert!(parse_rate_limit_mbps("inf").is_err());
    assert!(parse_rate_limit_mbps("NaN").is_err());
}

#[test]
fn progress_mode_flag_parses_and_defaults() {
    let args = Cli::try_parse_from(["hf-downloader", "download", "a/b"]).unwrap();
    let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
        panic!("expected download subcommand");
    };
    assert_eq!(args.run_output.progress, ProgressMode::Auto);

    // explicit modes on download
    for (raw, mode) in [
        ("auto", ProgressMode::Auto),
        ("plain", ProgressMode::Plain),
        ("none", ProgressMode::None),
    ] {
        let args =
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--progress", raw]).unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.run_output.progress, mode, "--progress {raw}");
    }

    // hf-cache sync accepts it too
    let args = Cli::try_parse_from([
        "hf-downloader",
        "hf-cache",
        "sync",
        "a/b",
        "--progress",
        "plain",
    ])
    .unwrap();
    let crate::cli::Command::HfCache(hf) = args.command.expect("subcommand") else {
        panic!("expected hf-cache subcommand");
    };
    let HfCacheCommand::Sync(args) = hf.command else {
        panic!("expected hf-cache sync subcommand");
    };
    assert_eq!(args.run_output.progress, ProgressMode::Plain);

    // unknown mode is a usage error
    assert!(
        Cli::try_parse_from(["hf-downloader", "download", "a/b", "--progress", "sparkly"]).is_err()
    );
}

#[test]
fn format_file_progress_layout() {
    let line = format_file_progress("model-Q4_K_M.gguf", 1_073_741_824, 2_147_483_648, 100.0);
    // 50% of a 20-cell bar, sizes in GiB units, eta from the remaining
    // 1 GiB at 100 MiB/s = 10.24s → 10s
    assert_eq!(
        line,
        "model-Q4_K_M.gguf 50% [██████████░░░░░░░░░░] 1.00 GB/2.00 GB 100.0 MB/s eta 10s"
    );
}

#[test]
fn format_overall_progress_layout_and_clamp() {
    let overall = OverallProgress {
        files_done: 3,
        files_total: 17,
        downloaded_bytes: 10_485_760,
        total_bytes: 84_102_439_308,
    };
    let line = format_overall_progress(
        "model-00004-of-00017.safetensors",
        2_900_000_000,
        4_947_802_324,
        &overall,
        88.0,
    );
    assert_eq!(
        line,
        "[3/17 files 0% │ 10.00 MB/78.33 GB │ 88.0 MB/s eta 15m11s] ▸ model-00004-of-00017.safetensors 59%"
    );

    // Actual bytes exceeding the tree-reported total clamp at 100%
    // (Content-Range vs tree size), not 104%.
    let over = OverallProgress {
        files_done: 2,
        files_total: 2,
        downloaded_bytes: 104_857_600,
        total_bytes: 102_760_448,
    };
    let line = format_overall_progress("f.bin", 104_857_600, 102_760_448, &over, 5.0);
    assert!(line.starts_with("[2/2 files 100% │ 100.00 MB/98.00 MB"));
}

#[test]
fn verification_heartbeat_line_format() {
    assert_eq!(
        verification_heartbeat_line(2, 41),
        "verifying: 2 in flight, 41 verified"
    );
}

#[test]
fn rate_limit_flags_parse() {
    let args = Cli::try_parse_from([
        "hf-downloader",
        "download",
        "a/b",
        "--rate-limit-mbps",
        "7.25",
    ])
    .unwrap();
    let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
        panic!("expected download subcommand");
    };
    assert_eq!(args.rate_limits.rate_limit_mbps, Some(7.25));
    assert!(!args.rate_limits.rate_limit);
    assert!(!args.rate_limits.no_rate_limit);

    // --rate-limit and --no-rate-limit conflict
    assert!(Cli::try_parse_from([
        "hf-downloader",
        "download",
        "a/b",
        "--rate-limit",
        "--no-rate-limit",
    ])
    .is_err());

    // --rate-limit-mbps and --no-rate-limit conflict
    assert!(Cli::try_parse_from([
        "hf-downloader",
        "download",
        "a/b",
        "--rate-limit-mbps",
        "5",
        "--no-rate-limit",
    ])
    .is_err());
}

// --- --revision (issue #28) -------------------------------------------

#[test]
fn revision_parser_accepts_branches_tags_and_shas() {
    assert_eq!(parse_revision("main").ok(), Some("main".to_string()));
    assert_eq!(parse_revision("2.0bpw").ok(), Some("2.0bpw".to_string()));
    // slash-separated branch names are legal
    assert_eq!(
        parse_revision("release/1.0").ok(),
        Some("release/1.0".to_string())
    );
    assert_eq!(
        parse_revision("0123456789abcdef").ok(),
        Some("0123456789abcdef".to_string())
    );

    // rejections: empty, traversal, slashes at the edges, whitespace
    assert!(parse_revision("").is_err());
    assert!(parse_revision("..").is_err());
    assert!(parse_revision("a/../b").is_err());
    assert!(parse_revision("/main").is_err());
    assert!(parse_revision("main/").is_err());
    assert!(parse_revision("two words").is_err());
    assert!(parse_revision("ta\tb").is_err());
}

#[test]
fn revision_flag_parses_into_args() {
    let args =
        Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "2.0bpw"]).unwrap();
    let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
        panic!("expected download subcommand");
    };
    assert_eq!(args.revision.as_deref(), Some("2.0bpw"));

    // absent → None (engine defaults to main)
    let args = Cli::try_parse_from(["hf-downloader", "download", "a/b"]).unwrap();
    let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
        panic!("expected download subcommand");
    };
    assert_eq!(args.revision, None);

    // traversal is rejected at the parser level
    assert!(
        Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "..",]).is_err()
    );
}

#[test]
fn version_string_matches_cargo_pkg_version() {
    // v1 CLI drifted here (stale hardcoded version) — never again.
    let err = Cli::try_parse_from(["rust-hf-downloader", "--version"]).unwrap_err();
    assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn parses_download_subcommand_flags() {
    let cli = Cli::try_parse_from([
        "rust-hf-downloader",
        "download",
        "org/model",
        "--quant",
        "Q4_K_M",
        "--file",
        "a.gguf",
        "--file",
        "b.gguf",
        "-o",
        "/tmp/x",
        "--no-verify",
        "--json",
    ])
    .unwrap();
    match cli.command {
        Some(Command::Download(args)) => {
            assert_eq!(args.model_id, "org/model");
            assert_eq!(args.quant.as_deref(), Some("Q4_K_M"));
            assert_eq!(args.file, vec!["a.gguf", "b.gguf"]);
            assert_eq!(args.output.as_deref(), Some("/tmp/x"));
            assert!(args.run_output.no_verify);
            assert!(args.run_output.json);
        }
        other => panic!("expected download, got {:?}", other),
    }
}

#[test]
fn no_subcommand_means_tui() {
    let cli = Cli::try_parse_from(["rust-hf-downloader"]).unwrap();
    assert!(cli.command.is_none());
}

// --- search --------------------------------------------------------------

#[test]
fn search_sort_flags_map_to_shared_enums() {
    // The v1 CLI died of drift: --sort was accepted and ignored. These
    // mappings are the boundary — if the flag stops reaching the shared
    // enum, this test fails.
    for (flag, expected) in [
        ("downloads", crate::models::SortField::Downloads),
        ("likes", crate::models::SortField::Likes),
        ("modified", crate::models::SortField::Modified),
        ("name", crate::models::SortField::Name),
    ] {
        let cli = Cli::try_parse_from(["hfd", "search", "q", "--sort", flag]).unwrap();
        match cli.command {
            Some(Command::Search(args)) => {
                let mapped: crate::models::SortField = args.sort.unwrap().into();
                assert_eq!(mapped, expected, "--sort {}", flag);
            }
            other => panic!("expected search, got {:?}", other),
        }
    }
}

#[test]
fn search_direction_accepts_asc_desc_aliases() {
    for (flag, expected) in [
        ("asc", crate::models::SortDirection::Ascending),
        ("ascending", crate::models::SortDirection::Ascending),
        ("desc", crate::models::SortDirection::Descending),
        ("descending", crate::models::SortDirection::Descending),
    ] {
        let cli = Cli::try_parse_from(["hfd", "search", "q", "--direction", flag]).unwrap();
        match cli.command {
            Some(Command::Search(args)) => {
                let mapped: crate::models::SortDirection = args.direction.unwrap().into();
                assert_eq!(mapped, expected, "--direction {}", flag);
            }
            other => panic!("expected search, got {:?}", other),
        }
    }
}

#[test]
fn search_rejects_invalid_enum_values_and_limit_range() {
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--sort", "bogus"]).is_err());
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--direction", "up"]).is_err());
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "0"]).is_err());
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "501"]).is_err());
    // boundaries pass
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "1"]).is_ok());
    assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "500"]).is_ok());
}

#[test]
fn search_params_flag_beats_config_default() {
    let options = crate::models::AppOptions {
        default_sort_field: crate::models::SortField::Name,
        default_sort_direction: crate::models::SortDirection::Ascending,
        default_min_downloads: 100,
        default_min_likes: 10,
        ..crate::models::AppOptions::default()
    };

    // no flags → config defaults
    let args = Cli::try_parse_from(["hfd", "search", "q"]).unwrap();
    let Command::Search(args) = args.command.unwrap() else {
        panic!()
    };
    assert_eq!(
        effective_search_params(&args, &options),
        (
            crate::models::SortField::Name,
            crate::models::SortDirection::Ascending,
            100,
            10
        )
    );

    // explicit flags override
    let args = Cli::try_parse_from([
        "hfd",
        "search",
        "q",
        "--sort",
        "likes",
        "--direction",
        "desc",
        "--min-downloads",
        "5",
        "--min-likes",
        "0",
    ])
    .unwrap();
    let Command::Search(args) = args.command.unwrap() else {
        panic!()
    };
    assert_eq!(
        effective_search_params(&args, &options),
        (
            crate::models::SortField::Likes,
            crate::models::SortDirection::Descending,
            5,
            0
        )
    );
}

#[test]
fn search_dto_preserves_fields() {
    let info = crate::models::ModelInfo {
        id: "a/b".to_string(),
        author: Some("a".to_string()),
        downloads: 1500,
        likes: 10,
        tags: vec!["gguf".to_string()],
        last_modified: Some("2026-08-01T12:00:00Z".to_string()),
    };
    let dto = ModelDto::from(&info);
    assert_eq!(dto.id, "a/b");
    assert_eq!(dto.author.as_deref(), Some("a"));
    assert_eq!(dto.downloads, 1500);
    assert_eq!(dto.likes, 10);
    assert_eq!(dto.tags, vec!["gguf".to_string()]);
    assert_eq!(dto.last_modified.as_deref(), Some("2026-08-01T12:00:00Z"));
}

#[test]
fn snapshot_search_json_array() {
    let dtos = vec![ModelDto {
        id: "bartowski/Qwen2.5-7B-GGUF".to_string(),
        author: Some("bartowski".to_string()),
        downloads: 123_456,
        likes: 1_200,
        last_modified: Some("2026-08-14T10:00:00Z".to_string()),
        tags: vec!["gguf".to_string(), "text-generation".to_string()],
    }];
    insta::assert_snapshot!(
        "search-result-array",
        serde_json::to_string_pretty(&dtos).unwrap()
    );
}

// --- rendering helpers --------------------------------------------------

#[test]
fn bar_and_eta_render() {
    assert_eq!(bar_cli(0, 10), format!("[{}]", "░".repeat(20)));
    assert_eq!(
        bar_cli(5, 10),
        format!("[{}{}]", "█".repeat(10), "░".repeat(10))
    );
    assert_eq!(bar_cli(10, 10), format!("[{}]", "█".repeat(20)));
    assert_eq!(eta_cli(59.4), "59s");
    assert_eq!(eta_cli(95.0), "1m35s");
    assert_eq!(eta_cli(3700.0), "1h1m");
}

#[test]
fn truncate_keeps_tail() {
    assert_eq!(truncate_path_cli("short.gguf", 20), "short.gguf");
    let long = "author/model-name/subdir/file-Q4_K_M.gguf";
    let cut = truncate_path_cli(long, 20);
    assert!(cut.starts_with('…'));
    assert!(cut.ends_with("Q4_K_M.gguf"));
    assert_eq!(cut.chars().count(), 20);
}

// --- JSON event schema snapshots (insta) --------------------------------

fn snap(event: &Event, name: &str) {
    let json = serde_json::to_string_pretty(event).unwrap();
    insta::assert_snapshot!(name, json);
}

#[test]
fn snapshot_event_resolved() {
    snap(
        &Event::Resolved {
            model: "org/model".to_string(),
            files: vec![FileDto {
                filename: "model-Q4_K_M.gguf".to_string(),
                size_bytes: 4_947_802_324,
                sha256: Some("a".repeat(64)),
            }],
            total_bytes: 4_947_802_324,
        },
        "event-resolved",
    );
}

#[test]
fn snapshot_event_progress() {
    snap(
        &Event::Progress {
            filename: "model-Q4_K_M.gguf".to_string(),
            downloaded_bytes: 1_048_576,
            total_bytes: 4_947_802_324,
            speed_mbps: 62.4,
            percent: 0.021_183,
            overall: None,
        },
        "event-progress",
    );
}

#[test]
fn snapshot_event_progress_overall() {
    snap(
        &Event::Progress {
            filename: "model-00003-of-00017.safetensors".to_string(),
            downloaded_bytes: 1_048_576,
            total_bytes: 4_947_802_324,
            speed_mbps: 62.4,
            percent: 0.021_183,
            overall: Some(OverallProgress {
                files_done: 2,
                files_total: 17,
                downloaded_bytes: 10_485_760,
                total_bytes: 84_102_439_308,
            }),
        },
        "event-progress-overall",
    );
}

#[test]
fn snapshot_event_file_complete() {
    snap(
        &Event::FileComplete {
            filename: "model-Q4_K_M.gguf".to_string(),
            status: FileStatus::Downloaded,
            bytes: 4_947_802_324,
        },
        "event-file-complete",
    );
}

#[test]
fn snapshot_event_verification_result_ok_omits_hashes() {
    snap(
        &Event::VerificationResult {
            filename: "model-Q4_K_M.gguf".to_string(),
            ok: true,
            expected_sha256: None,
            actual_sha256: None,
        },
        "event-verification-ok",
    );
}

#[test]
fn snapshot_event_verification_result_mismatch_includes_hashes() {
    snap(
        &Event::VerificationResult {
            filename: "model-Q4_K_M.gguf".to_string(),
            ok: false,
            expected_sha256: Some("e".repeat(64)),
            actual_sha256: Some("a".repeat(64)),
        },
        "event-verification-mismatch",
    );
}

#[test]
fn snapshot_event_done() {
    snap(
        &Event::Done {
            summary: Summary {
                files: 3,
                downloaded: 2,
                skipped: 1,
                verified: 3,
                failed: 0,
                hash_mismatch: 0,
                total_bytes: 4_947_802_324,
            },
        },
        "event-done",
    );
}

#[test]
fn snapshot_event_error_ambiguous_lists_available() {
    snap(
        &Event::Error {
            code: "ambiguous".to_string(),
            message:
                "model has 2 downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all"
                    .to_string(),
            available: Some(vec![
                FileDto {
                    filename: "model-Q4_K_M.gguf".to_string(),
                    size_bytes: 1,
                    sha256: None,
                },
                FileDto {
                    filename: "model-Q8_0.gguf".to_string(),
                    size_bytes: 2,
                    sha256: None,
                },
            ]),
        },
        "event-error-ambiguous",
    );
}

// --- hf-cache sync -------------------------------------------------------

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// Representative repo tree exercising every §2.3 preset category.
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
    // §2.2 precedence: positional FILE… beats --include/--exclude and
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
    // mode stays WholeRepo with the universal exclude on top (§2.2).
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
    // §2.3 via patterns::VLLM_ALLOW/VLLM_IGNORE: safetensors (incl.
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

    // §2.2 precedence: --include beats the preset; --exclude applies
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
    // --exclude applies on top of positional files (§2.2): dropping the
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

// --- sync JSON events ------------------------------------------------------

#[test]
fn sync_events_serialize_with_type_tags() {
    // Smoke: serde tagging mirrors the existing events (§2.4).
    let value = serde_json::to_value(&Event::SyncPlanned {
        model: "org/model".to_string(),
        sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        files: vec![FileDto {
            filename: "model.safetensors".to_string(),
            size_bytes: 42,
            sha256: Some("a".to_string()),
        }],
        skipped: 2,
        total_bytes: 42,
    })
    .unwrap();
    assert_eq!(value["type"], "sync_planned");
    assert_eq!(value["model"], "org/model");
    assert_eq!(value["skipped"], 2);

    let value = serde_json::to_value(&Event::FilePublished {
        path: "config.json".to_string(),
        blob: "deadbeef".to_string(),
    })
    .unwrap();
    assert_eq!(value["type"], "file_published");
    assert_eq!(value["path"], "config.json");
    assert_eq!(value["blob"], "deadbeef");

    let value = serde_json::to_value(&Event::SyncComplete {
        snapshot_path: "/x/snapshots/abc".to_string(),
        revision: "main".to_string(),
        sha: "abc".to_string(),
    })
    .unwrap();
    assert_eq!(value["type"], "sync_complete");
    assert_eq!(value["snapshot_path"], "/x/snapshots/abc");
    assert_eq!(value["revision"], "main");
}

#[test]
fn snapshot_event_sync_planned() {
    snap(
        &Event::SyncPlanned {
            model: "org/model".to_string(),
            sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            files: vec![FileDto {
                filename: "model-00001-of-00002.safetensors".to_string(),
                size_bytes: 4_947_802_324,
                sha256: Some("a".repeat(64)),
            }],
            skipped: 3,
            total_bytes: 4_947_802_324,
        },
        "event-sync-planned",
    );
}

#[test]
fn snapshot_event_file_published() {
    snap(
        &Event::FilePublished {
            path: "model-00001-of-00002.safetensors".to_string(),
            blob: "a".repeat(64),
        },
        "event-file-published",
    );
}

#[test]
fn snapshot_event_sync_complete() {
    snap(
        &Event::SyncComplete {
            snapshot_path: "/home/u/.cache/huggingface/hub/models--org--model/snapshots/f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            revision: "main".to_string(),
            sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
        },
        "event-sync-complete",
    );
}

// ---------------------------------------------------------------------------
// Plain-mode stderr seam (Reporter::new_with_stderr)
// ---------------------------------------------------------------------------

/// Shareable in-memory stderr sink for `Reporter::new_with_stderr`.
#[derive(Clone, Default)]
struct SharedStderr(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedStderr {
    /// Drain and return everything written so far.
    fn take(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().drain(..).collect()).unwrap()
    }
}

impl std::io::Write for SharedStderr {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn plain_verification_heartbeat_emits_once_per_interval() {
    let sink = SharedStderr::default();
    let mut reporter =
        Reporter::new_with_stderr(false, false, ProgressMode::Plain, Box::new(sink.clone()));
    reporter.plain_verification(2, 41);
    assert_eq!(sink.take(), "verifying: 2 in flight, 41 verified\n");

    // The ~10 s plain window is shared: an immediate second heartbeat is
    // swallowed.
    reporter.plain_verification(3, 42);
    assert_eq!(sink.take(), "");
}

// --- H2: full long-help snapshots ---------------------------------------
// Byte-identical enforcement for the upcoming args-flatten refactor
// (W4.1): any change to flag order, grouping, or help text of any command
// surface fails these snapshots. Regenerate deliberately via
// `cargo insta accept` after reviewing the diff.

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

#[test]
fn cli_command_tree_passes_clap_debug_assert() {
    // clap's internal consistency check: flag conflicts, arg groups,
    // subcommand wiring. Panics on any inconsistency.
    Cli::command().debug_assert();
}

// --- H6: error-event wire contract table ---------------------------------
// Additive-only NDJSON contract (plan H6). Every `code: "…"` literal in
// src/cli/*.rs (26 construction sites: download_cmd 11, hf_cache 13,
// search_cmd 2) collapses to the 13 distinct codes below; each entry pins
// the exact serialized bytes of `Event::Error` with that code. A new code
// MUST be added here; renaming or dropping one fails this table.

#[test]
fn error_event_code_wire_contract_table() {
    const EXPECTED: &[(&str, &str)] = &[
        // download_cmd.rs
        ("usage", r#"{"type":"error","code":"usage","message":"m"}"#),
        (
            "invalid_path",
            r#"{"type":"error","code":"invalid_path","message":"m"}"#,
        ),
        (
            "interrupted",
            r#"{"type":"error","code":"interrupted","message":"m"}"#,
        ),
        (
            "download_failed",
            r#"{"type":"error","code":"download_failed","message":"m"}"#,
        ),
        (
            "hash_mismatch",
            r#"{"type":"error","code":"hash_mismatch","message":"m"}"#,
        ),
        (
            "auth_required",
            r#"{"type":"error","code":"auth_required","message":"m"}"#,
        ),
        (
            "verification_error",
            r#"{"type":"error","code":"verification_error","message":"m"}"#,
        ),
        // hf_cache/sync.rs
        (
            "plan_failed",
            r#"{"type":"error","code":"plan_failed","message":"m"}"#,
        ),
        ("io", r#"{"type":"error","code":"io","message":"m"}"#),
        (
            "publish_failed",
            r#"{"type":"error","code":"publish_failed","message":"m"}"#,
        ),
        (
            "sync_lock",
            r#"{"type":"error","code":"sync_lock","message":"m"}"#,
        ),
        // search_cmd.rs
        (
            "internal",
            r#"{"type":"error","code":"internal","message":"m"}"#,
        ),
        (
            "network",
            r#"{"type":"error","code":"network","message":"m"}"#,
        ),
    ];
    assert_eq!(EXPECTED.len(), 13, "distinct error codes drifted");
    for (code, expected) in EXPECTED {
        let event = Event::Error {
            code: (*code).to_string(),
            message: "m".to_string(),
            available: None,
        };
        let actual = serde_json::to_string(&event).unwrap();
        assert_eq!(
            &actual, *expected,
            "error code {code:?} changed its wire bytes"
        );
    }
}

#[test]
fn error_event_available_variant_wire_contract() {
    // Real usage shape (download_cmd.rs ambiguity path): `available` lists
    // FileDtos of the repo; `sha256: null` is serialized, not omitted.
    let event = Event::Error {
        code: "ambiguous".to_string(),
        message: "model has 2 downloadable file(s)".to_string(),
        available: Some(vec![FileDto {
            filename: "model-Q4_K_M.gguf".to_string(),
            size_bytes: 4_947_802_324,
            sha256: None,
        }]),
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"error","code":"ambiguous","message":"model has 2 downloadable file(s)","available":[{"filename":"model-Q4_K_M.gguf","size_bytes":4947802324,"sha256":null}]}"#
    );
}

#[test]
fn file_complete_status_wire_contract() {
    // The `status` field is a FileStatus with exactly two variants, both
    // produced at download_cmd.rs — pin their serialized bytes.
    for (status, expected) in [
        (
            FileStatus::Downloaded,
            r#"{"type":"file_complete","filename":"model-Q4_K_M.gguf","status":"downloaded","bytes":4947802324}"#,
        ),
        (
            FileStatus::AlreadyExists,
            r#"{"type":"file_complete","filename":"model-Q4_K_M.gguf","status":"already_exists","bytes":4947802324}"#,
        ),
    ] {
        let event = Event::FileComplete {
            filename: "model-Q4_K_M.gguf".to_string(),
            status,
            bytes: 4_947_802_324,
        };
        let actual = serde_json::to_string(&event).unwrap();
        assert_eq!(
            actual, expected,
            "FileComplete status {status:?} changed its wire bytes"
        );
    }
}

#[tokio::test]
async fn plain_heartbeat_skips_when_progress_snapshot_missed() {
    use super::run::{poll_once, RunTally};

    let (state, _tx) = crate::engine::EngineState::new();
    state
        .verification_queue_size
        .store(1, std::sync::atomic::Ordering::Relaxed);
    state
        .verification_progress
        .lock()
        .await
        .push(crate::models::VerificationProgress {
            filename: "a.gguf".to_string(),
            verified_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            total_bytes: 10,
            speed_mbps: 0.0,
        });

    let sink = SharedStderr::default();
    let mut reporter =
        Reporter::new_with_stderr(false, false, ProgressMode::Plain, Box::new(sink.clone()));
    let mut seen_download = None;
    let mut seen_verifying = std::collections::HashSet::new();
    let mut tally = RunTally::default();
    let index_of = std::collections::HashMap::new();

    // Hold the lock across the poll: the try_lock snapshot misses, so the
    // heartbeat must be skipped rather than print "0 in flight" (a lock
    // artifact, not a fact).
    let guard = state.verification_progress.lock().await;
    poll_once(
        &state,
        1,
        &index_of,
        &mut seen_download,
        &mut seen_verifying,
        &mut tally,
        &mut reporter,
    )
    .await;
    drop(guard);
    assert_eq!(sink.take(), "", "heartbeat printed a lock artifact");

    // With the snapshot available again, the same poll emits the heartbeat.
    poll_once(
        &state,
        1,
        &index_of,
        &mut seen_download,
        &mut seen_verifying,
        &mut tally,
        &mut reporter,
    )
    .await;
    let out = sink.take();
    assert!(
        out.contains("verifying: 1 in flight, 0 verified"),
        "got: {out:?}"
    );
}

#[test]
fn error_with_available_constructor_wire_shape() {
    let event = Event::error_with_available(
        super::events::ErrorCode::Usage,
        "m",
        vec![FileDto {
            filename: "f.gguf".to_string(),
            size_bytes: 1,
            sha256: None,
        }],
    );
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"error","code":"usage","message":"m","available":[{"filename":"f.gguf","size_bytes":1,"sha256":null}]}"#
    );
}
