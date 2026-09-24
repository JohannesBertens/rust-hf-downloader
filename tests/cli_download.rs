//! End-to-end tests for the `download` CLI subcommand.
//!
//! Runs the real binary (`CARGO_BIN_EXE_rust-hf-downloader`) against an
//! in-process mock HuggingFace server (hyper, Range-request aware), with full
//! `HOME` isolation so config, registry, and download directory all land in
//! per-test temp dirs — see plans/add-cli.md §6.2.
//!
//! The mock-HOME config.toml shrinks `min_chunk_size`/`max_chunk_size` so
//! small fixtures still exercise the multi-chunk download path.

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock model repo
// ---------------------------------------------------------------------------

struct FileEntry {
    path: String,
    content: Vec<u8>,
    /// SHA256 advertised in the tree API's LFS metadata (may deliberately
    /// differ from the actual content to test hash mismatches; None = no LFS
    /// pointer, i.e. no verification possible).
    advertised_sha256: Option<String>,
}

struct MockRepo {
    model_id: String,
    files: Vec<FileEntry>,
    /// Respond 401 to /resolve/ requests (gated repo).
    gated: bool,
    /// Respond 404 to /resolve/ requests (forces the /raw/ fallback).
    resolve_404: bool,
    /// Sleep once on the first /resolve/ request (timeout/retry test).
    sleep_once: Option<Duration>,
    /// Delay before every request (stretches fast local downloads so the
    /// CLI monitor loop visibly samples progress).
    per_request_delay: Duration,
}

impl MockRepo {
    fn tree_json(&self) -> Vec<u8> {
        let entries: Vec<Value> = self
            .files
            .iter()
            .map(|f| {
                let mut entry = json!({
                    "type": "file",
                    "path": f.path,
                    "size": f.content.len(),
                });
                if let Some(oid) = &f.advertised_sha256 {
                    entry["lfs"] = json!({
                        "oid": oid,
                        "size": f.content.len(),
                        "pointerSize": 136,
                    });
                }
                entry
            })
            .collect();
        serde_json::to_vec(&entries).unwrap()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Parse a `Range: bytes=start-end` header.
fn parse_range(value: &str) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

async fn handle(req: Request<Body>, repo: Arc<MockRepo>) -> Response<Body> {
    let path = req.uri().path().to_string();

    let api_prefix = format!("/api/models/{}", repo.model_id);
    if path == api_prefix {
        let body = json!({ "id": repo.model_id });
        return response_json(StatusCode::OK, &body);
    }
    if path == format!("{}/tree/main", api_prefix) {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(repo.tree_json()))
            .unwrap();
    }

    let resolve_prefix = format!("/{}/resolve/main/", repo.model_id);
    let raw_prefix = format!("/{}/raw/main/", repo.model_id);

    let (file_path, is_raw) = if let Some(rest) = path.strip_prefix(&resolve_prefix) {
        (rest.to_string(), false)
    } else if let Some(rest) = path.strip_prefix(&raw_prefix) {
        (rest.to_string(), true)
    } else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    };

    if repo.gated && !is_raw {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::empty())
            .unwrap();
    }
    if repo.resolve_404 && !is_raw {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    }

    let entry = match repo.files.iter().find(|f| f.path == file_path) {
        Some(entry) => entry,
        None => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap()
        }
    };

    let range = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);

    let total = entry.content.len() as u64;
    let (status, body, content_range) = match range {
        Some((start, end)) => {
            let end = end.min(total - 1);
            let slice = entry.content[start as usize..=(end as usize)].to_vec();
            (
                StatusCode::PARTIAL_CONTENT,
                slice,
                format!("bytes {}-{}/{}", start, end, total),
            )
        }
        None => (StatusCode::OK, entry.content.clone(), String::new()),
    };

    let mut builder = Response::builder().status(status);
    if !content_range.is_empty() {
        builder = builder.header("content-range", content_range);
    }
    builder.body(Body::from(body)).unwrap()
}

fn response_json(status: StatusCode, value: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(value).unwrap()))
        .unwrap()
}

