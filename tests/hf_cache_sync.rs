//! End-to-end tests for the `hf-cache sync` / `hf-cache path` subcommands
//! (plans/hf-cache-sync.md §8, integration layer).
//!
//! Runs the real binary (`CARGO_BIN_EXE_rust-hf-downloader`) against an
//! in-process mock HuggingFace server — a slim adaptation of the
//! `tests/cli_download.rs` harness (hyper, Range-aware, `HF_ENDPOINT`).
//! The mock additionally serves the endpoints `hf-cache sync` needs —
//! `/api/models/{id}/revision/{rev}`, tree entries carrying `oid` plus
//! LFS blocks, and `/resolve/` under both the branch name and the commit
//! SHA the engine pins downloads to — and counts every `/resolve/` request
//! per file so tests can assert the R6 zero-network idempotent re-run and
//! the `--for vllm` preset's server-side selection.

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::Sha256;
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Mock model repo
// ---------------------------------------------------------------------------

/// One file in a mock revision. `lfs` mirrors the real tree API: LFS files
/// carry an `lfs {oid: sha256, size}` block (the digest the verification
/// worker checks), plain files only a git-blob `oid`.
struct FileEntry {
    path: String,
    content: Vec<u8>,
    lfs: bool,
    /// Serve *tampered* bytes from /resolve/ while the tree (and its LFS
    /// oid) still describes `content` — drives the publish-gate rejection
    /// test (bad bytes must never enter the cache).
    serve_corrupted: bool,
}

/// A branch/tag tip: served under its name AND its commit SHA (the engine
/// downloads via `/resolve/<commit-sha>/…` because `hf-cache sync` pins the
/// resolved revision; the tree walk uses the user-facing revision).
struct Revision {
    name: String,
    sha: String,
    files: Vec<FileEntry>,
}

/// Commit SHAs served by fixtures (40-hex, like the hub).
const MAIN_SHA: &str = "1111111111111111111111111111111111111111";
const TAG_SHA: &str = "2222222222222222222222222222222222222222";

struct MockRepo {
    model_id: String,
    /// First revision is the default (`main`).
    revisions: Vec<Revision>,
}

impl MockRepo {
    fn main(&self) -> &Revision {
        &self.revisions[0]
    }

    /// Resolve a revision by branch/tag name or commit SHA.
    fn find_revision(&self, rev: &str) -> Option<&Revision> {
        self.revisions
            .iter()
            .find(|r| r.name == rev || r.sha == rev)
    }
}

/// LFS oid: sha256 of the content (R1 — the blob name for LFS files).
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(Sha256::digest(data))
}

/// Git blob sha1 (`git hash-object`): the tree-entry `oid` for non-LFS
/// files, and their blob name in the hub cache (R1).
fn git_blob_oid(content: &[u8]) -> String {
    use sha1::Digest;
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// Tree listing for a directory path ("" = repo root), deriving directory
/// entries from file paths — the layout the recursive tree walk consumes.
/// File entries carry `oid` (git blob sha1); LFS files add the `lfs` block
/// with the sha256 oid, mirroring the real API shape `ModelFile` decodes.
fn tree_json_for(revision: &Revision, dir: &str) -> Vec<u8> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut entries: Vec<Value> = Vec::new();
    let mut seen_dirs = BTreeSet::new();

    for f in &revision.files {
        if !f.path.starts_with(&prefix) {
            continue;
        }
        let rest = &f.path[prefix.len()..];
        if let Some(slash) = rest.find('/') {
            let dir_name = &rest[..slash];
            if seen_dirs.insert(dir_name.to_string()) {
                entries.push(json!({
                    "type": "directory",
                    "path": format!("{}{}", prefix, dir_name),
                    "size": 0,
                }));
            }
        } else {
            let mut entry = json!({
                "type": "file",
                "path": f.path,
                "size": f.content.len(),
                "oid": git_blob_oid(&f.content),
            });
            if f.lfs {
                entry["lfs"] = json!({
                    "oid": sha256_hex(&f.content),
                    "size": f.content.len(),
                    "pointerSize": 136,
                });
            }
            entries.push(entry);
        }
    }
    serde_json::to_vec(&entries).unwrap()
}

