//! Issue #50 e2e reproduction suite: "Download does not resume if the
//! download fails halfway through".
//!
//! Today the transport destroys partial progress at every opportunity —
//! `prepare_download_paths` deletes a pre-existing `.incomplete` file at
//! start, the retry loop deletes it on every transient retry, and terminal
//! failures delete it too, so every attempt restarts from byte 0. On slow
//! or flaky connections this makes large models impossible to download.
//!
//! These tests pin the DESIRED behavior (they FAIL on the unfixed
//! transport):
//!
//! | # | scenario                                  | pins |
//! |---|-------------------------------------------|------|
//! | 1 | transient timeout mid-download, in-run retry | attempt 2 must not re-request byte ranges attempt 1 already completed |
//! | 2 | terminal 500 mid-download                    | `.incomplete` + a resume journal survive on disk for later runs |
//! | 3 | second CLI run after terminal failure        | run 2 resumes from the journal (no re-request of completed chunks) and finishes byte-exact |
//! | 4 | server content size changed between runs     | journal is discarded, download restarts from byte 0 (no over-resume) |
//! | 5 | server ETag changed between runs (same size) | journal is discarded, download restarts from byte 0 (no over-resume) |
//!
//! The wire-level truth comes from the mock's [`RangeLog`]: every
//! range-bearing request is recorded at arrival, so a restart-from-zero
//! attempt is directly observable as a re-requested `bytes=0-...` chunk
//! range.
//!
//! Test-layout math (TestEnv config): files are 100_000 bytes, chunk size
//! clamps to 4096 → 25 chunks (last one 1_696 bytes). The probe request is
//! recognizable as `(start, end) == (0, 0)`.

mod common;

