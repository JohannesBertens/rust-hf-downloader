//! CLI mode implementation for command-line operation
//!
//! This module provides functions for running the application without a TUI,
//! suitable for CI/CD automation and scripting.

use crate::api;
use crate::config;
use crate::models::*;
use crate::registry;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Error type for CLI operations
#[derive(Debug)]
#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
pub enum HeadlessError {
    ApiError(String),
    DownloadError(String),
    ConfigError(String),
    IoError(std::io::Error),
    AuthError(String),
}

impl std::fmt::Display for HeadlessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadlessError::ApiError(msg) => write!(f, "API error: {}", msg),
            HeadlessError::DownloadError(msg) => write!(f, "Download error: {}", msg),
            HeadlessError::ConfigError(msg) => write!(f, "Configuration error: {}", msg),
            HeadlessError::IoError(err) => write!(f, "IO error: {}", err),
            HeadlessError::AuthError(msg) => write!(f, "Authentication error: {}", msg),
        }
    }
}

impl std::error::Error for HeadlessError {}

impl From<reqwest::Error> for HeadlessError {
    fn from(err: reqwest::Error) -> Self {
        HeadlessError::ApiError(err.to_string())
    }
}

impl From<std::io::Error> for HeadlessError {
    fn from(err: std::io::Error) -> Self {
        HeadlessError::IoError(err)
    }
}

/// Type for download messages sent to the download manager
pub type DownloadMessage = (
    String,         // model_id
    String,         // filename
    PathBuf,        // output path
    Option<String>, // sha256
    Option<String>, // hf_token
    u64,            // total_size
);

/// Exit code constants
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_AUTH_ERROR: i32 = 2;
pub const EXIT_INVALID_ARGS: i32 = 3;

impl HeadlessError {
    pub fn exit_code(&self) -> i32 {
        match self {
            HeadlessError::AuthError(_) => EXIT_AUTH_ERROR,
            HeadlessError::ApiError(_)
            | HeadlessError::DownloadError(_)
            | HeadlessError::ConfigError(_)
            | HeadlessError::IoError(_) => EXIT_ERROR,
        }
    }
}

/// Format file size in human-readable format
pub fn format_file_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format duration in human-readable format
#[allow(dead_code)]
pub fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Validate model ID format (author/model-name)
pub fn validate_model_id(model_id: &str) -> Result<(), HeadlessError> {
    if !model_id.contains('/') {
        return Err(HeadlessError::DownloadError(
            "Invalid model ID format. Expected: 'author/model-name'".to_string(),
        ));
    }

    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(HeadlessError::DownloadError(
            "Invalid model ID format. Expected: 'author/model-name'".to_string(),
        ));
    }

    Ok(())
}

/// Search for models with optional filters
pub async fn search_models(
    query: &str,
    sort_field: Option<SortField>,
    sort_direction: Option<SortDirection>,
    min_downloads: Option<u64>,
    min_likes: Option<u64>,
    token: Option<&String>,
) -> Result<Vec<ModelInfo>, HeadlessError> {
    let sort = sort_field.unwrap_or(SortField::Downloads);
    let direction = sort_direction.unwrap_or(SortDirection::Descending);
    let min_dl = min_downloads.unwrap_or(0);
    let min_likes_val = min_likes.unwrap_or(0);

    api::fetch_models_filtered(query, sort, direction, min_dl, min_likes_val, token)
        .await
        .map_err(|e| HeadlessError::ApiError(e.to_string()))
}

/// Run search command with formatted output
pub async fn run_search(
    query: &str,
    sort_field: Option<SortField>,
    min_downloads: Option<u64>,
    min_likes: Option<u64>,
    token: Option<&String>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    let start = std::time::Instant::now();

    let models = search_models(query, sort_field, None, min_downloads, min_likes, token).await?;

    let elapsed = start.elapsed();

    reporter.report_search_with_timing(&models, elapsed);

    Ok(())
}

/// List quantizations and metadata for a model
pub async fn list_quantizations(
    model_id: &str,
    token: Option<&String>,
) -> Result<(Vec<QuantizationGroup>, ModelMetadata), HeadlessError> {
    // Try to fetch GGUF quantizations first
    let quantizations = api::fetch_model_files(model_id, token)
        .await
        .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

    // Always fetch full metadata for file tree
    let metadata = api::fetch_model_metadata(model_id, token)
        .await
        .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

    Ok((quantizations, metadata))
}

