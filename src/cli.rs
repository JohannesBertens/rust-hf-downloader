//! One-shot CLI download mode: `rust-hf-downloader download <MODEL_ID> …`
//!
//! Non-interactive frontend over the shared [`crate::engine`] pipeline,
//! designed for scripts and AI-agent skills:
//!
//! - human progress on **stderr** (single-line rewrites when interactive),
//!   the summary on **stdout**;
//! - `--json` emits stable NDJSON events on **stdout** (progress included,
//!   throttled; the `error` event is always the last line on failure);
//! - deterministic exit codes (see [`EXIT_USAGE`] etc.);
//! - ambiguous selections fail fast with the full structured file list so an
//!   agent can re-invoke with an explicit selector in one round-trip.
//!
//! The TUI remains the default when the binary is started without a
//! subcommand (see `main.rs`).

use crate::engine::{EngineState, ManagerHandle};
use crate::models::{ModelMetadata, QuantizationGroup};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Exit codes (see plans/add-cli.md §2.4)
// ---------------------------------------------------------------------------

/// All requested files present on disk (downloaded or already existed);
/// verification passed or skipped.
pub const EXIT_OK: i32 = 0;
/// Download failed after retries, or any hash mismatch.
pub const EXIT_FAILURE: i32 = 1;
/// Authentication required (gated repo / bad token).
pub const EXIT_AUTH: i32 = 2;
/// Usage error or resolution ambiguity (`EX_USAGE` convention). clap's own
/// usage errors are routed here too, so `2` stays reserved for auth.
pub const EXIT_USAGE: i32 = 64;
/// Interrupted by SIGINT.
pub const EXIT_INTERRUPTED: i32 = 130;

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "rust-hf-downloader",
    version, // from CARGO_PKG_VERSION; pinned by a unit test
    about = "TUI and CLI for downloading HuggingFace models",
    long_about = "TUI and CLI for downloading HuggingFace models.\n\nRun without a subcommand to start the interactive TUI."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// One-shot model download (non-interactive, script/AI-friendly)
    #[command(alias = "dl")]
    Download(DownloadArgs),
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// Model ID, e.g. "bartowski/Qwen2.5-7B-GGUF"
    #[arg(value_name = "MODEL_ID")]
    pub model_id: String,

    /// Quantization type filter for GGUF models, e.g. Q4_K_M, Q8_0
    #[arg(long, value_name = "TYPE")]
    pub quant: Option<String>,

    /// Exact repo-relative file path (repeatable)
    #[arg(long = "file", value_name = "PATH")]
    pub file: Vec<String>,

    /// Download the entire repository
    #[arg(long)]
    pub all: bool,

    /// Base output directory [default: config, usually ~/models]
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<String>,

    /// HuggingFace token [default: $HF_TOKEN, then config]
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Skip SHA256 verification
    #[arg(long)]
    pub no_verify: bool,

    /// JSON Lines events on stdout (progress included, throttled)
    #[arg(long)]
    pub json: bool,

    /// Suppress progress output; errors and the final summary only
    #[arg(short, long)]
    pub quiet: bool,
}

/// Token precedence: `--token` flag → `$HF_TOKEN` env → config file.
pub fn merge_token(
    flag: Option<String>,
    env: Option<String>,
    file: Option<String>,
) -> Option<String> {
    flag.filter(|token| !token.is_empty())
        .or_else(|| env.filter(|token| !token.is_empty()))
        .or_else(|| file.filter(|token| !token.is_empty()))
}

/// A model ID must be exactly `author/name` with non-empty parts.
pub fn valid_model_id(model_id: &str) -> bool {
    let parts: Vec<&str> = model_id.split('/').collect();
    parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty()
}

// ---------------------------------------------------------------------------
// File resolution (pure, unit-testable)
// ---------------------------------------------------------------------------

/// A concrete file selected for download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// No selector: only valid when the repo has exactly one downloadable file.
    Default,
    Quant(String),
    Files(Vec<String>),
    All,
}