use common::{
    assert_exit_code, assert_file_content, fixture_bytes, json_lines, sha256_hex,
    spawn_mock_observed, FileEntry, MockRepo, RangeLog, RangeReq, RangeScript, RangeStall, TestEnv,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The download under test: 100_000 bytes, 25 chunks of 4096 (tail 1_696).
const FILE_LEN: u64 = 100_000;
const CHUNK_SIZE: u64 = 4096;
const NUM_CHUNKS: u64 = 25;

fn journal_path(env: &TestEnv) -> PathBuf {
    env.models_dir()
        .join("a/b/model.gguf.incomplete.state.json")
}

fn incomplete_path(env: &TestEnv) -> PathBuf {
    env.models_dir().join("a/b/model.gguf.incomplete")
}

fn final_path(env: &TestEnv) -> PathBuf {
    env.models_dir().join("a/b/model.gguf")
}

/// Length of chunk `i` in the fixed 25×4096 layout.
fn chunk_len(i: u64) -> u64 {
    CHUNK_SIZE.min(FILE_LEN - i * CHUNK_SIZE)
}

/// The journal's on-disk JSON (issue #50 contract): `total_size` plus a
/// per-chunk `chunk_offsets` vector (bytes of chunk `i` already on disk).
fn read_journal(env: &TestEnv) -> serde_json::Value {
    let text = std::fs::read_to_string(journal_path(env))
        .unwrap_or_else(|e| panic!("journal should exist after a mid-download failure: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("journal is not valid JSON ({e}): {text}"))
}

/// Non-probe chunk requests belonging to the given CLI run. A run starts
/// at its probe (`(0, 0)`); `run` is 1-based.
fn chunk_requests_of_run(log: &RangeLog, run: usize) -> Vec<RangeReq> {
    let all = log.lock().unwrap().clone();
    let mut current_run = 0;
    let mut out = Vec::new();
    for r in all {
        if r.start == 0 && r.end == 0 {
            current_run += 1;
        } else if current_run == run {
            out.push(r);
        }
    }
    out
}

fn repo_with_file(content: &[u8], etag: Option<String>) -> MockRepo {
    MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(content)),
            advertised_size: None,
            etag,
            content: content.to_vec(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
        range_scripts: Vec::new(),
        fail_subdir: None,
        range_stalls: Vec::new(),
        content_override: None,
    }
}

/// Scenario 1 — in-run transient retry must RESUME, not restart.
///
/// With `concurrent_threads = 2` and stalls on the 4th/5th range requests
/// (the two chunk requests that start after the first two chunks finish),
/// attempt 1 deterministically completes exactly the first two chunk
/// requests before both workers stall past the 1 s client timeout. The
/// transient retry (attempt 2) must not re-request either completed range.
#[tokio::test]
async fn inrun_transient_retry_resumes_instead_of_restarting() {
    let content = fixture_bytes(FILE_LEN as usize);
    let mut repo = repo_with_file(&content, None);
    // Fail mid-download with a TRANSIENT error: requests #4 and #5 stall
    // 1.5 s against a 1 s client timeout (requests #2/#3 — the first two
    // chunks — complete normally before both workers stall).
    repo.range_stalls = vec![
        Arc::new(RangeStall {
            path: "model.gguf".to_string(),
            on_nth: 4,
            duration: Duration::from_millis(1500),
            seen: std::sync::atomic::AtomicU64::new(0),
        }),
        Arc::new(RangeStall {
            path: "model.gguf".to_string(),
            on_nth: 5,
            duration: Duration::from_millis(1500),
            seen: std::sync::atomic::AtomicU64::new(0),
        }),
    ];
    let (endpoint, log) = spawn_mock_observed(repo).await;

    let env = TestEnv::new(&endpoint);
    env.set("concurrent_threads", "2");
    env.set("download_timeout_secs", "1");
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);

    // The recovered download is byte-exact and verified.
    assert!(
        stderr.contains("✓ verified: model.gguf"),
        "resumed download still verifies: {stderr}"
    );
    assert_file_content(&final_path(&env), &content);

    // Success must clean up BOTH the .incomplete file and the journal.
    assert!(!incomplete_path(&env).exists());
    assert!(
        !journal_path(&env).exists(),
        "journal must be deleted after a successful download"
    );

    // THE resume assertion (wire-level): attempt 1's completed chunks —
    // its first two chunk requests, whichever chunk ids they were — must
    // not be re-requested by attempt 2. A restart-from-zero transport
    // re-requests `bytes=0-4095`, which is attempt 1's first chunk range.
    let attempt1 = chunk_requests_of_run(&log, 1);
    assert!(
        attempt1.len() >= 2,
        "attempt 1 must have at least the two completed chunk requests: {:?}",
        attempt1
    );
    let completed = (attempt1[0].clone(), attempt1[1].clone());
    let attempt2 = chunk_requests_of_run(&log, 2);
    assert!(
        !attempt2.is_empty(),
        "attempt 2 must download the remaining chunks"
    );
    let re_requested: Vec<RangeReq> = attempt2
        .iter()
        .filter(|r| **r == completed.0 || **r == completed.1)
        .cloned()
        .collect();
    assert!(
        re_requested.is_empty(),
        "attempt 2 re-requested ranges attempt 1 already completed \
         (restart-from-zero bug, issue #50): {:?}",
        re_requested
    );
}

/// Scenario 2 — a terminal failure must PRESERVE the partial download on
/// disk (`.incomplete` + journal) so a later run can resume it. The
/// registry still records the failure (Incomplete), and the CLI exits 1.
#[tokio::test]
async fn terminal_failure_preserves_incomplete_and_journal_for_resume() {
    let content = fixture_bytes(FILE_LEN as usize);
    let mut repo = repo_with_file(&content, None);
    // Probe (#1) ok, chunk requests #2/#3 ok, request #4 → 500 (terminal).
    let script = RangeScript {
        path: "model.gguf".to_string(),
        statuses: vec![200, 200, 200, 500, 200],
        seen: std::sync::atomic::AtomicU64::new(0),
    };
    let script = Arc::new(script);
    repo.range_scripts = vec![script.clone()];
    let (endpoint, _log) = spawn_mock_observed(repo).await;

    let env = TestEnv::new(&endpoint);
    // Determinism (plan review): 2 workers so the scripted failure fires only
    // after the in-flight chunks completed AND journaled.
    env.set("concurrent_threads", "2");
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);
    let events = json_lines(&stdout);
    let error = common::event_of(&events, "error", &stdout);
    assert_eq!(error["code"], "download_failed");

    // Registry semantics unchanged: entry marked Incomplete.
    let registry = env.registry_toml();
    assert!(registry.contains("Incomplete"), "registry: {registry}");

    // THE issue #50 assertion: the partial download SURVIVES the failure.
    assert!(
        incomplete_path(&env).exists(),
        ".incomplete must survive a terminal failure for a later resume"
    );
    let journal = read_journal(&env);
    assert_eq!(
        journal["total_size"].as_u64(),
        Some(FILE_LEN),
        "journal records the probed size: {journal}"
    );
    let offsets: Vec<u64> = journal["chunk_offsets"]
        .as_array()
        .unwrap_or_else(|| panic!("journal.chunk_offsets must be an array: {journal}"))
        .iter()
        .map(|v| v.as_u64().unwrap_or(0))
        .collect();
    assert_eq!(offsets.len() as u64, NUM_CHUNKS, "journal: {journal}");
    let completed: Vec<(usize, u64)> = offsets
        .iter()
        .enumerate()
        .filter(|(i, o)| **o == chunk_len(*i as u64))
        .map(|(i, o)| (i, *o))
        .collect();
    assert!(
        !completed.is_empty(),
        "at least one chunk must be journaled complete: {journal}"
    );
}

