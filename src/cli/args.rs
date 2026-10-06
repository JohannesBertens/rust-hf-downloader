//! Argument structs for every subcommand plus the small parse/merge
//! helpers they wire into clap.

use clap::{Args, Subcommand};
use serde::Serialize;

use super::report::ProgressMode;

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

/// Shared run-output flag block (W4.1): the identical `token` /
/// `no_verify` / `json` / `quiet` / `progress` flags on `download` and
/// `hf-cache sync`, extracted so the two surfaces cannot drift. Flattened
/// at the exact position the flags used to occupy — clap's display-order
/// counter runs across the flatten boundary, so each command's `--help`
/// byte-stays identical (H2 snapshots enforce this).
#[derive(Args, Debug)]
pub struct RunOutputArgs {
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

    /// Progress output mode: auto (tty rewrites), plain (one line every
    /// ~10 s, works without a tty), none
    #[arg(long, value_enum, default_value_t = ProgressMode::Auto, value_name = "MODE")]
    pub progress: ProgressMode,
}

/// Shared rate-limit flag block (W4.1): `rate_limit` / `no_rate_limit` /
/// `rate_limit_mbps` with their conflicts, extracted from `download` and
/// `hf-cache sync`. Same flatten-position rule as [`RunOutputArgs`].
#[derive(Args, Debug)]
pub struct RateLimitArgs {
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

    #[command(flatten)]
    pub run_output: RunOutputArgs,

    #[command(flatten)]
    pub rate_limits: RateLimitArgs,

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

    #[command(flatten)]
    pub run_output: RunOutputArgs,

    #[command(flatten)]
    pub rate_limits: RateLimitArgs,
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
pub(super) fn merge_token(
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
pub(super) fn parse_rate_limit_mbps(s: &str) -> Result<f64, String> {
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
pub(super) fn apply_rate_limit_overrides(
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
pub(super) fn parse_revision(s: &str) -> Result<String, String> {
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
pub(super) fn valid_model_id(model_id: &str) -> bool {
    let parts: Vec<&str> = model_id.split('/').collect();
    parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty()
}
