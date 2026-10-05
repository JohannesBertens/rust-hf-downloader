//! End-to-end tests for the `download` CLI subcommand.
//!
//! Runs the real binary (`CARGO_BIN_EXE_rust-hf-downloader`) against an
//! in-process mock HuggingFace server (hyper, Range-request aware), with full
//! isolation via the `RUST_HF_DOWNLOADER_CONFIG_DIR` / `_DATA_DIR` env
//! overrides so config, registry, and download directory all land in
//! per-test temp dirs on every OS (HOME-based isolation stopped working
//! cross-platform once path resolution moved to `dirs` in v2.6.0).
//! See plans/add-cli.md §6.2.
//!
//! The isolated config.toml shrinks `min_chunk_size`/`max_chunk_size` so
//! small fixtures still exercise the multi-chunk download path.

mod common;

use common::{
    assert_exit_code, assert_file_content, event_of, fixture_bytes, json_lines, model, parse_range,
    sha256_hex, spawn_mock, FileEntry, MockRepo, TestEnv,
};
use serde_json::{json, Value};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_downloads_verifies_and_exits_zero() {
    // 2 MiB with 4 KiB max chunks = 512 chunk requests; a 10ms per-request
    // delay stretches the (localhost-fast) download across several monitor
    // ticks so progress events are actually observable.
    let content = fixture_bytes(2 * 1024 * 1024);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model-Q4_K_M.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::from_millis(10),
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model-Q4_K_M.gguf", "--json"])
        .await;

    assert_exit_code(code, 0, &stdout, &stderr);

    let events = json_lines(&stdout);
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"resolved"), "types: {:?}", types);
    assert!(types.contains(&"download_start"), "types: {:?}", types);
    assert!(types.contains(&"progress"), "types: {:?}", types);
    assert!(types.contains(&"file_complete"), "types: {:?}", types);
    assert!(types.contains(&"verification_result"), "types: {:?}", types);
    // done is the last event on success
    assert_eq!(types.last(), Some(&"done"), "types: {:?}", types);

    // Multi-chunk config really split the file (chunk count > 1)
    let start = event_of(&events, "download_start", &stdout);
    assert_eq!(start["size_bytes"].as_u64(), Some(2 * 1024 * 1024));

    // File on disk with exact bytes, in the author/model layout
    assert_file_content(&env.models_dir().join("a/b/model-Q4_K_M.gguf"), &content);
    // Verification passed
    let verify = event_of(&events, "verification_result", &stdout);
    assert_eq!(verify["ok"], json!(true));
    // Registry marked complete
    let registry = env.registry_toml();
    assert!(registry.contains("Complete"), "registry: {}", registry);
}

