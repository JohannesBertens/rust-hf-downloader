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
//! `hf-cache sync` (plans/hf-cache-sync.md §2/§5.2) reuses the same engine
//! to populate the real HuggingFace hub cache, publishing staged downloads
//! atomically through [`crate::hf_cache`]; `hf-cache path` is the pure
//! path-math scripting helper.
//!
//! The TUI remains the default when the binary is started without a
//! subcommand (see `main.rs`).

use crate::engine::{EngineState, ManagerHandle};
use crate::models::{FileOutcome, ModelMetadata, QuantizationGroup, VerifyOutcome};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
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
/// `update --check` found a newer release (nothing was installed).
pub const EXIT_UPDATE_AVAILABLE: i32 = 70;
/// `update` downloaded an asset whose SHA256 did not match the manifest.
pub const EXIT_CHECKSUM: i32 = 71;
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

    /// Populate the HuggingFace hub cache for offline serving (vLLM,
    /// transformers)
    HfCache(HfCacheArgs),

    /// Search HuggingFace models (query-only; prints a table or a JSON array)
    Search(SearchArgs),

    /// Update the installed binary to the newest release
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only check for a newer release; exits 70 when one exists
    #[arg(long)]
    pub check: bool,

    /// Reinstall even if the newest version is already installed
    #[arg(long)]
    pub force: bool,

    /// Emit JSON Lines events instead of human text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Search query, e.g. "qwen 2.5 gguf"
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Sort field [default: config default_sort_field]
    #[arg(long, value_enum)]
    pub sort: Option<SortArg>,

    /// Sort direction [default: config default_sort_direction]
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,

    /// Minimum downloads filter (applied client-side)
    #[arg(long, value_name = "N")]
    pub min_downloads: Option<u64>,

    /// Minimum likes filter (applied client-side)
    #[arg(long, value_name = "N")]
    pub min_likes: Option<u64>,

    /// Maximum number of results (1-500)
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    pub limit: usize,

    /// HuggingFace token [default: $HF_TOKEN, then config]
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// One JSON array on stdout (queries emit a document, not NDJSON events)
    #[arg(long)]
    pub json: bool,
}

/// Validate `--limit` (1-500) with a plain parser fn — clap's ranged value
/// parsers need features this build intentionally omits.
fn parse_limit(s: &str) -> Result<usize, String> {
    let n: usize = s.parse().map_err(|_| format!("invalid limit: {:?}", s))?;
    if (1..=500).contains(&n) {
        Ok(n)
    } else {
        Err("limit must be between 1 and 500".to_string())
    }
}

/// CLI sort fields. Maps onto the shared `models::SortField` so the CLI and
/// TUI cannot drift (the v1 CLI's --sort flag was silently ignored).
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortArg {
    Downloads,
    Likes,
    Modified,
    Name,
}

impl From<SortArg> for crate::models::SortField {
    fn from(value: SortArg) -> Self {
        match value {
            SortArg::Downloads => crate::models::SortField::Downloads,
            SortArg::Likes => crate::models::SortField::Likes,
            SortArg::Modified => crate::models::SortField::Modified,
            SortArg::Name => crate::models::SortField::Name,
        }
    }
}

/// CLI sort directions (`asc`/`desc` aliases for the long forms).
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionArg {
    #[value(alias = "asc")]
    Ascending,
    #[value(alias = "desc")]
    Descending,
}

impl From<DirectionArg> for crate::models::SortDirection {
    fn from(value: DirectionArg) -> Self {
        match value {
            DirectionArg::Ascending => crate::models::SortDirection::Ascending,
            DirectionArg::Descending => crate::models::SortDirection::Descending,
        }
    }
}

/// Search result row (stable JSON contract; `ModelInfo` serialization is an
/// implementation detail, this DTO is a decision).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelDto {
    pub id: String,
    pub author: Option<String>,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub tags: Vec<String>,
}

impl From<&crate::models::ModelInfo> for ModelDto {
    fn from(m: &crate::models::ModelInfo) -> Self {
        Self {
            id: m.id.clone(),
            author: m.author.clone(),
            downloads: m.downloads,
            likes: m.likes,
            last_modified: m.last_modified.clone(),
            tags: m.tags.clone(),
        }
    }
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

    /// Enable download rate limiting (uses --rate-limit-mbps or the
    /// config-file value)
    #[arg(long, conflicts_with = "no_rate_limit")]
    pub rate_limit: bool,

    /// Disable download rate limiting (overrides the config file)
    #[arg(long)]
    pub no_rate_limit: bool,

    /// Download rate limit in Mbps (implies --rate-limit)
    #[arg(
        long,
        value_name = "MBPS",
        value_parser = parse_rate_limit_mbps,
        conflicts_with = "no_rate_limit"
    )]
    pub rate_limit_mbps: Option<f64>,

    /// Git revision to download from: branch, tag, or commit SHA
    /// [default: main]
    #[arg(long, value_name = "REV", value_parser = parse_revision)]
    pub revision: Option<String>,
}

/// `hf-cache` subcommand group (plans/hf-cache-sync.md §2.1): the hub-cache
/// writer plus its scripting helper.
#[derive(Args, Debug)]
pub struct HfCacheArgs {
    #[command(subcommand)]
    pub command: HfCacheCommand,
}

/// Nested `hf-cache` subcommands.
#[derive(Subcommand, Debug)]
pub enum HfCacheCommand {
    /// Populate the real HuggingFace hub cache (~/.cache/huggingface/hub)
    /// so vLLM/transformers/`hf download` find a revision-pinned snapshot
    /// with zero network calls
    Sync(HfCacheSyncArgs),

    /// Print the snapshot path for a model/revision (refs/ lookup with an
    /// online fallback; scripting helper)
    Path(HfCachePathArgs),
}

#[derive(Args, Debug)]
pub struct HfCacheSyncArgs {
    /// Model ID, e.g. "Qwen/Qwen2.5-7B-Instruct"
    #[arg(value_name = "MODEL_ID")]
    pub model_id: String,

    /// Exact repo-relative file paths (repeatable positional)
    #[arg(value_name = "FILE")]
    pub files: Vec<String>,

    /// Git revision to sync: branch, tag, or 40-hex commit SHA
    /// [default: main]
    #[arg(long, value_name = "REV", value_parser = parse_revision)]
    pub revision: Option<String>,

    /// Fetch only the files a target runtime reads (preset selection)
    #[arg(long = "for", value_name = "PRESET", value_parser = parse_preset)]
    pub for_preset: Option<String>,

    /// Include only files matching GLOB (repeatable, fnmatch semantics:
    /// `*` crosses `/`)
    #[arg(long = "include", value_name = "GLOB")]
    pub include: Vec<String>,

    /// Exclude files matching GLOB (repeatable; applies after every
    /// selection mode)
    #[arg(long = "exclude", value_name = "GLOB")]
    pub exclude: Vec<String>,

    /// HuggingFace hub cache directory [default: $HF_HUB_CACHE, then
    /// $HF_HOME/hub, then the platform default]
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<String>,

    /// Copy files into snapshots/ instead of symlinking to blobs/
    #[arg(long)]
    pub no_symlinks: bool,

    /// Re-download even if the blob already exists in the cache
    #[arg(long)]
    pub force: bool,

    /// List what would be fetched/skipped; no writes
    #[arg(long)]
    pub dry_run: bool,

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

    /// Enable download rate limiting (uses --rate-limit-mbps or the
    /// config-file value)
    #[arg(long, conflicts_with = "no_rate_limit")]
    pub rate_limit: bool,

    /// Disable download rate limiting (overrides the config file)
    #[arg(long)]
    pub no_rate_limit: bool,

    /// Download rate limit in Mbps (implies --rate-limit)
    #[arg(
        long,
        value_name = "MBPS",
        value_parser = parse_rate_limit_mbps,
        conflicts_with = "no_rate_limit"
    )]
    pub rate_limit_mbps: Option<f64>,
}

#[derive(Args, Debug)]
pub struct HfCachePathArgs {
    /// Model ID, e.g. "Qwen/Qwen2.5-7B-Instruct"
    #[arg(value_name = "MODEL_ID")]
    pub model_id: String,

    /// Git revision to look up: branch, tag, or commit SHA [default: main]
    #[arg(long, value_name = "REV", value_parser = parse_revision)]
    pub revision: Option<String>,

    /// HuggingFace hub cache directory [default: $HF_HUB_CACHE, then
    /// $HF_HOME/hub, then the platform default]
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<String>,

    /// HuggingFace token for the online revision fallback [default:
    /// $HF_TOKEN, then config]
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
}

/// Validate a `--for` preset name (§2.3): only `vllm` exists today; the
/// error names the valid choice so clap surfaces it in usage output.
fn parse_preset(s: &str) -> Result<String, String> {
    match s {
        "vllm" => Ok(s.to_string()),
        other => Err(format!(
            "unknown preset {other:?} — available presets: vllm"
        )),
    }
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

/// Parse a positive, finite `--rate-limit-mbps` value. Zero is rejected
/// because it would stall the transfer entirely.
fn parse_rate_limit_mbps(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .parse()
        .map_err(|_| format!("invalid MBPS value {s:?} — expected a positive number"))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(format!(
            "rate limit must be a positive MBPS value (got {v})"
        ));
    }
    Ok(v)
}

/// Apply the rate-limit CLI overrides to loaded config options (issue #26:
/// pipeline use without a config file). Explicit flags win over the config
/// file; `--no-rate-limit` wins over `--rate-limit`; `--rate-limit-mbps`
/// implies enabling.
pub fn apply_rate_limit_overrides(
    options: &mut crate::models::AppOptions,
    rate_limit: bool,
    no_rate_limit: bool,
    rate_limit_mbps: Option<f64>,
) {
    if no_rate_limit {
        options.download_rate_limit_enabled = false;
    } else if rate_limit || rate_limit_mbps.is_some() {
        options.download_rate_limit_enabled = true;
    }
    if let Some(mbps) = rate_limit_mbps {
        options.download_rate_limit_mbps = mbps;
    }
}