/// Derive the selector from parsed args; rejects combined selectors.
pub fn parse_selector(args: &DownloadArgs) -> Result<Selector, String> {
    let n = usize::from(args.quant.is_some())
        + usize::from(!args.file.is_empty())
        + usize::from(args.all);
    match n {
        0 => Ok(Selector::Default),
        1 => {
            if let Some(quant) = &args.quant {
                Ok(Selector::Quant(quant.clone()))
            } else if args.all {
                Ok(Selector::All)
            } else {
                Ok(Selector::Files(args.file.clone()))
            }
        }
        _ => Err("use only one of --quant, --file, or --all".to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No selector given and the repo has more than one (or zero) files.
    Ambiguous { available: Vec<FileSpec> },
    /// The requested selector matched nothing.
    NoFilesMatch {
        selector: String,
        available: Vec<FileSpec>,
    },
}

impl ResolveError {
    fn code(&self) -> &'static str {
        match self {
            ResolveError::Ambiguous { .. } => "ambiguous",
            ResolveError::NoFilesMatch { .. } => "no_files_match",
        }
    }

    fn message(&self) -> String {
        match self {
            ResolveError::Ambiguous { available } => format!(
                "model has {} downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all",
                available.len()
            ),
            ResolveError::NoFilesMatch { selector, .. } => {
                format!("no downloadable file matches {}", selector)
            }
        }
    }
}

/// Resolve which files to download. Pure function over API data — no I/O.
///
/// GGUF multipart archives are separate files with individual SHA256s (they
/// are NOT concatenated); `--quant` naturally selects all parts of that
/// quantization.
pub fn resolve_files(
    metadata: &ModelMetadata,
    quants: &[QuantizationGroup],
    selector: &Selector,
) -> Result<Vec<FileSpec>, ResolveError> {
    // All downloadable files from the recursive tree (directories filtered,
    // same rule as the TUI's repository download)
    let available: Vec<FileSpec> = metadata
        .siblings
        .iter()
        .filter(|f| f.size.is_some() && !f.rfilename.ends_with('/'))
        .map(|f| FileSpec {
            filename: f.rfilename.clone(),
            size_bytes: f.size.unwrap_or(0),
            sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
        })
        .collect();

    let mut picked: Vec<FileSpec> = match selector {
        Selector::Files(names) => {
            let mut out = Vec::new();
            for name in names {
                match available.iter().find(|f| f.filename == *name) {
                    Some(file) => out.push(file.clone()),
                    None => {
                        return Err(ResolveError::NoFilesMatch {
                            selector: format!("--file {}", name),
                            available,
                        })
                    }
                }
            }
            out
        }
        Selector::Quant(quant) => {
            let mut out = Vec::new();
            for group in quants {
                if group.quant_type.eq_ignore_ascii_case(quant) {
                    for file in &group.files {
                        out.push(FileSpec {
                            filename: file.filename.clone(),
                            size_bytes: file.size,
                            sha256: file.sha256.clone(),
                        });
                    }
                }
            }
            if out.is_empty() {
                return Err(ResolveError::NoFilesMatch {
                    selector: format!("--quant {}", quant),
                    available,
                });
            }
            out
        }
        Selector::All => available.clone(),
        Selector::Default => {
            if available.len() == 1 {
                available.clone()
            } else {
                return Err(ResolveError::Ambiguous { available });
            }
        }
    };

    // Dedup by filename, preserving order (repeatable --file etc.)
    let mut seen = HashSet::new();
    picked.retain(|f| seen.insert(f.filename.clone()));

    Ok(picked)
}

// ---------------------------------------------------------------------------
// JSON event schema (stable, additive-only; snapshot-tested)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileDto {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
}