/// Per-file `/resolve/` request counters — the wire-level assertion base
/// for the R6 zero-network re-run and preset selection tests.
#[derive(Clone, Default)]
struct ResolveCounts(Arc<Mutex<HashMap<String, usize>>>);

impl ResolveCounts {
    fn record(&self, file_path: &str) {
        *self
            .0
            .lock()
            .unwrap()
            .entry(file_path.to_string())
            .or_insert(0) += 1;
    }

    fn count(&self, file_path: &str) -> usize {
        *self.0.lock().unwrap().get(file_path).unwrap_or(&0)
    }

    fn total(&self) -> usize {
        self.0.lock().unwrap().values().sum()
    }
}

/// Parse a `Range: bytes=start-end` header (engine chunk contract).
fn parse_range(value: &str) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

fn response_json(status: StatusCode, value: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(value).unwrap()))
        .unwrap()
}

async fn handle(req: Request<Body>, repo: Arc<MockRepo>, counts: ResolveCounts) -> Response<Body> {
    let path = req.uri().path().to_string();
    let api_prefix = format!("/api/models/{}", repo.model_id);

    // Model info (revision-less call of fetch_model_metadata); siblings
    // are replaced by the recursive tree walk that follows.
    if path == api_prefix {
        return response_json(
            StatusCode::OK,
            &json!({"id": repo.model_id, "sha": repo.main().sha}),
        );
    }

    // Revision resolution (resolve_revision_sha) — authoritative SHA source.
    if let Some(rev) = path.strip_prefix(&format!("{api_prefix}/revision/")) {
        return match repo.find_revision(rev) {
            Some(revision) => response_json(StatusCode::OK, &json!({"sha": revision.sha})),
            None => not_found(),
        };
    }

    // Recursive tree listing: "<rev>" or "<rev>/<subdir>" (fixture
    // revisions are slash-free).
    if let Some(rest) = path.strip_prefix(&format!("{api_prefix}/tree/")) {
        let (rev, subdir) = match rest.split_once('/') {
            Some((rev, subdir)) => (rev, subdir),
            None => (rest, ""),
        };
        let Some(revision) = repo.find_revision(rev) else {
            return not_found();
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(tree_json_for(revision, subdir)))
            .unwrap();
    }

    // File bytes under every revision name AND commit SHA (the engine pins
    // downloads to the resolved SHA).
    let mut served: Option<(&Revision, String)> = None;
    for revision in &repo.revisions {
        for rev_key in [&revision.name, &revision.sha] {
            if let Some(file_path) =
                path.strip_prefix(&format!("/{}/resolve/{}/", repo.model_id, rev_key))
            {
                served = Some((revision, file_path.to_string()));
                break;
            }
        }
        if served.is_some() {
            break;
        }
    }
    let Some((revision, file_path)) = served else {
        return not_found();
    };
    let Some(entry) = revision.files.iter().find(|f| f.path == file_path) else {
        return not_found();
    };
    counts.record(&file_path);
    let serve_body: Vec<u8> = if entry.serve_corrupted {
        entry.content.iter().map(|b| !b).collect()
    } else {
        entry.content.clone()
    };

    let range = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);

    let total = serve_body.len() as u64;
    let (status, body, content_range) = match range {
        Some((start, end)) => {
            let end = end.min(total - 1);
            let slice = serve_body[start as usize..=(end as usize)].to_vec();
            (
                StatusCode::PARTIAL_CONTENT,
                slice,
                format!("bytes {}-{}/{}", start, end, total),
            )
        }
        None => (StatusCode::OK, serve_body, String::new()),
    };

    let mut builder = Response::builder().status(status);
    if !content_range.is_empty() {
        builder = builder.header("content-range", content_range);
    }
    builder.body(Body::from(body)).unwrap()
}

