//! Shared end-to-end harness for the CLI integration tests.
//!
//! Extracted verbatim from `tests/cli_download.rs` (mock HuggingFace server,
//! isolated per-test environment, assertion helpers) so
//! `tests/cli_exit_codes.rs` can drive the same real-binary harness without
//! duplicating it. Existing `cli_download` test behavior is unchanged.

#![allow(dead_code)] // shared helpers are not all used by every consumer

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Fixed commit SHA served by the `/revision/` stub endpoint below
/// (hf-cache sync pins snapshots/refs to it). The download flow never
/// requests this endpoint, so the stub is behavior-neutral for the
/// pre-existing tests.
pub const MOCK_REVISION_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

// ---------------------------------------------------------------------------
// Mock model repo
// ---------------------------------------------------------------------------

pub struct FileEntry {
    pub path: String,
    pub content: Vec<u8>,
    /// SHA256 advertised in the tree API's LFS metadata (may deliberately
    /// differ from the actual content to test hash mismatches; None = no LFS
    /// pointer, i.e. no verification possible).
    pub advertised_sha256: Option<String>,
    /// Size advertised by the tree API and the resolve endpoint's
    /// Content-Range total when it differs from the actual content length
    /// (failure-injection: "advertised LFS size != served bytes" — range
    /// requests starting at/after the real content length answer 416, like
    /// the real Hub answering for a blob shorter than advertised).
    /// None = advertise the actual content length.
    pub advertised_size: Option<u64>,
}

pub struct MockRepo {
    pub model_id: String,
    pub files: Vec<FileEntry>,
    /// Respond 401 to /resolve/ requests (gated repo).
    pub gated: bool,
    /// Respond 404 to /resolve/ requests (forces the /raw/ fallback).
    pub resolve_404: bool,
    /// Sleep once on the first /resolve/ request (timeout/retry test).
    pub sleep_once: Option<Duration>,
    /// Delay before every request (stretches fast local downloads so the
    /// CLI monitor loop visibly samples progress).
    pub per_request_delay: Duration,
    /// Fixture served for `/api/models` (search endpoint). The search term
    /// is ignored (full-text matching is the real API's job) but `limit=`
    /// from the query string is honored, mirroring the upstream contract.
    pub search_results: Vec<Value>,
    /// Extra revisions (branch names, slash-free) this repo serves in
    /// addition to `main`. When set, `main` serves an EMPTY tree — the
    /// issue #28 layout where all files live on a branch. Unknown
    /// revisions 404, like the real Hub.
    pub branches: Vec<String>,
    /// Failure injection: respond this HTTP status to every range-bearing
    /// request AFTER the first one (the probe `bytes=0-0` succeeds; every
    /// subsequent chunk request fails). None = serve normally. The counter
    /// is global across paths (single-file scenarios), like a reverse
    /// proxy starting to 500 mid-download.
    pub fail_status_after_first_range: Option<u16>,
}

impl MockRepo {
    /// Tree listing for a directory path ("" = repo root), deriving
    /// directory entries from file paths — mirrors the real API where
    /// subdir listings carry full paths (`Dynamic/model.gguf`).
    pub fn tree_json_for(&self, dir: &str) -> Vec<u8> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{}/", dir)
        };
        let mut entries: Vec<Value> = Vec::new();
        let mut seen_dirs = std::collections::BTreeSet::new();

        for f in &self.files {
            if !f.path.starts_with(&prefix) {
                continue;
            }
            let rest = &f.path[prefix.len()..];
            if let Some(slash) = rest.find('/') {
                // First-level subdirectory: one directory entry per name
                let dir_name = &rest[..slash];
                if seen_dirs.insert(dir_name.to_string()) {
                    entries.push(json!({
                        "type": "directory",
                        "path": format!("{}{}", prefix, dir_name),
                        "size": 0,
                    }));
                }
            } else {
                let size = f.advertised_size.unwrap_or(f.content.len() as u64);
                let mut entry = json!({
                    "type": "file",
                    "path": f.path,
                    "size": size,
                });
                if let Some(oid) = &f.advertised_sha256 {
                    entry["lfs"] = json!({
                        "oid": oid,
                        "size": size,
                        "pointerSize": 136,
                    });
                }
                entries.push(entry);
            }
        }
        serde_json::to_vec(&entries).unwrap()
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Parse a `Range: bytes=start-end` header.
pub fn parse_range(value: &str) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