impl From<&FileSpec> for FileDto {
    fn from(f: &FileSpec) -> Self {
        Self {
            filename: f.filename.clone(),
            size_bytes: f.size_bytes,
            sha256: f.sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub files: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub verified: usize,
    pub failed: usize,
    pub hash_mismatch: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Resolved {
        model: String,
        files: Vec<FileDto>,
        total_bytes: u64,
    },
    DownloadStart {
        filename: String,
        index: usize,
        count: usize,
        size_bytes: u64,
    },
    Progress {
        filename: String,
        downloaded_bytes: u64,
        total_bytes: u64,
        speed_mbps: f64,
        percent: f64,
    },
    FileComplete {
        filename: String,
        status: &'static str, // "downloaded" | "already_exists"
        bytes: u64,
    },
    VerificationStart {
        filename: String,
    },
    VerificationResult {
        filename: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actual_sha256: Option<String>,
    },
    Done {
        summary: Summary,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        available: Option<Vec<FileDto>>,
    },
}

// ---------------------------------------------------------------------------
// Reporters
// ---------------------------------------------------------------------------

const PROGRESS_BAR_WIDTH: usize = 20;
/// Minimum interval between JSON `progress` events per run.
const JSON_PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Renders [`Event`]s either as human-readable text (progress to stderr,
/// summary to stdout) or as NDJSON on stdout.
pub struct Reporter {
    json: bool,
    quiet: bool,
    /// Single-line `\r` progress rewrites are only used when stderr is a tty.
    progress_to_tty: bool,
    progress_line_active: bool,
    last_json_progress: Option<Instant>,
}

impl Reporter {
    pub fn new(json: bool, quiet: bool) -> Self {
        Self {
            json,
            quiet,
            progress_to_tty: !json && !quiet && std::io::stderr().is_terminal(),
            progress_line_active: false,
            last_json_progress: None,
        }
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
            let mut stderr = std::io::stderr().lock();
            let _ = write!(stderr, "\r\x1b[2K");
            self.progress_line_active = false;
        }
    }

    fn line_stderr(&mut self, text: &str) {
        self.clear_progress_line();
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{}", text);
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

        use crate::utils::format_size;
        match event {
            Event::Resolved {
                files, total_bytes, ..
            } => {
                if !self.quiet {
                    self.line_stderr(&format!(
                        "{} file(s) to download, {} total",
                        files.len(),
                        format_size(*total_bytes)
                    ));
                }
            }
            Event::DownloadStart { .. } => {} // covered by the progress line
            Event::Progress {
                filename,
                downloaded_bytes,
                total_bytes,
                speed_mbps,
                ..
            } => {
                if self.progress_to_tty {
                    let bar = render_bar(*downloaded_bytes, *total_bytes);
                    let eta = if *speed_mbps > 0.01 {
                        let remaining = total_bytes.saturating_sub(*downloaded_bytes);
                        let secs = remaining as f64 / (speed_mbps * 1_048_576.0);
                        format!(" eta {}", format_eta(secs))
                    } else {
                        String::new()
                    };
                    let percent = if *total_bytes > 0 {
                        (*downloaded_bytes as f64 / *total_bytes as f64) * 100.0
                    } else {
                        0.0
                    };
                    let mut stderr = std::io::stderr().lock();
                    let _ = write!(
                        stderr,
                        "\r{} {}% {} {}/{} {:.1} MB/s{}",
                        truncate_path(filename, 42),
                        percent.round() as u64,
                        bar,
                        format_size(*downloaded_bytes),
                        format_size(*total_bytes),
                        speed_mbps,
                        eta
                    );
                    self.progress_line_active = true;
                }
            }
            Event::FileComplete {
                filename,
                status,
                bytes,
            } => {
                if !self.quiet {
                    let mark = if *status == "already_exists" {
                        "="
                    } else {
                        "+"
                    };
                    self.line_stderr(&format!(
                        " {} {} ({})",
                        mark,
                        truncate_path(filename, 60),
                        format_size(*bytes)
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
                        truncate_path(filename, 60)
                    ));
                }
            }
            Event::Done { summary } => {
                self.line_stdout(&format!(
                    "Done: {} file(s), {} → {} downloaded, {} skipped (exists), {} verified, {} failed",
                    summary.files,
                    format_size(summary.total_bytes),
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
        if let Some(model_id) = message.strip_prefix("AUTH_ERROR:") {
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

fn render_bar(done: u64, total: u64) -> String {
    let filled = if total == 0 {
        PROGRESS_BAR_WIDTH
    } else {
        ((done as f64 / total as f64) * PROGRESS_BAR_WIDTH as f64).round() as usize
    }
    .min(PROGRESS_BAR_WIDTH);
    format!(
        "[{}{}]",
        "█".repeat(filled),
        "░".repeat(PROGRESS_BAR_WIDTH - filled)
    )
}

fn format_eta(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "?".to_string();
    }
    let secs = secs.round() as u64;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate a path-like string for single-line display, keeping the
/// (differing) tail.
pub fn truncate_path(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(s.chars().count() + 1 - max).collect();
        format!("…{}", tail)
    }
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Run a parsed CLI command; returns the process exit code.
pub async fn run(command: Command) -> i32 {
    let Command::Download(args) = command;
    run_download(args).await
}

/// Everything the monitor loop accumulates for the summary and exit code.
#[derive(Default)]
struct RunTally {
    files: usize,
    downloaded: usize,
    skipped: usize,
    verified: usize,
    failed: usize,
    hash_mismatch: usize,
    total_bytes: u64,
    auth_required: bool,
    failures: Vec<String>,
    mismatches: Vec<String>,
}

impl RunTally {
    fn exit_code(&self) -> i32 {
        if self.auth_required {
            EXIT_AUTH
        } else if self.failed > 0 || self.hash_mismatch > 0 {
            EXIT_FAILURE
        } else {
            EXIT_OK
        }
    }
}

async fn run_download(args: DownloadArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, args.quiet);

    // --- 1. Configuration ------------------------------------------------
    let mut options = crate::config::load_config();
    if let Some(dir) = &args.output {
        options.default_directory = dir.clone();
    }
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );
    options.hf_token = token.clone();
    crate::config::apply_options(&options);
    if args.no_verify {
        crate::download::DOWNLOAD_CONFIG
            .enable_verification
            .store(false, Ordering::Relaxed);
    }

    // --- 2. Validate usage ------------------------------------------------
    if !valid_model_id(&args.model_id) {
        reporter.emit(&Event::Error {
            code: "usage".to_string(),
            message: format!(
                "invalid model ID {:?} — expected \"author/model-name\"",
                args.model_id
            ),
            available: None,
        });
        return EXIT_USAGE;
    }
    let selector = match parse_selector(&args) {
        Ok(selector) => selector,
        Err(message) => {
            reporter.emit(&Event::Error {
                code: "usage".to_string(),
                message,
                available: None,
            });
            return EXIT_USAGE;
        }
    };

    // --- 3. Resolve files ---------------------------------------------------
    let metadata = match crate::api::fetch_model_metadata(&args.model_id, token.as_ref()).await {
        Ok(metadata) => metadata,
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::Error {
                code: if not_found {
                    "not_found".to_string()
                } else {
                    "network".to_string()
                },
                message: format!("failed to fetch model info for {}: {}", args.model_id, e),
                available: None,
            });
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
    };

    // Quantization listing is only needed to resolve --quant
    let quants = if matches!(selector, Selector::Quant(_)) {
        match crate::api::fetch_model_files(&args.model_id, token.as_ref()).await {
            Ok(quants) => quants,
            Err(e) => {
                reporter.emit(&Event::Error {
                    code: "network".to_string(),
                    message: format!("failed to list model files: {}", e),
                    available: None,
                });
                return EXIT_FAILURE;
            }
        }
    } else {
        Vec::new()
    };

    let files = match resolve_files(&metadata, &quants, &selector) {
        Ok(files) => files,
        Err(err) => {
            reporter.emit(&Event::Error {
                code: err.code().to_string(),
                message: err.message(),
                available: Some(err.available().iter().map(FileDto::from).collect()),
            });
            return EXIT_USAGE;
        }
    };

    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    reporter.emit(&Event::Resolved {
        model: args.model_id.clone(),
        files: files.iter().map(FileDto::from).collect(),
        total_bytes,
    });

    // --- 4. Register + queue through the shared engine ---------------------
    let base = options.default_directory.clone();
    let pending: Vec<(String, u64, Option<String>)> = files
        .iter()
        .map(|f| (f.filename.clone(), f.size_bytes, f.sha256.clone()))
        .collect();
    if let Err(message) = crate::engine::register_pending(&args.model_id, &pending, &base) {
        reporter.emit(&Event::Error {
            code: "invalid_path".to_string(),
            message,
            available: None,
        });
        return EXIT_FAILURE;
    }

    let (state, download_tx) = EngineState::new();
    crate::engine::spawn_verification_worker(state.clone());
    let manager = crate::engine::spawn_manager(state.clone());

    // Model files land under base/author/model-name (same layout as the TUI)
    let parts: Vec<&str> = args.model_id.split('/').collect();
    let model_path = PathBuf::from(&base).join(parts[0]).join(parts[1]);

    // Queue accounting + sends (mirrors the TUI's confirm_download)
    {
        let mut queue = state.download_queue.lock().await;
        queue.add(files.len(), total_bytes);
    }
    {
        let mut items = state.download_queue_items.lock().await;
        for file in &files {
            items.push(crate::models::QueueItemSummary {
                filename: file.filename.clone(),
                total_size: file.size_bytes,
            });
        }
    }
    for file in &files {
        let _ = download_tx.send((
            args.model_id.clone(),
            file.filename.clone(),
            model_path.clone(),
            file.sha256.clone(),
            token.clone(),
            file.size_bytes,
        ));
    }
    // Dropping the sender closes the channel — the manager drains, then its
    // join handle resolves. This is the deterministic completion signal.
    drop(download_tx);

    // --- 5. Monitor until drained ------------------------------------------
    let mut tally = RunTally {
        files: files.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, &mut reporter).await;

    // --- 6. Summary + exit code --------------------------------------------
    let summary = Summary {
        files: tally.files,
        downloaded: tally.downloaded,
        skipped: tally.skipped,
        verified: tally.verified,
        failed: tally.failed,
        hash_mismatch: tally.hash_mismatch,
        total_bytes: tally.total_bytes,
    };
    reporter.emit(&Event::Done {
        summary: summary.clone(),
    });
    reporter.destination_line(&model_path.display().to_string());

    reporter.finish();

    if interrupted {
        reporter.emit(&Event::Error {
            code: "interrupted".to_string(),
            message: "interrupted by SIGINT; unfinished files stay registered as incomplete and restart from scratch on the next run".to_string(),
            available: None,
        });
        return EXIT_INTERRUPTED;
    }
    if !tally.failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "download_failed".to_string(),
            message: tally.failures.join("; "),
            available: None,
        });
    }
    if !tally.mismatches.is_empty() {
        reporter.emit(&Event::Error {
            code: "hash_mismatch".to_string(),
            message: tally.mismatches.join("; "),
            available: None,
        });
    }
    if tally.auth_required {
        reporter.emit(&Event::Error {
            code: "auth_required".to_string(),
            message: format!(
                "authentication required for {} (pass --token or set $HF_TOKEN)",
                args.model_id
            ),
            available: None,
        });
    }

    tally.exit_code()
}

/// Poll shared engine state, render, and wait for deterministic drain.
/// Returns true when interrupted by SIGINT.
async fn monitor(
    state: &EngineState,
    manager: ManagerHandle,
    files: &[FileSpec],
    tally: &mut RunTally,
    reporter: &mut Reporter,
) -> bool {
    let count = files.len();
    let index_of: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.filename.as_str(), i + 1))
        .collect();

    let mut seen_download: Option<String> = None;
    let mut seen_verifying: HashSet<String> = HashSet::new();

    let mut join = manager.join;
    let mut ticker = tokio::time::interval(Duration::from_millis(400));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick

    let interrupted = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break true,
            result = &mut join => {
                if let Ok(outcomes) = result {
                    count_outcomes(&outcomes, tally);
                }
                break false;
            }
            _ = ticker.tick() => {
                poll_once(
                    state,
                    count,
                    &index_of,
                    &mut seen_download,
                    &mut seen_verifying,
                    tally,
                    reporter,
                )
                .await;
            }
        }
    };

    if interrupted {
        // Abort the manager: the in-flight file stops mid-download; any
        // .incomplete file is deleted on the next run (existing
        // restart-from-scratch semantics) and registry entries stay
        // Incomplete for the TUI's resume view.
        join.abort();
        return true;
    }

    // All downloads drained; every queue_verification call has happened.
    // Wait for the verification worker to run dry (race-free idle signal).
    loop {
        if state.verification_idle() {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                return true;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                poll_once(
                    state,
                    count,
                    &index_of,
                    &mut seen_download,
                    &mut seen_verifying,
                    tally,
                    reporter,
                )
                .await;
            }
        }
    }

    // Final drain of everything that arrived during the last tick
    poll_once(
        state,
        count,
        &index_of,
        &mut seen_download,
        &mut seen_verifying,
        tally,
        reporter,
    )
    .await;

    false
}