/// Download a model with optional quantization filter
pub async fn download_model(
    model_id: &str,
    quantization_filter: Option<&str>,
    download_all: bool,
    output_dir: &str,
    hf_token: Option<String>,
    progress_tx: mpsc::UnboundedSender<String>,
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
) -> Result<(), HeadlessError> {
    let options = config::load_config();
    let token = hf_token.or(options.hf_token);

    // Fetch model metadata
    let metadata = api::fetch_model_metadata(model_id, token.as_ref())
        .await
        .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

    // Check if model has GGUF files
    let has_gguf = api::has_gguf_files(&metadata);

    if has_gguf {
        let quantizations = api::fetch_model_files(model_id, token.as_ref())
            .await
            .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

        // Filter by quantization type if specified
        let files_to_download: Vec<_> = if let Some(q_filter) = quantization_filter {
            quantizations
                .iter()
                .filter(|q| q.quant_type == q_filter)
                .flat_map(|q| q.files.iter())
                .collect()
        } else if download_all {
            quantizations.iter().flat_map(|q| q.files.iter()).collect()
        } else {
            return Err(HeadlessError::DownloadError(
                "Must specify --quantization or --all for GGUF models".to_string(),
            ));
        };

        // Queue downloads
        for quant_file in files_to_download {
            let path = PathBuf::from(output_dir);
            let total_size = quant_file.size;
            download_tx
                .send((
                    model_id.to_string(),
                    quant_file.filename.clone(),
                    path,
                    quant_file.sha256.clone(),
                    token.clone(),
                    total_size,
                ))
                .map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

            let _ = progress_tx.send(format!("Queued: {}", quant_file.filename));
        }
    } else {
        // Non-GGUF model: download all files from metadata
        if !download_all {
            return Err(HeadlessError::DownloadError(
                "Non-GGUF models require --all flag".to_string(),
            ));
        }

        for file in &metadata.siblings {
            let path = PathBuf::from(output_dir);
            let size = file.size.unwrap_or(0);
            let sha256 = file.lfs.as_ref().map(|l| l.oid.clone());

            download_tx
                .send((
                    model_id.to_string(),
                    file.rfilename.clone(),
                    path,
                    sha256,
                    token.clone(),
                    size,
                ))
                .map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

            let _ = progress_tx.send(format!("Queued: {}", file.rfilename));
        }
    }

    Ok(())
}

/// Calculate download summary for GGUF models
fn calculate_gguf_download_summary(
    quantizations: &[QuantizationGroup],
    filter: Option<&str>,
    download_all: bool,
) -> Result<(Vec<String>, u64), HeadlessError> {
    if let Some(q_filter) = filter {
        let group = quantizations
            .iter()
            .find(|q| q.quant_type == q_filter)
            .ok_or_else(|| {
                // Build helpful error message with available quantizations
                let available: Vec<String> = quantizations
                    .iter()
                    .map(|q| {
                        format!(
                            "{} ({} files, {})",
                            q.quant_type,
                            q.files.len(),
                            format_file_size(q.total_size)
                        )
                    })
                    .collect();

                HeadlessError::DownloadError(format!(
                    "Quantization '{}' not found\n\nAvailable quantizations:\n  {}",
                    q_filter,
                    available.join("\n  ")
                ))
            })?;

        let files: Vec<String> = group.files.iter().map(|f| f.filename.clone()).collect();
        let total_size = group.total_size;
        Ok((files, total_size))
    } else if download_all {
        let files: Vec<String> = quantizations
            .iter()
            .flat_map(|q| q.files.iter().map(|f| f.filename.clone()))
            .collect();
        let total_size: u64 = quantizations.iter().map(|q| q.total_size).sum();
        Ok((files, total_size))
    } else {
        // Build list of available quantizations for the error message
        let available: Vec<String> = quantizations
            .iter()
            .map(|q| {
                format!(
                    "{} ({} files, {})",
                    q.quant_type,
                    q.files.len(),
                    format_file_size(q.total_size)
                )
            })
            .collect();

        Err(HeadlessError::DownloadError(format!(
            "Must specify --quantization or --all\n\nAvailable quantizations:\n  {}",
            available.join("\n  ")
        )))
    }
}

/// Calculate download summary for non-GGUF models
fn calculate_non_gguf_download_summary(
    metadata: &ModelMetadata,
    download_all: bool,
) -> Result<(Vec<String>, u64), HeadlessError> {
    if !download_all {
        return Err(HeadlessError::DownloadError(
            "Non-GGUF model requires --all flag".to_string(),
        ));
    }

    let files: Vec<String> = metadata
        .siblings
        .iter()
        .filter_map(|f| f.size.map(|_| f.rfilename.clone()))
        .collect();

    let total_size: u64 = metadata.siblings.iter().filter_map(|f| f.size).sum();

    Ok((files, total_size))
}