/// Validate a `--revision` value (issue #28): branch names, tags, and
/// commit SHAs. Slash-separated branch names (`release/1.0`) are allowed;
/// empty values, `..`, and leading/trailing slashes are not (they would
/// corrupt the resolve/tree URL paths).
fn parse_revision(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("revision must not be empty".to_string());
    }
    if s == ".." || s.starts_with('/') || s.ends_with('/') || s.contains("..") {
        return Err(format!(
            "invalid revision {s:?} — not a branch, tag, or commit SHA"
        ));
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!(
            "invalid revision {s:?} — control characters are not allowed"
        ));
    }
    Ok(s.to_string())
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
            // `--quant mmproj` selects every multimodal-projector group
            // (MMPROJ, MMPROJ-Q8_0, …) in one go (issue #25)
            let wants_mmproj = quant.eq_ignore_ascii_case(crate::api::MMPROJ_QUANT_TYPE);
            for group in quants {
                let matches = group.quant_type.eq_ignore_ascii_case(quant)
                    || (wants_mmproj
                        && group.quant_type.starts_with(crate::api::MMPROJ_QUANT_TYPE));
                if matches {
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

/// Aggregate run progress for multi-file runs (engine downloads serially,
/// so the active file's speed is the aggregate speed). Omitted on
/// single-file runs and in JSON when absent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OverallProgress {
    /// Files fully processed (downloaded + skipped + failed).
    pub files_done: usize,
    pub files_total: usize,
    /// Bytes of finished files + the active file's partial bytes.
    pub downloaded_bytes: u64,
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
        /// Present when the run covers multiple files.
        #[serde(skip_serializing_if = "Option::is_none")]
        overall: Option<OverallProgress>,
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
    /// `hf-cache sync`: the plan against the current cache (§2.4) — files
    /// to fetch, how many were already up to date, and the fetch total.
    SyncPlanned {
        model: String,
        sha: String,
        files: Vec<FileDto>,
        skipped: usize,
        total_bytes: u64,
    },
    /// `hf-cache sync`: one staged file passed the publish gate and landed
    /// in the cache (snapshot entry + blob).
    FilePublished {
        path: String,
        blob: String,
    },
    /// `hf-cache sync`: terminal success event; in human mode the snapshot
    /// path is printed as the last output line (hf CLI parity, §2.4).
    SyncComplete {
        snapshot_path: String,
        revision: String,
        sha: String,
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
                overall,
                ..
            } => {
                if self.progress_to_tty {
                    // Percent is recomputed from the raw byte counts: the
                    // event's `percent` field is pre-rounded to 0.1%, so
                    // rounding it again would double-round.
                    let file_pct = if *total_bytes > 0 {
                        (*downloaded_bytes as f64 / *total_bytes as f64) * 100.0
                    } else {
                        0.0
                    };
                    let mut stderr = std::io::stderr().lock();
                    if let Some(overall) = overall {
                        // Multi-file run: one line, aggregate first; the
                        // active file is demoted to name + percent (its
                        // bar/bytes/eta are redundant with the aggregate).
                        // Clamped: actual bytes can exceed the tree-reported
                        // total (Content-Range vs tree size) — cap at 100%.
                        let overall_pct = if overall.total_bytes > 0 {
                            ((overall.downloaded_bytes as f64 / overall.total_bytes as f64)
                                * 100.0)
                                .min(100.0)
                        } else {
                            0.0
                        };
                        let remaining = overall
                            .total_bytes
                            .saturating_sub(overall.downloaded_bytes);
                        // \x1b[K erases to end of line so shrinking fields
                        // (unit crossings, a vanishing eta) leave no residue.
                        let _ = write!(
                            stderr,
                            "\r[{}/{} files {}% │ {}/{} │ {:.1} MB/s{}] ▸ {} {}%\x1b[K",
                            overall.files_done,
                            overall.files_total,
                            overall_pct.round() as u64,
                            format_size(overall.downloaded_bytes),
                            format_size(overall.total_bytes),
                            speed_mbps,
                            eta_suffix(*speed_mbps, remaining),
                            truncate_path(filename, 42),
                            file_pct.round() as u64,
                        );
                    } else {
                        let bar = render_bar(*downloaded_bytes, *total_bytes);
                        let eta = eta_suffix(
                            *speed_mbps,
                            total_bytes.saturating_sub(*downloaded_bytes),
                        );
                        let _ = write!(
                            stderr,
                            "\r{} {}% {} {}/{} {:.1} MB/s{}\x1b[K",
                            truncate_path(filename, 42),
                            file_pct.round() as u64,
                            bar,
                            format_size(*downloaded_bytes),
                            format_size(*total_bytes),
                            speed_mbps,
                            eta
                        );
                    }
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
                        format_size(*total_bytes),
                        skipped
                    ));
                }
            }
            Event::FilePublished { path, blob } => {
                if !self.quiet {
                    let short_oid = if blob.len() > 12 { &blob[..12] } else { blob };
                    self.line_stderr(&format!(
                        " ✓ published {} → blobs/{}…",
                        truncate_path(path, 52),
                        short_oid
                    ));
                }
            }
            Event::SyncComplete { snapshot_path, .. } => {
                // §2.4: in human mode the snapshot path IS the last line
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

/// Suffix `" eta <t>"` for the given remaining bytes at the given speed
/// (empty while the speed estimate is still warming up).
fn eta_suffix(speed_mbps: f64, remaining: u64) -> String {
    if speed_mbps > 0.01 {
        let secs = remaining as f64 / (speed_mbps * 1_048_576.0);
        format!(" eta {}", format_eta(secs))
    } else {
        String::new()
    }
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
    match command {
        Command::Download(args) => run_download(args).await,
        Command::HfCache(args) => run_hf_cache(args).await,
        Command::Search(args) => run_search(args).await,
        Command::Update(args) => run_update(args).await,
    }
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
    /// Bytes of fully-fetched files (Complete + AlreadyExists outcomes) —
    /// the base for aggregate run progress (see [`OverallProgress`]).
    done_bytes: u64,
    auth_required: bool,
    failures: Vec<String>,
    mismatches: Vec<String>,
    /// Authoritative per-file download outcomes, set from the manager's
    /// join list once it resolves (see [`count_outcomes`]). `run_download`
    /// only needs the counts; the `hf-cache sync` publish gate reads the
    /// per-file detail.
    outcomes: Vec<FileOutcome>,
    /// Every verification result drained from `verify_rx`, in arrival
    /// order — the per-file input of the `hf-cache sync` publish gate.
    verify_outcomes: Vec<VerifyOutcome>,
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
    apply_rate_limit_overrides(
        &mut options,
        args.rate_limit,
        args.no_rate_limit,
        args.rate_limit_mbps,
    );
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
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
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
    let metadata =
        match crate::api::fetch_model_metadata(&args.model_id, &revision, token.as_ref()).await {
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

    // Quantization groups derive (pure) from the recursive tree already
    // fetched with the metadata — no second API round-trip. Issue #25:
    // this now finds GGUFs stored in subdirectories and keeps mmproj
    // files in their own groups.
    let quants = if matches!(selector, Selector::Quant(_)) {
        crate::api::classify_quantizations(&metadata.siblings)
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
    if let Err(message) =
        crate::engine::register_pending(&args.model_id, &revision, &pending, &base)
    {
        reporter.emit(&Event::Error {
            code: "invalid_path".to_string(),
            message,
            available: None,
        });
        return EXIT_FAILURE;
    }

    let (state, download_tx) = EngineState::new();
    // Load the on-disk registry into the engine mirror (parity with the
    // TUI's startup scan) so verification updates find their entries.
    {
        let mut mirror = state.download_registry.lock().await;
        *mirror = crate::registry::load_registry();
    }
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
            revision.clone(),
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

// ---------------------------------------------------------------------------
// Search (query-only: one bounded API call, no engine involvement)
// ---------------------------------------------------------------------------

/// Effective search parameters: explicit flag → config default (the same
/// defaults the TUI's filter toolbar starts with).
fn effective_search_params(
    args: &SearchArgs,
    options: &crate::models::AppOptions,
) -> (
    crate::models::SortField,
    crate::models::SortDirection,
    u64,
    u64,
) {
    (
        args.sort
            .map(Into::into)
            .unwrap_or(options.default_sort_field),
        args.direction
            .map(Into::into)
            .unwrap_or(options.default_sort_direction),
        args.min_downloads.unwrap_or(options.default_min_downloads),
        args.min_likes.unwrap_or(options.default_min_likes),
    )
}

/// Fixed-column human table on stdout; the result count goes to stderr so
/// the table stays pipeable.
fn render_search_table(models: &[ModelDto]) {
    use std::io::Write;
    let id_width = models
        .iter()
        .map(|m| m.id.chars().count())
        .chain(std::iter::once("MODEL ID".len()))
        .max()
        .unwrap()
        .min(48);

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "{:<id_w$}  {:>10}  {:>7}  UPDATED",
        "MODEL ID",
        "DOWNLOADS",
        "LIKES",
        id_w = id_width
    );
    let _ = writeln!(stdout, "{}", "-".repeat(id_width + 31));
    for m in models {
        let updated = m
            .last_modified
            .as_deref()
            .and_then(|s| s.split('T').next())
            .unwrap_or("-");
        let _ = writeln!(
            stdout,
            "{:<id_w$}  {:>10}  {:>7}  {}",
            truncate_path(&m.id, id_width),
            crate::utils::format_number(m.downloads),
            crate::utils::format_number(m.likes),
            updated,
            id_w = id_width
        );
    }
    let _ = stdout.flush();
    eprintln!("{} model(s)", models.len());
}

async fn run_search(args: SearchArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, false);
    let options = crate::config::load_config();
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );

    let (sort, direction, min_downloads, min_likes) = effective_search_params(&args, &options);

    match crate::api::fetch_models_filtered(
        &args.query,
        sort,
        direction,
        min_downloads,
        min_likes,
        args.limit,
        token.as_ref(),
    )
    .await
    {
        Ok(models) => {
            let dtos: Vec<ModelDto> = models.iter().map(ModelDto::from).collect();
            if args.json {
                // Queries emit one JSON document (an array), not NDJSON
                // events — events are for streaming pipelines. On failure the
                // only stdout output is a single error event (see below).
                let mut stdout = std::io::stdout().lock();
                match serde_json::to_string_pretty(&dtos) {
                    Ok(json) => {
                        let _ = writeln!(stdout, "{}", json);
                        let _ = stdout.flush();
                    }
                    Err(e) => {
                        drop(stdout);
                        reporter.emit(&Event::Error {
                            code: "internal".to_string(),
                            message: format!("failed to serialize results: {}", e),
                            available: None,
                        });
                        return EXIT_FAILURE;
                    }
                }
            } else if dtos.is_empty() {
                // A successful query with zero hits is still success (exit 0);
                // scripts distinguish via the empty array / table absence.
                eprintln!("No models found.");
            } else {
                render_search_table(&dtos);
            }
            EXIT_OK
        }
        Err(e) => {
            reporter.emit(&Event::Error {
                code: "network".to_string(),
                message: format!("search failed: {}", e),
                available: None,
            });
            EXIT_FAILURE
        }
    }
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
            _ = tokio::signal::ctrl_c() => break None,
            result = &mut join => {
                // Keep the authoritative outcome list; counting happens
                // after the final channel drain below so streamed outcomes
                // are never double-counted.
                break match result {
                    Ok(outcomes) => Some(Some(outcomes)),
                    // Manager task failed (panicked): keep streamed tallies
                    Err(_) => Some(None),
                };
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

    let outcomes = match interrupted {
        Some(outcomes) => outcomes,
        None => {
            join.abort();
            return true; // interrupted by SIGINT
        }
    };

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

    // Authoritative recount of download counters from the manager's full
    // outcome list (verification counters come from the drained channel —
    // every send happens before the in-flight counter drops to zero).
    if let Some(outcomes) = outcomes {
        count_outcomes(&outcomes, tally);
    }

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
            // Aggregate view for multi-file runs: finished-file bytes
            // (tally.done_bytes) plus the active file's partial bytes. The
            // engine downloads serially, so the active file's speed is the
            // aggregate speed.
            let overall = if count > 1 {
                Some(OverallProgress {
                    files_done: (tally.downloaded + tally.skipped + tally.failed).min(count),
                    files_total: count,
                    downloaded_bytes: tally.done_bytes + progress.downloaded,
                    total_bytes: tally.total_bytes,
                })
            } else {
                None
            };
            reporter.emit(&Event::Progress {
                filename: progress.filename.clone(),
                downloaded_bytes: progress.downloaded,
                total_bytes: progress.total,
                speed_mbps: progress.speed_mbps,
                percent: (percent * 10.0).round() / 10.0,
                overall,
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
            tally.done_bytes += *bytes;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: "downloaded",
                bytes: *bytes,
            });
        }
        FileOutcome::AlreadyExists { filename, bytes } => {
            tally.skipped += 1;
            tally.done_bytes += *bytes;
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

fn count_outcomes(outcomes: &[FileOutcome], tally: &mut RunTally) {
    // The join handle returns the authoritative full list. The monitor loop
    // already counted streamed outcomes in the common case; recount from
    // scratch to stay correct if any events were missed.
    tally.downloaded = 0;
    tally.skipped = 0;
    tally.failed = 0;
    tally.done_bytes = 0;
    tally.auth_required = false;
    tally.failures.clear();
    tally.outcomes = outcomes.to_vec();
    for outcome in outcomes {
        match outcome {
            FileOutcome::Complete { bytes, .. } => {
                tally.downloaded += 1;
                tally.done_bytes += bytes;
            }
            FileOutcome::AlreadyExists { bytes, .. } => {
                tally.skipped += 1;
                tally.done_bytes += bytes;
            }
            FileOutcome::AuthRequired { .. } => tally.auth_required = true,
            FileOutcome::Failed { filename, reason } => {
                tally.failed += 1;
                tally.failures.push(format!("{}: {}", filename, reason));
            }
        }
    }
}

fn apply_verify_outcome(outcome: &VerifyOutcome, reporter: &mut Reporter, tally: &mut RunTally) {
    tally.verify_outcomes.push(outcome.clone());
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

// ---------------------------------------------------------------------------
// `hf-cache` subcommand (plans/hf-cache-sync.md §2, §5.2)
// ---------------------------------------------------------------------------

/// Whole-repo sync hint (§2.2 precedence step 4).
const TIP_USE_FOR_VLLM: &str = "tip: use --for vllm to fetch only what vLLM reads";

/// Dispatch the `hf-cache` subcommand group.
async fn run_hf_cache(args: HfCacheArgs) -> i32 {
    match args.command {
        HfCacheCommand::Sync(args) => run_hf_cache_sync(args).await,
        HfCacheCommand::Path(args) => run_hf_cache_path(args).await,
    }
}

// --- selection (§2.2 precedence; pure, unit-testable) ---------------------

/// How a sync's file selection was derived (§2.2) — `WholeRepo` triggers
/// the `--for vllm` tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Explicit positional `FILE…` — exactly those files.
    Files,
    /// `--include`/`--exclude` globs over the full tree.
    Patterns,
    /// A `--for <PRESET>` allow/ignore table (§2.3).
    Preset,
    /// No selector: the whole repository (hf `download` parity).
    WholeRepo,
}

/// Selection failures (§2.2): all map to [`EXIT_USAGE`] with the full
/// structured file list attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncSelectionError {
    /// A positional `FILE` is not present in the repository tree.
    MissingPositional {
        path: String,
        available: Vec<String>,
    },
    /// `--for` named a preset this binary does not know (unreachable via
    /// clap's `parse_preset`; kept for direct callers).
    UnknownPreset { name: String },
    /// Every mode plus `--exclude` filtering left nothing to sync.
    EmptySelection { available: Vec<String> },
}

impl SyncSelectionError {
    fn code(&self) -> &'static str {
        match self {
            SyncSelectionError::MissingPositional { .. } => "no_files_match",
            SyncSelectionError::UnknownPreset { .. } => "unknown_preset",
            SyncSelectionError::EmptySelection { .. } => "empty_selection",
        }
    }

    fn message(&self) -> String {
        match self {
            SyncSelectionError::MissingPositional { path, .. } => {
                format!("file not present in repository: {path}")
            }
            SyncSelectionError::UnknownPreset { name } => {
                format!("unknown preset {name:?} — available presets: vllm")
            }
            SyncSelectionError::EmptySelection { .. } => {
                "selection matched no files in the repository".to_string()
            }
        }
    }
}

/// Resolve the files a sync targets, per the §2.2 precedence:
///
/// 1. positional `FILE…` → exactly those files (each must exist in the
///    tree; duplicates collapse, order preserved),
/// 2. else `--include`/`--exclude` → Python-fnmatch globs over the full
///    tree (`*` crosses `/`, §7),
/// 3. else `--for vllm` → the preset allow/ignore table from §2.3
///    ([`crate::patterns::VLLM_ALLOW`]/[`crate::patterns::VLLM_IGNORE`]),
/// 4. else the whole repository ([`SelectionMode::WholeRepo`] — the
///    caller prints the `--for vllm` tip).
///
/// `--exclude` applies on top of every mode (§2.2). An empty selection
/// after all filtering is an error carrying the available file list.
pub fn select_sync_files(
    tree: &[&str],
    files: &[String],
    include: &[String],
    exclude: &[String],
    preset: Option<&str>,
) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
    let available = || {
        tree.iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<_>>()
    };
    let (mut selected, mode) = if !files.is_empty() {
        let mut picked: Vec<String> = Vec::with_capacity(files.len());
        for file in files {
            if !tree.contains(&file.as_str()) {
                return Err(SyncSelectionError::MissingPositional {
                    path: file.clone(),
                    available: available(),
                });
            }
            if !picked.contains(file) {
                picked.push(file.clone());
            }
        }
        (picked, SelectionMode::Files)
    } else if !include.is_empty() {
        let picked = crate::patterns::filter_paths(tree, include, exclude)
            .into_iter()
            .map(String::from)
            .collect();
        (picked, SelectionMode::Patterns)
    } else if let Some(preset) = preset {
        match preset {
            "vllm" => {
                let allow: Vec<String> = crate::patterns::VLLM_ALLOW
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                // The preset's own ignore table filters here; a user
                // --exclude is additionally applied by the universal
                // post-filter below (§2.2: exclude applies on top of every
                // mode).
                let ignore: Vec<String> = crate::patterns::VLLM_IGNORE
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                let picked = crate::patterns::filter_paths(tree, &allow, &ignore)
                    .into_iter()
                    .map(String::from)
                    .collect();
                (picked, SelectionMode::Preset)
            }
            other => {
                return Err(SyncSelectionError::UnknownPreset {
                    name: other.to_string(),
                })
            }
        }
    } else {
        (available(), SelectionMode::WholeRepo)
    };

    // §2.2: --exclude applies on top of every mode (filter_paths already
    // applied it for the Patterns/Preset paths; re-applying is idempotent).
    if !exclude.is_empty() {
        selected.retain(|path| {
            !exclude
                .iter()
                .any(|pattern| crate::patterns::fnmatch(pattern, path))
        });
    }

    if selected.is_empty() {
        return Err(SyncSelectionError::EmptySelection {
            available: available(),
        });
    }
    Ok((selected, mode))
}