/// Non-blocking snapshot of engine state: drain channels, detect new
/// downloads/verifications, emit events.
#[allow(clippy::too_many_arguments)]
async fn poll_once(
    state: &EngineState,
    count: usize,
    index_of: &HashMap<&str, usize>,
    seen_download: &mut Option<String>,
    seen_verifying: &mut HashSet<String>,
    tally: &mut RunTally,
    reporter: &mut Reporter,
) {
    // Free-text status lines (human mode only; JSON uses typed events)
    if let Ok(mut rx) = state.status_rx.try_lock() {
        while let Ok(message) = rx.try_recv() {
            reporter.status_line(&message);
        }
    }

    // Streaming per-file outcomes
    if let Ok(mut rx) = state.outcome_rx.try_lock() {
        while let Ok(outcome) = rx.try_recv() {
            apply_outcome_event(&outcome, index_of, count, reporter, tally);
        }
    }

    // Verification starts (new entries in the active-progress list)
    if let Ok(progress) = state.verification_progress.try_lock() {
        for entry in progress.iter() {
            if seen_verifying.insert(entry.filename.clone()) {
                reporter.emit(&Event::VerificationStart {
                    filename: entry.filename.clone(),
                });
            }
        }
    }

    // Typed verification results
    if let Ok(mut rx) = state.verify_rx.try_lock() {
        while let Ok(outcome) = rx.try_recv() {
            apply_verify_outcome(&outcome, reporter, tally);
        }
    }

    // Download progress (single line for the currently-active file)
    if let Ok(guard) = state.download_progress.try_lock() {
        if let Some(progress) = guard.as_ref() {
            if seen_download.as_deref() != Some(progress.filename.as_str()) {
                *seen_download = Some(progress.filename.clone());
                reporter.emit(&Event::DownloadStart {
                    filename: progress.filename.clone(),
                    index: index_of
                        .get(progress.filename.as_str())
                        .copied()
                        .unwrap_or(0),
                    count,
                    size_bytes: progress.total,
                });
            }
            let percent = if progress.total > 0 {
                (progress.downloaded as f64 / progress.total as f64) * 100.0
            } else {
                0.0
            };
            reporter.emit(&Event::Progress {
                filename: progress.filename.clone(),
                downloaded_bytes: progress.downloaded,
                total_bytes: progress.total,
                speed_mbps: progress.speed_mbps,
                percent: (percent * 10.0).round() / 10.0,
            });
        }
    }
}