#[tokio::test]
async fn human_mode_summary_on_stdout() {
    let content = fixture_bytes(50_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "only.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    // Single-file repo: implicit selector works without --file/--all
    let (code, stdout, stderr) = env.run(&["download", "a/b"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert!(
        stdout.starts_with("Done: 1 file(s)"),
        "stdout: {:?}",
        stdout
    );
    assert!(stdout.contains("Destination:"), "stdout: {:?}", stdout);
    // Piped (non-tty) auto mode must stay silent on stderr: no \r
    // rewrites, no progress lines.
    assert!(
        !stderr.contains('\r'),
        "piped auto rewrote stderr: {stderr}"
    );
    assert!(
        !stderr.contains("MB/s"),
        "piped auto printed progress: {stderr}"
    );
    assert_file_content(&env.models_dir().join("a/b/only.gguf"), &content);
}

#[tokio::test]
async fn already_exists_skips_download_and_verifies() {
    let content = fixture_bytes(20_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    // Pre-place the exact file (registry pre-registration not needed: the
    // engine short-circuits when the final path exists)
    let dest = env.models_dir().join("a/b/model.gguf");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, &content).unwrap();

    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let events = json_lines(&stdout);
    let complete = event_of(&events, "file_complete", &stdout);
    assert_eq!(complete["status"], json!("already_exists"));
    // Existing file was still hash-verified
    let verify = event_of(&events, "verification_result", &stdout);
    assert_eq!(verify["ok"], json!(true));
}

#[tokio::test]
async fn hash_mismatch_exits_one_and_marks_registry() {
    let content = fixture_bytes(20_000);
    let wrong = sha256_hex(b"totally different bytes");
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(wrong.clone()),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);

    let events = json_lines(&stdout);
    // done first, error event is always the LAST line on failure
    let last = events.last().unwrap();
    assert_eq!(last["type"], json!("error"));
    assert_eq!(last["code"], json!("hash_mismatch"));
    // mismatch event carries both hashes
    let mismatch = event_of(&events, "verification_result", &stdout);
    assert_eq!(mismatch["ok"], json!(false));
    assert_eq!(mismatch["expected_sha256"], json!(wrong));
    assert_eq!(mismatch["actual_sha256"], json!(sha256_hex(&content)));
    assert!(env.registry_toml().contains("HashMismatch"));
    // The downloaded bytes are still on disk (engine keeps the file)
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
}

#[tokio::test]
async fn gated_repo_exits_two() {
    let content = fixture_bytes(10_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: None,
            advertised_size: None,
            content,
        }],
        gated: true,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 2, &stdout, &stderr);

    let events = json_lines(&stdout);
    let last = events.last().unwrap();
    assert_eq!(last["type"], json!("error"));
    assert_eq!(last["code"], json!("auth_required"));
}