/// Run download command in dry-run mode (show what would be downloaded)
pub async fn run_download_dry_run(
    model_id: &str,
    quantization: Option<&str>,
    download_all: bool,
    output_dir: &str,
    hf_token: Option<String>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    // Validate model ID first
    validate_model_id(model_id)?;

    reporter.report_info("Dry run mode - no files will be downloaded\n");

    // Get download summary
    let (quantizations, metadata) = list_quantizations(model_id, hf_token.as_ref()).await?;

    // Check if model is gated and token is provided (even in dry-run)
    check_gated_model(&metadata, &hf_token)?;

    let has_gguf = api::has_gguf_files(&metadata);

    let (files_to_download, total_size) = if has_gguf {
        calculate_gguf_download_summary(&quantizations, quantization, download_all)?
    } else {
        calculate_non_gguf_download_summary(&metadata, download_all)?
    };

    // Report what would be downloaded
    reporter.report_dry_run_summary(&files_to_download, total_size, output_dir, has_gguf);

    Ok(())
}

/// Check if a model is gated and requires authentication
fn check_gated_model(
    metadata: &ModelMetadata,
    hf_token: &Option<String>,
) -> Result<(), HeadlessError> {
    // Check if model is gated
    let is_gated = match &metadata.gated {
        serde_json::Value::String(s) if s == "auto" || s == "manual" => true,
        serde_json::Value::Bool(true) => true,
        _ => false,
    };

    if is_gated {
        // Check if token is provided
        if hf_token.is_none() || hf_token.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
            return Err(HeadlessError::AuthError(format!(
                "Model '{}' is gated and requires authentication.\n\n\
                To download this model:\n\
                1. Get a HuggingFace token from: https://huggingface.co/settings/tokens\n\
                2. Accept the model terms at: https://huggingface.co/{}/\n\
                3. Provide the token via --token flag or config file\n\n\
                Example:\n\
                  rust-hf-downloader --headless download \"{}\" --all --token \"hf_...\"\n\
                  \n\
                Or add to ~/.config/jreb/config.toml:\n\
                  hf_token = \"hf_...\"",
                metadata.model_id, metadata.model_id, metadata.model_id
            )));
        }
    }

    Ok(())
}

/// Run download command with summary and progress tracking
#[allow(clippy::too_many_arguments)]
pub async fn run_download(
    model_id: &str,
    quantization: Option<&str>,
    download_all: bool,
    output_dir: &str,
    hf_token: Option<String>,
    reporter: &ProgressReporter,
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    progress_tx: mpsc::UnboundedSender<String>,
    download_queue: Arc<tokio::sync::Mutex<QueueState>>,
    download_progress: Arc<tokio::sync::Mutex<Option<DownloadProgress>>>,
    verification_queue_size: Arc<AtomicUsize>,
    verification_progress: Arc<tokio::sync::Mutex<Vec<VerificationProgress>>>,
    shutdown_signal: Arc<tokio::sync::Mutex<bool>>,
) -> Result<(), HeadlessError> {
    // Validate model ID first
    validate_model_id(model_id)?;

    // Get download summary
    let (quantizations, metadata) = list_quantizations(model_id, hf_token.as_ref()).await?;

    // Check if model is gated and token is provided
    check_gated_model(&metadata, &hf_token)?;
    let has_gguf = api::has_gguf_files(&metadata);

    let (files_to_download, total_size) = if has_gguf {
        calculate_gguf_download_summary(&quantizations, quantization, download_all)?
    } else {
        calculate_non_gguf_download_summary(&metadata, download_all)?
    };

    // Report what will be downloaded
    reporter.report_download_summary(&files_to_download, total_size);

    // Update queue state before enqueueing downloads
    {
        let mut queue = download_queue.lock().await;
        queue.add(files_to_download.len(), total_size);
    }

    // Queue the actual downloads
    download_model(
        model_id,
        quantization,
        download_all,
        output_dir,
        hf_token,
        progress_tx,
        download_tx,
    )
    .await?;

    // Wait for downloads to complete
    wait_for_downloads(
        download_queue,
        download_progress,
        reporter,
        shutdown_signal.clone(),
    )
    .await?;

    // Wait for verification to complete
    wait_for_verification(
        verification_queue_size,
        verification_progress,
        reporter,
        shutdown_signal,
    )
    .await?;

    Ok(())
}