fn apply_outcome_event(
    outcome: &crate::models::FileOutcome,
    index_of: &HashMap<&str, usize>,
    count: usize,
    reporter: &mut Reporter,
    tally: &mut RunTally,
) {
    use crate::models::FileOutcome;
    match outcome {
        FileOutcome::Complete { filename, bytes } => {
            tally.downloaded += 1;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: "downloaded",
                bytes: *bytes,
            });
        }
        FileOutcome::AlreadyExists { filename, bytes } => {
            tally.skipped += 1;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: "already_exists",
                bytes: *bytes,
            });
        }
        FileOutcome::AuthRequired { model_id } => {
            tally.auth_required = true;
            reporter.emit(&Event::Error {
                code: "auth_required".to_string(),
                message: format!(
                    "authentication required for {} (pass --token or set $HF_TOKEN)",
                    model_id
                ),
                available: None,
            });
        }
        FileOutcome::Failed { filename, reason } => {
            tally.failed += 1;
            tally.failures.push(format!("{}: {}", filename, reason));
            reporter.emit(&Event::Error {
                code: "download_failed".to_string(),
                message: format!("{}: {}", filename, reason),
                available: None,
            });
            let _ = (index_of, count);
        }
    }
}

fn count_outcomes(outcomes: &[crate::models::FileOutcome], tally: &mut RunTally) {
    // The join handle returns the authoritative full list. The monitor loop
    // already counted streamed outcomes in the common case; recount from
    // scratch to stay correct if any events were missed.
    tally.downloaded = 0;
    tally.skipped = 0;
    tally.failed = 0;
    tally.auth_required = false;
    tally.failures.clear();
    for outcome in outcomes {
        match outcome {
            crate::models::FileOutcome::Complete { .. } => tally.downloaded += 1,
            crate::models::FileOutcome::AlreadyExists { .. } => tally.skipped += 1,
            crate::models::FileOutcome::AuthRequired { .. } => tally.auth_required = true,
            crate::models::FileOutcome::Failed { filename, reason } => {
                tally.failed += 1;
                tally.failures.push(format!("{}: {}", filename, reason));
            }
        }
    }
}

