# Phase 3: Implement Headless Commands

**Status**: ✅ Complete
**Estimated Time**: 4 hours
**Actual Time**: 2.5 hours
**Dependencies**: Phase 1 (CLI), Phase 2 (Core Logic)
**Blocked By**: Phase 1 and Phase 2 completion

## Overview
Implement the four main headless commands (search, download, list, resume) with full functionality, proper output formatting, and integration with existing systems.

## Objectives
- Implement search command with filters and JSON output ✅
- Implement download command with quantization filtering ✅
- Implement list command for model file exploration ✅
- Implement resume command for batch download resumption ✅
- Test each command thoroughly ✅

## Tasks Checklist

### 3.1 Implement Search Command
- [x] Create `run_search()` function in `headless.rs`
- [x] Add support for all filter parameters (sort, min-downloads, min-likes)
- [x] Implement human-readable table output
- [x] Implement JSON output format
- [x] Add result count and timing information

**Expected Implementation:**
```rust
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

    if reporter.is_json() {
        reporter.report_search_json(&models, elapsed);
    } else {
        reporter.report_search_table(&models, elapsed);
    }

    Ok(())
}

// In ProgressReporter
pub fn report_search_table(&self, models: &[ModelInfo], elapsed: std::time::Duration) {
    println!("Found {} models in {:.2}s:", models.len(), elapsed.as_secs_f64());
    println!();

    if models.is_empty() {
        println!("No models found matching your criteria.");
        return;
    }

    // Calculate column widths
    let max_id_width = models.iter()
        .map(|m| m.id.len())
        .max()
        .unwrap_or(40)
        .min(60);

    // Print header
    println!("{:<id_width$} | {:>12} | {:>10} | {}", "Model", "Downloads", "Likes", "Last Modified", id_width = max_id_width);
    println!("{:-<id_width$}-+-{:-<12}-+-{:-<10}-+-{}", "-------", "------------", "----------", "--------------", id_width = max_id_width);

    // Print each model
    for model in models {
        let last_mod = model.last_modified.as_deref().unwrap_or("N/A");
        let downloads = utils::format_number(model.downloads);
        let likes = utils::format_number(model.likes);

        println!("{:<id_width$} | {:>12} | {:>10} | {}", model.id, downloads, likes, last_mod, id_width = max_id_width);
    }
}

pub fn report_search_json(&self, models: &[ModelInfo], elapsed: std::time::Duration) {
    let output = serde_json::json!({
        "count": models.len(),
        "query_time_seconds": elapsed.as_secs_f64(),
        "results": models
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
```

**Testing:**
```bash
# Basic search
cargo run -- --headless search "llama"

# With filters
cargo run -- --headless search "gpt" --min-downloads 10000 --min-likes 100

# JSON output
cargo run -- --headless --json search "stable diffusion" | jq '.results[].id'
```

### 3.2 Implement Download Command (GGUF Models)
- [ ] Create `run_download()` function
- [ ] Add quantization type filtering
- [ ] Implement --all flag for downloading all quantizations
- [ ] Add file size estimation before download
- [ ] Show download queue summary
- [ ] Implement progress tracking per file