/// Spawn the mock server; returns its base URL (`HF_ENDPOINT` value) and
/// the per-file resolve-request counters.
async fn spawn_mock(repo: MockRepo) -> (String, ResolveCounts) {
    spawn_mock_with_delay(repo, Duration::ZERO).await
}

/// `per_request_delay` slows every mock HTTP response, keeping downloads
/// in flight across the CLI monitor's 400 ms poll tick so progress events
/// exist to render (mirrors tests/cli_download.rs's knob).
async fn spawn_mock_with_delay(
    repo: MockRepo,
    per_request_delay: Duration,
) -> (String, ResolveCounts) {
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let repo = Arc::new(repo);
    let counts = ResolveCounts::default();
    let counts_for_service = counts.clone();

    let make_service = make_service_fn(move |_| {
        let repo = repo.clone();
        let counts = counts_for_service.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let repo = repo.clone();
                let counts = counts.clone();
                async move {
                    if per_request_delay > Duration::ZERO {
                        tokio::time::sleep(per_request_delay).await;
                    }
                    Ok::<Response<Body>, hyper::Error>(handle(req, repo, counts).await)
                }
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_service);
    let url = format!("http://{}", server.local_addr());
    tokio::spawn(server);
    (url, counts)
}

// ---------------------------------------------------------------------------
// Test environment (per-test temp home + hub cache dir)
// ---------------------------------------------------------------------------

struct TestEnv {
    home: PathBuf,
    endpoint: String,
}

impl TestEnv {
    /// Isolated home with the small-fixture engine config from
    /// tests/cli_download.rs (multi-chunk downloads for tens-of-KB files).
    fn new(endpoint: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "hf-cache-e2e-{}-{}",
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

    /// Per-test hub cache directory (passed as `--cache-dir`).
    fn cache_dir(&self) -> PathBuf {
        self.home.join("hub")
    }

    /// Run the real binary headlessly with this environment.
    async fn run(&self, args: &[&str]) -> (i32, String, String) {
        let binary = env!("CARGO_BIN_EXE_rust-hf-downloader");
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(binary)
                .args(args)
                .env("RUST_HF_DOWNLOADER_CONFIG_DIR", self.home.join("config"))
                .env("RUST_HF_DOWNLOADER_DATA_DIR", self.home.join("models"))
                .env("HF_ENDPOINT", &self.endpoint)
                .env_remove("HF_TOKEN")
                .env_remove("HF_HUB_CACHE")
                .env_remove("HUGGINGFACE_HUB_CACHE")
                .env_remove("HF_HOME")
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

/// Unique per-process suffix for test-home directories (see
/// tests/cli_download.rs: same-tick nanos collide under parallel load).
fn nanos_suffix() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (nanos << 21) | u128::from(seq)
}

fn fixture_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

/// Assert the child's exit code, dumping both streams on failure.
fn assert_exit_code(code: i32, expected: i32, stdout: &str, stderr: &str) {
    assert_eq!(
        code, expected,
        "exit {code} != {expected}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Assert file content, listing sibling entries when the read fails so a
/// missing file is distinguishable from a renamed one.
fn assert_file_content(path: &Path, expected: &[u8]) {
    let actual = std::fs::read(path).unwrap_or_else(|e| {
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

/// Assert a snapshot entry per R3: on unix a relative symlink to
/// `../../blobs/<oid>` (and readable through the link — not dangling); on
/// symlink-less platforms the copied file (R4 fallback) with the same
/// bytes.
fn assert_snapshot_entry(path: &Path, blob_oid: &str, expected: &[u8]) {
    #[cfg(unix)]
    {
        let md = std::fs::symlink_metadata(path)
            .unwrap_or_else(|e| panic!("lstat {}: {}", path.display(), e));
        assert!(
            md.file_type().is_symlink(),
            "snapshot entry {} is not a symlink",
            path.display()
        );
        assert_eq!(
            std::fs::read_link(path).unwrap(),
            PathBuf::from(format!("../../blobs/{blob_oid}")),
            "symlink target of {}",
            path.display()
        );
    }
    #[cfg(not(unix))]
    {
        let _ = blob_oid;
    }
    assert_file_content(path, expected);
}

/// Sorted entry names of a directory (layout assertions).
fn sorted_entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {}", dir.display(), e))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
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

/// Find the first event of `ty`, dumping the present event types on
/// failure.
fn event_of<'a>(events: &'a [Value], ty: &str, stdout: &str) -> &'a Value {
    events.iter().find(|e| e["type"] == ty).unwrap_or_else(|| {
        let present: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
        panic!("no {ty:?} event; event types present: {present:?}\n--- stdout ---\n{stdout}")
    })
}

// ---------------------------------------------------------------------------
// Fixture repos
// ---------------------------------------------------------------------------

/// Small repo with one LFS weights file and a plain config.json on `main`.
fn plain_repo() -> MockRepo {
    let weights = fixture_bytes(48 * 1024);
    let config = br#"{"model_type":"qwen","architectures":["Qwen2ForCausalLM"]}"#.to_vec();
    MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![Revision {
            name: "main".to_string(),
            sha: MAIN_SHA.to_string(),
            files: vec![
                FileEntry {
                    path: "model.safetensors".to_string(),
                    content: weights,
                    lfs: true,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "config.json".to_string(),
                    content: config,
                    lfs: false,
                    serve_corrupted: false,
                },
            ],
        }],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// §5.2 happy path: whole-repo sync writes the normative hub layout —
/// refs/main pins the commit SHA, blobs are named by the LFS oid and the
/// git blob sha1, and snapshot entries are relative symlinks whose bytes
/// resolve to the blob content.
#[tokio::test]
async fn full_sync_writes_refs_blobs_and_snapshot_layout() {
    let weights = fixture_bytes(48 * 1024);
    let config = br#"{"model_type":"qwen","architectures":["Qwen2ForCausalLM"]}"#.to_vec();
    let lfs_oid = sha256_hex(&weights);
    let config_oid = git_blob_oid(&config);

    let (endpoint, counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let repo_dir = cache.join("models--a--b");

    // R2: refs/main holds the raw commit SHA.
    let refs_main = std::fs::read_to_string(repo_dir.join("refs/main"))
        .unwrap_or_else(|e| panic!("read refs/main: {e}"));
    assert_eq!(refs_main.trim(), MAIN_SHA);

    // R1: blobs named by LFS sha256 oid and git blob sha1 oid, with the
    // exact content (multi-chunk reassembly).
    let blobs = repo_dir.join("blobs");
    assert_eq!(
        sorted_entry_names(&blobs),
        vec![config_oid.clone(), lfs_oid.clone()]
    );
    assert_file_content(&blobs.join(&lfs_oid), &weights);
    assert_file_content(&blobs.join(&config_oid), &config);

    // R3: snapshot entries resolve to the blobs (symlink on unix).
    let snapshot = repo_dir.join("snapshots").join(MAIN_SHA);
    assert_snapshot_entry(&snapshot.join("model.safetensors"), &lfs_oid, &weights);
    assert_snapshot_entry(&snapshot.join("config.json"), &config_oid, &config);

    // §2.4: human mode prints the snapshot path as the last line
    // (component-wise tail compare — separator style is platform-dependent).
    let last = stdout.lines().last().unwrap_or_default();
    let expected_tail = Path::new("models--a--b").join("snapshots").join(MAIN_SHA);
    assert!(
        Path::new(last).ends_with(&expected_tail),
        "last stdout line: {last:?}\n--- stdout ---\n{stdout}"
    );

    // Both files were actually fetched over the wire.
    assert!(counts.count("model.safetensors") >= 1);
    assert!(counts.count("config.json") >= 1);
}

/// R6: re-running a completed sync exits 0 and performs ZERO additional
/// resolve downloads (server counter assertion) — the plan finds every
/// blob present with a matching size.
#[tokio::test]
async fn second_sync_is_a_zero_network_no_op() {
    let (endpoint, counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let first_run_total = counts.total();
    assert!(first_run_total >= 2, "first run must fetch both files");

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // Zero additional /resolve/ requests.
    assert_eq!(
        counts.total(),
        first_run_total,
        "idempotent re-run must not download anything"
    );

    let events = json_lines(&stdout);
    let planned = event_of(&events, "sync_planned", &stdout);
    assert!(
        planned["files"]
            .as_array()
            .expect("sync_planned.files")
            .is_empty(),
        "second run plans no fetches: {planned}"
    );
    assert_eq!(planned["skipped"], json!(2));
    let complete = event_of(&events, "sync_complete", &stdout);
    assert_eq!(complete["sha"], json!(MAIN_SHA));

    // Layout unchanged and still valid.
    assert_eq!(
        std::fs::read_to_string(cache.join("models--a--b/refs/main"))
            .unwrap()
            .trim(),
        MAIN_SHA
    );
}

/// §5.2 step 4: `--dry-run` exits 0, prints the planned files, and writes
/// nothing — not even CACHEDIR.TAG — under the cache dir.
#[tokio::test]
async fn dry_run_prints_the_plan_and_writes_nothing() {
    let (endpoint, counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--dry-run",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // The plan lists every file with its disposition.
    assert!(
        stdout.contains("model.safetensors"),
        "stdout must list the weights file: {stdout}"
    );
    assert!(
        stdout.contains("config.json"),
        "stdout must list the config file: {stdout}"
    );
    assert!(stdout.contains("dry run"), "stdout: {stdout}");

    // Nothing was written under the cache dir (it must not even exist),
    // and nothing was downloaded.
    assert!(
        !cache.exists(),
        "dry run must create nothing under {}",
        cache.display()
    );
    assert_eq!(counts.total(), 0);
}

/// §2.3: `--for vllm` fetches only `*.safetensors`/`*.json` — the
/// fallback `.bin` weights and the README never hit the server, and the
/// cache layout contains exactly the selected files.
#[tokio::test]
async fn for_vllm_preset_fetches_only_safetensors_and_json() {
    let weights = fixture_bytes(32 * 1024);
    let bin_weights = fixture_bytes(64 * 1024);
    let repo = MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![Revision {
            name: "main".to_string(),
            sha: MAIN_SHA.to_string(),
            files: vec![
                FileEntry {
                    path: "model.safetensors".to_string(),
                    content: weights.clone(),
                    lfs: true,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "config.json".to_string(),
                    content: br#"{"model_type":"qwen"}"#.to_vec(),
                    lfs: false,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "pytorch_model.bin".to_string(),
                    content: bin_weights,
                    lfs: true,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "README.md".to_string(),
                    content: b"# a/b".to_vec(),
                    lfs: false,
                    serve_corrupted: false,
                },
            ],
        }],
    };

    let (endpoint, counts) = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--for",
            "vllm",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // Server-side selection: allowed files fetched, ignored files never
    // requested.
    assert!(counts.count("model.safetensors") >= 1);
    assert!(counts.count("config.json") >= 1);
    assert_eq!(counts.count("pytorch_model.bin"), 0);
    assert_eq!(counts.count("README.md"), 0);

    // Cache layout holds exactly the two selected files.
    let repo_dir = cache.join("models--a--b");
    let snapshot = repo_dir.join("snapshots").join(MAIN_SHA);
    assert_eq!(
        sorted_entry_names(&snapshot),
        vec!["config.json", "model.safetensors"]
    );
    let blobs = repo_dir.join("blobs");
    assert!(blobs.join(sha256_hex(&weights)).is_file());
    assert_eq!(sorted_entry_names(&blobs).len(), 2);
    assert_file_content(&snapshot.join("model.safetensors"), &weights);
}

/// `hf-cache path` prints the snapshot path matching the synced refs —
/// pure path math, no network.
#[tokio::test]
async fn hf_cache_path_prints_the_ref_snapshot_path() {
    let (endpoint, _counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // The path command resolves through refs/main (default revision).
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "path",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let expected = cache.join("models--a--b").join("snapshots").join(MAIN_SHA);
    assert_eq!(stdout.trim(), expected.display().to_string());
    assert!(expected.is_dir(), "printed path must exist on disk");

    // Explicit --revision main resolves to the same snapshot.
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "path",
            "a/b",
            "--revision",
            "main",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_eq!(stdout.trim(), expected.display().to_string());
}

/// §2.4 JSON mode: the NDJSON stream carries SyncPlanned, FilePublished
/// (one per file), and SyncComplete, whose snapshot_path matches the
/// on-disk snapshot directory.
#[tokio::test]
async fn json_stream_carries_sync_events_matching_disk_layout() {
    let (endpoint, _counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--json",
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let events = json_lines(&stdout);
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"sync_planned"), "types: {types:?}");
    assert!(types.contains(&"file_published"), "types: {types:?}");
    assert!(types.contains(&"sync_complete"), "types: {types:?}");

    // The plan announces both files.
    let planned = event_of(&events, "sync_planned", &stdout);
    assert_eq!(planned["files"].as_array().unwrap().len(), 2);
    assert_eq!(planned["sha"], json!(MAIN_SHA));

    // One FilePublished per file, each naming its blob.
    let published: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "file_published")
        .collect();
    assert_eq!(published.len(), 2, "events: {stdout}");
    for event in &published {
        let blob = event["blob"].as_str().expect("file_published.blob");
        assert!(
            cache.join("models--a--b/blobs").join(blob).is_file(),
            "published blob {blob} must exist"
        );
    }

    // SyncComplete is the terminal event and matches the disk layout.
    assert_eq!(types.last(), Some(&"sync_complete"), "types: {types:?}");
    let complete = event_of(&events, "sync_complete", &stdout);
    let snapshot_path = complete["snapshot_path"].as_str().expect("snapshot_path");
    let expected = cache.join("models--a--b").join("snapshots").join(MAIN_SHA);
    assert_eq!(
        Path::new(snapshot_path),
        expected.as_path(),
        "SyncComplete.snapshot_path vs on-disk snapshot dir"
    );
    assert!(expected.is_dir());
    assert_eq!(complete["revision"], json!("main"));
    assert_eq!(complete["sha"], json!(MAIN_SHA));
}

/// `--revision <tag>`: refs/<tag> is written with the tag's commit SHA and
/// the snapshot directory uses that SHA (not main's); the engine pins
/// downloads to the resolved SHA.
#[tokio::test]
async fn revision_tag_syncs_the_tags_commit() {
    let weights_v1 = fixture_bytes(16 * 1024);
    let weights_v2 = fixture_bytes(24 * 1024);
    let config = br#"{"model_type":"qwen2"}"#.to_vec();
    let repo = MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![
            Revision {
                name: "main".to_string(),
                sha: MAIN_SHA.to_string(),
                files: vec![FileEntry {
                    path: "model.safetensors".to_string(),
                    content: weights_v1,
                    lfs: true,
                    serve_corrupted: false,
                }],
            },
            Revision {
                name: "v1.0".to_string(),
                sha: TAG_SHA.to_string(),
                files: vec![
                    FileEntry {
                        path: "model.safetensors".to_string(),
                        content: weights_v2.clone(),
                        lfs: true,
                        serve_corrupted: false,
                    },
                    FileEntry {
                        path: "config.json".to_string(),
                        content: config.clone(),
                        lfs: false,
                        serve_corrupted: false,
                    },
                ],
            },
        ],
    };

    let (endpoint, counts) = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--revision",
            "v1.0",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let repo_dir = cache.join("models--a--b");

    // R2: refs/v1.0 pins the tag's SHA; main's ref is untouched (absent).
    let refs_tag = std::fs::read_to_string(repo_dir.join("refs/v1.0"))
        .unwrap_or_else(|e| panic!("read refs/v1.0: {e}"));
    assert_eq!(refs_tag.trim(), TAG_SHA);
    assert!(!repo_dir.join("refs/main").exists());

    // Snapshot dir is the tag's commit, holding the tag's bytes.
    let snapshot = repo_dir.join("snapshots").join(TAG_SHA);
    assert!(snapshot.is_dir());
    assert_file_content(&snapshot.join("model.safetensors"), &weights_v2);
    assert_file_content(&snapshot.join("config.json"), &config);
    assert!(!repo_dir.join("snapshots").join(MAIN_SHA).exists());

    // Human mode's last line is the tag's snapshot path (component-wise
    // tail compare — separator style is platform-dependent).
    let last = stdout.lines().last().unwrap_or_default();
    let expected_tail = Path::new("models--a--b").join("snapshots").join(TAG_SHA);
    assert!(
        Path::new(last).ends_with(&expected_tail),
        "last stdout line: {last:?}\n--- stdout ---\n{stdout}"
    );

    // The weights really came over the wire (once under the tag's SHA).
    assert!(counts.count("model.safetensors") >= 1);
}

// ---------------------------------------------------------------------------
// Review-driven additions: nested paths (P0), publish-gate rejection,
// selection/refs/symlink-mode coverage (plan §8 gaps)
// ---------------------------------------------------------------------------

/// Nested repo paths publish depth-aware links that actually resolve —
/// the vLLM subfolder-weights case (`text_encoder/*.safetensors`) the
/// original fixed-`../../` target broke.
#[tokio::test]
async fn nested_subfolder_files_publish_readable_depth_aware_links() {
    let weights = fixture_bytes(32 * 1024);
    let cfg = br#"{"encoder":"tiny"}"#.to_vec();
    let repo = MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![Revision {
            name: "main".to_string(),
            sha: MAIN_SHA.to_string(),
            files: vec![
                FileEntry {
                    path: "text_encoder/model-00001-of-00002.safetensors".to_string(),
                    content: weights.clone(),
                    lfs: true,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "text_encoder/config.json".to_string(),
                    content: cfg.clone(),
                    lfs: false,
                    serve_corrupted: false,
                },
            ],
        }],
    };
    let (endpoint, _counts) = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);

    let snap = cache
        .join("models--a--b")
        .join("snapshots")
        .join(MAIN_SHA)
        .join("text_encoder");
    // The P0 regression assertion: read THROUGH the nested links.
    assert_file_content(&snap.join("model-00001-of-00002.safetensors"), &weights);
    assert_file_content(&snap.join("config.json"), &cfg);
    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::read_link(snap.join("config.json")).unwrap(),
            std::path::PathBuf::from("../../../blobs/").join(git_blob_oid(&cfg))
        );
    }
}

/// R5 publish gate: LFS bytes that fail the SHA256 check never enter
/// blobs/, the staged copy is deleted, and the sync fails — good files
/// still publish, but refs stay unwritten (no partial-repo pin).
#[tokio::test]
async fn publish_gate_rejects_mismatched_lfs_bytes() {
    let weights = fixture_bytes(48 * 1024);
    let config = br#"{"model_type":"qwen"}"#.to_vec();
    let repo = MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![Revision {
            name: "main".to_string(),
            sha: MAIN_SHA.to_string(),
            files: vec![
                FileEntry {
                    path: "model.safetensors".to_string(),
                    content: weights.clone(),
                    lfs: true,
                    serve_corrupted: true, // bytes on the wire != tree oid
                },
                FileEntry {
                    path: "config.json".to_string(),
                    content: config.clone(),
                    lfs: false,
                    serve_corrupted: false,
                },
            ],
        }],
    };
    let (endpoint, _counts) = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);
    assert!(
        stdout.contains("Hash mismatches: 1"),
        "summary should report the hash mismatch:\n{stdout}"
    );

    let repo_dir = cache.join("models--a--b");
    let blobs = sorted_entry_names(&repo_dir.join("blobs"));
    // The good non-LFS file published; the corrupt LFS blob did not.
    assert_eq!(blobs, vec![git_blob_oid(&config)]);
    assert!(!repo_dir.join("blobs").join(sha256_hex(&weights)).exists());
    // No refs pin for a failed sync, and no snapshot entry for the bad file.
    assert!(!repo_dir.join("refs").join("main").exists());
    let snap = repo_dir.join("snapshots").join(MAIN_SHA);
    assert_file_content(&snap.join("config.json"), &config);
    assert!(!snap.join("model.safetensors").exists());
}

/// §2.2: a pattern that selects nothing is a usage error, not an empty
/// sync — and nothing is created on disk.
#[tokio::test]
async fn empty_selection_exits_usage() {
    let (endpoint, _counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--include",
            "*.nomatch",
            "--cache-dir",
            env.cache_dir().to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 64, &stdout, &stderr);
    assert!(!env.cache_dir().join("models--a--b").exists());
}

/// R2: a raw 40-hex SHA revision syncs that commit's snapshot but writes
/// no refs/ entry (hub behavior — SHAs address snapshots directly).
#[tokio::test]
async fn raw_sha_revision_writes_no_refs() {
    let (endpoint, _counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--revision",
            MAIN_SHA,
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let repo_dir = cache.join("models--a--b");
    assert!(repo_dir
        .join("snapshots")
        .join(MAIN_SHA)
        .join("config.json")
        .exists());
    // R2: no ref is written for a raw-SHA revision — the refs/ directory
    // is not even created.
    let refs_dir = repo_dir.join("refs");
    let refs_empty = !refs_dir.exists() || sorted_entry_names(&refs_dir).is_empty();
    assert!(refs_empty, "raw-SHA revision must not write refs");
}

/// R4: --no-symlinks puts real files (not links) into snapshots/, blobs/
/// still content-addressed.
#[tokio::test]
async fn no_symlinks_mode_copies_real_files() {
    let (endpoint, _counts) = spawn_mock(plain_repo()).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--no-symlinks",
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .await;
    assert_exit_code(code, 0, &stdout, &stderr);
    let snap = cache.join("models--a--b").join("snapshots").join(MAIN_SHA);
    let cfg = snap.join("config.json");
    let md = std::fs::symlink_metadata(&cfg).expect("snapshot entry");
    assert!(
        md.file_type().is_file(),
        "R4 copy mode must be a regular file"
    );
    assert!(cfg.is_file());
    assert!(cache
        .join("models--a--b")
        .join("blobs")
        .join(git_blob_oid(
            br#"{"model_type":"qwen","architectures":["Qwen2ForCausalLM"]}"#
        ))
        .exists());
}

/// `--progress plain` on `hf-cache sync`: the sync pipeline threads the
/// flag into the same Reporter as `download` (rewired by the cli/ module
/// split). Plain mode must print aggregate newline progress with TestEnv's
/// piped stderr (where `auto` stays silent) and never use `\r` rewrites.
/// The per-request delay keeps the 768 KB weights file in flight across
/// the monitor's 400 ms tick so a progress event exists to render.
#[tokio::test]
async fn sync_progress_plain_prints_lines_without_tty() {
    let weights = fixture_bytes(768 * 1024);
    let config = br#"{"model_type":"qwen","architectures":["Qwen2ForCausalLM"]}"#.to_vec();
    let repo = MockRepo {
        model_id: "a/b".to_string(),
        revisions: vec![Revision {
            name: "main".to_string(),
            sha: MAIN_SHA.to_string(),
            files: vec![
                FileEntry {
                    path: "model.safetensors".to_string(),
                    content: weights,
                    lfs: true,
                    serve_corrupted: false,
                },
                FileEntry {
                    path: "config.json".to_string(),
                    content: config,
                    lfs: false,
                    serve_corrupted: false,
                },
            ],
        }],
    };
    let (endpoint, _counts) = spawn_mock_with_delay(repo, Duration::from_millis(30)).await;
    let env = TestEnv::new(&endpoint);
    let cache = env.cache_dir();
    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--progress",
            "plain",
        ])
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
    // The progress assertions are about a run that really synced.
    assert!(cache.join("models--a--b/refs/main").exists());
}
