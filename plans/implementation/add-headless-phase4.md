# Phase 4: Progress & Error Handling

**Status**: 📋 Not Started
**Estimated Time**: 2 hours
**Dependencies**: Phase 1 (CLI), Phase 2 (Core Logic), Phase 3 (Commands)
**Blocked By**: Phase 3 completion

## Overview
Implement robust progress reporting and comprehensive error handling for headless mode. This ensures users get clear feedback and the system handles failures gracefully.

## Objectives
- Replace UI progress bars with console-based progress reporting
- Implement proper exit codes for automation
- Add comprehensive error handling
- Preserve existing retry logic in headless mode
- Handle signals gracefully (Ctrl+C)

## Tasks Checklist

### 4.1 Enhance ProgressReporter
- [ ] Add progress bar animation for text mode
- [ ] Implement multi-file progress tracking
- [ ] Add ETA calculation and display
- [ ] Implement download speed display
- [ ] Add completion status indicators

**Expected Implementation:**
```rust
pub struct ProgressReporter {
    json_mode: bool,
    show_eta: bool,
    last_update: Arc<Mutex<std::time::Instant>>,
    last_bytes: Arc<Mutex<u64>>,
}

impl ProgressReporter {
    pub fn new(json_mode: bool, show_eta: bool) -> Self {
        Self {
            json_mode,
            show_eta,
            last_update: Arc::new(Mutex::new(std::time::Instant::now())),
            last_bytes: Arc::new(Mutex::new(0)),
        }
    }

    pub fn report_download_progress_enhanced(
        &self,
        filename: &str,
        downloaded: u64,
        total: u64,
        speed_mbps: f64,
        active_downloads: usize,
        queued_downloads: usize,
    ) {
        if self.json_mode {
            self.report_download_progress_json(filename, downloaded, total, speed_mbps);
        } else {
            self.report_download_progress_text(filename, downloaded, total, speed_mbps, active_downloads, queued_downloads);
        }
    }

    fn report_download_progress_text(
        &self,
        filename: &str,
        downloaded: u64,
        total: u64,
        speed_mbps: f64,
        active: usize,
        queued: usize,
    ) {
        let percent = (downloaded as f64 / total as f64 * 100.0) as u32;
        let bar_width = 30;
        let filled = (percent as f32 / 100.0 * bar_width as f32) as usize;
        let bar: String = "=".repeat(filled) + &" ".repeat(bar_width - filled);

        // Calculate ETA if speed > 0
        let eta_str = if self.show_eta && speed_mbps > 0.0 {
            let remaining_bytes = total.saturating_sub(downloaded);
            let remaining_mb = remaining_bytes as f64 / 1_048_576.0;
            let eta_seconds = remaining_mb / speed_mbps;
            format!(" ETA: {}", format_eta(Duration::from_secs_f64(eta_seconds)))
        } else {
            String::new()
        };

        // Build status line
        let queue_info = if active > 1 || queued > 0 {
            format!(" ({} active, {} queued)", active, queued)
        } else {
            String::new()
        };

        // Print progress with carriage return for in-place update
        print!("\r[{}] {}% {:.2} MB/s{}{} - {}",
            bar,
            percent,
            speed_mbps,
            eta_str,
            queue_info,
            filename
        );

        let _ = std::io::stdout().flush();
    }

    fn report_download_progress_json(&self, filename: &str, downloaded: u64, total: u64, speed_mbps: f64) {
        let json = serde_json::json!({
            "status": "downloading",
            "filename": filename,
            "downloaded": downloaded,
            "total": total,
            "progress_percent": (downloaded as f64 / total as f64 * 100.0),
            "speed_mbps": speed_mbps,
            "timestamp": chrono::Utc::now().to_rfc3339()
        });
        println!("{}", json);
    }

    pub fn report_download_complete(&self, filename: &str, duration: Duration, final_size: u64) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "complete",
                "filename": filename,
                "size_bytes": final_size,
                "duration_seconds": duration.as_secs_f64(),
                "average_speed_mbps": (final_size as f64 / 1_048_576.0) / duration.as_secs_f64()
            });
            println!("{}", json);
        } else {
            println!("\n✓ Complete: {} ({:.2} MB in {})",
                filename,
                final_size as f64 / 1_048_576.0,
                format_duration(duration)
            );
        }
    }
}

fn format_eta(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
```

### 4.2 Implement Exit Codes
- [ ] Define exit code constants
- [ ] Exit 0: Success
- [ ] Exit 1: Download/API error
- [ ] Exit 2: Authentication error
- [ ] Exit 3: Invalid arguments
- [ ] Update main.rs to use appropriate codes