fn apply_verify_outcome(
    outcome: &crate::models::VerifyOutcome,
    reporter: &mut Reporter,
    tally: &mut RunTally,
) {
    use crate::models::VerifyOutcome;
    match outcome {
        VerifyOutcome::Ok { filename } => {
            tally.verified += 1;
            reporter.emit(&Event::VerificationResult {
                filename: filename.clone(),
                ok: true,
                expected_sha256: None,
                actual_sha256: None,
            });
        }
        VerifyOutcome::Mismatch {
            filename,
            expected_sha256,
            actual_sha256,
        } => {
            tally.hash_mismatch += 1;
            tally.mismatches.push(filename.clone());
            reporter.emit(&Event::VerificationResult {
                filename: filename.clone(),
                ok: false,
                expected_sha256: Some(expected_sha256.clone()),
                actual_sha256: Some(actual_sha256.clone()),
            });
        }
        VerifyOutcome::Error { filename, reason } => {
            reporter.emit(&Event::Error {
                code: "verification_error".to_string(),
                message: format!("{}: {}", filename, reason),
                available: None,
            });
        }
        VerifyOutcome::Missing { filename } => {
            reporter.emit(&Event::Error {
                code: "verification_error".to_string(),
                message: format!("{}: file not found for verification", filename),
                available: None,
            });
        }
    }
}