/// Spawn the mock server; returns its base URL (`HF_ENDPOINT` value).
async fn spawn_mock(repo: MockRepo) -> String {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));

    // One-shot stall flag for the timeout/retry test; extracted before the
    // repo is frozen behind an Arc.
    let sleep_once = repo.sleep_once;
    let repo = Arc::new(repo);
    let sleep_flag = Arc::new(Mutex::new(sleep_once));

    let make_service = make_service_fn(move |_| {
        let repo = repo.clone();
        let sleep_flag = sleep_flag.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let repo = repo.clone();
                let sleep_flag = sleep_flag.clone();
                async move {
                    let is_resolve = req.uri().path().contains("/resolve/main/");
                    if is_resolve {
                        let mut guard = sleep_flag.lock().await;
                        if let Some(delay) = guard.take() {
                            tokio::time::sleep(delay).await;
                        }
                    }
                    if !repo.per_request_delay.is_zero() {
                        tokio::time::sleep(repo.per_request_delay).await;
                    }
                    Ok::<Response<Body>, hyper::Error>(handle(req, repo).await)
                }
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_service);
    let url = format!("http://{}", server.local_addr());
    tokio::spawn(server);
    url
}

// ---------------------------------------------------------------------------
// Test environment (fake HOME: config + registry + downloads)
// ---------------------------------------------------------------------------

struct TestEnv {
    home: PathBuf,
    endpoint: String,
}

impl TestEnv {
    /// Create an isolated HOME with an engine config tuned for small
    /// fixtures: 1 KiB chunks so a 100 KB file splits into many chunks.
    fn new(endpoint: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "hf-cli-e2e-{}-{}",
            std::process::id(),
            nanos_suffix()
        ));
        std::fs::create_dir_all(home.join(".config/jreb")).unwrap();

        let config = format!(
            r#"
default_directory = "{models_dir}"
hf_token = ""
concurrent_threads = 4
num_chunks = 8
min_chunk_size = 1024
max_chunk_size = 4096
max_retries = 5
download_timeout_secs = 300
retry_delay_secs = 0
progress_update_interval_ms = 50
verification_on_completion = true
concurrent_verifications = 2
verification_buffer_size = 65536
verification_update_interval = 16
download_rate_limit_enabled = false
download_rate_limit_mbps = 50.0
"#,
            models_dir = home.join("models").display(),
        );
        std::fs::write(home.join(".config/jreb/config.toml"), config).unwrap();

        Self {
            home,
            endpoint: endpoint.to_string(),
        }
    }

    /// Override scalar engine options in the child config (timeout/retry tests).
    fn set(&self, key: &str, value: &str) {
        let path = self.home.join(".config/jreb/config.toml");
        let mut text = std::fs::read_to_string(&path).unwrap();
        let mut updated = false;
        text = text
            .lines()
            .map(|line| {
                if line.starts_with(key) {
                    updated = true;
                    format!("{} = {}", key, value)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !updated {
            text.push_str(&format!("\n{} = {}", key, value));
        }
        std::fs::write(&path, text + "\n").unwrap();
    }

    fn models_dir(&self) -> PathBuf {
        self.home.join("models")
    }

    fn registry_toml(&self) -> String {
        std::fs::read_to_string(self.home.join("models/hf-downloads.toml")).unwrap_or_default()
    }

    /// Run the real binary headlessly with this environment.
    async fn run(&self, args: &[&str]) -> (i32, String, String) {
        let binary = env!("CARGO_BIN_EXE_rust-hf-downloader");
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(binary)
                .args(args)
                .env("HOME", &self.home)
                .env("HF_ENDPOINT", &self.endpoint)
                .env_remove("HF_TOKEN")
                .current_dir(&self.home)
                .output(),
        )
        .await
        .expect("child timed out")
        .expect("failed to spawn binary");

        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn nanos_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn fixture_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

/// Parse stdout into NDJSON events; panics on any non-JSON line.
fn json_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {:?}: {}", line, e))
        })
        .collect()
}