**Expected Implementation:**
```rust
// In headless.rs
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
            | HeadlessError::IoError(_)
            | HeadlessError::ConfigError(_) => EXIT_ERROR,
        }
    }
}

// In main.rs
match result {
    Ok(_) => std::process::exit(headless::EXIT_SUCCESS),
    Err(e) => {
        reporter.report_error(&e.to_string());
        std::process::exit(e.exit_code());
    }
}
```

### 4.3 Add Comprehensive Error Handling
- [ ] Handle network timeouts gracefully
- [ ] Handle insufficient disk space
- [ ] Handle authentication failures
- [ ] Handle invalid model IDs
- [ ] Handle filesystem permission errors

**Expected Implementation:**
```rust
impl HeadlessError {
    pub fn with_context(mut self, context: &str) -> Self {
        match &mut self {
            HeadlessError::ApiError(msg) => {
                *msg = format!("{}: {}", context, msg);
            }
            HeadlessError::DownloadError(msg) => {
                *msg = format!("{}: {}", context, msg);
            }
            HeadlessError::ConfigError(msg) => {
                *msg = format!("{}: {}", context, msg);
            }
            HeadlessError::AuthError(msg) => {
                *msg = format!("{}: {}", context, msg);
            }
            HeadlessError::IoError(err) => {
                // Keep IoError as-is for proper error chaining
            }
        }
        self
    }
}

// Enhanced error messages
pub fn map_reqwest_error(err: &reqwest::Error) -> HeadlessError {
    if err.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
        return HeadlessError::AuthError(
            "Authentication failed. Check your HuggingFace token.".to_string()
        );
    }

    if err.status() == Some(reqwest::StatusCode::NOT_FOUND) {
        return HeadlessError::ApiError(
            "Model or file not found. Verify the model ID is correct.".to_string()
        );
    }

    if err.is_timeout() {
        return HeadlessError::ApiError(
            format!("Request timed out. Try again or check your network connection.")
        );
    }

    if err.is_connect() {
        return HeadlessError::ApiError(
            "Failed to connect to HuggingFace API. Check your internet connection.".to_string()
        );
    }

    HeadlessError::ApiError(err.to_string())
}

pub fn map_io_error(err: &std::io::Error, path: &str) -> HeadlessError {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => {
            HeadlessError::IoError(
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Permission denied: {}. Check file/directory permissions.", path)
                )
            )
        }
        std::io::ErrorKind::StorageFull => {
            HeadlessError::IoError(
                std::io::Error::new(
                    std::io::ErrorKind::StorageFull,
                    format!("Insufficient disk space to download to: {}", path)
                )
            )
        }
        _ => HeadlessError::IoError(
            std::io::Error::new(
                err.kind(),
                format!("IO error at '{}': {}", path, err)
            )
        )
    }
}
```

### 4.4 Preserve Retry Logic in Headless Mode
- [ ] Verify retry logic works without UI
- [ ] Add retry count logging
- [ ] Implement exponential backoff
- [ ] Handle max retries exceeded

**Expected Implementation:**
```rust
// The existing download.rs already has retry logic
// We just need to ensure it logs properly in headless mode

// In headless.rs, enhance the status channel handler
tokio::spawn(async move {
    let mut rx = status_rx;
    while let Some(msg) = rx.recv().await {
        if msg.contains("Retrying") {
            // Log retry attempts
            eprintln!("Warning: {}", msg);
        } else if msg.contains("AUTH_ERROR") {
            // Handle auth errors specially
            reporter.report_error(&msg);
            std::process::exit(EXIT_AUTH_ERROR);
        } else {
            // Normal status message
            reporter.report_info(&msg);
        }
    }
});
```

### 4.5 Add Signal Handling
- [ ] Handle SIGINT (Ctrl+C) gracefully
- [ ] Handle SIGTERM for clean shutdown
- [ ] Ensure partial downloads are saved
- [ ] Update registry before exit

**Expected Implementation:**
```rust
// In main.rs, for headless mode
use tokio::signal;

// Create a cancellation token
let shutdown = Arc::new(Mutex::new(false));
let shutdown_clone = shutdown.clone();

// Spawn signal handler
tokio::spawn(async move {
    let mut sigint = signal::ctrl_c();
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
        .expect("Failed to setup SIGTERM handler");

    tokio::select! {
        _ = sigint => {
            eprintln!("\nReceived interrupt signal, shutting down gracefully...");
            *shutdown_clone.lock().await = true;
        }
        _ = sigterm.recv() => {
            eprintln!("\nReceived termination signal, shutting down gracefully...");
            *shutdown_clone.lock().await = true;
        }
    }
});

// Pass shutdown flag to download functions
// They should check it periodically and exit cleanly
```