/// Scenario 3 — a second CLI run resumes from the journal of a failed
/// first run: chunks completed in run 1 are not re-requested, and the
/// finished file is byte-exact and verified.
#[tokio::test]
async fn second_run_resumes_from_journal_and_completes() {
    let content = fixture_bytes(FILE_LEN as usize);
    let mut repo = repo_with_file(&content, None);
    // Requests #1-#3 ok (probe + 2 chunks), #4 → 500 (run 1 fails
    // terminally), every request from #5 on serves normally → run 2
    // succeeds against the SAME mock.
    let script = RangeScript {
        path: "model.gguf".to_string(),
        statuses: vec![200, 200, 200, 500, 200],
        seen: std::sync::atomic::AtomicU64::new(0),
    };
    let script = Arc::new(script);
    repo.range_scripts = vec![script.clone()];
    let (endpoint, log) = spawn_mock_observed(repo).await;

    let env = TestEnv::new(&endpoint);
    // Determinism (plan review): 2 workers so the scripted failure fires only
    // after the in-flight chunks completed AND journaled.
    env.set("concurrent_threads", "2");

    // Run 1: terminal failure, partial state preserved (scenario 2).
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);
    assert!(incomplete_path(&env).exists(), "run 1 keeps .incomplete");

    // Which chunks run 1 completed, per its journal.
    let journal = read_journal(&env);
    let offsets: Vec<u64> = journal["chunk_offsets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap_or(0))
        .collect();
    let completed_ranges: Vec<(u64, u64)> = offsets
        .iter()
        .enumerate()
        .filter(|(i, o)| **o == chunk_len(*i as u64))
        .map(|(i, _)| {
            let start = i as u64 * CHUNK_SIZE;
            (start, start + chunk_len(i as u64) - 1)
        })
        .collect();
    assert!(
        !completed_ranges.is_empty(),
        "run 1 must have completed at least one chunk: {journal}"
    );

    // Run 2: succeeds against the same server.
    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert!(
        stderr.contains("✓ verified: model.gguf"),
        "resumed run 2 verifies: {stderr}"
    );
    assert_file_content(&final_path(&env), &content);
    assert!(!incomplete_path(&env).exists());
    assert!(!journal_path(&env).exists(), "journal consumed on success");

    // THE resume assertion: run 2 never re-requests a chunk range that
    // run 1's journal records as complete. A restart-from-zero transport
    // re-requests `bytes=0-4095` (chunk 0) among others.
    let run2 = chunk_requests_of_run(&log, 2);
    assert!(!run2.is_empty());
    let re_requested: Vec<(u64, u64)> = run2
        .iter()
        .filter(|r| completed_ranges.contains(&(r.start, r.end)))
        .map(|r| (r.start, r.end))
        .collect();
    assert!(
        re_requested.is_empty(),
        "run 2 re-requested chunk ranges run 1 already completed \
         (restart-from-zero bug, issue #50): {re_requested:?}"
    );
}

