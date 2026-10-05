//! Failure-injection e2e tests for the download transport
//! (`download::start_download` / `download_chunked`): wire-level error
//! scenarios driven through the shared real-binary mock harness
//! (`tests/common/mod.rs`), pinning the exit codes, on-disk state, and
//! final registry state the W5.1a/b function decomposition must not
//! disturb.
//!
//! Coverage table (scenario → test or GAP):
//!
//! | scenario                            | mechanism                                    | test |
//! |-------------------------------------|----------------------------------------------|------|
//! | 500 mid-chunk (probe OK, chunks 500)| `MockRepo::fail_status_after_first_range`    | `server_500_mid_download_fails_and_marks_registry` |
//! | size mismatch (advertised LFS size != served bytes) | `FileEntry::advertised_size` (probe Content-Range lies; tail ranges 416) | `advertised_size_mismatch_fails_on_tail_chunk_416` |
//! | 416 on resume (wrong If-Range/offset) | —                                          | GAP — see below |
//!
//! GAP (416 on resume): the transport has NO resume path —
//! `start_download` deletes any existing `.incomplete` file and always
//! restarts from byte 0, and no `If-Range` header is ever sent. A
//! wrong-offset resume request is therefore not constructible against
//! this codebase (there is no client-side state to be wrong). The
//! nearest buildable cousin — the server answering 416 to a chunk range
//! request — IS pinned by the size-mismatch test above (tail chunks
//! beyond the served length receive real 416 responses) and flows through
//! the identical `error_for_status()?` → non-transient → `mark_failed`
//! path as any other status error.
//!
//! Expected behavior pinned (today's, both scenarios): a non-transient
//! HTTP status error aborts the retry loop without retrying
//! (`is_transient_error` only passes timeouts/connection errors), the
//! `.incomplete` file is deleted, `registry::mark_failed` leaves the
//! entry `Incomplete` with `downloaded_size = 0`, and the CLI exits
//! `EXIT_FAILURE` (1) with a `download_failed` error event.

mod common;

use common::{
    assert_exit_code, event_of, fixture_bytes, json_lines, sha256_hex, spawn_mock, FileEntry,
    MockRepo, TestEnv,
};
use std::time::Duration;

/// Scenario 1 — 500 mid-chunk: the probe (`bytes=0-0`, the first
/// range-bearing request) succeeds, every subsequent chunk request
/// answers 500. Non-transient → no retry → terminal failure.
#[tokio::test]
async fn server_500_mid_download_fails_and_marks_registry() {
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
        fail_status_after_first_range: Some(500),
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

    // Terminal error event (last line in JSON mode)
    let events = json_lines(&stdout);
    let last = events.last().unwrap();
    assert_eq!(last["type"], "error");
    assert_eq!(last["code"], "download_failed");
    assert!(
        last["message"].as_str().unwrap_or_default().contains("500"),
        "error message should mention the status: {}",
        last
    );

    // Registry: entry exists, marked Incomplete with 0 downloaded bytes
    // (mark_failed semantics), url = resolve URL.
    let registry = env.registry_toml();
    assert!(registry.contains("Incomplete"), "registry: {}", registry);
    assert!(
        registry.contains("downloaded_size = 0"),
        "registry: {}",
        registry
    );
    assert!(
        registry.contains("/resolve/main/model.gguf"),
        "registry: {}",
        registry
    );

    // Neither the final file nor a leftover .incomplete exists
    let models = env.models_dir().join("a/b");
    assert!(!models.join("model.gguf").exists());
    assert!(!models.join("model.gguf.incomplete").exists());
    // The failure event names the file
    let error = event_of(&events, "error", &stdout);
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("model.gguf"),
        "error event: {}",
        error
    );
}

/// Scenario 2 — size mismatch: the tree API and the probe's Content-Range
/// advertise 2× the bytes actually served. Tail chunks (ranges starting
/// at/after the served length) receive 416 (real-Hub semantics), which is
/// non-transient → terminal failure. Pins that the registry records the
/// ADVERTISED size (from the probe) while nothing survives on disk.
#[tokio::test]
async fn advertised_size_mismatch_fails_on_tail_chunk_416() {
    let content = fixture_bytes(20_000);
    let advertised = 2 * content.len() as u64; // 40_000 advertised, 20_000 served
    let endpoint = spawn_mock(MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(&content)),
            advertised_size: Some(advertised),
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
    let last = events.last().unwrap();
    assert_eq!(last["type"], "error");
    assert_eq!(last["code"], "download_failed");

    // Registry: entry exists with the ADVERTISED total (what the probe
    // reported), status Incomplete, 0 downloaded bytes.
    let registry = env.registry_toml();
    assert!(registry.contains("Incomplete"), "registry: {}", registry);
    assert!(
        registry.contains(&format!("total_size = {}", advertised)),
        "registry: {}",
        registry
    );
    assert!(
        registry.contains("downloaded_size = 0"),
        "registry: {}",
        registry
    );

    // Nothing survives on disk (no final file, .incomplete cleaned up)
    let models = env.models_dir().join("a/b");
    assert!(!models.join("model.gguf").exists());
    assert!(!models.join("model.gguf.incomplete").exists());
}