// --- small pure helpers ----------------------------------------------------

/// The `refs/` name for a revision (R2): branch/tag revisions are written
/// to `refs/<name>`; a raw 40-hex commit-SHA revision gets **no** ref (hub
/// behavior — the snapshot is addressed by SHA alone).
fn ref_name_for_revision(revision: &str) -> Option<&str> {
    let is_commit_sha = revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit());
    if is_commit_sha {
        None
    } else {
        Some(revision)
    }
}

/// Hub-parity truthiness for `HF_HUB_DISABLE_SYMLINKS` (E10):
/// `1`/`on`/`yes`/`true`, case-insensitive, like huggingface_hub's
/// constants module.
fn env_flag_is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "1" | "ON" | "YES" | "TRUE"
    )
}

/// Whether snapshot entries should be symlinks (R4): `--no-symlinks`
/// forces the copy fallback, and `HF_HUB_DISABLE_SYMLINKS` disables them
/// too (E10, hub parity). On Windows symlinks are **off by default** —
/// hub's own degraded-cache default — because relative targets with `/`
/// separators fail to resolve (os error 123) and creation needs developer
/// mode; snapshots get real files instead.
fn symlinks_enabled(no_symlinks_flag: bool, env_value: Option<&str>) -> bool {
    if no_symlinks_flag || env_value.is_some_and(env_flag_is_true) {
        return false;
    }
    // huggingface_hub's own default on Windows is the degraded no-symlink
    // cache: relative symlink targets with `/` separators resolve as
    // ERROR_INVALID_NAME on Windows, and creation needs developer mode.
    // Mirror that default (snapshots get real files); Unix keeps symlinks.
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Absolute form of `path` without canonicalization's symlink resolution:
/// already-absolute paths pass through verbatim, relative paths anchor at
/// the current directory. §2.4's "last line: the snapshot path" wants a
/// stable, predictable absolute path (containers mount caches elsewhere).
fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Filename carried by a download outcome (`AuthRequired` is repo-wide
/// and carries none).
fn outcome_filename(outcome: &FileOutcome) -> Option<&str> {
    match outcome {
        FileOutcome::Complete { filename, .. }
        | FileOutcome::AlreadyExists { filename, .. }
        | FileOutcome::Failed { filename, .. } => Some(filename),
        FileOutcome::AuthRequired { .. } => None,
    }
}

/// Filename every verification-result variant carries.
fn verify_outcome_filename(outcome: &VerifyOutcome) -> &str {
    match outcome {
        VerifyOutcome::Ok { filename }
        | VerifyOutcome::Mismatch { filename, .. }
        | VerifyOutcome::Error { filename, .. }
        | VerifyOutcome::Missing { filename } => filename,
    }
}

/// FileDto listing of a repo tree (selection-error `available` payloads,
/// mirroring `resolve_files`' ambiguity lists).
fn tree_file_dtos(metadata: &ModelMetadata) -> Vec<FileDto> {
    metadata
        .siblings
        .iter()
        .filter(|f| !f.rfilename.ends_with('/'))
        .map(|f| FileDto {
            filename: f.rfilename.clone(),
            size_bytes: f.size.unwrap_or(0),
            sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
        })
        .collect()
}

/// R6/E9 relink: ensure snapshot entries exist for already-cached files
/// (blobs present per the plan; their entries may be missing or dangling
/// after an interrupted publish). Returns the first failure, if any.
fn relink_up_to_date(
    repo_dir: &Path,
    sha: &str,
    tree: &[crate::models::RepoFile],
    up_to_date: &[String],
    use_symlinks: bool,
) -> Result<(), String> {
    for path in up_to_date {
        let Some(file) = tree.iter().find(|f| f.rfilename == *path) else {
            continue; // plan() only reports tree paths; skip defensively
        };
        if let Err(e) = crate::hf_cache::ensure_snapshot_entry(repo_dir, sha, file, use_symlinks) {
            return Err(format!("cannot link snapshot entry for {path}: {e}"));
        }
    }
    Ok(())
}

/// Human dry-run table (§5.2 step 4): per-file fetch/cached rows with
/// sizes (hf CLI parity). `--json` mode prints only the `SyncPlanned`
/// event, so this is human-only output.
fn print_sync_dry_run(
    model: &str,
    revision: &str,
    sha: &str,
    plan: &crate::hf_cache::SyncPlan,
    tree: &[crate::models::RepoFile],
    cache_dir: &Path,
) {
    let size_of = |path: &str| {
        tree.iter()
            .find(|f| f.rfilename == path)
            .and_then(|f| f.size.or_else(|| f.lfs.as_ref().map(|lfs| lfs.size)))
            .unwrap_or(0)
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "would sync {} (revision {}, commit {}) into {}",
        model,
        revision,
        sha,
        cache_dir.display()
    );
    let mut fetch_bytes = 0u64;
    for item in &plan.fetch {
        fetch_bytes += item.size;
        let _ = writeln!(
            out,
            " {:<6} {:<58} {:>9}",
            "fetch",
            truncate_path(&item.repo_path, 58),
            crate::utils::format_size(item.size)
        );
    }
    for path in &plan.up_to_date {
        let _ = writeln!(
            out,
            " {:<6} {:<58} {:>9}",
            "cached",
            truncate_path(path, 58),
            crate::utils::format_size(size_of(path))
        );
    }
    let _ = writeln!(
        out,
        "{} file(s) to fetch ({}), {} already cached — dry run, nothing written",
        plan.fetch.len(),
        crate::utils::format_size(fetch_bytes),
        plan.up_to_date.len()
    );
    let _ = out.flush();
}

// --- `hf-cache sync` (§5.2 pipeline, followed exactly) ---------------------

async fn run_hf_cache_sync(args: HfCacheSyncArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, args.quiet);

    // --- 1. Configuration (mirrors run_download, minus the output-dir
    //        override: the destination is the hub cache, §4.1) -------------
    let mut options = crate::config::load_config();
    apply_rate_limit_overrides(
        &mut options,
        args.rate_limit,
        args.no_rate_limit,
        args.rate_limit_mbps,
    );
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

    // --- 2. Validate usage (§5.2 step 1: revision already parsed by
    //        clap's parse_revision) ----------------------------------------
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
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());

    // --- 3. Tree + commit SHA in parallel (§5.2 step 2) --------------------
    let (metadata_res, sha_res) = tokio::join!(
        crate::api::fetch_model_metadata(&args.model_id, &revision, token.as_ref()),
        crate::api::resolve_revision_sha(&args.model_id, &revision, token.as_ref()),
    );
    let metadata = match metadata_res {
        Ok(metadata) => metadata,
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::Error {
                code: if not_found { "not_found" } else { "network" }.to_string(),
                message: format!("failed to fetch model info for {}: {}", args.model_id, e),
                available: None,
            });
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
    };
    let sha = match sha_res {
        Ok(sha) => sha,
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::Error {
                code: if not_found {
                    "unknown_revision"
                } else {
                    "network"
                }
                .to_string(),
                message: format!(
                    "failed to resolve revision {} of {}: {}",
                    revision, args.model_id, e
                ),
                available: None,
            });
            // Unknown revision (404) is a usage error (§2.4).
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
    };

    // --- 4. Selection precedence (§2.2) -------------------------------------
    let tree_paths: Vec<&str> = metadata
        .siblings
        .iter()
        .filter(|f| !f.rfilename.ends_with('/'))
        .map(|f| f.rfilename.as_str())
        .collect();
    let (selected, mode) = match select_sync_files(
        &tree_paths,
        &args.files,
        &args.include,
        &args.exclude,
        args.for_preset.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(err) => {
            reporter.emit(&Event::Error {
                code: err.code().to_string(),
                message: err.message(),
                available: Some(tree_file_dtos(&metadata)),
            });
            return EXIT_USAGE;
        }
    };
    if mode == SelectionMode::WholeRepo {
        reporter.status_line(TIP_USE_FOR_VLLM);
    }

    // --- 5. Plan against the current cache (R6) + SyncPlanned (§2.4) --------
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let plan = match crate::hf_cache::plan(
        &cache_dir,
        &args.model_id,
        &metadata.siblings,
        &selected,
        &sha,
        args.force,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            reporter.emit(&Event::Error {
                code: "plan_failed".to_string(),
                message: format!("cannot plan cache sync for {}: {}", args.model_id, e),
                available: None,
            });
            return EXIT_FAILURE;
        }
    };
    let total_bytes: u64 = plan.fetch.iter().map(|item| item.size).sum();
    reporter.emit(&Event::SyncPlanned {
        model: args.model_id.clone(),
        sha: sha.clone(),
        files: plan
            .fetch
            .iter()
            .map(|item| FileDto {
                filename: item.repo_path.clone(),
                size_bytes: item.size,
                sha256: item.sha256.clone(),
            })
            .collect(),
        skipped: plan.up_to_date.len(),
        total_bytes,
    });

    // --- 6. Dry run: the plan is the output; no writes (§5.2 step 4) --------
    if args.dry_run {
        if !args.json {
            print_sync_dry_run(
                &args.model_id,
                &revision,
                &sha,
                &plan,
                &metadata.siblings,
                &cache_dir,
            );
        }
        reporter.finish();
        return EXIT_OK;
    }

    // --- 7. Fully-cached no-op (§5.2 step 3): write refs, relink entries,
    //        print the snapshot path, exit 0.
    let repo_dir = cache_dir.join(crate::hf_cache::repo_dir_name(&args.model_id));
    let staging = crate::hf_cache::staging_dir(&repo_dir);
    let use_symlinks = symlinks_enabled(
        args.no_symlinks,
        std::env::var("HF_HUB_DISABLE_SYMLINKS").ok().as_deref(),
    );
    if plan.fetch.is_empty() {
        let _sync_lock = match acquire_sync_lock_or_fail(&staging, &mut reporter) {
            Ok(guard) => guard,
            Err(code) => return code,
        };
        if let Err(message) = relink_up_to_date(
            &repo_dir,
            &sha,
            &metadata.siblings,
            &plan.up_to_date,
            use_symlinks,
        ) {
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message,
                available: None,
            });
            return EXIT_FAILURE;
        }
        if let Err(e) =
            crate::hf_cache::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message: format!("cannot write refs: {e}"),
                available: None,
            });
            return EXIT_FAILURE;
        }
        reporter.emit(&Event::Done {
            summary: Summary {
                files: selected.len(),
                downloaded: 0,
                skipped: plan.up_to_date.len(),
                verified: 0,
                failed: 0,
                hash_mismatch: 0,
                total_bytes: 0,
            },
        });
        let snapshot_path = absolute_path(&crate::hf_cache::snapshot_dir(
            &cache_dir,
            &args.model_id,
            &sha,
        ));
        // SyncComplete's human rendering is the snapshot path itself —
        // the last line, hf CLI parity (§2.4).
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        reporter.finish();
        return EXIT_OK;
    }

    // --- 8. Layout dirs + CACHEDIR.TAG (§5.2 step 5) -------------------------
    let snapshot_root = crate::hf_cache::snapshot_dir(&cache_dir, &args.model_id, &sha);
    for dir in [repo_dir.join("blobs"), snapshot_root, staging.clone()] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message: format!("cannot create {}: {}", dir.display(), e),
                available: None,
            });
            return EXIT_FAILURE;
        }
    }
    if let Err(e) = crate::paths::write_cachedir_tag(&cache_dir) {
        reporter.emit(&Event::Error {
            code: "io".to_string(),
            message: format!(
                "cannot write CACHEDIR.TAG in {}: {}",
                cache_dir.display(),
                e
            ),
            available: None,
        });
        return EXIT_FAILURE;
    }

    // --- 9. Sync lock (§5.2 step 6) ------------------------------------------
    let _sync_lock = match acquire_sync_lock_or_fail(&staging, &mut reporter) {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    if let Err(message) = relink_up_to_date(
        &repo_dir,
        &sha,
        &metadata.siblings,
        &plan.up_to_date,
        use_symlinks,
    ) {
        reporter.emit(&Event::Error {
            code: "io".to_string(),
            message,
            available: None,
        });
        return EXIT_FAILURE;
    }

    // --- 10. Engine bootstrap, exactly like run_download (§5.2 step 7) -------
    // Note: engine::register_pending is deliberately NOT called — the
    // flat-download registry is TUI-resume state (§4.6). The engine still
    // writes registry entries for files it fetches (staging paths); those
    // are swept after the run and at the start of the next one so the
    // TUI's resume/complete views stay clean.
    purge_staging_registry_entries();
    let (state, download_tx) = EngineState::new();
    // Load the on-disk registry into the engine mirror (parity with the
    // TUI's startup scan) so verification updates find their entries.
    {
        let mut mirror = state.download_registry.lock().await;
        *mirror = crate::registry::load_registry();
    }
    crate::engine::spawn_verification_worker(state.clone());
    let manager = crate::engine::spawn_manager(state.clone());

    let files: Vec<FileSpec> = plan
        .fetch
        .iter()
        .map(|item| FileSpec {
            filename: item.repo_path.clone(),
            size_bytes: item.size,
            sha256: item.sha256.clone(),
        })
        .collect();
    // Queue accounting + sends (mirrors run_download's confirm_download)
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
    for item in &plan.fetch {
        // base_path = staging dir, filename = repo path (§5.1); the
        // revision is the resolved commit SHA, so a moving branch cannot
        // race the plan. Expected sha = LFS oid; total size from the tree.
        let _ = download_tx.send((
            args.model_id.clone(),
            sha.clone(),
            item.repo_path.clone(),
            staging.clone(),
            item.sha256.clone(),
            token.clone(),
            item.size,
        ));
    }
    // Dropping the sender closes the channel — the manager drains, then
    // its join handle resolves. This is the deterministic completion signal.
    drop(download_tx);

    // --- 11. Monitor until drained (§5.2 step 8, reusing run_download's
    //         monitor/verification-idle machinery) ---------------------------
    let mut tally = RunTally {
        files: selected.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, &mut reporter).await;

    // --- 12. Publish gate (§5.2 step 9) --------------------------------------
    let verification_active = crate::download::DOWNLOAD_CONFIG
        .enable_verification
        .load(Ordering::Relaxed);
    let verify_of: HashMap<&str, &VerifyOutcome> = tally
        .verify_outcomes
        .iter()
        .map(|outcome| (verify_outcome_filename(outcome), outcome))
        .collect();
    let mut published: Vec<String> = Vec::new();
    let mut publish_failures: Vec<String> = Vec::new();
    for item in &plan.fetch {
        let Some(outcome) = tally
            .outcomes
            .iter()
            .find(|o| outcome_filename(o) == Some(item.repo_path.as_str()))
        else {
            publish_failures.push(format!("{}: no download outcome", item.repo_path));
            continue;
        };
        // Failed/AuthRequired files never reach the gate; the monitor
        // already surfaced them as Error events.
        if !matches!(
            outcome,
            FileOutcome::Complete { .. } | FileOutcome::AlreadyExists { .. }
        ) {
            continue;
        }
        let staged = staging.join(&item.repo_path);
        // Registry hygiene (§4.6): the engine records every fetch in the
        // flat-download registry with staging paths; sweep those entries
        // so the TUI's resume/complete views stay clean. Done here — after
        // the verification drain — because verification updates land in
        // the registry too.
        purge_staging_registry_entries();
        // Verification gate (R5): publish only with no hub digest,
        // verification deliberately skipped, or an explicit Ok.
        match &item.sha256 {
            None => {}
            Some(_) if args.no_verify => {
                reporter.status_line(&format!(
                    "Warning: publishing {} without SHA256 verification (--no-verify)",
                    item.repo_path
                ));
            }
            Some(_) if !verification_active => {} // standing config choice
            Some(_) => match verify_of.get(item.repo_path.as_str()) {
                Some(VerifyOutcome::Ok { .. }) => {}
                Some(VerifyOutcome::Mismatch { .. }) => {
                    // The bad bytes never enter the cache (§2.4).
                    let _ = std::fs::remove_file(&staged);
                    publish_failures.push(format!(
                        "{}: SHA256 mismatch; staged copy deleted",
                        item.repo_path
                    ));
                    continue;
                }
                Some(VerifyOutcome::Error { reason, .. }) => {
                    publish_failures.push(format!(
                        "{}: verification error: {}",
                        item.repo_path, reason
                    ));
                    continue;
                }
                Some(VerifyOutcome::Missing { .. }) | None => {
                    publish_failures.push(format!(
                        "{}: no verification result before publish",
                        item.repo_path
                    ));
                    continue;
                }
            },
        }
        match crate::hf_cache::publish_one(&repo_dir, &sha, item, &staged, use_symlinks) {
            Ok(blob_oid) => {
                published.push(item.repo_path.clone());
                reporter.emit(&Event::FilePublished {
                    path: item.repo_path.clone(),
                    blob: blob_oid,
                });
            }
            Err(e) => publish_failures.push(format!("{}: {}", item.repo_path, e)),
        }
    }

    // --- 13. refs + staging cleanup (§5.2 step 10) ----------------------------
    let mut failed = tally.failed > 0
        || tally.hash_mismatch > 0
        || tally.auth_required
        || !publish_failures.is_empty();
    if !interrupted && !failed {
        // R2: refs only for branch/tag revisions, never raw SHAs.
        if let Err(e) =
            crate::hf_cache::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            publish_failures.push(format!("refs/{}: {}", revision, e));
            failed = true;
        } else {
            // Full success: drop staging remnants of published files;
            // .incomplete files of failed runs keep their resume value.
            let _ = crate::hf_cache::cleanup_staging(&repo_dir, &published);
        }
    }

    // --- 14. Summary + snapshot path (§5.2 step 11, §2.4) ---------------------
    let summary = Summary {
        files: selected.len(),
        downloaded: tally.downloaded,
        skipped: plan.up_to_date.len() + tally.skipped,
        verified: tally.verified,
        failed: tally.failed,
        hash_mismatch: tally.hash_mismatch,
        total_bytes,
    };
    reporter.emit(&Event::Done { summary });
    reporter.finish();

    let snapshot_path = absolute_path(&crate::hf_cache::snapshot_dir(
        &cache_dir,
        &args.model_id,
        &sha,
    ));
    if interrupted {
        reporter.emit(&Event::Error {
            code: "interrupted".to_string(),
            message: "interrupted by SIGINT; staged partial files resume on the next run"
                .to_string(),
            available: None,
        });
        return EXIT_INTERRUPTED;
    }
    if !failed {
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        // SyncComplete's human rendering IS the last line (§2.4) — the
        // snapshot path, printed even under --quiet since it is the
        // command's scripted output.
        return EXIT_OK;
    }
    if !tally.failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "download_failed".to_string(),
            message: tally.failures.join("; "),
            available: None,
        });
    }
    if !publish_failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "publish_failed".to_string(),
            message: publish_failures.join("; "),
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
        return EXIT_AUTH;
    }
    EXIT_FAILURE
}

