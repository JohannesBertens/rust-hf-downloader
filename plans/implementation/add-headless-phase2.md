# Phase 2: Extract Core Logic from UI

**Status**: 📋 Not Started
**Estimated Time**: 2 hours
**Dependencies**: Phase 1 (CLI parsing)
**Blocked By**: Phase 1 completion

## Overview
Create the `headless.rs` module with reusable functions that wrap existing API and download logic. This phase bridges CLI commands with core functionality.

## Objectives
- Create `src/headless.rs` module
- Implement headless versions of core operations
- Set up progress reporting for console output
- Integrate with `main.rs` for headless mode entry point

## Tasks Checklist

### 2.1 Create headless.rs Module Structure
- [ ] Create `src/headless.rs` file
- [ ] Add `mod headless;` to `src/main.rs`
- [ ] Define module-level structs and error types
- [ ] Set up imports from existing modules

**File Structure:**
```rust
// src/headless.rs
use crate::models::*;
use crate::api;
use crate::download;
use crate::config;
use std::path::PathBuf;

// Module will contain:
// - search_models()
// - list_quantizations()
// - download_model()
// - resume_downloads()
// - ProgressReporter struct
// - HeadlessError enum
```

### 2.2 Define Headless Error Type
- [ ] Create `HeadlessError` enum
- [ ] Implement variants: ApiError, DownloadError, ConfigError, IoError
- [ ] Implement `std::fmt::Display` for user-friendly messages
- [ ] Implement `From` traits for existing error types

**Expected Code:**
```rust
#[derive(Debug)]
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
```

### 2.3 Implement search_models() Function
- [ ] Create function with signature:
  ```rust
  pub async fn search_models(
      query: &str,
      sort_field: Option<SortField>,
      sort_direction: Option<SortDirection>,
      min_downloads: Option<u64>,
      min_likes: Option<u64>,
      token: Option<&String>,
  ) -> Result<Vec<ModelInfo>, HeadlessError>
  ```
- [ ] Use existing `api::fetch_models_filtered()`
- [ ] Apply default values for None parameters
- [ ] Return results or propagate errors

**Expected Implementation:**
```rust
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
```

### 2.4 Implement list_quantizations() Function
- [ ] Create function with signature:
  ```rust
  pub async fn list_quantizations(
      model_id: &str,
      token: Option<&String>,
  ) -> Result<(Vec<QuantizationGroup>, ModelMetadata), HeadlessError>
  ```
- [ ] Call `api::fetch_model_files()` for GGUF models
- [ ] Call `api::fetch_model_metadata()` for file tree
- [ ] Return both for comprehensive listing

**Expected Implementation:**
```rust
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
```

### 2.5 Implement download_model() Function
- [ ] Create function with signature:
  ```rust
  pub async fn download_model(
      model_id: &str,
      quantization_filter: Option<&str>,
      download_all: bool,
      output_dir: &str,
      hf_token: Option<String>,
      progress_tx: mpsc::UnboundedSender<String>,
      download_tx: mpsc::UnboundedSender<DownloadMessage>,
  ) -> Result<(), HeadlessError>
  ```
- [ ] Load config from `config::load_config()`
- [ ] Filter quantizations if quantization_filter is Some
- [ ] Queue downloads via download_tx channel
- [ ] Handle both GGUF and non-GGUF models

