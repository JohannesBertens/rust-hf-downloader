//! `cli::report` (+ its `fmt` helper) tests (M6/T1 split of
//! `cli/tests.rs`): the progress-line formatters, the plain-mode
//! heartbeat throttle, and the heartbeat's skip-on-missed-snapshot rule
//! through the production `run::poll_once` drain. Run:
//! `cargo test report_tests`

use super::events::OverallProgress;
use super::report::{
    format_file_progress, format_overall_progress, verification_heartbeat_line, ProgressMode,
    Reporter,
};
use super::run::poll_once;
use super::testutil::SharedStderr;
use crate::fmt::{bar_cli, eta_cli, truncate_path_cli};

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

// ---------------------------------------------------------------------------
// Plain-mode stderr seam (Reporter::new_with_stderr)
// ---------------------------------------------------------------------------

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

#[tokio::test]
async fn plain_heartbeat_skips_when_progress_snapshot_missed() {
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
    let mut tally = super::run::RunTally::default();
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