/// Resume incomplete downloads from registry
pub async fn resume_downloads(
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    progress_tx: mpsc::UnboundedSender<String>,
) -> Result<Vec<DownloadMetadata>, HeadlessError> {
    let registry = registry::load_registry();
    let incomplete: Vec<_> = registry
        .downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::Incomplete)
        .cloned()
        .collect();

    if incomplete.is_empty() {
        let _ = progress_tx.send("No incomplete downloads found".to_string());
        return Ok(Vec::new());
    }

    for download in &incomplete {
        let local_path = PathBuf::from(&download.local_path);
        let filename_path = std::path::Path::new(&download.filename);
        let mut base_path = local_path.clone();
        let strip_count = filename_path.components().count();
        for _ in 0..strip_count {
            if let Some(parent) = base_path.parent() {
                base_path = parent.to_path_buf();
            } else {
                break;
            }
        }
        download_tx
            .send((
                download.model_id.clone(),
                download.filename.clone(),
                base_path,
                download.expected_sha256.clone(),
                None, // Use token from config
                download.total_size,
            ))
            .map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

        let _ = progress_tx.send(format!("Resumed: {}", download.filename));
    }

    Ok(incomplete)
}

/// Wait for all downloads to complete and report progress
pub async fn wait_for_downloads(
    download_queue: Arc<tokio::sync::Mutex<QueueState>>,
    download_progress: Arc<tokio::sync::Mutex<Option<DownloadProgress>>>,
    reporter: &ProgressReporter,
    shutdown_signal: Arc<tokio::sync::Mutex<bool>>,
) -> Result<(), HeadlessError> {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));
    let mut last_progress: Option<DownloadProgress> = None;
    let mut last_report_time = std::time::Instant::now();
    let mut had_active_download = false;

    loop {
        interval.tick().await;

        // Check for shutdown signal
        if *shutdown_signal.lock().await {
            reporter.report_info("\nShutdown requested, downloads will resume on next run");
            return Ok(());
        }

        // Check download progress
        let progress_guard = download_progress.try_lock();
        if let Ok(ref progress_opt) = progress_guard {
            if let Some(progress) = progress_opt.as_ref() {
                had_active_download = true;
                // Only report if progress changed significantly (>1% or new file)
                let should_report = match &last_progress {
                    None => true,
                    Some(last) => {
                        progress.filename != last.filename
                            || (progress.downloaded as f64 - last.downloaded as f64)
                                > progress.total as f64 * 0.01
                    }
                };

                if should_report {
                    // Calculate speed using actual elapsed time since last report
                    let now = std::time::Instant::now();
                    let elapsed_secs = now.duration_since(last_report_time).as_secs_f64();
                    let speed_mbps = if progress.total > 0 && elapsed_secs > 0.0 {
                        let bytes_diff = progress.downloaded.saturating_sub(
                            last_progress.as_ref().map(|l| l.downloaded).unwrap_or(0),
                        );
                        (bytes_diff as f64 / elapsed_secs) / 1_048_576.0
                    } else {
                        0.0
                    };

                    reporter.report_download_progress(
                        &progress.filename,
                        progress.downloaded,
                        progress.total,
                        speed_mbps,
                    );
                    last_progress = Some(progress.clone());
                    last_report_time = now;
                }
            }
        }
        drop(progress_guard);

        // Check if queue is empty and no active downloads
        let queue_size = download_queue.lock().await.size;
        let has_progress = download_progress
            .try_lock()
            .map(|p| p.is_some())
            .unwrap_or(false);

        if queue_size == 0 && !has_progress {
            // Print newline to clear the progress bar line if we had an active download
            if had_active_download && !reporter.is_json() {
                println!();
            }
            break;
        }
    }

    Ok(())
}