pub async fn handle(req: Request<Body>, repo: Arc<MockRepo>) -> Response<Body> {
    let path = req.uri().path().to_string();

    // Search endpoint (exact match; /api/models/{id} is the metadata route)
    if path == "/api/models" {
        let limit = req
            .uri()
            .query()
            .and_then(|q| q.split('&').find(|p| p.starts_with("limit=")))
            .and_then(|p| p.split('=').nth(1))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100);
        let results: Vec<Value> = repo.search_results.iter().take(limit).cloned().collect();
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&results).unwrap()))
            .unwrap();
    }

    let api_prefix = format!("/api/models/{}", repo.model_id);
    if path == api_prefix {
        let body = json!({ "id": repo.model_id });
        return response_json(StatusCode::OK, &body);
    }
    // Revision-resolution endpoint (hf-cache sync pins refs/snapshots to
    // MOCK_REVISION_SHA). Never requested by the download flow.
    if path.starts_with(&format!("{api_prefix}/revision/")) {
        return response_json(StatusCode::OK, &json!({ "sha": MOCK_REVISION_SHA }));
    }
    if let Some(rest) = path.strip_prefix(&format!("{}/tree/", api_prefix)) {
        // rest is "<rev>" or "<rev>/<subdir>" (mock branches are slash-free)
        let (rev, subdir) = match rest.split_once('/') {
            Some((rev, subdir)) => (rev, subdir),
            None => (rest, ""),
        };
        let known = rev == "main" || repo.branches.iter().any(|b| b == rev);
        // With branches configured, main is the empty branch (issue #28)
        let empty_main = rev == "main" && !repo.branches.is_empty();
        if !known || empty_main {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap();
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(repo.tree_json_for(subdir)))
            .unwrap();
    }

    // main plus any configured branch revision (mock branches are slash-free);
    // MOCK_REVISION_SHA is accepted too because `hf-cache sync` downloads via
    // the commit SHA resolved by the /revision/ stub above.
    let mut revisions = vec!["main".to_string(), MOCK_REVISION_SHA.to_string()];
    revisions.extend(repo.branches.iter().cloned());

    let mut file_path: Option<(String, bool)> = None;
    for rev in &revisions {
        if let Some(rest) = path.strip_prefix(&format!("/{}/resolve/{}/", repo.model_id, rev)) {
            file_path = Some((rest.to_string(), false));
            break;
        }
        if let Some(rest) = path.strip_prefix(&format!("/{}/raw/{}/", repo.model_id, rev)) {
            file_path = Some((rest.to_string(), true));
            break;
        }
    }
    let (file_path, is_raw) = match file_path {
        Some(pair) => pair,
        None => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap();
        }
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

    // Served length is the ACTUAL content; the advertised total (tree
    // `size`/`lfs.size` and the Content-Range total below) may be larger
    // (failure-injection knob `advertised_size`). Ranges starting at/after
    // the served length answer 416 with the `bytes */served` header — the
    // real Hub's shape for a blob shorter than advertised. A straddling
    // range serves up to the last served byte (the real Hub clamps `end`).
    let served_total = entry.content.len() as u64;
    let advertised_total = entry.advertised_size.unwrap_or(served_total);
    let (status, body, content_range) = match range {
        Some((start, _)) if start >= served_total => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            Vec::new(),
            format!("bytes */{}", served_total),
        ),
        Some((start, end)) => {
            let end = end.min(served_total - 1);
            let slice = entry.content[start as usize..=(end as usize)].to_vec();
            (
                StatusCode::PARTIAL_CONTENT,
                slice,
                format!("bytes {}-{}/{}", start, end, advertised_total),
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

pub fn response_json(status: StatusCode, value: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(value).unwrap()))
        .unwrap()
}