**Expected Implementation:**
```rust
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
    let token = hf_token.or_else(|| options.hf_token);

    // Fetch model metadata
    let metadata = api::fetch_model_metadata(model_id, token.as_ref()).await
        .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

    // Check if model has GGUF files
    let has_gguf = api::has_gguf_files(&metadata);

    if has_gguf {
        let quantizations = api::fetch_model_files(model_id, token.as_ref()).await
            .map_err(|e| HeadlessError::ApiError(e.to_string()))?;

        // Filter by quantization type if specified
        let files_to_download: Vec<_> = if let Some(q_filter) = quantization_filter {
            quantizations.iter()
                .filter(|q| q.quant_type == q_filter)
                .flat_map(|q| q.files.iter())
                .collect()
        } else if download_all {
            quantizations.iter()
                .flat_map(|q| q.files.iter())
                .collect()
        } else {
            return Err(HeadlessError::DownloadError(
                "Must specify --quantization or --all for GGUF models".to_string()
            ));
        };

        // Queue downloads
        for quant_file in files_to_download {
            let path = PathBuf::from(output_dir);
            let total_size = quant_file.size;
            download_tx.send((
                model_id.to_string(),
                quant_file.filename.clone(),
                path,
                quant_file.sha256.clone(),
                token.clone(),
                total_size,
            )).map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

            let _ = progress_tx.send(format!("Queued: {}", quant_file.filename));
        }
    } else {
        // Non-GGUF model: download all files from metadata
        if !download_all {
            return Err(HeadlessError::DownloadError(
                "Non-GGUF models require --all flag".to_string()
            ));
        }

        for file in &metadata.siblings {
            let path = PathBuf::from(output_dir);
            let size = file.size.unwrap_or(0);
            let sha256 = file.lfs.as_ref().map(|l| lfs.oid.clone());

            download_tx.send((
                model_id.to_string(),
                file.rfilename.clone(),
                path,
                sha256,
                token.clone(),
                size,
            )).map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

            let _ = progress_tx.send(format!("Queued: {}", file.rfilename));
        }
    }

    Ok(())
}
```

### 2.6 Implement resume_downloads() Function
- [ ] Create function with signature:
  ```rust
  pub async fn resume_downloads(
      download_tx: mpsc::UnboundedSender<DownloadMessage>,
      progress_tx: mpsc::UnboundedSender<String>,
  ) -> Result<Vec<DownloadMetadata>, HeadlessError>
  ```
- [ ] Load download registry
- [ ] Filter for Incomplete status
- [ ] Queue all incomplete downloads
- [ ] Return list of resumed downloads

**Expected Implementation:**
```rust
pub async fn resume_downloads(
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    progress_tx: mpsc::UnboundedSender<String>,
) -> Result<Vec<DownloadMetadata>, HeadlessError> {
    let registry = crate::registry::load_registry();
    let incomplete: Vec<_> = registry.downloads.iter()
        .filter(|d| d.status == DownloadStatus::Incomplete)
        .cloned()
        .collect();

    if incomplete.is_empty() {
        let _ = progress_tx.send("No incomplete downloads found".to_string());
        return Ok(Vec::new());
    }

    for download in &incomplete {
        let path = PathBuf::from(&download.local_path);
        download_tx.send((
            download.model_id.clone(),
            download.filename.clone(),
            path,
            download.expected_sha256.clone(),
            None, // Use token from config
            download.total_size,
        )).map_err(|e| HeadlessError::DownloadError(e.to_string()))?;

        let _ = progress_tx.send(format!("Resumed: {}", download.filename));
    }

    Ok(incomplete)
}
```

### 2.7 Create ProgressReporter Struct
- [ ] Define struct for console progress reporting
- [ ] Implement methods for simple text progress bars
- [ ] Handle status messages from channels
- [ ] Support both stdout (progress) and stderr (errors)

**Expected Implementation:**
```rust
pub struct ProgressReporter {
    json_mode: bool,
}

impl ProgressReporter {
    pub fn new(json_mode: bool) -> Self {
        Self { json_mode }
    }

    pub fn report_search(&self, models: &[ModelInfo]) {
        if self.json_mode {
            println!("{}", serde_json::to_string(models).unwrap());
        } else {
            println!("Found {} models:", models.len());
            for model in models {
                println!("  - {} ({} downloads, {} likes)",
                    model.id, model.downloads, model.likes);
            }
        }
    }

    pub fn report_download_start(&self, filename: &str, total_size: u64) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "starting",
                "filename": filename,
                "size": total_size
            });
            println!("{}", json);
        } else {
            println!("Downloading: {} ({} MB)",
                filename,
                total_size / 1_048_576
            );
        }
    }

    pub fn report_download_progress(&self, filename: &str, downloaded: u64, total: u64, speed_mbps: f64) {
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
            println!("\r[{}] {}% ({:.2} MB/s)", bar, percent, speed_mbps);
            let _ = std::io::stdout().flush();
        }
    }

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
}
```