/// Acquire the per-repo sync lock (§5.2 step 6), emitting the error event
/// and exit code on failure.
fn acquire_sync_lock_or_fail(
    staging: &Path,
    reporter: &mut Reporter,
) -> Result<crate::hf_cache::SyncLockGuard, i32> {
    crate::hf_cache::acquire_sync_lock(staging).map_err(|e| {
        reporter.emit(&Event::Error {
            code: "sync_lock".to_string(),
            message: format!("cannot acquire sync lock: {e}"),
            available: None,
        });
        EXIT_FAILURE
    })
}

/// Registry hygiene (§4.6): drop every on-disk registry entry whose
/// `local_path` lives in a `.rhd-staging` directory. The engine records
/// hf-cache fetches there (it cannot tell cache syncs from flat
/// downloads); those paths are renamed away or cleaned at publish, so the
/// entries would dangle forever in the TUI's resume/complete views.
/// Best-effort: registry errors are ignored (the sync itself must not
/// fail because housekeeping did).
fn purge_staging_registry_entries() {
    let mut registry = crate::registry::load_registry();
    let before = registry.downloads.len();
    registry
        .downloads
        .retain(|d| !d.local_path.contains(".rhd-staging"));
    if registry.downloads.len() != before {
        crate::registry::save_registry(&registry);
    }
}

