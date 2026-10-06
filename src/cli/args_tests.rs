//! `cli::args` + clap-surface tests (M6/T1 split of `cli/tests.rs`): the
//! args helpers (`valid_model_id`, `merge_token`, rate-limit overrides,
//! `parse_revision`, `ModelDto`), the token-precedence matrix over the
//! PRODUCTION resolvers, and flag-parsing of the `download`/`search`
//! surfaces. Run: `cargo test args_tests` (or `cargo test token_matrix`).

use clap::Parser;

use super::args::{
    apply_rate_limit_overrides, merge_token, parse_rate_limit_mbps, parse_revision, valid_model_id,
    HfCacheCommand, ModelDto,
};
use super::report::ProgressMode;
use super::search_cmd::effective_search_params;
use super::testutil::VarGuard;
use super::{Cli, Command};

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
// The docs/DEFERRED.md#options-default-env-token-read subtlety this
// matrix pins: `AppOptions::default()` itself reads
// `HF_TOKEN`, so the "no config file" column carries the env token in
// `options.hf_token` BEFORE `merge_token` runs. That dual read is
// unobservable through the resolved token (the env axis wins over the
// file axis either way), which is exactly why it is declared safe to keep
// (removing it is a behavior change requiring sign-off, not this pass).

/// The config-file axis of the matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConfigFile {
    /// config.toml exists and carries `hf_token = "cfg"`.
    WithToken,
    /// config.toml exists without an `hf_token` key (serde → None).
    WithoutToken,
    /// No config.toml at all → `AppOptions::default()` (the
    /// docs/DEFERRED.md#options-default-env-token-read env read).
    NoFile,
}

/// Expected resolved token for one matrix cell. Pure function of the
/// documented precedence: first non-empty of flag > env > file, where the
/// file axis is "cfg" / None / non-empty-env respectively (the
/// docs/DEFERRED.md#options-default-env-token-read default
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
