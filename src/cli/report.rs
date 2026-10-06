//! Reporters: render [`Event`]s as human text (stderr) or NDJSON (stdout),
//! plus `--progress` modes; the progress-line building blocks (bar, ETA,
//! path truncation) come from [`crate::fmt`].

use crate::fmt::size_full;
use crate::fmt::{bar_cli, eta_cli, truncate_path_cli};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use super::events::{Event, FileStatus, OverallProgress};

/// Minimum interval between JSON `progress` events per run.
const JSON_PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
/// Minimum interval between `--progress plain` heartbeat lines.
const PLAIN_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);

/// Human progress output mode (`--progress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProgressMode {
    /// Single-line `\r` rewrites on a tty; nothing when piped (default)
    Auto,
    /// One newline progress line every ~10 s, tty-independent (for docker
    /// run / CI logs); adds a verification heartbeat during the
    /// post-download SHA256 drain
    Plain,
    /// No progress output at all
    None,
}

/// Renders [`Event`]s either as human-readable text (progress to stderr,
/// summary to stdout) or as NDJSON on stdout.
pub struct Reporter {
    json: bool,
    quiet: bool,
    /// Single-line `\r` progress rewrites are only used when stderr is a tty.
    progress_to_tty: bool,
    /// `--progress plain`: throttled newline progress lines regardless of
    /// tty (shares one throttle window with the verification heartbeat).
    progress_plain: bool,
    progress_line_active: bool,
    last_json_progress: Option<Instant>,
    last_plain_progress: Option<Instant>,
    /// Sink for every human-mode stderr write. Production uses the real
    /// stderr; tests inject a capture buffer.
    stderr: Box<dyn Write>,
}

impl Reporter {
    pub fn new(json: bool, quiet: bool, progress: ProgressMode) -> Self {
        Self::new_with_stderr(json, quiet, progress, Box::new(std::io::stderr()))
    }

    /// Test constructor with an injected stderr sink (see `stderr`).
    pub(super) fn new_with_stderr(
        json: bool,
        quiet: bool,
        progress: ProgressMode,
        stderr: Box<dyn Write>,
    ) -> Self {
        let human_progress = !json && !quiet && progress != ProgressMode::None;
        Self {
            json,
            quiet,
            progress_to_tty: human_progress
                && progress == ProgressMode::Auto
                && std::io::stderr().is_terminal(),
            progress_plain: human_progress && progress == ProgressMode::Plain,
            progress_line_active: false,
            last_json_progress: None,
            last_plain_progress: None,
            stderr,
        }
    }

    /// True when a `--progress plain` heartbeat line is due (and records
    /// the emission).
    fn plain_progress_due(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_plain_progress
            .is_some_and(|last| now.duration_since(last) < PLAIN_PROGRESS_INTERVAL)
        {
            return false;
        }
        self.last_plain_progress = Some(now);
        true
    }

    /// `--progress plain` heartbeat for the verification drain: downloads
    /// finished, SHA256 still hashing — no `progress` events fire in that
    /// phase, so without this the tail of a run is silent between
    /// `✓ verified` milestones.
    pub(super) fn plain_verification(&mut self, active: usize, done: usize) {
        if !self.progress_plain || !self.plain_progress_due() {
            return;
        }
        self.line_stderr(&verification_heartbeat_line(active, done));
    }

    fn emit_json(&mut self, event: &Event) {
        let mut stdout = std::io::stdout().lock();
        if let Event::Progress { .. } = event {
            let now = Instant::now();
            if let Some(last) = self.last_json_progress {
                if now.duration_since(last) < JSON_PROGRESS_INTERVAL {
                    return; // throttled
                }
            }
            self.last_json_progress = Some(now);
        }
        if let Ok(line) = serde_json::to_string(event) {
            let _ = writeln!(stdout, "{}", line);
            let _ = stdout.flush();
        }
    }

    /// Clear an active single-line progress render (before printing real
    /// lines). Only ever called when the progress line went to a tty.
    fn clear_progress_line(&mut self) {
        if self.progress_line_active {
            let _ = write!(self.stderr, "\r\x1b[2K");
            self.progress_line_active = false;
        }
    }

    fn line_stderr(&mut self, text: &str) {
        self.clear_progress_line();
        let _ = writeln!(self.stderr, "{}", text);
    }

    fn line_stdout(&mut self, text: &str) {
        self.clear_progress_line();
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{}", text);
        let _ = stdout.flush();
    }