/// Spawn the mock server; returns its base URL (`HF_ENDPOINT` value).
pub async fn spawn_mock(repo: MockRepo) -> String {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));

    // One-shot stall flag for the timeout/retry test; extracted before the
    // repo is frozen behind an Arc.
    let sleep_once = repo.sleep_once;
    // Failure injection (see MockRepo::fail_status_after_first_range):
    // counts range-bearing requests so exactly the first (the transport's
    // probe) succeeds.
    let fail_after_first = repo.fail_status_after_first_range;
    let range_requests = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let repo = Arc::new(repo);
    let sleep_flag = Arc::new(Mutex::new(sleep_once));

    let make_service = make_service_fn(move |_| {
        let repo = repo.clone();
        let sleep_flag = sleep_flag.clone();
        let range_requests = range_requests.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let repo = repo.clone();
                let sleep_flag = sleep_flag.clone();
                let range_requests = range_requests.clone();
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
                    // Failure injection: the transport's probe (`bytes=0-0`)
                    // is the FIRST range-bearing request; every later one
                    // (the chunk requests) gets the injected status.
                    if let Some(status) = fail_after_first {
                        if req.headers().contains_key("range") {
                            let seen =
                                range_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            if seen >= 1 {
                                return Ok::<Response<Body>, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::from_u16(status).unwrap())
                                        .body(Body::empty())
                                        .unwrap(),
                                );
                            }
                        }
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

pub struct TestEnv {
    /// Per-test isolated home (config + models); also used by consumers
    /// that need extra per-test dirs (e.g. a `--cache-dir` under it).
    pub home: PathBuf,
    endpoint: String,
}

impl TestEnv {
    /// Create an isolated HOME with an engine config tuned for small
    /// fixtures: 1 KiB chunks so a 100 KB file splits into many chunks.
    pub fn new(endpoint: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "hf-cli-e2e-{}-{}",
            std::process::id(),
            nanos_suffix()
        ));
        std::fs::create_dir_all(home.join("config")).unwrap();

        let config = format!(
            r#"
default_directory = '{models_dir}'
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
        std::fs::write(home.join("config/config.toml"), config).unwrap();

        Self {
            home,
            endpoint: endpoint.to_string(),
        }
    }

    /// Override scalar engine options in the child config (timeout/retry tests).
    pub fn set(&self, key: &str, value: &str) {
        let path = self.home.join("config/config.toml");
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

    pub fn models_dir(&self) -> PathBuf {
        self.home.join("models")
    }

    pub fn registry_toml(&self) -> String {
        std::fs::read_to_string(self.home.join("models/hf-downloads.toml")).unwrap_or_default()
    }

    /// Run the real binary headlessly with this environment.
    pub async fn run(&self, args: &[&str]) -> (i32, String, String) {
        let binary = env!("CARGO_BIN_EXE_rust-hf-downloader");
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(binary)
                .args(args)
                .env("RUST_HF_DOWNLOADER_CONFIG_DIR", self.home.join("config"))
                .env("RUST_HF_DOWNLOADER_DATA_DIR", self.home.join("models"))
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

/// Unique per-process suffix for test-home directories.
///
/// Incident #37 root cause: `SystemTime::as_nanos()` alone COLLIDES when
/// two tests construct a `TestEnv` in the same clock tick — the tests then
/// share one home, one test's cleanup deletes the other's downloads
/// mid-flight (sporadic ENOENT / missing files / clobbered registry),
/// reproducing only under parallel load. The atomic counter guarantees
/// in-process uniqueness; the pid covers cross-process runs.
pub fn nanos_suffix() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (nanos << 21) | u128::from(seq) // counter can never collide within the process
}

pub fn fixture_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

/// A ModelInfo-shaped fixture for the search endpoint.
pub fn model(id: &str, downloads: u64, likes: u64) -> Value {
    json!({
        "id": id,
        "author": id.split('/').next(),
        "downloads": downloads,
        "likes": likes,
        "tags": [],
        "lastModified": "2026-08-14T10:00:00Z",
    })
}

/// Parse stdout into NDJSON events; panics on any non-JSON line.
pub fn json_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {:?}: {}", line, e))
        })
        .collect()
}

/// Assert the child's exit code, dumping BOTH streams on failure.
///
/// Incident #37: the Windows-only flake produced exit 1 with empty stderr
/// because JSON-mode error events go to stdout — asserts that only printed
/// stderr were blind.
pub fn assert_exit_code(code: i32, expected: i32, stdout: &str, stderr: &str) {
    assert_eq!(
        code, expected,
        "exit {code} != {expected}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Find the first event of `ty`, dumping every event type present (plus the
/// raw stdout) on failure. Incident #37: bare `.unwrap()` on the find
/// printed nothing, hiding whether e.g. a `verification_error` (file
/// reported missing) replaced the expected `verification_result`.
pub fn event_of<'a>(events: &'a [Value], ty: &str, stdout: &str) -> &'a Value {
    events.iter().find(|e| e["type"] == ty).unwrap_or_else(|| {
        let present: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
        panic!("no {ty:?} event; event types present: {present:?}\n--- stdout ---\n{stdout}")
    })
}

pub fn assert_file_content(path: &Path, expected: &[u8]) {
    let actual = std::fs::read(path).unwrap_or_else(|e| {
        // Incident #37 diagnostics: list the directory so a missing file is
        // distinguishable from a renamed-elsewhere file (e.g. an orphaned
        // `.incomplete` sibling).
        let mut siblings = Vec::new();
        if let Some(parent) = path.parent() {
            if let Ok(dir) = std::fs::read_dir(parent) {
                siblings.extend(dir.flatten().map(|d| d.path().display().to_string()));
            }
        }
        panic!(
            "read {}: {}\ndirectory contents of {}: {:?}",
            path.display(),
            e,
            path.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            siblings
        )
    });
    assert_eq!(
        actual,
        expected,
        "file content mismatch at {}",
        path.display()
    );
}