### 2.8 Integrate with main.rs
- [ ] Update `main.rs` to call headless functions
- [ ] Set up channels for progress reporting
- [ ] Match on CLI commands and dispatch appropriately
- [ ] Handle errors and return proper exit codes

**Expected Code Change in main.rs:**
```rust
mod cli;
mod headless;

async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli_args = cli::Cli::parse();

    if cli_args.headless {
        let json_mode = cli_args.json;
        let reporter = headless::ProgressReporter::new(json_mode);

        // Create channels for download manager
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let download_rx = Arc::new(Mutex::new(download_rx));

        // Spawn download manager task (reused from TUI)
        let download_progress = Arc::new(Mutex::new(None));
        let complete_downloads = Arc::new(Mutex::new(HashMap::new()));
        let verification_queue = Arc::new(Mutex::new(Vec::new()));
        let verification_queue_size = Arc::new(Mutex::new(0));
        let download_queue_size = Arc::new(Mutex::new(0));
        let download_queue_bytes = Arc::new(Mutex::new(0));
        let download_registry = Arc::new(Mutex::new(DownloadRegistry::default()));

        tokio::spawn(async move {
            let mut rx = download_rx.lock().await;
            while let Some((model_id, filename, path, sha256, hf_token, total_size)) = rx.recv().await {
                // ... (reuse existing download manager logic)
            }
        });

        // Spawn progress reporter task
        tokio::spawn(async move {
            while let Some(msg) = progress_rx.recv().await {
                reporter.report_error(&msg); // Or appropriate report method
            }
        });

        // Execute command
        let result = match cli_args.command {
            Some(cli::Commands::Search { query, sort, min_downloads, min_likes }) => {
                headless::search_models(&query, None, None, min_downloads, min_likes, cli_args.token.as_ref()).await
            }
            Some(cli::Commands::Download { model_id, quantization, all, output }) => {
                let output_dir = output.unwrap_or_else(|| {
                    let options = config::load_config();
                    options.default_directory
                });
                headless::download_model(&model_id, quantization.as_deref(), all, &output_dir, cli_args.token, progress_tx, download_tx).await
            }
            Some(cli::Commands::List { model_id }) => {
                headless::list_quantizations(&model_id, cli_args.token.as_ref()).await
            }
            Some(cli::Commands::Resume) => {
                headless::resume_downloads(download_tx, progress_tx).await
            }
            None => {
                eprintln!("Error: No command specified");
                std::process::exit(1);
            }
        };

        match result {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                reporter.report_error(&e.to_string());
                std::process::exit(1);
            }
        }
    }

    // Original TUI flow...
}
```

## Verification Steps

### Unit Testing
- [ ] Test `search_models()` with various filters
- [ ] Test `list_quantizations()` with GGUF model
- [ ] Test `list_quantizations()` with non-GGUF model
- [ ] Test `resume_downloads()` with empty registry
- [ ] Test error handling (invalid model_id, network errors)

### Integration Testing
- [ ] Run `--headless search "test"` and verify output
- [ ] Run `--headless list "model_id"` and verify file listing
- [ ] Run `--headless resume` and verify error message (no incomplete downloads)
- [ ] Verify channels don't deadlock
- [ ] Verify proper cleanup on Ctrl+C

## Success Criteria

### Must Have
- ✅ All headless functions implemented
- ✅ Reuses existing API and download logic
- ✅ Progress reporter works in both text and JSON modes
- ✅ Integration with main.rs complete
- ✅ No blocking calls in async context

### Nice to Have
- Progress bar animation for text mode
- Colored output for errors
- Verbose mode with detailed logging

## Next Phase Link
Once this phase is complete, proceed to **Phase 3: Implement Headless Commands** (`add-headless-phase3.md`).

## Notes
- Keep functions async and non-blocking
- Use channels for loose coupling
- Prioritize code reuse over optimization
- Ensure graceful error propagation