// --- `hf-cache path` (§2.1: pure path math + refs lookup) ------------------

async fn run_hf_cache_path(args: HfCachePathArgs) -> i32 {
    if !valid_model_id(&args.model_id) {
        eprintln!(
            "error [usage]: invalid model ID {:?} — expected \"author/model-name\"",
            args.model_id
        );
        return EXIT_USAGE;
    }
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let repo_dir = cache_dir.join(crate::hf_cache::repo_dir_name(&args.model_id));

    // refs/<rev> lookup: pure path math, no network (§2.1).
    if let Ok(sha) = std::fs::read_to_string(repo_dir.join("refs").join(&revision)) {
        let sha = sha.trim();
        if !sha.is_empty() {
            let snapshot = absolute_path(&crate::hf_cache::snapshot_dir(
                &cache_dir,
                &args.model_id,
                sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot directory is missing from the cache; \
                     re-run the sync to repair it"
                );
            }
            println!("{}", snapshot.display());
            return EXIT_OK;
        }
    }

    // Online fallback: resolve the revision to a commit SHA.
    let options = crate::config::load_config();
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );
    match crate::api::resolve_revision_sha(&args.model_id, &revision, token.as_ref()).await {
        Ok(sha) => {
            let snapshot = absolute_path(&crate::hf_cache::snapshot_dir(
                &cache_dir,
                &args.model_id,
                &sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot not present in the cache yet; run \
                     `rust-hf-downloader hf-cache sync {} --revision {}` to populate it",
                    args.model_id, revision
                );
            }
            println!("{}", snapshot.display());
            EXIT_OK
        }
        Err(e) => {
            eprintln!(
                "error [network]: cannot resolve revision {} of {}: {}",
                revision, args.model_id, e
            );
            eprintln!(
                "hint: run `rust-hf-downloader hf-cache sync {}` to populate the cache first",
                args.model_id
            );
            EXIT_FAILURE
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
// `update` subcommand (self-update; see plans/self-update.md)
// ---------------------------------------------------------------------------

/// NDJSON event stream for `update --json` (mirrors `Event`'s style).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UpdateEvent {
    Checking,
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes_url: Option<String>,
    },
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
        percent: f64,
    },
    Verified {
        sha256: String,
    },
    Updated {
        from: String,
        to: String,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Minimum interval between JSON `downloading` events.
const UPDATE_JSON_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

fn update_error_code(err: &crate::update::UpdateError) -> &'static str {
    use crate::update::UpdateError;
    match err {
        UpdateError::Network(_) => "network",
        UpdateError::UnsupportedPlatform => "unsupported_platform",
        UpdateError::NoAssetForPlatform { .. } => "no_asset_for_platform",
        UpdateError::MalformedManifest(_) => "manifest",
        UpdateError::Checksum { .. } => "checksum",
        UpdateError::Archive(_) => "archive",
        UpdateError::Swap(_) => "swap",
        UpdateError::Io(_) => "io",
    }
}

