//! Exit-code matrix (ITEM H3) and non-TTY human-output goldens (ITEM H4)
//! for the one-shot CLI, driving the same real-binary harness as
//! `tests/cli_download.rs` (shared: `tests/common/mod.rs`).
//!
//! Exit-code matrix coverage (constant from `src/cli/mod.rs`):
//!
//! | row                          | exit const              | test |
//! |------------------------------|-------------------------|------|
//! | success single-file download | `EXIT_OK` (0)           | `exit_ok_success_single_file` |
//! | already-exists (reachable)   | `EXIT_OK` (0)           | `exit_ok_already_exists` |
//! | usage error (bad model id)   | `EXIT_USAGE` (64)       | `exit_usage_invalid_model_id` |
//! | not_found (metadata 404)     | `EXIT_USAGE` (64)       | `exit_not_found_metadata_404` |
//! | network (closed port)        | `EXIT_FAILURE` (1)      | `exit_network_closed_port` |
//! | auth_required (mock 401)     | `EXIT_AUTH` (2)         | `exit_auth_required_gated_repo` |
//! | hash mismatch                | `EXIT_FAILURE` (1)      | `exit_hash_mismatch` |
//! | auth + failure mixed (2 files, G4a) | `EXIT_AUTH` (2) | `exit_auth_beats_download_failure_gated_plus_500` |
//! | one ok + one 500 (2 files, G4b) | `EXIT_FAILURE` (1)   | `exit_failure_mixed_one_ok_one_500` |
//!
//! No GAP rows: every reachable outcome class is produced with the shared
//! mock (401 via `gated: true`, 404 via an unknown model id, mismatch via a
//! wrong advertised LFS sha). `EXIT_UPDATE_AVAILABLE` (70) /
//! `EXIT_CHECKSUM` (71) / `EXIT_INTERRUPTED` (130) are outside the download
//! matrix: 70/71 are pinned by `tests/update_e2e.rs`, 130 needs a real
//! SIGINT (not simulatable through this harness — GAP, pre-existing).
//!
//! Human-output goldens: all children run with piped stdout/stderr
//! (non-TTY), where `--progress auto` prints NO `\r` rewrites at all
//! (asserted in `cli_download.rs`), so snapshots capture plain newline
//! lines only — no split-on-`\r` step is needed. TTY behavior (single-line
//! `\r` rewrites) is a marked GAP: it needs a pty harness this slice does
//! not build. Every `assert_snapshot!` value goes through the ONE
//! documented volatility normalizer [`normalize`] first.

mod common;

use common::{
    assert_exit_code, assert_file_content, fixture_bytes, sha256_hex, spawn_mock, FileEntry,
    MockRepo, RangeScript, TestEnv,
};

// Mirrors the EXIT_* constants in src/cli/mod.rs (no lib target, so the
// integration tests cannot import them; keep this table in sync).
const EXIT_OK: i32 = 0;
const EXIT_FAILURE: i32 = 1;
const EXIT_AUTH: i32 = 2;
const EXIT_USAGE: i32 = 64;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use serde_json::json;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Volatility normalizer (the single documented one for this file)
// ---------------------------------------------------------------------------