    pub fn emit(&mut self, event: &Event) {
        if self.json {
            self.emit_json(event);
            return;
        }

        match event {
            Event::Resolved {
                files, total_bytes, ..
            } => {
                if !self.quiet {
                    self.line_stderr(&format!(
                        "{} file(s) to download, {} total",
                        files.len(),
                        size_full(*total_bytes)
                    ));
                }
            }
            Event::DownloadStart { .. } => {} // covered by the progress line
            Event::Progress {
                filename,
                downloaded_bytes,
                total_bytes,
                speed_mbps,
                overall,
                ..
            } => {
                if self.progress_plain || self.progress_to_tty {
                    let content = match overall {
                        Some(overall) => format_overall_progress(
                            filename,
                            *downloaded_bytes,
                            *total_bytes,
                            overall,
                            *speed_mbps,
                        ),
                        None => format_file_progress(
                            filename,
                            *downloaded_bytes,
                            *total_bytes,
                            *speed_mbps,
                        ),
                    };
                    if self.progress_plain {
                        // tty-independent heartbeat: one line per interval
                        // through the normal line printer (no \r rewrites,
                        // so piped/docker logs stay clean).
                        if self.plain_progress_due() {
                            self.line_stderr(&content);
                        }
                    } else {
                        // \x1b[K erases to end of line so shrinking fields
                        // (unit crossings, a vanishing eta) leave no
                        // residue.
                        let _ = write!(self.stderr, "\r{content}\x1b[K");
                        self.progress_line_active = true;
                    }
                }
            }
            Event::FileComplete {
                filename,
                status,
                bytes,
            } => {
                if !self.quiet {
                    let mark = if *status == FileStatus::AlreadyExists {
                        "="
                    } else {
                        "+"
                    };
                    self.line_stderr(&format!(
                        " {} {} ({})",
                        mark,
                        truncate_path_cli(filename, 60),
                        size_full(*bytes)
                    ));
                }
            }
            Event::VerificationStart { .. } => {}
            Event::VerificationResult { filename, ok, .. } => {
                if !self.quiet {
                    let mark = if *ok { "✓" } else { "✗" };
                    let note = if *ok {
                        String::new()
                    } else {
                        " hash mismatch".to_string()
                    };
                    self.line_stderr(&format!(
                        " {} verified{}: {}",
                        mark,
                        note,
                        truncate_path_cli(filename, 60)
                    ));
                }
            }
            Event::Done { summary } => {
                self.line_stdout(&format!(
                    "Done: {} file(s), {} → {} downloaded, {} skipped (exists), {} verified, {} failed",
                    summary.files,
                    size_full(summary.total_bytes),
                    summary.downloaded,
                    summary.skipped,
                    summary.verified,
                    summary.failed,
                ));
                if summary.hash_mismatch > 0 {
                    self.line_stdout(&format!(
                        "Hash mismatches: {} (registry marked HashMismatch)",
                        summary.hash_mismatch
                    ));
                }
            }
            Event::SyncPlanned {
                model,
                files,
                skipped,
                total_bytes,
                ..
            } => {
                if !self.quiet {
                    self.line_stderr(&format!(
                        "{} file(s) to sync for {} ({} total, {} already cached)",
                        files.len(),
                        model,
                        size_full(*total_bytes),
                        skipped
                    ));
                }
            }
            Event::FilePublished { path, blob } => {
                if !self.quiet {
                    let short_oid = if blob.len() > 12 { &blob[..12] } else { blob };
                    self.line_stderr(&format!(
                        " ✓ published {} → blobs/{}…",
                        truncate_path_cli(path, 52),
                        short_oid
                    ));
                }
            }
            Event::SyncComplete { snapshot_path, .. } => {
                // plans/hf-cache-sync.md §2.4: in human mode the snapshot path IS the last line
                // (hf CLI parity). JSON consumers read the typed event.
                self.line_stdout(snapshot_path);
            }
            Event::Error { code, message, .. } => {
                self.line_stderr(&format!("error [{}]: {}", code, message));
            }
        }
    }