**Expected Implementation:**
```rust
pub async fn run_download(
    model_id: &str,
    quantization: Option<&str>,
    download_all: bool,
    output_dir: &str,
    hf_token: Option<String>,
    reporter: &ProgressReporter,
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    download_rx: Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>,
    progress_rx: mpsc::UnboundedReceiver<String>,
    download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    complete_downloads: Arc<Mutex<CompleteDownloads>>,
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    download_queue_size: Arc<Mutex<usize>>,
    download_queue_bytes: Arc<Mutex<u64>>,
) -> Result<(), HeadlessError> {
    // First, list what will be downloaded
    let (quantizations, metadata) = list_quantizations(model_id, hf_token.as_ref()).await?;

    let has_gguf = api::has_gguf_files(&metadata);

    if !has_gguf && !download_all {
        return Err(HeadlessError::DownloadError(
            "Non-GGUF model requires --all flag".to_string()
        ));
    }

    // Calculate total size and show summary
    let (files_to_download, total_size) = if has_gguf {
        calculate_gguf_download_summary(&quantizations, quantization, download_all)?
    } else {
        calculate_non_gguf_download_summary(&metadata, download_all)?
    };

    reporter.report_download_summary(&files_to_download, total_size);

    // Queue downloads
    download_model(model_id, quantization, download_all, output_dir, hf_token, progress_tx.clone(), download_tx).await?;

    // Wait for downloads to complete
    wait_for_downloads(
        download_rx,
        download_progress,
        complete_downloads,
        verification_queue,
        verification_queue_size,
        download_queue_size,
        download_queue_bytes,
        reporter,
    ).await?;

    Ok(())
}

fn calculate_gguf_download_summary(
    quantizations: &[QuantizationGroup],
    filter: Option<&str>,
    download_all: bool,
) -> Result<(Vec<String>, u64), HeadlessError> {
    if let Some(q_filter) = filter {
        let group = quantizations.iter()
            .find(|q| q.quant_type == q_filter)
            .ok_or_else(|| HeadlessError::DownloadError(
                format!("Quantization '{}' not found", q_filter)
            ))?;

        let files: Vec<String> = group.files.iter().map(|f| f.filename.clone()).collect();
        let total_size = group.total_size;
        Ok((files, total_size))
    } else if download_all {
        let files: Vec<String> = quantizations.iter()
            .flat_map(|q| q.files.iter().map(|f| f.filename.clone()))
            .collect();
        let total_size = quantizations.iter().map(|q| q.total_size).sum();
        Ok((files, total_size))
    } else {
        Err(HeadlessError::DownloadError(
            "Must specify --quantization or --all".to_string()
        ))
    }
}

fn calculate_non_gguf_download_summary(
    metadata: &ModelMetadata,
    download_all: bool,
) -> Result<(Vec<String>, u64), HeadlessError> {
    if !download_all {
        return Err(HeadlessError::DownloadError(
            "Non-GGUF model requires --all flag".to_string()
        ));
    }

    let files: Vec<String> = metadata.siblings.iter()
        .filter_map(|f| {
            f.size.map(|_| f.rfilename.clone())
        })
        .collect();

    let total_size: u64 = metadata.siblings.iter()
        .filter_map(|f| f.size)
        .sum();

    Ok((files, total_size))
}

async fn wait_for_downloads(
    download_rx: Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>,
    download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    complete_downloads: Arc<Mutex<CompleteDownloads>>,
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    download_queue_size: Arc<Mutex<usize>>,
    download_queue_bytes: Arc<Mutex<u64>>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));

    loop {
        interval.tick().await;

        // Check download progress
        let progress = download_progress.try_lock();
        if let Ok(prog) = progress {
            if let Some(p) = prog.as_ref() {
                reporter.report_download_progress(&p.filename, p.downloaded, p.total, p.speed_mbps);
            }
        }
        drop(progress);

        // Check if queue is empty and no active downloads
        let queue_size = download_queue_size.lock().await;
        let has_progress = download_progress.try_lock()
            .map(|p| p.is_some())
            .unwrap_or(false);

        if *queue_size == 0 && !has_progress {
            break;
        }
    }

    Ok(())
}
```

**Testing:**
```bash
# Download specific quantization
cargo run -- --headless download "TheBloke/llama-2-7b-GGUF" --quantization "Q4_K_M" --output "/tmp/models"

# Download all quantizations
cargo run -- --headless download "TheBloke/llama-2-7b-GGUF" --all --output "/tmp/models"

# Download non-GGUF model
cargo run -- --headless download "bert-base-uncased" --all --output "/tmp/models"
```

### 3.3 Implement List Command
- [ ] Create `run_list()` function
- [ ] Display GGUF quantizations with file sizes
- [ ] Display full file tree for non-GGUF models
- [ ] Show download status indicators
- [ ] Support both table and JSON output