/// THE volatility normalizer applied before every `assert_snapshot!` in
/// this file (one pass, three regexes):
///
/// 1. per-test temp-home prefixes `…/hf-cli-e2e-<pid>-<nanos>` → `$TMP`
///    (temp dir + unique suffix differ on every run and machine);
/// 2. `127.0.0.1:<ephemeral-port>` → `127.0.0.1:$PORT` (mock servers bind
///    port 0, so the port changes per run; reqwest error strings embed it);
/// 3. decimal sizes/speeds/percents — a number with a fractional part
///    directly followed by a space or `%` (`48.8 KB`, `1.2 MB/s`,
///    `37.5%`) → `$N` (format_size rounding and speed/eta values depend
///    on timing). Version strings like `v2.13.1` are deliberately NOT
///    matched (their decimals are followed by `.` or `)`);
/// 4. `\` → `/` — Windows binaries print native separators in the
///    destination/snapshot paths; canonicalizing keeps the snapshots
///    platform-independent (no pinned output contains a legitimate
///    backslash).
fn normalize(s: &str) -> String {
    let tmp = regex::Regex::new(r#"[^\s"']*hf-cli-e2e-\d+-\d+"#).unwrap();
    let port = regex::Regex::new(r"127\.0\.0\.1:\d+").unwrap();
    let decimal = regex::Regex::new(r"\d+\.\d+([ %])").unwrap();
    let s = tmp.replace_all(s, "$$TMP");
    let s = port.replace_all(&s, "127.0.0.1:$$PORT");
    // `$$` escapes to a literal `$` (regex crate replacement syntax); the
    // captured separator keeps `48.8 KB` and `37.5%` word-shaped.
    let s = decimal.replace_all(&s, "$$N$1");
    s.replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Exit-code matrix (H3)
// ---------------------------------------------------------------------------

fn single_file_repo(content: &[u8]) -> MockRepo {
    MockRepo {
        model_id: "a/b".to_string(),
        files: vec![FileEntry {
            path: "model.gguf".to_string(),
            advertised_sha256: Some(sha256_hex(content)),
            advertised_size: None,
            content: content.to_vec(),
        }],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: std::time::Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
        range_scripts: Vec::new(),
        fail_subdir: None,
    }
}

/// Two-file repo (a.gguf + b.gguf) sharing one content fixture — the
/// G4 rows' base.
fn two_file_repo(content: &[u8]) -> MockRepo {
    MockRepo {
        model_id: "a/b".to_string(),
        files: vec![
            FileEntry {
                path: "a.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(content)),
                advertised_size: None,
                content: content.to_vec(),
            },
            FileEntry {
                path: "b.gguf".to_string(),
                advertised_sha256: Some(sha256_hex(content)),
                advertised_size: None,
                content: content.to_vec(),
            },
        ],
        gated: false,
        fail_status_after_first_range: None,
        resolve_404: false,
        sleep_once: None,
        per_request_delay: std::time::Duration::ZERO,
        search_results: Vec::new(),
        branches: Vec::new(),
        range_scripts: Vec::new(),
        fail_subdir: None,
    }
}

/// Row 1: success → `EXIT_OK`, summary on stdout, destination hint, file
/// on disk with exact bytes.
#[tokio::test]
async fn exit_ok_success_single_file() {
    let content = fixture_bytes(50_000);
    let endpoint = spawn_mock(single_file_repo(&content)).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_OK, &stdout, &stderr);
    assert!(
        stdout.starts_with("Done: 1 file(s)"),
        "summary missing on stdout: {stdout:?}"
    );
    assert!(stdout.contains("Destination:"), "stdout: {stdout:?}");
    assert!(
        stderr.contains("1 file(s) to download"),
        "resolved line missing on stderr: {stderr:?}"
    );
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
}

/// Row 2: file already on disk → still `EXIT_OK`, counted as skipped
/// (exists) and hash-verified, nothing re-downloaded.
#[tokio::test]
async fn exit_ok_already_exists() {
    let content = fixture_bytes(20_000);
    let endpoint = spawn_mock(single_file_repo(&content)).await;
    let env = TestEnv::new(&endpoint);
    let dest = env.models_dir().join("a/b/model.gguf");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, &content).unwrap();

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_OK, &stdout, &stderr);
    assert!(
        stdout.contains("0 downloaded, 1 skipped (exists)"),
        "skip not accounted on stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("= model.gguf"),
        "already-exists marker missing on stderr: {stderr:?}"
    );
}

/// Row 3: usage error (invalid model id, no server contact) →
/// `EXIT_USAGE`, nothing on stdout, `error [usage]:` on stderr.
#[tokio::test]
async fn exit_usage_invalid_model_id() {
    let endpoint = spawn_mock(single_file_repo(&fixture_bytes(10))).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "no-slash"]).await;
    assert_exit_code(code, EXIT_USAGE, &stdout, &stderr);
    assert!(stdout.is_empty(), "usage errors print nothing to stdout");
    assert!(stderr.starts_with("error [usage]:"), "stderr: {stderr:?}");

    insta::assert_snapshot!("human-usage-error", normalize(&stderr));
}

/// Row 4: metadata fetch 404s (unknown model id against the mock) →
/// `not_found` maps to `EXIT_USAGE`, `error [not_found]:` on stderr.
#[tokio::test]
async fn exit_not_found_metadata_404() {
    let endpoint = spawn_mock(single_file_repo(&fixture_bytes(10))).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "x/y", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_USAGE, &stdout, &stderr);
    assert!(stdout.is_empty(), "stdout: {stdout:?}");
    assert!(
        stderr.starts_with("error [not_found]:"),
        "stderr: {stderr:?}"
    );

    insta::assert_snapshot!("human-not-found-error", normalize(&stderr));
}