/// Wait for all verifications to complete and report progress
pub async fn wait_for_verification(
    verification_queue_size: Arc<AtomicUsize>,
    verification_progress: Arc<tokio::sync::Mutex<Vec<VerificationProgress>>>,
    reporter: &ProgressReporter,
    shutdown_signal: Arc<tokio::sync::Mutex<bool>>,
) -> Result<(), HeadlessError> {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));
    let mut last_progress: Option<VerificationProgress> = None;
    let mut consecutive_idle_checks = 0;
    let mut shown_initial = false;
    let mut seen_verification_activity = false;

    // Give verification worker a moment to pick up items from the queue
    // This helps avoid race conditions where queue is empty before progress starts
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Show initial 0% progress bar if there's work queued
    {
        let queue_size = verification_queue_size.load(Ordering::Relaxed);
        if queue_size > 0 && !reporter.is_json() {
            print!("\r[{}] 0% verifying...", " ".repeat(40));
            let _ = std::io::stdout().flush();
            shown_initial = true;
        }
    }

    // If nothing is queued and no progress is active, exit early
    {
        let queue_size = verification_queue_size.load(Ordering::Relaxed);
        let has_progress = !verification_progress.lock().await.is_empty();
        if queue_size == 0 && !has_progress {
            return Ok(());
        }
    }

    loop {
        interval.tick().await;

        // Check for shutdown signal
        if *shutdown_signal.lock().await {
            return Ok(());
        }

        // Check verification progress
        let progress_guard = verification_progress.try_lock();
        if let Ok(ref progress_vec) = progress_guard {
            if let Some(progress) = progress_vec.first() {
                consecutive_idle_checks = 0; // Reset idle counter when we see activity
                seen_verification_activity = true; // Mark that we've seen verification start

                // Always show progress when we first see it (even at 0 bytes)
                let should_report = last_progress.is_none()
                    || progress.filename
                        != last_progress
                            .as_ref()
                            .map(|p| p.filename.as_str())
                            .unwrap_or("")
                    || (progress.verified_bytes as f64
                        - last_progress
                            .as_ref()
                            .map(|l| l.verified_bytes)
                            .unwrap_or(0) as f64)
                        > progress.total_bytes as f64 * 0.01;

                if should_report {
                    reporter.report_verification_progress(
                        &progress.filename,
                        progress.verified_bytes,
                        progress.total_bytes,
                        progress.speed_mbps,
                    );
                    last_progress = Some(progress.clone());
                    shown_initial = true;
                }
            }
        }
        drop(progress_guard);

        // Check if queue is empty and no active verifications
        let queue_size = verification_queue_size.load(Ordering::Relaxed);
        let has_active = verification_progress
            .try_lock()
            .map(|p| !p.is_empty())
            .unwrap_or(true); // Assume active if we can't get lock

        if queue_size == 0 && !has_active {
            // Only consider complete if we've seen verification activity
            // This prevents exiting prematurely due to race condition where
            // queue is emptied but verification hasn't started yet
            if seen_verification_activity {
                consecutive_idle_checks += 1;
                if consecutive_idle_checks >= 3 {
                    // Print newline to clear progress line
                    if shown_initial && !reporter.is_json() {
                        println!();
                    }
                    break;
                }
            }
            // If we haven't seen activity yet, keep waiting (don't increment counter)
        } else {
            consecutive_idle_checks = 0;
        }
    }

    Ok(())
}

/// Run list command with formatted output
pub async fn run_list(
    model_id: &str,
    token: Option<&String>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    // Validate model ID first
    validate_model_id(model_id)?;

    let (quantizations, metadata) = list_quantizations(model_id, token).await?;

    let has_gguf = api::has_gguf_files(&metadata);

    if reporter.is_json() {
        reporter.report_list_json(&quantizations, &metadata, has_gguf);
    } else if has_gguf {
        reporter.report_quantizations_table(&quantizations);
    } else {
        reporter.report_file_tree(&metadata);
    }

    Ok(())
}

/// Run resume command with formatted output
#[allow(clippy::too_many_arguments)]
pub async fn run_resume(
    reporter: &ProgressReporter,
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    progress_tx: mpsc::UnboundedSender<String>,
    download_queue: Arc<tokio::sync::Mutex<QueueState>>,
    download_progress: Arc<tokio::sync::Mutex<Option<DownloadProgress>>>,
    verification_queue_size: Arc<AtomicUsize>,
    verification_progress: Arc<tokio::sync::Mutex<Vec<VerificationProgress>>>,
    shutdown_signal: Arc<tokio::sync::Mutex<bool>>,
) -> Result<(), HeadlessError> {
    let incomplete = resume_downloads(download_tx, progress_tx).await?;

    if incomplete.is_empty() {
        reporter.report_no_incomplete();
        return Ok(());
    }

    reporter.report_resume_summary(&incomplete);

    // Update queue state before downloads begin
    {
        let total_size: u64 = incomplete.iter().map(|d| d.total_size).sum();
        let mut queue = download_queue.lock().await;
        queue.add(incomplete.len(), total_size);
    }

    // Wait for downloads to complete
    wait_for_downloads(
        download_queue,
        download_progress,
        reporter,
        shutdown_signal.clone(),
    )
    .await?;

    // Wait for verification to complete
    wait_for_verification(
        verification_queue_size,
        verification_progress,
        reporter,
        shutdown_signal,
    )
    .await?;

    Ok(())
}