impl ResolveError {
    fn available(&self) -> &Vec<FileSpec> {
        match self {
            ResolveError::Ambiguous { available }
            | ResolveError::NoFilesMatch { available, .. } => available,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file_spec(filename: &str, size: u64) -> FileSpec {
        FileSpec {
            filename: filename.to_string(),
            size_bytes: size,
            sha256: None,
        }
    }

    fn metadata_with(files: &[(&str, Option<u64>)]) -> ModelMetadata {
        ModelMetadata {
            model_id: "a/b".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: files
                .iter()
                .map(|(name, size)| crate::models::RepoFile {
                    rfilename: name.to_string(),
                    size: *size,
                    lfs: None,
                })
                .collect(),
            tags: Vec::new(),
        }
    }

    // --- resolve_files -----------------------------------------------------

    #[test]
    fn resolve_default_single_file_repo() {
        let metadata = metadata_with(&[("only.gguf", Some(10))]);
        let files = resolve_files(&metadata, &[], &Selector::Default).unwrap();
        assert_eq!(files, vec![file_spec("only.gguf", 10)]);
    }

    #[test]
    fn resolve_default_multi_file_repo_is_ambiguous_with_list() {
        let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
        let err = resolve_files(&metadata, &[], &Selector::Default).unwrap_err();
        match err {
            ResolveError::Ambiguous { available } => {
                assert_eq!(available.len(), 2);
                assert_eq!(available[1].filename, "b.gguf");
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn resolve_quant_case_insensitive_from_groups() {
        let metadata = metadata_with(&[]);
        let quants = vec![QuantizationGroup {
            quant_type: "Q4_K_M".to_string(),
            files: vec![
                crate::models::QuantizationInfo {
                    quant_type: "Q4_K_M".to_string(),
                    filename: "m-00001-of-00002.gguf".to_string(),
                    size: 1,
                    sha256: Some("dead".to_string()),
                },
                crate::models::QuantizationInfo {
                    quant_type: "Q4_K_M".to_string(),
                    filename: "m-00002-of-00002.gguf".to_string(),
                    size: 2,
                    sha256: None,
                },
            ],
            total_size: 3,
        }];
        let files =
            resolve_files(&metadata, &quants, &Selector::Quant("q4_k_m".to_string())).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].sha256.as_deref(), Some("dead"));
        // all parts of the quantization are selected
        assert_eq!(files[1].filename, "m-00002-of-00002.gguf");
    }

    #[test]
    fn resolve_quant_miss_lists_available() {
        let metadata = metadata_with(&[("a.gguf", Some(10))]);
        let err = resolve_files(&metadata, &[], &Selector::Quant("Q8_0".to_string())).unwrap_err();
        assert!(matches!(err, ResolveError::NoFilesMatch { .. }));
        assert_eq!(err.code(), "no_files_match");
    }

    #[test]
    fn resolve_files_exact_and_missing() {
        let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
        let files =
            resolve_files(&metadata, &[], &Selector::Files(vec!["b.gguf".to_string()])).unwrap();
        assert_eq!(files, vec![file_spec("b.gguf", 20)]);

        let err = resolve_files(
            &metadata,
            &[],
            &Selector::Files(vec!["nope.gguf".to_string()]),
        )
        .unwrap_err();
        assert_eq!(err.code(), "no_files_match");
        assert_eq!(err.available().len(), 2);
    }

    #[test]
    fn resolve_files_dedups_repeatable_selectors() {
        let metadata = metadata_with(&[("a.gguf", Some(10))]);
        let files = resolve_files(
            &metadata,
            &[],
            &Selector::Files(vec!["a.gguf".to_string(), "a.gguf".to_string()]),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn resolve_all_skips_directories_and_unsized() {
        let metadata = metadata_with(&[
            ("a.gguf", Some(10)),
            ("subdir/", None), // directory marker
            ("broken", None),  // no size → skipped
        ]);
        let files = resolve_files(&metadata, &[], &Selector::All).unwrap();
        assert_eq!(files, vec![file_spec("a.gguf", 10)]);
    }

    // --- selectors / args ---------------------------------------------------

    fn download_args(quant: Option<&str>, file: &[&str], all: bool) -> DownloadArgs {
        DownloadArgs {
            model_id: "a/b".to_string(),
            quant: quant.map(String::from),
            file: file.iter().map(|f| f.to_string()).collect(),
            all,
            output: None,
            token: None,
            no_verify: false,
            json: false,
            quiet: false,
        }
    }

    #[test]
    fn selector_default_when_none_given() {
        assert_eq!(
            parse_selector(&download_args(None, &[], false)).unwrap(),
            Selector::Default
        );
    }

    #[test]
    fn selector_conflicts_rejected() {
        assert!(parse_selector(&download_args(Some("Q4"), &[], true)).is_err());
        assert!(parse_selector(&download_args(None, &["a.gguf"], true)).is_err());
        assert!(parse_selector(&download_args(Some("Q4"), &["a.gguf"], false)).is_err());
    }

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

    // --- CLI parsing --------------------------------------------------------

    #[test]
    fn version_string_matches_cargo_pkg_version() {
        // v1 CLI drifted here (stale hardcoded version) — never again.
        let err = Cli::try_parse_from(["rust-hf-downloader", "--version"]).unwrap_err();
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
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
                assert!(args.no_verify);
                assert!(args.json);
            }
            other => panic!("expected download, got {:?}", other),
        }
    }

    #[test]
    fn no_subcommand_means_tui() {
        let cli = Cli::try_parse_from(["rust-hf-downloader"]).unwrap();
        assert!(cli.command.is_none());
    }

    // --- rendering helpers --------------------------------------------------

    #[test]
    fn bar_and_eta_render() {
        assert_eq!(render_bar(0, 10), format!("[{}]", "░".repeat(20)));
        assert_eq!(
            render_bar(5, 10),
            format!("[{}{}]", "█".repeat(10), "░".repeat(10))
        );
        assert_eq!(render_bar(10, 10), format!("[{}]", "█".repeat(20)));
        assert_eq!(format_eta(59.4), "59s");
        assert_eq!(format_eta(95.0), "1m35s");
        assert_eq!(format_eta(3700.0), "1h1m");
    }

    #[test]
    fn truncate_keeps_tail() {
        assert_eq!(truncate_path("short.gguf", 20), "short.gguf");
        let long = "author/model-name/subdir/file-Q4_K_M.gguf";
        let cut = truncate_path(long, 20);
        assert!(cut.starts_with('…'));
        assert!(cut.ends_with("Q4_K_M.gguf"));
        assert_eq!(cut.chars().count(), 20);
    }

    // --- JSON event schema snapshots (insta) --------------------------------

    fn snap(event: &Event, name: &str) {
        let json = serde_json::to_string_pretty(event).unwrap();
        insta::assert_snapshot!(name, json);
    }

    #[test]
    fn snapshot_event_resolved() {
        snap(
            &Event::Resolved {
                model: "org/model".to_string(),
                files: vec![FileDto {
                    filename: "model-Q4_K_M.gguf".to_string(),
                    size_bytes: 4_947_802_324,
                    sha256: Some("a".repeat(64)),
                }],
                total_bytes: 4_947_802_324,
            },
            "event-resolved",
        );
    }

    #[test]
    fn snapshot_event_progress() {
        snap(
            &Event::Progress {
                filename: "model-Q4_K_M.gguf".to_string(),
                downloaded_bytes: 1_048_576,
                total_bytes: 4_947_802_324,
                speed_mbps: 62.4,
                percent: 0.021_183,
            },
            "event-progress",
        );
    }

    #[test]
    fn snapshot_event_file_complete() {
        snap(
            &Event::FileComplete {
                filename: "model-Q4_K_M.gguf".to_string(),
                status: "downloaded",
                bytes: 4_947_802_324,
            },
            "event-file-complete",
        );
    }

    #[test]
    fn snapshot_event_verification_result_ok_omits_hashes() {
        snap(
            &Event::VerificationResult {
                filename: "model-Q4_K_M.gguf".to_string(),
                ok: true,
                expected_sha256: None,
                actual_sha256: None,
            },
            "event-verification-ok",
        );
    }

    #[test]
    fn snapshot_event_verification_result_mismatch_includes_hashes() {
        snap(
            &Event::VerificationResult {
                filename: "model-Q4_K_M.gguf".to_string(),
                ok: false,
                expected_sha256: Some("e".repeat(64)),
                actual_sha256: Some("a".repeat(64)),
            },
            "event-verification-mismatch",
        );
    }

    #[test]
    fn snapshot_event_done() {
        snap(
            &Event::Done {
                summary: Summary {
                    files: 3,
                    downloaded: 2,
                    skipped: 1,
                    verified: 3,
                    failed: 0,
                    hash_mismatch: 0,
                    total_bytes: 4_947_802_324,
                },
            },
            "event-done",
        );
    }

    #[test]
    fn snapshot_event_error_ambiguous_lists_available() {
        snap(
            &Event::Error {
                code: "ambiguous".to_string(),
                message: "model has 2 downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all".to_string(),
                available: Some(vec![
                    FileDto {
                        filename: "model-Q4_K_M.gguf".to_string(),
                        size_bytes: 1,
                        sha256: None,
                    },
                    FileDto {
                        filename: "model-Q8_0.gguf".to_string(),
                        size_bytes: 2,
                        sha256: None,
                    },
                ]),
            },
            "event-error-ambiguous",
        );
    }
}