/// Row 5: endpoint points at a guaranteed-closed port → network failure,
/// `EXIT_FAILURE`, `error [network]:` on stderr.
#[tokio::test]
async fn exit_network_closed_port() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let env = TestEnv::new(&format!("http://127.0.0.1:{}", port));

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_FAILURE, &stdout, &stderr);
    assert!(stdout.is_empty(), "stdout: {stdout:?}");
    assert!(stderr.starts_with("error [network]:"), "stderr: {stderr:?}");
}

/// Row 6: gated repo (mock answers 401 on `/resolve/`) →
/// `EXIT_AUTH`, `error [auth_required]:` on stderr.
#[tokio::test]
async fn exit_auth_required_gated_repo() {
    let mut repo = single_file_repo(&fixture_bytes(10_000));
    repo.gated = true;
    let endpoint = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_AUTH, &stdout, &stderr);
    // Human mode still prints the final summary even on failure; the typed
    // auth error goes to stderr.
    assert!(stdout.starts_with("Done: 1 file(s)"), "stdout: {stdout:?}");
    assert!(
        stderr.contains("error [auth_required]: authentication required for a/b"),
        "stderr: {stderr:?}"
    );
}

/// Row 7: advertised LFS sha does not match the served bytes → hash
/// mismatch, `EXIT_FAILURE`, mismatch summary on stdout.
#[tokio::test]
async fn exit_hash_mismatch() {
    let content = fixture_bytes(20_000);
    let mut repo = single_file_repo(&content);
    repo.files[0].advertised_sha256 = Some(sha256_hex(b"totally different bytes"));
    let endpoint = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_FAILURE, &stdout, &stderr);
    assert!(stdout.contains("Hash mismatches: 1"), "stdout: {stdout:?}");
    assert!(
        stderr.contains("✗ verified hash mismatch"),
        "stderr: {stderr:?}"
    );
    // The bytes are kept on disk (engine keeps the file for inspection)
    assert_file_content(&env.models_dir().join("a/b/model.gguf"), &content);
}

/// Row 8 (G4a): two-file gated repo where file 2 additionally fails
/// with an injected 500 — auth beats failure in the exit-code ranking
/// (`RunTally::exit_code` checks `auth_required` FIRST), so the run
/// exits `EXIT_AUTH` (2) even though a download also failed.
#[tokio::test]
async fn exit_auth_beats_download_failure_gated_plus_500() {
    let content = fixture_bytes(10_000);
    let fail_b = RangeScript::always("b.gguf", 500);
    let mut repo = two_file_repo(&content);
    repo.gated = true; // a.gguf 401s; b.gguf is intercepted by the script
    repo.range_scripts = vec![fail_b];
    let endpoint = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "a.gguf", "--file", "b.gguf"])
        .await;
    assert_exit_code(code, EXIT_AUTH, &stdout, &stderr);
    // BOTH failure classes are reported (auth on a.gguf, 500 on b.gguf) —
    // the ranking only decides the exit code, not the emissions.
    assert!(
        stderr.contains("error [auth_required]: authentication required for a/b"),
        "stderr: {stderr:?}"
    );
    assert!(
        stderr.contains("error [download_failed]: b.gguf:"),
        "stderr: {stderr:?}"
    );
    assert!(stdout.starts_with("Done: 2 file(s)"), "stdout: {stdout:?}");
    assert!(stdout.contains("1 failed"), "stdout: {stdout:?}");
}