    /// Human rendering for free-text engine status lines (JSON mode drops
    /// them — the typed events carry the same information).
    pub fn status_line(&mut self, message: &str) {
        if self.json {
            return;
        }
        // Lines already covered by typed events
        let covered = message.starts_with("Starting download:")
            || message.starts_with("Verifying integrity of")
            || message.starts_with("Download complete")
            || message.starts_with("File ")
            || message.starts_with("✓ Hash verified")
            || message.starts_with("✗ Hash mismatch")
            || message.starts_with("Queued ")
            || message.starts_with("404 error, trying raw");
        if covered {
            return;
        }
        if let Some(model_id) = crate::engine::parse_auth_status(message) {
            self.line_stderr(&format!(
                "authentication required for {} (pass --token or set $HF_TOKEN)",
                model_id
            ));
            return;
        }
        let important = message.starts_with("Error:")
            || message.starts_with("Warning:")
            || message.contains("Retrying");
        if self.quiet && !important {
            return;
        }
        self.line_stderr(message);
    }

    /// Clear any trailing progress line (call before exiting).
    pub fn finish(&mut self) {
        self.clear_progress_line();
    }

    /// Human-mode destination hint on stdout after the summary (suppressed
    /// in --quiet, and never emitted in --json to keep stdout pure NDJSON).
    pub fn destination_line(&mut self, path: &str) {
        if !self.json && !self.quiet {
            self.line_stdout(&format!("Destination: {}", path));
        }
    }
}

/// Single-file progress line content (no `\r`/erase wrapper — the tty
/// renderer adds those; `--progress plain` prints it as-is).
///
/// Percent is computed from the raw byte counts: the event's `percent`
/// field is pre-rounded to 0.1%, so rounding it again would double-round.
pub(super) fn format_file_progress(
    filename: &str,
    downloaded: u64,
    total: u64,
    speed_mbps: f64,
) -> String {
    let pct = if total > 0 {
        (downloaded as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    format!(
        "{} {}% {} {}/{} {:.1} MB/s{}",
        truncate_path_cli(filename, 42),
        pct.round() as u64,
        bar_cli(downloaded, total),
        size_full(downloaded),
        size_full(total),
        speed_mbps,
        eta_suffix(speed_mbps, total.saturating_sub(downloaded)),
    )
}

/// Multi-file (aggregate) progress line content: aggregate first, the
/// active file demoted to name + percent. The aggregate percent is
/// clamped at 100% — actual bytes can exceed the tree-reported total
/// (Content-Range vs tree size).
pub(super) fn format_overall_progress(
    filename: &str,
    file_downloaded: u64,
    file_total: u64,
    overall: &OverallProgress,
    speed_mbps: f64,
) -> String {
    let file_pct = if file_total > 0 {
        (file_downloaded as f64 / file_total as f64) * 100.0
    } else {
        0.0
    };
    let overall_pct = if overall.total_bytes > 0 {
        ((overall.downloaded_bytes as f64 / overall.total_bytes as f64) * 100.0).min(100.0)
    } else {
        0.0
    };
    format!(
        "[{}/{} files {}% │ {}/{} │ {:.1} MB/s{}] ▸ {} {}%",
        overall.files_done,
        overall.files_total,
        overall_pct.round() as u64,
        size_full(overall.downloaded_bytes),
        size_full(overall.total_bytes),
        speed_mbps,
        eta_suffix(
            speed_mbps,
            overall.total_bytes.saturating_sub(overall.downloaded_bytes),
        ),
        truncate_path_cli(filename, 42),
        file_pct.round() as u64,
    )
}

/// `--progress plain` verification-drain heartbeat.
pub(super) fn verification_heartbeat_line(active: usize, done: usize) -> String {
    format!("verifying: {active} in flight, {done} verified")
}

/// Progress-line content for the self-update asset download (plan
/// M4/C5): the SAME shapes as the download progress lines — `bar_cli`,
/// rounded percent, `size_full` bytes — minus speed/eta (a one-shot
/// asset fetch has no speed estimate). Replaces `update_cmd`'s
/// hand-rolled `downloading… X / Y (Z%)` line so the two progress
/// disciplines cannot drift apart again.
pub(super) fn format_update_progress(downloaded: u64, total: u64) -> String {
    if total == 0 {
        return format!("downloading… {}", size_full(downloaded));
    }
    let pct = (downloaded as f64 / total as f64) * 100.0;
    format!(
        "downloading… {} {}% {}/{}",
        bar_cli(downloaded, total),
        pct.round() as u64,
        size_full(downloaded),
        size_full(total)
    )
}

/// Suffix `" eta <t>"` for the given remaining bytes at the given speed
/// (empty while the speed estimate is still warming up).
fn eta_suffix(speed_mbps: f64, remaining: u64) -> String {
    if speed_mbps > 0.01 {
        let secs = remaining as f64 / (speed_mbps * 1_048_576.0);
        format!(" eta {}", eta_cli(secs))
    } else {
        String::new()
    }
}