#[tokio::test]
async fn ambiguous_selector_exits_64_with_available_list() {
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![
            FileEntry {
                path: "model-Q4_K_M.gguf".to_string(),
                advertised_sha256: None,
                advertised_size: None,
                content: fixture_bytes(10),
            },
            FileEntry {
                path: "model-Q8_0.gguf".to_string(),
                advertised_sha256: None,
                advertised_size: None,
                content: fixture_bytes(20),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--json"]).await;
    assert_exit_code(code, 64, &stdout, &stderr);

    let events = json_lines(&stdout);
    let error = events.last().unwrap();
    assert_eq!(error["type"], json!("error"));
    assert_eq!(error["code"], json!("ambiguous"));
    let available = error["available"].as_array().unwrap();
    assert_eq!(available.len(), 2);
    assert_eq!(available[0]["filename"], json!("model-Q4_K_M.gguf"));
    assert_eq!(available[1]["size_bytes"], json!(20));
}

#[tokio::test]
async fn quant_selector_downloads_only_that_quantization() {
    let q4 = fixture_bytes(30_000);
    let q8 = fixture_bytes(40_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![
            FileEntry {
                path: "model-Q4_K_M.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&q4)),
                advertised_size: None,
                content: q4.clone(),
            },
            FileEntry {
                path: "model-Q8_0.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&q8)),
                advertised_size: None,
                content: q8.clone(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--quant", "Q4_K_M", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    assert_file_content(&env.models_dir().join("a/b/model-Q4_K_M.gguf"), &q4);
    assert!(!env.models_dir().join("a/b/model-Q8_0.gguf").exists());

    let events = json_lines(&stdout);
    let resolved = event_of(&events, "resolved", &stdout);
    assert_eq!(resolved["files"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn all_selector_downloads_every_file() {
    let one = fixture_bytes(5_000);
    let two = fixture_bytes(6_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![
            FileEntry {
                path: "one.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&one)),
                advertised_size: None,
                content: one.clone(),
            },
            FileEntry {
                path: "two.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&two)),
                advertised_size: None,
                content: two.clone(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--all", "--json"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);

    assert_file_content(&env.models_dir().join("a/b/one.gguf"), &one);
    assert_file_content(&env.models_dir().join("a/b/two.gguf"), &two);

    let events = json_lines(&stdout);
    let done = event_of(&events, "done", &stdout);
    assert_eq!(done["summary"]["downloaded"], json!(2));
    assert_eq!(done["summary"]["verified"], json!(2));
}

#[tokio::test]
async fn transient_timeout_is_retried() {
    let content = fixture_bytes(20_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        // First resolve request stalls 3s; client timeout is 1s (below)
        sleep_once: Some(Duration::from_secs(3)),
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    env.set("download_timeout_secs", "1");
    env.set("retry_delay_secs", "0");
    env.set("max_retries", "3");

    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
}

#[tokio::test]
async fn no_verify_skips_verification_events() {
    let content = fixture_bytes(10_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "a/b",
            "--file",
            "model.gguf",
            "--no-verify",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let events = json_lines(&stdout);
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(
        !types.contains(&"verification_result"),
        "types: {:?}",
        types
    );
    let events = json_lines(&stdout);
    let done = event_of(&events, "done", &stdout);
    assert_eq!(done["summary"]["verified"], json!(0));
}

#[tokio::test]
async fn raw_endpoint_fallback_after_resolve_404() {
    let content = fixture_bytes(10_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            // no LFS pointer: nothing to verify anyway
            advertised_sha256: None,
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: true,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
    // Registry URL was updated to the successful raw endpoint
    assert!(
        env.registry_toml().contains("/raw/main/model.gguf"),
        "registry: {}",
        env.registry_toml()
    );
}

// --- --revision (issue #28) -----------------------------------------------

/// The issue #28 layout: `main` is empty and all files live on a branch.
/// `--revision` must switch the tree listing AND the resolve URLs.
#[tokio::test]
async fn revision_flag_downloads_from_branch() {
    let content = fixture_bytes(60_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model-2.0bpw.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: vec!["2.0bpw".to_string()],
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "a/b",
            "--revision",
            "2.0bpw",
            "--file",
            "model-2.0bpw.gguf",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let events = json_lines(&stdout);
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"file_complete"), "types: {:?}", types);

    // Correct bytes from the branch, in the standard layout
    assert_file_content(&env.models_dir().join("a/b/model-2.0bpw.gguf"), &content);

    // Registry bookkeeping keeps the branch URL and records the revision
    let registry = env.registry_toml();
    assert!(
        registry.contains("/resolve/2.0bpw/model-2.0bpw.gguf"),
        "registry: {}",
        registry
    );
    assert!(
        registry.contains("revision = \"2.0bpw\""),
        "registry: {}",
        registry
    );
    assert!(registry.contains("Complete"), "registry: {}", registry);
}

/// Without `--revision`, an empty `main` branch is the pre-#28 failure:
/// nothing to download → ambiguous/no-files exit 64.
#[tokio::test]
async fn revision_default_main_empty_exits_64() {
    let content = fixture_bytes(10_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: None,
            content: content.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: vec!["2.0bpw".to_string()],
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 64, &stdout, &stderr);
    assert!(stdout.contains("error"), "stdout: {}", stdout);
}

/// An unknown revision 404s on the tree endpoint and maps to exit 64
/// (not_found), like an unknown model.
#[tokio::test]
async fn revision_unknown_branch_exits_64() {
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: vec!["2.0bpw".to_string()],
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "a/b",
            "--revision",
            "no-such-branch",
            "--file",
            "model.gguf",
            "--json",
        ])
        .await;
    assert_exit_code(code, 64, &stdout, &stderr);
    assert!(stdout.contains("not_found"), "stdout: {}", stdout);
}

#[tokio::test]
async fn usage_errors_exit_64() {
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;
    let env = TestEnv::new(&endpoint);

    // invalid model id (no subcommand-level download attempt)
    let (code, stdout, _) = env.run(&["download", "no-slash"]).await;
    assert_eq!(code, 64);
    assert!(stdout.is_empty(), "usage errors print nothing to stdout");

    // conflicting selectors
    let (code, _, _) = env
        .run(&["download", "a/b", "--all", "--quant", "Q4"])
        .await;
    assert_eq!(code, 64);

    // unknown file
    let (code, stdout, _) = env
        .run(&["download", "a/b", "--file", "missing.gguf", "--json"])
        .await;
    assert_eq!(code, 64);
    let events = json_lines(&stdout);
    let error = events.last().unwrap();
    assert_eq!(error["code"], json!("no_files_match"));
}

/// The Range parser is the wire-level contract the chunked engine speaks.
#[test]
fn range_parser_shapes() {
    assert_eq!(parse_range("bytes=0-0"), Some((0, 0)));
    assert_eq!(parse_range("bytes=5-1023"), Some((5, 1023)));
    assert_eq!(parse_range("bytes=0-"), None);
    assert_eq!(parse_range("items=1-2"), None);
}

// ---------------------------------------------------------------------------
// Search subcommand
// ---------------------------------------------------------------------------

fn search_repo() -> MockRepo {
    MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: None,
            advertised_size: None,
            content: fixture_bytes(10),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: vec![
            model("zeta/large-model", 200_000, 5_000),
            model("alpha/small-model", 1_500, 10),
            model("mid/obscure-model", 50, 0),
        ],
        branches: Vec::new(),
    }
}

#[tokio::test]
async fn search_json_returns_array_and_applies_filters() {
    let endpoint = spawn_mock(search_repo()).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&["search", "model", "--min-downloads", "1000", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let results: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    // The 50-download model was filtered out client-side
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["id"], json!("zeta/large-model"));
    assert!(results[0]["downloads"].as_u64().unwrap() >= 1000);
}

#[tokio::test]
async fn search_sort_name_orders_client_side() {
    let endpoint = spawn_mock(search_repo()).await;
    let env = TestEnv::new(&endpoint);

    // explicit ascending: fixture order (by downloads) is re-sorted a..z
    let (code, stdout, stderr) = env
        .run(&[
            "search",
            "model",
            "--sort",
            "name",
            "--direction",
            "asc",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let results: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    let ids: Vec<&str> = results.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec!["alpha/small-model", "mid/obscure-model", "zeta/large-model"]
    );

    // default direction (config default_sort_direction = Descending) → z..a
    let (code, stdout, stderr) = env
        .run(&["search", "model", "--sort", "name", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let results: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    let ids: Vec<&str> = results.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec!["zeta/large-model", "mid/obscure-model", "alpha/small-model"]
    );
}

#[tokio::test]
async fn search_limit_is_forwarded_to_the_api() {
    let endpoint = spawn_mock(search_repo()).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&["search", "model", "--limit", "2", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // The mock honors limit= from the query string; 3 fixtures -> 2 results
    let results: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn search_human_table_and_empty_results() {
    let endpoint = spawn_mock(search_repo()).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&["search", "model", "--min-downloads", "1000"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert!(stdout.contains("MODEL ID"), "stdout: {:?}", stdout);
    assert!(stdout.contains("zeta/large-model"));
    // utils::format_number rendering in the table
    assert!(stdout.contains("200.0K"), "stdout: {:?}", stdout);
    assert!(stderr.contains("2 model(s)"), "stderr: {:?}", stderr);

    // Successful query with zero hits is still exit 0
    let (code, stdout, stderr) = env
        .run(&["search", "model", "--min-downloads", "999999999", "--json"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_eq!(stdout.trim(), "[]");

    let (code, stdout, stderr) = env
        .run(&["search", "model", "--min-downloads", "999999999"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert!(stdout.is_empty());
    assert!(stderr.contains("No models found"), "stderr: {:?}", stderr);
}

#[tokio::test]
async fn search_usage_errors_exit_64() {
    let endpoint = spawn_mock(search_repo()).await;
    let env = TestEnv::new(&endpoint);

    let (code, _, _) = env.run(&["search", "model", "--sort", "bogus"]).await;
    assert_eq!(code, 64);
    let (code, _, _) = env.run(&["search", "model", "--limit", "0"]).await;
    assert_eq!(code, 64);
}

#[tokio::test]
async fn search_network_error_emits_error_event_and_exits_one() {
    // Bind then drop a listener to get a guaranteed-closed port
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let env = TestEnv::new(&format!("http://127.0.0.1:{}", port));

    let (code, stdout, stderr) = env.run(&["search", "model", "--json"]).await;
    assert_exit_code(code, 1, &stdout, &stderr);
    let events = json_lines(&stdout);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], json!("error"));
    assert_eq!(events[0]["code"], json!("network"));
}

// ---------------------------------------------------------------------------
// Issue #25 regression: subdirectory layouts and mmproj/mxfp4 classification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_subdirectory_files_download_and_verify() {
    // Ex0bit/Qwen3.5-122B-A10B-PRISM-LITE-GGUF layout: every GGUF lives in a
    // non-quant-named `Dynamic/` directory. The quant walk used to skip it
    // entirely (zero groups, nothing downloadable).
    let weights = fixture_bytes(50_000);
    let mmproj = fixture_bytes(5_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "Ex0bit/PRISM-LITE-GGUF".to_string(),
        files: vec![
            FileEntry {
                path: "README.md".to_string(),
                advertised_sha256: None,
                advertised_size: None,
                content: b"# readme".to_vec(),
            },
            FileEntry {
                path: "Dynamic/model.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&weights)),
                advertised_size: None,
                content: weights.clone(),
            },
            FileEntry {
                path: "Dynamic/mmproj-model.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&mmproj)),
                advertised_size: None,
                content: mmproj.clone(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);

    // --file with the full nested path downloads + verifies, preserving the
    // subdirectory structure on disk
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Ex0bit/PRISM-LITE-GGUF",
            "--file",
            "Dynamic/model.gguf",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(
        &env.models_dir()
            .join("Ex0bit/PRISM-LITE-GGUF/Dynamic/model.gguf"),
        &weights,
    );
    let events = json_lines(&stdout);
    let verify = event_of(&events, "verification_result", &stdout);
    assert_eq!(verify["ok"], json!(true));

    // --quant other resolves the unclassified nested GGUF (was: dropped)
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Ex0bit/PRISM-LITE-GGUF",
            "--quant",
            "other",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let events = json_lines(&stdout);
    let resolved = event_of(&events, "resolved", &stdout);
    assert_eq!(
        resolved["files"].as_array().unwrap().len(),
        1,
        "resolved: {}",
        resolved
    );

    // --quant mmproj resolves the nested projector file
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Ex0bit/PRISM-LITE-GGUF",
            "--quant",
            "mmproj",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(
        &env.models_dir()
            .join("Ex0bit/PRISM-LITE-GGUF/Dynamic/mmproj-model.gguf"),
        &mmproj,
    );
}

#[tokio::test]
async fn mmproj_and_mxfp4_moe_quant_selectors() {
    // Sabomako/mradermacher layout: `mxfp4_moe` multiparts were silently
    // dropped, and `*.mmproj-Q8_0.gguf` was mixed into the Q8_0 weight group.
    let weights = fixture_bytes(30_000);
    let mmproj = fixture_bytes(3_000);
    let mxfp4_p1 = fixture_bytes(10_000);
    let mxfp4_p2 = fixture_bytes(11_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "Sabomako/heretic-GGUF".to_string(),
        files: vec![
            FileEntry {
                path: "model.Q8_0.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&weights)),
                advertised_size: None,
                content: weights.clone(),
            },
            FileEntry {
                path: "model.mmproj-Q8_0.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&mmproj)),
                advertised_size: None,
                content: mmproj.clone(),
            },
            FileEntry {
                path: "model.mxfp4_moe-00001-of-00002.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&mxfp4_p1)),
                advertised_size: None,
                content: mxfp4_p1.clone(),
            },
            FileEntry {
                path: "model.mxfp4_moe-00002-of-00002.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&mxfp4_p2)),
                advertised_size: None,
                content: mxfp4_p2.clone(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let base = env.models_dir().join("Sabomako/heretic-GGUF");

    // --quant Q8_0 grabs the weights ONLY (mmproj used to pollute this group)
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Sabomako/heretic-GGUF",
            "--quant",
            "Q8_0",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&base.join("model.Q8_0.gguf"), &weights);
    assert!(!base.join("model.mmproj-Q8_0.gguf").exists());

    // --quant mmproj selects the projector (own group now)
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Sabomako/heretic-GGUF",
            "--quant",
            "mmproj",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&base.join("model.mmproj-Q8_0.gguf"), &mmproj);

    // --quant mxfp4 selects BOTH multipart files (group used to vanish)
    let (code, stdout, stderr) = env
        .run(&[
            "download",
            "Sabomako/heretic-GGUF",
            "--quant",
            "mxfp4",
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let events = json_lines(&stdout);
    let resolved = event_of(&events, "resolved", &stdout);
    assert_eq!(resolved["files"].as_array().unwrap().len(), 2);
    assert_file_content(&base.join("model.mxfp4_moe-00001-of-00002.gguf"), &mxfp4_p1);
    assert_file_content(&base.join("model.mxfp4_moe-00002-of-00002.gguf"), &mxfp4_p2);
}

#[tokio::test]
async fn download_progress_plain_prints_lines_without_tty() {
    let one = fixture_bytes(50_000);
    let two = fixture_bytes(50_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![
            FileEntry {
                path: "one.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&one)),
                advertised_size: None,
                content: one.clone(),
            },
            FileEntry {
                path: "two.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&two)),
                advertised_size: None,
                content: two.clone(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        // Slow requests keep file 1 in flight across the monitor's first
        // 400 ms poll tick, so a progress event exists to render.
        per_request_delay: Duration::from_millis(250),
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    // TestEnv pipes stdout/stderr (non-tty): auto mode prints nothing,
    // plain must print aggregate progress as plain newline lines.
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--all", "--progress", "plain"])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    assert!(
        stderr.contains(" files "),
        "expected an aggregate progress line on stderr, got: {stderr}"
    );
    assert!(stderr.contains("MB/s"), "speed missing: {stderr}");
    assert!(
        !stderr.contains('\r'),
        "plain mode must not use \\r rewrites"
    );
    assert_file_content(&env.models_dir().join("a/b/one.gguf"), &one);
    assert_file_content(&env.models_dir().join("a/b/two.gguf"), &two);
}

#[tokio::test]
async fn help_renders_progress_default_exactly_once() {
    // clap renders `[default: auto]` itself (after the possible-values
    // list); the --progress doc comment must not repeat it — through
    // v2.13.0 it printed twice. Pins the dedup on both subcommands that
    // expose the flag. `--help` never touches the network.
    let env = TestEnv::new("http://127.0.0.1:1");
    for args in [
        vec!["download", "--help"],
        vec!["download", "-h"],
        vec!["hf-cache", "sync", "--help"],
    ] {
        let (code, stdout, stderr) = env.run(&args).await;
        assert_exit_code(code, 0, &stdout, &stderr);
        assert!(
            stdout.contains("--progress <MODE>"),
            "--progress missing from {args:?} help:\n{stdout}"
        );
        assert_eq!(
            stdout.matches("[default: auto]").count(),
            1,
            "[default: auto] must render exactly once in {args:?} help:\n{stdout}"
        );
    }
}

#[tokio::test]
async fn download_progress_plain_single_file_has_no_aggregate() {
    let one = fixture_bytes(50_000);
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "one.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&one)),
            advertised_size: None,
            content: one.clone(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        // Keeps the file in flight across the monitor's 400 ms poll tick so
        // a progress event exists to render.
        per_request_delay: Duration::from_millis(250),
        search_results: Vec::new(),
        branches: Vec::new(),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--progress", "plain"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // Single-file runs print the file line (bar/speed), never the
    // multi-file aggregate (" N files ").
    assert!(stderr.contains("MB/s"), "progress line missing: {stderr}");
    assert!(
        !stderr.contains(" files "),
        "single-file run printed an aggregate line: {stderr}"
    );
    assert!(
        !stderr.contains('\r'),
        "plain mode must not use \\r rewrites"
    );
    assert_file_content(&env.models_dir().join("a/b/one.gguf"), &one);
}