/// Row 9 (G4b): two-file repo, file 1 downloads fine, file 2 fails with
/// an injected 500 → `EXIT_FAILURE` (1), summary counts "1 downloaded",
/// and the run tail carries the `download_failed` error naming file 2.
#[tokio::test]
async fn exit_failure_mixed_one_ok_one_500() {
    let content = fixture_bytes(10_000);
    let other = fixture_bytes(6_000);
    let fail_b = RangeScript::always("b.gguf", 500);
    let mut repo = two_file_repo(&content);
    repo.files[1] = FileEntry {
        path: "b.gguf".to_string(),
        advertised_sha256: Some(sha256_hex(&other)),
        advertised_size: None,
        content: other.clone(),
    };
    repo.range_scripts = vec![fail_b];
    let endpoint = spawn_mock(repo).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&["download", "a/b", "--file", "a.gguf", "--file", "b.gguf"])
        .await;
    assert_exit_code(code, EXIT_FAILURE, &stdout, &stderr);
    assert!(
        stdout.contains("1 downloaded"),
        "summary must count the successful file: {stdout:?}"
    );
    assert!(stdout.contains("1 failed"), "stdout: {stdout:?}");
    assert!(
        stderr.contains("error [download_failed]: b.gguf:"),
        "download_failed tail missing: {stderr:?}"
    );
    // File 1 is fully on disk and hash-verified despite file 2's failure.
    assert_file_content(&env.models_dir().join("a/b/a.gguf"), &content);
    assert!(
        stderr.contains("✓ verified: a.gguf"),
        "the surviving file still verifies: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------
// Human-output goldens (H4, non-TTY only)
// ---------------------------------------------------------------------------

/// (a) Successful plain-mode single-file download: final summary lines on
/// stdout (snapshot) and the human event lines on stderr (snapshot).
#[tokio::test]
async fn snapshot_success_human_output() {
    let content = fixture_bytes(50_000);
    let endpoint = spawn_mock(single_file_repo(&content)).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env.run(&["download", "a/b", "--file", "model.gguf"]).await;
    assert_exit_code(code, EXIT_OK, &stdout, &stderr);

    insta::assert_snapshot!("human-success-stdout", normalize(&stdout));
    insta::assert_snapshot!("human-success-stderr", normalize(&stderr));
}

/// (e) Plain-mode `hf-cache sync` output: planned/published/verified lines
/// on stderr, `Done:` + the snapshot path (the command's scripted output,
/// hf CLI parity) on stdout.
#[tokio::test]
async fn snapshot_hf_cache_sync_human_output() {
    let content = fixture_bytes(48 * 1024);
    let endpoint = spawn_mock(single_file_repo(&content)).await;
    let env = TestEnv::new(&endpoint);

    let (code, stdout, stderr) = env
        .run(&[
            "hf-cache",
            "sync",
            "a/b",
            "--cache-dir",
            &env.home.join("hub").display().to_string(),
        ])
        .await;
    assert_exit_code(code, EXIT_OK, &stdout, &stderr);

    insta::assert_snapshot!("human-hf-cache-sync-stdout", normalize(&stdout));
    insta::assert_snapshot!("human-hf-cache-sync-stderr", normalize(&stderr));
}

// (d) `update --check` up-to-date line: needs the release-manifest server
// approach from tests/update_e2e.rs, recreated minimally below (only
// latest.json is served — the up-to-date branch never downloads the asset).

/// Serve a `latest.json` for this platform triple (asset entry present, as
/// `run_update` resolves the platform asset before comparing versions).
async fn spawn_manifest_server(version: &str) -> String {
    let triple = target_triple();
    let ext = if cfg!(windows) { "zip" } else { "tar-gz" };
    let asset_name = format!(
        "rust-hf-downloader-{triple}.{}",
        if cfg!(windows) { "zip" } else { "tar.gz" }
    );
    let manifest = json!({
        "version": version,
        "released_at": "2026-01-01T00:00:00Z",
        "notes_url": format!("https://example.com/notes/{version}"),
        "assets": {
            triple: {
                "name": asset_name,
                "format": ext,
                "sha256": "0".repeat(64),
            }
        },
    })
    .to_string();
    let manifest = Arc::new(manifest);

    let make = make_service_fn(move |_| {
        let manifest = manifest.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req: Request<Body>| {
                let manifest = manifest.clone();
                async move {
                    let body = if req.uri().path() == "/latest.json" {
                        (*manifest).clone()
                    } else {
                        "not found".to_string()
                    };
                    Ok::<Response<Body>, hyper::Error>(Response::new(Body::from(body)))
                }
            }))
        }
    });
    let srv = Server::bind(&([127, 0, 0, 1], 0).into()).serve(make);
    let url = format!("http://{}", srv.local_addr());
    tokio::spawn(srv);
    url
}

fn target_triple() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        panic!("tests must run on a published target triple")
    }
}

/// (d) `update --check` against an older manifest: up-to-date line on
/// stderr, `EXIT_OK`. `--check` never swaps, so the real cargo-built
/// binary can run directly under `RHD_UPDATE_BASE`.
#[tokio::test]
async fn snapshot_update_check_up_to_date() {
    let base = spawn_manifest_server("0.0.1").await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_rust-hf-downloader"))
        .args(["update", "--check"])
        .env("RHD_UPDATE_BASE", &base)
        .output()
        .await
        .expect("spawn binary");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_exit_code(code, EXIT_OK, "", &stderr);

    insta::assert_snapshot!("human-update-check-up-to-date", normalize(&stderr));
}