/// Progress reporter for console output (text and JSON modes)
pub struct ProgressReporter {
    json_mode: bool,
}

impl ProgressReporter {
    pub fn new(json_mode: bool) -> Self {
        Self { json_mode }
    }

    #[allow(dead_code)]
    pub fn report_search(&self, models: &[ModelInfo]) {
        if self.json_mode {
            println!("{}", serde_json::to_string(models).unwrap());
        } else {
            println!("Found {} models:", models.len());
            for model in models {
                println!(
                    "  - {} ({} downloads, {} likes)",
                    model.id, model.downloads, model.likes
                );
            }
        }
    }

    pub fn report_search_with_timing(&self, models: &[ModelInfo], elapsed: std::time::Duration) {
        if self.json_mode {
            let json = serde_json::json!({
                "count": models.len(),
                "query_time_seconds": elapsed.as_secs_f64(),
                "results": models
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            println!(
                "Found {} models in {:.2}s:",
                models.len(),
                elapsed.as_secs_f64()
            );
            println!();

            if models.is_empty() {
                println!("No models found matching your criteria.");
                return;
            }

            // Calculate column widths
            let max_id_width = models
                .iter()
                .map(|m| m.id.len())
                .max()
                .unwrap_or(40)
                .min(60);

            // Print header
            println!(
                "{:<width$} | {:>12} | {:>10} | Last Modified",
                "Model",
                "Downloads",
                "Likes",
                width = max_id_width
            );
            println!(
                "{:-<width$}-+-{:-<12}-+-{:-<10}-+---------------",
                "----",
                "------------",
                "----------",
                width = max_id_width
            );

            // Print each model
            for model in models {
                let last_mod = model.last_modified.as_deref().unwrap_or("N/A");
                println!(
                    "{:<width$} | {:>12} | {:>10} | {}",
                    model.id,
                    model.downloads,
                    model.likes,
                    last_mod,
                    width = max_id_width
                );
            }
        }
    }

    #[allow(dead_code)]
    pub fn report_download_start(&self, filename: &str, total_size: u64) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "starting",
                "filename": filename,
                "size": total_size
            });
            println!("{}", json);
        } else {
            println!("Downloading: {} ({} MB)", filename, total_size / 1_048_576);
        }
    }

    pub fn report_download_progress(
        &self,
        filename: &str,
        downloaded: u64,
        total: u64,
        speed_mbps: f64,
    ) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "downloading",
                "filename": filename,
                "progress": (downloaded as f64 / total as f64 * 100.0),
                "speed_mbps": speed_mbps
            });
            println!("{}", json);
        } else {
            let percent = (downloaded as f64 / total as f64 * 100.0) as u32;
            let bar_width = 40;
            let filled = (percent as f32 / 100.0 * bar_width as f32) as usize;
            let bar: String = "=".repeat(filled) + &" ".repeat(bar_width - filled);
            print!(
                "\r[{}] {}% ({:.2} MB/s) - {}",
                bar, percent, speed_mbps, filename
            );
            let _ = std::io::stdout().flush();
        }
    }

    #[allow(dead_code)]
    pub fn report_download_complete(&self, filename: &str) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "complete",
                "filename": filename
            });
            println!("{}", json);
        } else {
            println!("\n✓ Complete: {}", filename);
        }
    }

    pub fn report_verification_progress(
        &self,
        filename: &str,
        verified: u64,
        total: u64,
        speed_mbps: f64,
    ) {
        if self.json_mode {
            let eta_seconds = if speed_mbps > 0.0 && total > verified {
                Some((total - verified) as f64 / (speed_mbps * 1_048_576.0))
            } else {
                None
            };
            let json = serde_json::json!({
                "status": "verifying",
                "filename": filename,
                "progress": (verified as f64 / total as f64 * 100.0),
                "speed_mbps": speed_mbps,
                "eta_seconds": eta_seconds,
            });
            println!("{}", json);
        } else {
            let percent = if total > 0 {
                (verified as f64 / total as f64 * 100.0) as u32
            } else {
                0
            };
            let bar_width = 40;
            let filled = (percent as f32 / 100.0 * bar_width as f32) as usize;
            let bar: String = "=".repeat(filled) + &" ".repeat(bar_width - filled);

            // Calculate ETA
            let eta_str = if speed_mbps > 0.0 && total > verified {
                let remaining_bytes = total - verified;
                let eta_secs = (remaining_bytes as f64 / (speed_mbps * 1_048_576.0)) as u64;
                if eta_secs >= 3600 {
                    format!(" ETA {}h {}m", eta_secs / 3600, (eta_secs % 3600) / 60)
                } else if eta_secs >= 60 {
                    format!(" ETA {}m {}s", eta_secs / 60, eta_secs % 60)
                } else {
                    format!(" ETA {}s", eta_secs)
                }
            } else {
                String::new()
            };

            print!(
                "\r[{}] {}% ({:.2} MB/s){} verifying - {}",
                bar, percent, speed_mbps, eta_str, filename
            );
            let _ = std::io::stdout().flush();
        }
    }

    pub fn report_error(&self, error: &str) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "error",
                "error": error
            });
            eprintln!("{}", json);
        } else {
            eprintln!("Error: {}", error);
        }
    }

    pub fn report_info(&self, message: &str) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "info",
                "message": message
            });
            println!("{}", json);
        } else {
            println!("{}", message);
        }
    }

    #[allow(dead_code)]
    pub fn report_list_quantizations(
        &self,
        quantizations: &[QuantizationGroup],
        metadata: &ModelMetadata,
    ) {
        if self.json_mode {
            // Simplified JSON output without full serialization
            println!("{{");
            println!("  \"model_id\": \"{}\",", metadata.model_id);
            println!("  \"quantizations\": [");
            for (i, quant) in quantizations.iter().enumerate() {
                if i > 0 {
                    println!(",");
                }
                println!("    {{");
                println!("      \"quant_type\": \"{}\",", quant.quant_type);
                println!("      \"total_size\": {},", quant.total_size);
                println!("      \"file_count\": {}", quant.files.len());
                print!("      \"files\": [");
                for (j, file) in quant.files.iter().enumerate() {
                    if j > 0 {
                        print!(", ");
                    }
                    print!("\"{}\"", file.filename);
                }
                println!("]");
                print!("    }}");
            }
            println!();
            println!("  ]");
            println!("}}");
        } else {
            println!("Model: {}", metadata.model_id);
            println!("\nQuantizations:");
            for quant in quantizations {
                println!(
                    "  - {} ({} files, {} MB total)",
                    quant.quant_type,
                    quant.files.len(),
                    quant.total_size / 1_048_576
                );
                for file in &quant.files {
                    println!("      - {} ({} MB)", file.filename, file.size / 1_048_576);
                }
            }

            // Show file tree for non-GGUF models
            if !quantizations.is_empty() {
                println!("\nFile Tree:");
                let file_tree = api::build_file_tree(metadata.siblings.clone());
                print_tree_node(&file_tree, 0);
            }
        }
    }

    #[allow(dead_code)]
    pub fn report_resume(&self, resumed: &[DownloadMetadata]) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "resumed",
                "count": resumed.len(),
                "downloads": resumed
            });
            println!("{}", json);
        } else if resumed.is_empty() {
            self.report_info("No incomplete downloads to resume");
        } else {
            println!("Resumed {} downloads:", resumed.len());
            for download in resumed {
                println!("  - {}", download.filename);
            }
        }
    }

    pub fn report_download_summary(&self, files: &[String], total_size: u64) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "queued",
                "file_count": files.len(),
                "total_size_bytes": total_size,
                "files": files
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            println!("Download Summary:");
            println!("  Files: {}", files.len());
            println!("  Total Size: {}", format_file_size(total_size));
            println!();

            if files.len() <= 10 {
                for file in files {
                    println!("  - {}", file);
                }
            } else {
                for file in files.iter().take(5) {
                    println!("  - {}", file);
                }
                println!("  ... and {} more", files.len() - 5);
            }
            println!();
        }
    }

    pub fn report_dry_run_summary(
        &self,
        files: &[String],
        total_size: u64,
        output_dir: &str,
        is_gguf: bool,
    ) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "dry_run",
                "model_type": if is_gguf { "GGUF" } else { "Non-GGUF" },
                "file_count": files.len(),
                "total_size_bytes": total_size,
                "output_directory": output_dir,
                "files": files
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            println!("Download Plan:");
            println!(
                "  Model type: {}",
                if is_gguf { "GGUF" } else { "Non-GGUF" }
            );
            println!("  Files to download: {}", files.len());
            println!("  Total size: {}", format_file_size(total_size));
            println!("  Output directory: {}", output_dir);
            println!();

            println!("Files:");
            for (i, file) in files.iter().enumerate() {
                println!("  {}. {}", i + 1, file);
            }
            println!();

            println!("Run without --dry-run to execute the download.");
        }
    }

    pub fn report_no_incomplete(&self) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "no_incomplete",
                "message": "No incomplete downloads found"
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            println!("No incomplete downloads found.");
        }
    }

    pub fn is_json(&self) -> bool {
        self.json_mode
    }

    pub fn report_quantizations_table(&self, quantizations: &[QuantizationGroup]) {
        println!("Available Quantizations:");
        println!();

        for group in quantizations {
            let total_size_str = format_file_size(group.total_size);
            println!(
                "  {} ({} total, {} file{})",
                group.quant_type,
                total_size_str,
                group.files.len(),
                if group.files.len() == 1 { "" } else { "s" }
            );

            for file in &group.files {
                let size_str = format_file_size(file.size);
                println!("    - {} ({})", file.filename, size_str);
            }
            println!();
        }
    }

    pub fn report_file_tree(&self, metadata: &ModelMetadata) {
        println!("Model Files:");
        println!();
        println!("  Model ID: {}", metadata.model_id);
        println!(
            "  Pipeline: {}",
            metadata.pipeline_tag.as_deref().unwrap_or("N/A")
        );
        println!("  Files: {}", metadata.siblings.len());
        println!();

        let tree = api::build_file_tree(metadata.siblings.clone());
        print_tree_node(&tree, 0);
    }

    pub fn report_list_json(
        &self,
        quantizations: &[QuantizationGroup],
        metadata: &ModelMetadata,
        has_gguf: bool,
    ) {
        println!("{{");
        println!("  \"model_id\": \"{}\",", metadata.model_id);
        println!(
            "  \"pipeline_tag\": \"{}\",",
            metadata.pipeline_tag.as_deref().unwrap_or("N/A")
        );
        println!("  \"has_gguf\": {},", has_gguf);

        if has_gguf {
            println!("  \"quantizations\": [");
            for (i, quant) in quantizations.iter().enumerate() {
                if i > 0 {
                    println!(",");
                }
                println!("    {{");
                println!("      \"quant_type\": \"{}\",", quant.quant_type);
                println!("      \"total_size\": {},", quant.total_size);
                println!("      \"file_count\": {}", quant.files.len());
                print!("      \"files\": [");
                for (j, file) in quant.files.iter().enumerate() {
                    if j > 0 {
                        print!(", ");
                    }
                    print!("\"{}\"", file.filename);
                }
                print!("]");
                print!("    }}");
            }
            println!();
            println!("  ]");
        } else {
            println!("  \"file_count\": {},", metadata.siblings.len());
            println!("  \"files\": [");
            for (i, file) in metadata.siblings.iter().enumerate() {
                if i > 0 {
                    println!(",");
                }
                print!(
                    "    {{ \"filename\": \"{}\", \"size\": {} }}",
                    file.rfilename,
                    file.size.unwrap_or(0)
                );
            }
            println!();
            println!("  ]");
        }

        println!("}}");
    }

    pub fn report_resume_summary(&self, incomplete: &[DownloadMetadata]) {
        let total_size: u64 = incomplete.iter().map(|d| d.total_size).sum();

        if self.json_mode {
            let json = serde_json::json!({
                "status": "resumed",
                "count": incomplete.len(),
                "total_size_bytes": total_size,
                "downloads": incomplete.iter().map(|d| serde_json::json!({
                    "filename": d.filename,
                    "model_id": d.model_id,
                    "size": d.total_size
                })).collect::<Vec<_>>()
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            let total_size_str = format_file_size(total_size);
            println!(
                "Resuming {} download(s) ({} total):",
                incomplete.len(),
                total_size_str
            );
            println!();

            for download in incomplete {
                let size_str = format_file_size(download.total_size);
                println!("  - {} ({})", download.filename, size_str);
            }
            println!();
        }
    }
}

fn print_tree_node(node: &FileTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let size_str = if let Some(size) = node.size {
        format!(" ({} MB)", size / 1_048_576)
    } else {
        String::new()
    };

    println!("{}{}{}", indent, node.name, size_str);

    for child in &node.children {
        print_tree_node(child, depth + 1);
    }
}