### 4.6 Add Disk Space Checking
- [ ] Check available disk space before download
- [ ] Calculate total size of all queued downloads
- [ ] Fail early if insufficient space
- [ ] Show helpful error message

**Expected Implementation:**
```rust
pub fn check_disk_space(path: &Path, required_bytes: u64) -> Result<(), HeadlessError> {
    // Get filesystem stats
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(path)
            .map_err(|e| map_io_error(&e, &path.to_string_lossy()))?;

        let stat = nix::sys::statvfs::statvfs(path)
            .map_err(|e| HeadlessError::IoError(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))?;

        let available_bytes = stat.blocks_available() * stat.block_size();

        if available_bytes < required_bytes as i64 {
            let available_gb = available_bytes as f64 / 1_073_741_824.0;
            let required_gb = required_bytes as f64 / 1_073_741_824.0;

            return Err(HeadlessError::DownloadError(format!(
                "Insufficient disk space: {:.2} GB required, but only {:.2} GB available at '{}'",
                required_gb, available_gb, path.display()
            )));
        }
    }

    #[cfg(windows)]
    {
        // Windows implementation using GetDiskFreeSpaceEx
        // Simplified - in real code use winapi crate
        // For now, skip check on Windows or use alternative method
    }

    Ok(())
}

// Call before starting downloads
check_disk_space(PathBuf::from(output_dir), total_size)?;
```

### 4.7 Add Validation Functions
- [ ] Validate output directory exists or can be created
- [ ] Validate output directory is writable
- [ ] Validate model_id format
- [ ] Validate quantization type (if specified)

**Expected Implementation:**
```rust
pub fn validate_output_directory(path: &str) -> Result<PathBuf, HeadlessError> {
    let path_buf = PathBuf::from(path);

    // Check if directory exists
    if !path_buf.exists() {
        // Try to create it
        std::fs::create_dir_all(&path_buf)
            .map_err(|e| map_io_error(&e, path))?;
    }

    // Check if it's a directory
    if !path_buf.is_dir() {
        return Err(HeadlessError::DownloadError(format!(
            "Output path exists but is not a directory: {}", path
        )));
    }

    // Check if it's writable
    let test_file = path_buf.join(".write_test");
    std::fs::write(&test_file, b"test")
        .map_err(|_| HeadlessError::DownloadError(format!(
            "Output directory is not writable: {}", path
        )))?;
    std::fs::remove_file(&test_file)
        .map_err(|e| map_io_error(&e, &test_file.to_string_lossy()))?;

    Ok(path_buf)
}

pub fn validate_quantization_type(
    quantization: &str,
    available: &[QuantizationGroup]
) -> Result<(), HeadlessError> {
    let valid_types: Vec<_> = available.iter()
        .map(|q| q.quant_type.as_str())
        .collect();

    if !valid_types.contains(&quantization) {
        return Err(HeadlessError::DownloadError(format!(
            "Invalid quantization type '{}'. Available types: {}",
            quantization,
            valid_types.join(", ")
        )));
    }

    Ok(())
}
```

## Verification Steps

### Progress Reporting Tests
- [ ] Verify progress bar updates smoothly
- [ ] Check ETA calculation is reasonable
- [ ] Test with multiple concurrent downloads
- [ ] Verify JSON output is valid
- [ ] Check text output formatting

### Error Handling Tests
- [ ] Test with invalid model ID
- [ ] Test with non-existent model
- [ ] Test with insufficient disk space
- [ ] Test with read-only output directory
- [ ] Test with invalid token
- [ ] Test with network timeout
- [ ] Verify correct exit codes

### Signal Handling Tests
- [ ] Test Ctrl+C during download
- [ ] Verify partial files are saved
- [ ] Check registry is updated
- [ ] Test SIGTERM if on Unix

### Retry Logic Tests
- [ ] Simulate network failures
- [ ] Verify retry messages are logged
- [ ] Check exponential backoff works
- [ ] Test max retries behavior

## Success Criteria

### Must Have
- ✅ Progress reporting works in both text and JSON modes
- ✅ All error cases handled gracefully
- ✅ Proper exit codes returned
- ✅ Retry logic preserved
- ✅ Signal handling works
- ✅ Disk space checking implemented

### Nice to Have
- Colored output for errors
- Verbose mode with detailed logs
- Progress bar animation
- Resume after Ctrl+C

## Next Phase Link
Once this phase is complete, proceed to **Phase 5: Testing & Documentation** (`add-headless-phase5.md`).

## Notes
- Test error scenarios thoroughly
- Ensure error messages are helpful
- Keep retry logic consistent with TUI mode
- Verify signal handling on all platforms