**Expected Implementation:**
```rust
pub async fn run_list(
    model_id: &str,
    token: Option<&String>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    let (quantizations, metadata) = list_quantizations(model_id, token).await?;

    let has_gguf = api::has_gguf_files(&metadata);

    if reporter.is_json() {
        reporter.report_list_json(&quantizations, &metadata, has_gguf);
    } else {
        if has_gguf {
            reporter.report_quantizations_table(&quantizations);
        } else {
            reporter.report_file_tree(&metadata);
        }
    }

    Ok(())
}

// In ProgressReporter
pub fn report_quantizations_table(&self, quantizations: &[QuantizationGroup]) {
    println!("Available Quantizations:");
    println!();

    for group in quantizations {
        let total_size_gb = group.total_size as f64 / 1_073_741_824.0;
        println!("  {} ({:.2} GB total, {} file{})",
            group.quant_type,
            total_size_gb,
            group.files.len(),
            if group.files.len() == 1 { "" } else { "s" }
        );

        for file in &group.files {
            let size_gb = file.size as f64 / 1_073_741_824.0;
            println!("    - {} ({:.2} GB)", file.filename, size_gb);
        }
        println!();
    }
}

pub fn report_file_tree(&self, metadata: &ModelMetadata) {
    println!("Model Files:");
    println!();
    println!("  Model ID: {}", metadata.model_id);
    println!("  Pipeline: {}", metadata.pipeline_tag.as_deref().unwrap_or("N/A"));
    println!("  Files: {}", metadata.siblings.len());
    println!();

    let tree = api::build_file_tree(metadata.siblings.clone());
    print_tree_node(&tree, 0);
}

fn print_tree_node(node: &FileTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let size_str = node.size.map(|s| format!(" ({:.2} MB)", s as f64 / 1_048_576.0)).unwrap_or_default();

    println!("{}{}{}{}", indent, node.name, size_str, if node.is_dir { "/" } else { "" });

    for child in &node.children {
        print_tree_node(child, depth + 1);
    }
}
```

**Testing:**
```bash
# List GGUF model
cargo run -- --headless list "TheBloke/llama-2-7b-GGUF"

# List non-GGUF model
cargo run -- --headless list "bert-base-uncased"

# JSON output
cargo run -- --headless --json list "TheBloke/llama-2-7b-GGUF" | jq '.quantizations'
```

### 3.4 Implement Resume Command
- [ ] Create `run_resume()` function
- [ ] Scan for incomplete downloads in registry
- [ ] Display resume summary
- [ ] Queue all incomplete downloads
- [ ] Wait for completion
- [ ] Report final status

**Expected Implementation:**
```rust
pub async fn run_resume(
    reporter: &ProgressReporter,
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    download_rx: Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>,
    download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    complete_downloads: Arc<Mutex<CompleteDownloads>>,
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    download_queue_size: Arc<Mutex<usize>>,
    download_queue_bytes: Arc<Mutex<u64>>,
) -> Result<(), HeadlessError> {
    let incomplete = resume_downloads(download_tx, reporter.progress_tx(), download_rx.clone()).await?;

    if incomplete.is_empty() {
        reporter.report_no_incomplete();
        return Ok(());
    }

    reporter.report_resume_summary(&incomplete);

    // Wait for downloads to complete (reuse wait_for_downloads)
    wait_for_downloads(
        download_rx,
        download_progress,
        complete_downloads,
        verification_queue,
        verification_queue_size,
        download_queue_size,
        download_queue_bytes,
        reporter,
    ).await?;

    Ok(())
}

// In ProgressReporter
pub fn report_resume_summary(&self, incomplete: &[DownloadMetadata]) {
    let total_size: u64 = incomplete.iter().map(|d| d.total_size).sum();
    let total_size_gb = total_size as f64 / 1_073_741_824.0;

    println!("Resuming {} incomplete download(s) ({:.2} GB total):", incomplete.len(), total_size_gb);
    println!();

    for download in incomplete {
        let size_gb = download.total_size as f64 / 1_073_741_824.0;
        println!("  - {} ({:.2} GB)", download.filename, size_gb);
    }
    println!();
}

pub fn report_no_incomplete(&self) {
    println!("No incomplete downloads found.");
}
```

**Testing:**
```bash
# Create incomplete download first, then resume
cargo run -- --headless download "model" --quantization "Q4" --output "/tmp"
# ^C during download
cargo run -- --headless resume
```

### 3.5 Add Helper Functions
- [ ] Create `format_file_size()` utility
- [ ] Create `format_duration()` utility
- [ ] Add validation for model_id format
- [ ] Add path validation for output directory

**Expected Implementation:**
```rust
// In utils.rs or headless.rs
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

pub fn validate_model_id(model_id: &str) -> Result<(), HeadlessError> {
    if !model_id.contains('/') {
        return Err(HeadlessError::DownloadError(
            "Invalid model ID format. Expected: 'author/model-name'".to_string()
        ));
    }

    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() != 2 {
        return Err(HeadlessError::DownloadError(
            "Invalid model ID format. Expected: 'author/model-name'".to_string()
        ));
    }

    Ok(())
}
```