/// Scenario 4 — the server's file SIZE changed between runs: the journal
/// no longer describes the file being served, so it must be DISCARDED and
/// the download restarted from byte 0 (no over-resume into wrong bytes).
#[tokio::test]
async fn server_size_change_discards_journal_and_restarts() {
    let content = fixture_bytes(FILE_LEN as usize);
    let mut repo = repo_with_file(&content, None);
    let script = RangeScript {
        path: "model.gguf".to_string(),
        statuses: vec![200, 200, 200, 500, 200],
        seen: std::sync::atomic::AtomicU64::new(0),
    };
    repo.range_scripts = vec![Arc::new(script)];
    let override_cell: Arc<std::sync::Mutex<Option<FileEntry>>> =
        Arc::new(std::sync::Mutex::new(None));
    repo.content_override = Some(override_cell.clone());
    let (endpoint, log) = spawn_mock_observed(repo).await;

    let env = TestEnv::new(&endpoint);
    // Determinism (plan review): 2 workers so the scripted failure fires only
    // after the in-flight chunks completed AND journaled.
    env.set("concurrent_threads", "2");
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);
    assert!(journal_path(&env).exists());

    // The server's copy changes (different size AND content).
    let new_content = fixture_bytes((FILE_LEN + 20_000) as usize);
    *override_cell.lock().unwrap() = Some(FileEntry {
        path: "model.gguf".to_string(),
        advertised_sha256: Some(sha256_hex(&new_content)),
        advertised_size: None,
        etag: None,
        content: new_content.clone(),
    });

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&final_path(&env), &new_content);

    // Restart happened: run 2 requested chunk 0's full range again
    // (bytes=0-4095) instead of resuming, and no journal survives.
    let run2 = chunk_requests_of_run(&log, 2);
    assert!(
        run2.iter().any(|r| r.start == 0 && r.end == CHUNK_SIZE - 1),
        "run 2 must restart from byte 0 when the server size changed: {run2:?}"
    );
    assert!(!journal_path(&env).exists());
}

/// Scenario 5 — same SIZE but different content AND ETag: the journal's
/// recorded ETag no longer matches, so it must be discarded (restart from
/// byte 0) even though the size still validates.
#[tokio::test]
async fn server_etag_change_discards_journal_and_restarts() {
    let content = fixture_bytes(FILE_LEN as usize);
    let mut repo = repo_with_file(&content, Some("\"etag-v1\"".to_string()));
    let script = RangeScript {
        path: "model.gguf".to_string(),
        statuses: vec![200, 200, 200, 500, 200],
        seen: std::sync::atomic::AtomicU64::new(0),
    };
    repo.range_scripts = vec![Arc::new(script)];
    let override_cell: Arc<std::sync::Mutex<Option<FileEntry>>> =
        Arc::new(std::sync::Mutex::new(None));
    repo.content_override = Some(override_cell.clone());
    let (endpoint, log) = spawn_mock_observed(repo).await;

    let env = TestEnv::new(&endpoint);
    // Determinism (plan review): 2 workers so the scripted failure fires only
    // after the in-flight chunks completed AND journaled.
    env.set("concurrent_threads", "2");
    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "model.gguf", "--json"])
        .await;
    assert_exit_code(code, 1, &stdout, &stderr);
    assert!(journal_path(&env).exists());

    // Same size, different bytes, different ETag.
    let mut new_content = fixture_bytes(FILE_LEN as usize);
    new_content[0] = new_content[0].wrapping_add(1);
    *override_cell.lock().unwrap() = Some(FileEntry {
        path: "model.gguf".to_string(),
        advertised_sha256: Some(sha256_hex(&new_content)),
        advertised_size: None,
        etag: Some("\"etag-v2\"".to_string()),
        content: new_content.clone(),
    });

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, 0, &stdout, &stderr);
    assert_file_content(&final_path(&env), &new_content);

    let run2 = chunk_requests_of_run(&log, 2);
    assert!(
        run2.iter().any(|r| r.start == 0 && r.end == CHUNK_SIZE - 1),
        "run 2 must restart from byte 0 when the ETag changed: {run2:?}"
    );
    assert!(!journal_path(&env).exists());
}