fn update_error_exit(err: &crate::update::UpdateError) -> i32 {
    match err {
        crate::update::UpdateError::Checksum { .. } => EXIT_CHECKSUM,
        _ => EXIT_FAILURE,
    }
}

pub async fn run_update(args: UpdateArgs) -> i32 {
    use crate::update;
    use crate::utils::format_size;

    let human = !args.json && std::io::stderr().is_terminal();
    let mut last_emit = Instant::now();
    let emit = |event: UpdateEvent| {
        if args.json {
            // Same flush discipline as the download reporter.
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", serde_json::to_string(&event).unwrap());
            let _ = out.flush();
        }
    };

    if !args.json {
        eprintln!("Checking for updates…");
    }
    emit(UpdateEvent::Checking);

    let client = reqwest::Client::builder().build();
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            let err = update::UpdateError::Network(format!("building HTTP client: {e}"));
            return update_fail(&err, &emit);
        }
    };
    let base = update::base_url();

    let (manifest, latest) = match update::fetch_manifest(&client, &base).await {
        Ok(v) => v,
        Err(e) => return update_fail(&e, &emit),
    };
    let current = update::VersionTriple::current();
    let triple = match update::target_triple() {
        Some(t) => t,
        None => return update_fail(&update::UpdateError::UnsupportedPlatform, &emit),
    };
    let asset = match update::manifest_asset(&manifest, triple) {
        Ok(a) => a.clone(),
        Err(e) => return update_fail(&e, &emit),
    };

    if latest <= current && !args.force {
        if args.json {
            emit(UpdateEvent::UpToDate {
                current: current.to_string(),
            });
        } else {
            eprintln!("rust-hf-downloader is up to date (v{current})");
        }
        return EXIT_OK;
    }

    if args.json {
        emit(UpdateEvent::Available {
            current: current.to_string(),
            latest: latest.to_string(),
            notes_url: manifest.notes_url.clone(),
        });
    } else {
        eprintln!("Update available: v{current} → v{latest}");
    }
    if args.check {
        if !args.json {
            eprintln!("(check only; nothing was installed — run without --check)");
        }
        return EXIT_UPDATE_AVAILABLE;
    }

    // Download + verify. Progress: humans get a carriage-return line on
    // stderr; JSON gets throttled `downloading` events.
    let mut progress = |downloaded: u64, total: u64| {
        let percent = if total > 0 {
            downloaded as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        if args.json {
            if last_emit.elapsed() >= UPDATE_JSON_PROGRESS_INTERVAL || downloaded == total {
                last_emit = Instant::now();
                emit(UpdateEvent::Downloading {
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    percent,
                });
            }
        } else if human {
            if total > 0 {
                eprint!(
                    "\r  downloading… {} / {} ({percent:.0}%)",
                    format_size(downloaded),
                    format_size(total)
                );
            } else {
                eprint!("\r  downloading… {}", format_size(downloaded));
            }
            let _ = std::io::stderr().flush();
        }
    };
    let archive_path = match update::download_asset(&client, &base, &asset, &mut progress).await {
        Ok(p) => p,
        Err(e) => return update_fail(&e, &emit),
    };
    if !args.json {
        eprintln!("\r{}", " ".repeat(40));
    }
    emit(UpdateEvent::Verified {
        sha256: asset.sha256.clone(),
    });
    if !args.json {
        eprintln!("  checksum ok");
    }

    let binary_path = match update::extract_binary(&archive_path, asset.format) {
        Ok(p) => p,
        Err(e) => return update_fail(&e, &emit),
    };
    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            let err = update::UpdateError::Io(format!("locating current executable: {e}"));
            return update_fail(&err, &emit);
        }
    };
    if let Err(e) = update::swap(&current_exe, &binary_path) {
        return update_fail(&e, &emit);
    }
    // Best-effort temp cleanup; self_replace consumed the staged binary.
    if let Some(parent) = archive_path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }

    if args.json {
        emit(UpdateEvent::Updated {
            from: current.to_string(),
            to: latest.to_string(),
        });
    } else {
        eprintln!("Updated rust-hf-downloader v{current} → v{latest}");
        eprintln!("Restart any running instance to pick up the new version.");
    }
    EXIT_OK
}