### 3.6 Update main.rs Integration
- [ ] Add match arm for each command
- [ ] Pass all required parameters to headless functions
- [ ] Set up download manager task for download/resume commands
- [ ] Handle errors and return appropriate exit codes
- [ ] Add proper signal handling (Ctrl+C)

**Expected Code:**
```rust
async fn main() -> color_eyre::Result<()> {
    // ... CLI parsing and TUI setup ...

    if cli_args.headless {
        let reporter = headless::ProgressReporter::new(cli_args.json);
        let token = cli_args.token.or_else(|| {
            let options = config::load_config();
            options.hf_token
        });

        match cli_args.command {
            Some(cli::Commands::Search { query, sort, min_downloads, min_likes }) => {
                let sort_field = sort.and_then(|s| parse_sort_field(&s));
                headless::run_search(&query, sort_field, min_downloads, min_likes, token.as_ref(), &reporter).await
            }
            Some(cli::Commands::Download { model_id, quantization, all, output }) => {
                headless::validate_model_id(&model_id)?;
                let output_dir = output.unwrap_or_else(|| {
                    let options = config::load_config();
                    options.default_directory
                });

                // Set up download infrastructure
                let (download_tx, download_rx) = mpsc::unbounded_channel();
                let download_rx = Arc::new(Mutex::new(download_rx));
                let download_progress = Arc::new(Mutex::new(None));
                let complete_downloads = Arc::new(Mutex::new(HashMap::new()));
                let verification_queue = Arc::new(Mutex::new(Vec::new()));
                let verification_queue_size = Arc::new(Mutex::new(0));
                let download_queue_size = Arc::new(Mutex::new(0));
                let download_queue_bytes = Arc::new(Mutex::new(0));
                let download_registry = Arc::new(Mutex::new(DownloadRegistry::default()));

                headless::run_download(
                    &model_id,
                    quantization.as_deref(),
                    all,
                    &output_dir,
                    token,
                    &reporter,
                    download_tx,
                    download_rx,
                    download_progress,
                    complete_downloads,
                    verification_queue,
                    verification_queue_size,
                    download_queue_size,
                    download_queue_bytes,
                ).await
            }
            Some(cli::Commands::List { model_id }) => {
                headless::validate_model_id(&model_id)?;
                headless::run_list(&model_id, token.as_ref(), &reporter).await
            }
            Some(cli::Commands::Resume) => {
                // Set up download infrastructure (same as download command)
                headless::run_resume(
                    &reporter,
                    download_tx,
                    download_rx,
                    download_progress,
                    complete_downloads,
                    verification_queue,
                    verification_queue_size,
                    download_queue_size,
                    download_queue_bytes,
                ).await
            }
            None => {
                eprintln!("Error: No command specified. Use --help for usage.");
                std::process::exit(1);
            }
        }
        .map_err(|e| {
            reporter.report_error(&e.to_string());
            std::process::exit(1);
        });

        std::process::exit(0);
    }

    // ... TUI mode ...
}
```

## Verification Steps

### Command Testing
- [ ] Test search with various queries
- [ ] Test search with filters (sort, min-downloads, min-likes)
- [ ] Test download with quantization filter
- [ ] Test download with --all flag
- [ ] Test list for GGUF model
- [ ] Test list for non-GGUF model
- [ ] Test resume with incomplete downloads

### Output Format Testing
- [ ] Verify table output is readable
- [ ] Verify JSON output is valid
- [ ] Test JSON parsing with jq
- [ ] Check progress bar updates smoothly

### Error Handling
- [ ] Test with invalid model_id
- [ ] Test with non-existent model
- [ ] Test with network errors
- [ ] Test with insufficient disk space
- [ ] Test with authentication errors

## Success Criteria

### Must Have
- ✅ All four commands implemented
- ✅ Table and JSON output working
- ✅ Progress tracking functional
- ✅ Error handling comprehensive
- ✅ Integration with main.rs complete

### Nice to Have
- Colored output for errors
- Verbose mode with detailed logging
- Progress bar animation
- ETA calculation

## Next Phase Link
Once this phase is complete, proceed to **Phase 4: Progress & Error Handling** (`add-headless-phase4.md`).

## Notes
- Test with small models first (<1GB)
- Use existing download infrastructure
- Ensure graceful shutdown on signals
- Keep output format consistent