fn assert_file_content(path: &Path, expected: &[u8]) {
    let actual = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    assert_eq!(
        actual,
        expected,
        "file content mismatch at {}",
        path.display()
    );
}

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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::from_millis(10),
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model-Q4_K_M.gguf", "--json"])
        .await;

    assert_eq!(code, 0, "stdout:\n{}\nstderr:\n{}", stdout, stderr);

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
    let start = events
        .iter()
        .find(|e| e["type"] == "download_start")
        .unwrap();
    assert_eq!(start["size_bytes"].as_u64(), Some(2 * 1024 * 1024));

    // File on disk with exact bytes, in the author/model layout
    assert_file_content(&env.models_dir().join("a/b/model-Q4_K_M.gguf"), &content);
    // Verification passed
    let verify = events
        .iter()
        .find(|e| e["type"] == "verification_result")
        .unwrap();
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    // Single-file repo: implicit selector works without --file/--all
    let (code, stdout, stderr) = env.run(&["download", "a/b"]).await;
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(
        stdout.starts_with("Done: 1 file(s)"),
        "stdout: {:?}",
        stdout
    );
    assert!(stdout.contains("Destination:"), "stdout: {:?}", stdout);
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    // Pre-place the exact file (registry pre-registration not needed: the
    // engine short-circuits when the final path exists)
    let dest = env.models_dir().join("a/b/model.gguf");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, &content).unwrap();

    let (code, stdout, _stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_eq!(code, 0);

    let events = json_lines(&stdout);
    let complete = events
        .iter()
        .find(|e| e["type"] == "file_complete")
        .unwrap();
    assert_eq!(complete["status"], json!("already_exists"));
    // Existing file was still hash-verified
    let verify = events
        .iter()
        .find(|e| e["type"] == "verification_result")
        .unwrap();
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_eq!(code, 1, "stderr: {}", stderr);

    let events = json_lines(&stdout);
    // done first, error event is always the LAST line on failure
    let last = events.last().unwrap();
    assert_eq!(last["type"], json!("error"));
    assert_eq!(last["code"], json!("hash_mismatch"));
    // mismatch event carries both hashes
    let mismatch = events
        .iter()
        .find(|e| e["type"] == "verification_result")
        .unwrap();
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
            content,
        }],
        gated: true,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_eq!(code, 2, "stdout: {}\nstderr: {}", stdout, stderr);

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
                content: fixture_bytes(10),
            },
            FileEntry {
                path: "model-Q8_0.gguf".to_string(),
                advertised_sha256: None,
                content: fixture_bytes(20),
            },
        ],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, _stderr) = env.run(&["download", "a/b", "--json"]).await;
    assert_eq!(code, 64);

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
                content: q4.clone(),
            },
            FileEntry {
                path: "model-Q8_0.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&q8)),
                content: q8.clone(),
            },
        ],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--quant", "Q4_K_M", "--json"])
        .await;
    assert_eq!(code, 0, "stderr: {}", stderr);

    assert_file_content(&env.models_dir().join("a/b/model-Q4_K_M.gguf"), &q4);
    assert!(!env.models_dir().join("a/b/model-Q8_0.gguf").exists());

    let resolved = json_lines(&stdout)
        .into_iter()
        .find(|e| e["type"] == "resolved")
        .unwrap();
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
                content: one.clone(),
            },
            FileEntry {
                path: "two.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(&two)),
                content: two.clone(),
            },
        ],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--all", "--json"]).await;
    assert_eq!(code, 0, "stderr: {}", stderr);

    assert_file_content(&env.models_dir().join("a/b/one.gguf"), &one);
    assert_file_content(&env.models_dir().join("a/b/two.gguf"), &two);

    let done = json_lines(&stdout)
        .into_iter()
        .find(|e| e["type"] == "done")
        .unwrap();
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        // First resolve request stalls 3s; client timeout is 1s (below)
        sleep_once: Some(Duration::from_secs(3)),
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    env.set("download_timeout_secs", "1");
    env.set("retry_delay_secs", "0");
    env.set("max_retries", "3");

    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_eq!(code, 0, "stdout: {}\nstderr: {}", stdout, stderr);
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, _stderr) = env
        .run(&[
            "download",
            "a/b",
            "--file",
            "model.gguf",
            "--no-verify",
            "--json",
        ])
        .await;
    assert_eq!(code, 0);

    let events = json_lines(&stdout);
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(
        !types.contains(&"verification_result"),
        "types: {:?}",
        types
    );
    let done = json_lines(&stdout)
        .into_iter()
        .find(|e| e["type"] == "done")
        .unwrap();
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
            content: content.clone(),
        }],
        gated: false,
        resolve_404: true,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
    })
    .await;

    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_eq!(code, 0, "stdout: {}\nstderr: {}", stdout, stderr);
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
    // Registry URL was updated to the successful raw endpoint
    assert!(
        env.registry_toml().contains("/raw/main/model.gguf"),
        "registry: {}",
        env.registry_toml()
    );
}

#[tokio::test]
async fn usage_errors_exit_64() {
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![],
        gated: false,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
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