fn update_fail(err: &crate::update::UpdateError, emit: &dyn Fn(UpdateEvent)) -> i32 {
    eprintln!("update failed: {err}");
    emit(UpdateEvent::Error {
        code: update_error_code(err).to_string(),
        message: err.to_string(),
    });
    update_error_exit(err)
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
                    oid: None,
                    lfs: None,
                })
                .collect(),
            tags: Vec::new(),
            sha: None,
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
    fn resolve_quant_mmproj_selects_all_projector_groups() {
        // Issue #25: `--quant mmproj` spans every MMPROJ* group; exact names
        // (MMPROJ-Q8_0) still match directly and never pull weight files in.
        let metadata = metadata_with(&[
            ("model.Q8_0.gguf", Some(10)),
            ("model.mmproj-Q8_0.gguf", Some(2)),
            ("mmproj-F32.gguf", Some(3)),
        ]);
        let quants = crate::api::classify_quantizations(&metadata.siblings);

        let files =
            resolve_files(&metadata, &quants, &Selector::Quant("mmproj".to_string())).unwrap();
        assert_eq!(
            files,
            vec![
                file_spec("mmproj-F32.gguf", 3),
                file_spec("model.mmproj-Q8_0.gguf", 2)
            ]
        );

        let files = resolve_files(
            &metadata,
            &quants,
            &Selector::Quant("MMPROJ-Q8_0".to_string()),
        )
        .unwrap();
        assert_eq!(files, vec![file_spec("model.mmproj-Q8_0.gguf", 2)]);
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
            rate_limit: false,
            no_rate_limit: false,
            rate_limit_mbps: None,
            revision: None,
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

    // --- Rate-limit overrides (issue #26) ---------------------------------

    #[test]
    fn rate_limit_overrides() {
        let mut options = crate::models::AppOptions::default();
        assert!(!options.download_rate_limit_enabled); // default: off
        assert_eq!(options.download_rate_limit_mbps, 50.0);

        // --rate-limit-mbps implies enable and sets the rate
        apply_rate_limit_overrides(&mut options, false, false, Some(12.5));
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 12.5);

        // --no-rate-limit disables again (rate stays but is unused)
        apply_rate_limit_overrides(&mut options, false, true, None);
        assert!(!options.download_rate_limit_enabled);

        // config-enabled survives when no flags are passed
        options.download_rate_limit_enabled = true;
        options.download_rate_limit_mbps = 42.0;
        apply_rate_limit_overrides(&mut options, false, false, None);
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 42.0);

        // --rate-limit alone enables with the configured rate
        options.download_rate_limit_enabled = false;
        apply_rate_limit_overrides(&mut options, true, false, None);
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 42.0);
    }

    #[test]
    fn rate_limit_mbps_parser_rejects_bad_values() {
        assert_eq!(parse_rate_limit_mbps("10").ok(), Some(10.0));
        assert_eq!(parse_rate_limit_mbps("10.5").ok(), Some(10.5));
        assert!(parse_rate_limit_mbps("0").is_err());
        assert!(parse_rate_limit_mbps("-3").is_err());
        assert!(parse_rate_limit_mbps("abc").is_err());
        assert!(parse_rate_limit_mbps("inf").is_err());
        assert!(parse_rate_limit_mbps("NaN").is_err());
    }

    #[test]
    fn rate_limit_flags_parse() {
        let args = Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit-mbps",
            "7.25",
        ])
        .unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.rate_limit_mbps, Some(7.25));
        assert!(!args.rate_limit);
        assert!(!args.no_rate_limit);

        // --rate-limit and --no-rate-limit conflict
        assert!(Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit",
            "--no-rate-limit",
        ])
        .is_err());

        // --rate-limit-mbps and --no-rate-limit conflict
        assert!(Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit-mbps",
            "5",
            "--no-rate-limit",
        ])
        .is_err());
    }

    // --- --revision (issue #28) -------------------------------------------

    #[test]
    fn revision_parser_accepts_branches_tags_and_shas() {
        assert_eq!(parse_revision("main").ok(), Some("main".to_string()));
        assert_eq!(parse_revision("2.0bpw").ok(), Some("2.0bpw".to_string()));
        // slash-separated branch names are legal
        assert_eq!(
            parse_revision("release/1.0").ok(),
            Some("release/1.0".to_string())
        );
        assert_eq!(
            parse_revision("0123456789abcdef").ok(),
            Some("0123456789abcdef".to_string())
        );

        // rejections: empty, traversal, slashes at the edges, whitespace
        assert!(parse_revision("").is_err());
        assert!(parse_revision("..").is_err());
        assert!(parse_revision("a/../b").is_err());
        assert!(parse_revision("/main").is_err());
        assert!(parse_revision("main/").is_err());
        assert!(parse_revision("two words").is_err());
        assert!(parse_revision("ta\tb").is_err());
    }

    #[test]
    fn revision_flag_parses_into_args() {
        let args =
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "2.0bpw"])
                .unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.revision.as_deref(), Some("2.0bpw"));

        // absent → None (engine defaults to main)
        let args = Cli::try_parse_from(["hf-downloader", "download", "a/b"]).unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.revision, None);

        // traversal is rejected at the parser level
        assert!(
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "..",]).is_err()
        );
    }

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

    // --- search --------------------------------------------------------------

    #[test]
    fn search_sort_flags_map_to_shared_enums() {
        // The v1 CLI died of drift: --sort was accepted and ignored. These
        // mappings are the boundary — if the flag stops reaching the shared
        // enum, this test fails.
        for (flag, expected) in [
            ("downloads", crate::models::SortField::Downloads),
            ("likes", crate::models::SortField::Likes),
            ("modified", crate::models::SortField::Modified),
            ("name", crate::models::SortField::Name),
        ] {
            let cli = Cli::try_parse_from(["hfd", "search", "q", "--sort", flag]).unwrap();
            match cli.command {
                Some(Command::Search(args)) => {
                    let mapped: crate::models::SortField = args.sort.unwrap().into();
                    assert_eq!(mapped, expected, "--sort {}", flag);
                }
                other => panic!("expected search, got {:?}", other),
            }
        }
    }

    #[test]
    fn search_direction_accepts_asc_desc_aliases() {
        for (flag, expected) in [
            ("asc", crate::models::SortDirection::Ascending),
            ("ascending", crate::models::SortDirection::Ascending),
            ("desc", crate::models::SortDirection::Descending),
            ("descending", crate::models::SortDirection::Descending),
        ] {
            let cli = Cli::try_parse_from(["hfd", "search", "q", "--direction", flag]).unwrap();
            match cli.command {
                Some(Command::Search(args)) => {
                    let mapped: crate::models::SortDirection = args.direction.unwrap().into();
                    assert_eq!(mapped, expected, "--direction {}", flag);
                }
                other => panic!("expected search, got {:?}", other),
            }
        }
    }

    #[test]
    fn search_rejects_invalid_enum_values_and_limit_range() {
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--sort", "bogus"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--direction", "up"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "501"]).is_err());
        // boundaries pass
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "1"]).is_ok());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "500"]).is_ok());
    }

    #[test]
    fn search_params_flag_beats_config_default() {
        let options = crate::models::AppOptions {
            default_sort_field: crate::models::SortField::Name,
            default_sort_direction: crate::models::SortDirection::Ascending,
            default_min_downloads: 100,
            default_min_likes: 10,
            ..crate::models::AppOptions::default()
        };

        // no flags → config defaults
        let args = Cli::try_parse_from(["hfd", "search", "q"]).unwrap();
        let Command::Search(args) = args.command.unwrap() else {
            panic!()
        };
        assert_eq!(
            effective_search_params(&args, &options),
            (
                crate::models::SortField::Name,
                crate::models::SortDirection::Ascending,
                100,
                10
            )
        );

        // explicit flags override
        let args = Cli::try_parse_from([
            "hfd",
            "search",
            "q",
            "--sort",
            "likes",
            "--direction",
            "desc",
            "--min-downloads",
            "5",
            "--min-likes",
            "0",
        ])
        .unwrap();
        let Command::Search(args) = args.command.unwrap() else {
            panic!()
        };
        assert_eq!(
            effective_search_params(&args, &options),
            (
                crate::models::SortField::Likes,
                crate::models::SortDirection::Descending,
                5,
                0
            )
        );
    }

    #[test]
    fn search_dto_preserves_fields() {
        let info = crate::models::ModelInfo {
            id: "a/b".to_string(),
            author: Some("a".to_string()),
            downloads: 1500,
            likes: 10,
            tags: vec!["gguf".to_string()],
            last_modified: Some("2026-08-01T12:00:00Z".to_string()),
        };
        let dto = ModelDto::from(&info);
        assert_eq!(dto.id, "a/b");
        assert_eq!(dto.author.as_deref(), Some("a"));
        assert_eq!(dto.downloads, 1500);
        assert_eq!(dto.likes, 10);
        assert_eq!(dto.tags, vec!["gguf".to_string()]);
        assert_eq!(dto.last_modified.as_deref(), Some("2026-08-01T12:00:00Z"));
    }

    #[test]
    fn snapshot_search_json_array() {
        let dtos = vec![ModelDto {
            id: "bartowski/Qwen2.5-7B-GGUF".to_string(),
            author: Some("bartowski".to_string()),
            downloads: 123_456,
            likes: 1_200,
            last_modified: Some("2026-08-14T10:00:00Z".to_string()),
            tags: vec!["gguf".to_string(), "text-generation".to_string()],
        }];
        insta::assert_snapshot!(
            "search-result-array",
            serde_json::to_string_pretty(&dtos).unwrap()
        );
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
                overall: None,
            },
            "event-progress",
        );
    }

    #[test]
    fn snapshot_event_progress_overall() {
        snap(
            &Event::Progress {
                filename: "model-00003-of-00017.safetensors".to_string(),
                downloaded_bytes: 1_048_576,
                total_bytes: 4_947_802_324,
                speed_mbps: 62.4,
                percent: 0.021_183,
                overall: Some(OverallProgress {
                    files_done: 2,
                    files_total: 17,
                    downloaded_bytes: 10_485_760,
                    total_bytes: 84_102_439_308,
                }),
            },
            "event-progress-overall",
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

    // --- hf-cache sync -------------------------------------------------------

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// Representative repo tree exercising every §2.3 preset category.
    fn sync_tree() -> Vec<&'static str> {
        vec![
            "config.json",
            "model.safetensors",
            "model.safetensors.index.json",
            "pytorch_model.bin",
            "README.md",
            "original/consolidated.safetensors",
            "tokenizer.model",
        ]
    }

    fn run_select(
        tree: &[&str],
        files: &[&str],
        include: &[&str],
        exclude: &[&str],
        preset: Option<&str>,
    ) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
        select_sync_files(
            tree,
            &strings(files),
            &strings(include),
            &strings(exclude),
            preset,
        )
    }

    #[test]
    fn sync_selection_positional_files_win_over_every_other_selector() {
        // §2.2 precedence: positional FILE… beats --include/--exclude and
        // the preset, even when all are given at once.
        let (selected, mode) = run_select(
            &sync_tree(),
            &["config.json", "README.md"],
            &["*.safetensors"],
            &[],
            Some("vllm"),
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json", "README.md"]));
        assert_eq!(mode, SelectionMode::Files);
    }

    #[test]
    fn sync_selection_positional_missing_and_dedup() {
        let tree = sync_tree();
        let err = run_select(&tree, &["nope.json"], &[], &[], None).unwrap_err();
        match err {
            SyncSelectionError::MissingPositional { path, available } => {
                assert_eq!(path, "nope.json");
                assert_eq!(available, strings(&tree));
            }
            other => panic!("expected MissingPositional, got {:?}", other),
        }
        // Repeated positional files collapse (order preserved).
        let (selected, _) = run_select(
            &tree,
            &["config.json", "config.json", "README.md"],
            &[],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json", "README.md"]));
    }

    #[test]
    fn sync_selection_include_exclude_glob_mode() {
        let tree = sync_tree();
        let (selected, mode) = run_select(
            &tree,
            &[],
            &["*.json"],
            &["model.safetensors.index.json"],
            None,
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json"]));
        assert_eq!(mode, SelectionMode::Patterns);

        // Exclude alone (no include, no preset) filters the whole repo —
        // mode stays WholeRepo with the universal exclude on top (§2.2).
        let (selected, mode) =
            run_select(&tree, &[], &[], &["*.bin", "*.md", "original/**"], None).unwrap();
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
                "tokenizer.model",
            ])
        );
        assert_eq!(mode, SelectionMode::WholeRepo);
    }

    #[test]
    fn sync_selection_vllm_preset_applies_allow_and_ignore_tables() {
        // §2.3 via patterns::VLLM_ALLOW/VLLM_IGNORE: safetensors (incl.
        // subfolders, `*` crosses `/`), config/tokenizer files in; fallback
        // weight formats, docs, and original/ out.
        let (selected, mode) = run_select(&sync_tree(), &[], &[], &[], Some("vllm")).unwrap();
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
                "tokenizer.model",
            ])
        );
        assert_eq!(mode, SelectionMode::Preset);

        // §2.2 precedence: --include beats the preset; --exclude applies
        // ON TOP of the preset — the preset selection minus the excluded
        // file, still reported as Preset mode.
        let (selected, mode) =
            run_select(&sync_tree(), &[], &[], &["tokenizer.model"], Some("vllm")).unwrap();
        assert_eq!(mode, SelectionMode::Preset);
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
            ])
        );
    }

    #[test]
    fn sync_selection_whole_repo_is_the_last_resort() {
        let tree = sync_tree();
        let (selected, mode) = run_select(&tree, &[], &[], &[], None).unwrap();
        assert_eq!(selected, strings(&tree));
        assert_eq!(mode, SelectionMode::WholeRepo); // caller prints the tip
    }

    #[test]
    fn sync_selection_empty_after_filtering_is_a_usage_error() {
        let tree = sync_tree();
        // Include matching nothing.
        let err = run_select(&tree, &[], &["*.nonexistent"], &[], None).unwrap_err();
        match &err {
            SyncSelectionError::EmptySelection { available } => {
                assert_eq!(*available, strings(&tree));
            }
            other => panic!("expected EmptySelection, got {:?}", other),
        }
        // --exclude applies on top of positional files (§2.2): dropping the
        // only positional file empties the selection.
        let err = run_select(&tree, &["README.md"], &[], &["*.md"], None).unwrap_err();
        assert_eq!(err.code(), "empty_selection");
        assert_eq!(
            err.message(),
            "selection matched no files in the repository"
        );
        // Unknown preset (unreachable via clap's parse_preset, direct callers).
        let err = run_select(&tree, &[], &[], &[], Some("transformers")).unwrap_err();
        assert_eq!(err.code(), "unknown_preset");
    }

    // --- hf-cache parsing -----------------------------------------------------

    #[test]
    fn parses_hf_cache_sync_positional_files_and_flags() {
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "a/b",
            "config.json",
            "tokenizer.model",
            "--revision",
            "2.0bpw",
            "--include",
            "*.safetensors",
            "--exclude",
            "original/**",
            "--cache-dir",
            "/tmp/hub",
            "--no-symlinks",
            "--force",
            "--dry-run",
            "--no-verify",
            "--json",
            "--quiet",
            "--rate-limit-mbps",
            "7.25",
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Sync(args),
        })) = cli.command
        else {
            panic!("expected hf-cache sync");
        };
        assert_eq!(args.model_id, "a/b");
        assert_eq!(args.files, vec!["config.json", "tokenizer.model"]);
        assert_eq!(args.revision.as_deref(), Some("2.0bpw"));
        assert_eq!(args.include, vec!["*.safetensors"]);
        assert_eq!(args.exclude, vec!["original/**"]);
        assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
        assert!(args.no_symlinks);
        assert!(args.force);
        assert!(args.dry_run);
        assert!(args.no_verify);
        assert!(args.json);
        assert!(args.quiet);
        assert_eq!(args.rate_limit_mbps, Some(7.25));
        assert!(!args.rate_limit);
        assert!(!args.no_rate_limit);
    }

    #[test]
    fn parses_hf_cache_sync_for_vllm_and_sha_revision() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "Qwen/Qwen2.5-7B-Instruct",
            "--for",
            "vllm",
            "--revision",
            sha,
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Sync(args),
        })) = cli.command
        else {
            panic!("expected hf-cache sync");
        };
        assert_eq!(args.model_id, "Qwen/Qwen2.5-7B-Instruct");
        assert!(args.files.is_empty());
        assert_eq!(args.for_preset.as_deref(), Some("vllm"));
        assert_eq!(args.revision.as_deref(), Some(sha));
    }

    #[test]
    fn hf_cache_sync_rejects_unknown_preset_bad_revision_and_flag_conflicts() {
        // --for validates its preset with a clear error.
        let err = Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--for", "transformers"])
            .unwrap_err();
        assert!(err.to_string().contains("available presets: vllm"));
        // parse_revision runs at the parser level.
        assert!(
            Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--revision", ".."]).is_err()
        );
        // Shared rate-limit flags keep their conflicts.
        assert!(Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "a/b",
            "--rate-limit",
            "--no-rate-limit",
        ])
        .is_err());
        // Model id required.
        assert!(Cli::try_parse_from(["rhd", "hf-cache", "sync"]).is_err());
    }

    #[test]
    fn parses_hf_cache_path_flags() {
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "path",
            "a/b",
            "--revision",
            "main",
            "--cache-dir",
            "/tmp/hub",
            "--token",
            "hf_x",
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Path(args),
        })) = cli.command
        else {
            panic!("expected hf-cache path");
        };
        assert_eq!(args.model_id, "a/b");
        assert_eq!(args.revision.as_deref(), Some("main"));
        assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
        assert_eq!(args.token.as_deref(), Some("hf_x"));

        // Absent revision → None (main is applied at run time).
        let cli = Cli::try_parse_from(["rhd", "hf-cache", "path", "a/b"]).unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Path(args),
        })) = cli.command
        else {
            panic!()
        };
        assert_eq!(args.revision, None);
    }

    // --- refs/symlink/path pure helpers ---------------------------------------

    #[test]
    fn refs_are_written_for_branches_tags_but_not_raw_shas() {
        // Branch/tag names get a refs/<name> entry (R2) — including
        // slash-separated branches and hex-looking short names.
        assert_eq!(ref_name_for_revision("main"), Some("main"));
        assert_eq!(ref_name_for_revision("2.0bpw"), Some("2.0bpw"));
        assert_eq!(ref_name_for_revision("release/v2"), Some("release/v2"));
        assert_eq!(
            ref_name_for_revision("0123456789abcdef"),
            Some("0123456789abcdef")
        );
        // 40-hex commit SHAs (either case) get NO refs/ entry.
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(ref_name_for_revision(sha), None);
        let upper = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        assert_eq!(ref_name_for_revision(upper), None);
        // 40 chars but not hex: a (weird) branch name — still gets a ref.
        let not_hex = "z".repeat(40);
        assert_eq!(ref_name_for_revision(&not_hex), Some(not_hex.as_str()));
    }

    #[test]
    fn symlink_policy_honors_flag_and_hub_env_var() {
        // Default: symlinks on Unix, hub's degraded no-symlink cache on
        // Windows (relative `/`-separator targets fail to resolve there).
        #[cfg(unix)]
        assert!(symlinks_enabled(false, None));
        #[cfg(windows)]
        assert!(!symlinks_enabled(false, None));
        // --no-symlinks forces the copy fallback (R4).
        assert!(!symlinks_enabled(true, None));
        // HF_HUB_DISABLE_SYMLINKS disables too (E10, hub parity).
        assert!(!symlinks_enabled(false, Some("1")));
        assert!(!symlinks_enabled(false, Some("true")));
        assert!(!symlinks_enabled(false, Some(" YES ")));
        #[cfg(unix)]
        {
            assert!(symlinks_enabled(false, Some("0")));
            assert!(symlinks_enabled(false, Some("")));
        }
        assert!(!symlinks_enabled(true, Some("1")));
    }

    #[test]
    fn absolute_path_anchors_relative_paths_at_cwd() {
        // Platform-neutral: on Unix a leading-/ path is already absolute and
        // passes through verbatim; on Windows it is drive-relative, so only
        // assert absoluteness and the trailing components (same on both).
        let abs = absolute_path(Path::new("/cache/hub"));
        assert!(abs.is_absolute(), "rooted input must come back absolute");
        assert!(abs.ends_with("cache/hub"), "components preserved: {abs:?}");
        let rel = absolute_path(Path::new("hub/models--a--b"));
        assert!(rel.is_absolute());
        assert!(rel.ends_with("hub/models--a--b"));
    }

    // --- sync JSON events ------------------------------------------------------

    #[test]
    fn sync_events_serialize_with_type_tags() {
        // Smoke: serde tagging mirrors the existing events (§2.4).
        let value = serde_json::to_value(&Event::SyncPlanned {
            model: "org/model".to_string(),
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            files: vec![FileDto {
                filename: "model.safetensors".to_string(),
                size_bytes: 42,
                sha256: Some("a".to_string()),
            }],
            skipped: 2,
            total_bytes: 42,
        })
        .unwrap();
        assert_eq!(value["type"], "sync_planned");
        assert_eq!(value["model"], "org/model");
        assert_eq!(value["skipped"], 2);

        let value = serde_json::to_value(&Event::FilePublished {
            path: "config.json".to_string(),
            blob: "deadbeef".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "file_published");
        assert_eq!(value["path"], "config.json");
        assert_eq!(value["blob"], "deadbeef");

        let value = serde_json::to_value(&Event::SyncComplete {
            snapshot_path: "/x/snapshots/abc".to_string(),
            revision: "main".to_string(),
            sha: "abc".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "sync_complete");
        assert_eq!(value["snapshot_path"], "/x/snapshots/abc");
        assert_eq!(value["revision"], "main");
    }

    #[test]
    fn snapshot_event_sync_planned() {
        snap(
            &Event::SyncPlanned {
                model: "org/model".to_string(),
                sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
                files: vec![FileDto {
                    filename: "model-00001-of-00002.safetensors".to_string(),
                    size_bytes: 4_947_802_324,
                    sha256: Some("a".repeat(64)),
                }],
                skipped: 3,
                total_bytes: 4_947_802_324,
            },
            "event-sync-planned",
        );
    }

    #[test]
    fn snapshot_event_file_published() {
        snap(
            &Event::FilePublished {
                path: "model-00001-of-00002.safetensors".to_string(),
                blob: "a".repeat(64),
            },
            "event-file-published",
        );
    }

    #[test]
    fn snapshot_event_sync_complete() {
        snap(
            &Event::SyncComplete {
                snapshot_path: "/home/u/.cache/huggingface/hub/models--org--model/snapshots/f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
                revision: "main".to_string(),
                sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            },
            "event-sync-complete",
        );
    }
}
