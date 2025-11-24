use crate::models::{ChunkProgress, CompleteDownloads, DownloadMetadata, DownloadProgress, DownloadStatus, VerificationQueueItem};
use crate::registry;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicU64, AtomicU32, AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc, Semaphore};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use std::io::SeekFrom;

pub fn sanitize_path_component(component: &str) -> Option<String> {
    // Reject path components that contain path traversal or are invalid
    if component.is_empty() 
        || component == "." 
        || component == ".." 
        || component.contains('/') 
        || component.contains('\\')
        || component.contains('\0') {
        return None;
    }
    
    // Remove any leading/trailing whitespace and dots
    let trimmed = component.trim().trim_start_matches('.').trim_end_matches('.');
    
    if trimmed.is_empty() {
        return None;
    }
    
    Some(trimmed.to_string())
}

pub fn validate_and_sanitize_path(base_path: &str, model_id: &str, filename: &str) -> Result<PathBuf, String> {
    // Validate base path
    let base = PathBuf::from(base_path);
    
    // Canonicalize base path if it exists, otherwise just validate it doesn't contain traversal
    let canonical_base = if base.exists() {
        base.canonicalize().map_err(|e| format!("Invalid base path: {}", e))?
    } else {
        // For non-existent paths, ensure they're absolute or under home/current dir
        if base.is_absolute() {
            base.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Cannot determine current directory: {}", e))?
                .join(&base)
        }
    };
    
    // Validate and sanitize model_id (format: "author/model-name")
    let model_parts: Vec<&str> = model_id.split('/').collect();
    if model_parts.len() != 2 {
        return Err(format!("Invalid model ID format: {}", model_id));
    }
    
    let author = sanitize_path_component(model_parts[0])
        .ok_or_else(|| format!("Invalid author in model ID: {}", model_parts[0]))?;
    let model_name = sanitize_path_component(model_parts[1])
        .ok_or_else(|| format!("Invalid model name in model ID: {}", model_parts[1]))?;
    
    // Validate and sanitize filename - may contain subdirectory (e.g., "Q4_K_M/file.gguf")
    let filename_parts: Vec<&str> = filename.split('/').collect();
    let mut sanitized_filename_parts = Vec::new();
    
    for part in filename_parts {
        let sanitized = sanitize_path_component(part)
            .ok_or_else(|| format!("Invalid filename component: {}", part))?;
        sanitized_filename_parts.push(sanitized);
    }
    
    // Build the final path: base/author/model_name/[subdir/]filename
    let mut final_path = canonical_base.join(&author).join(&model_name);
    for part in sanitized_filename_parts {
        final_path = final_path.join(&part);
    }
    
    // Final safety check: ensure the resulting path is still under the base directory
    if let Ok(canonical_final) = final_path.canonicalize() {
        if !canonical_final.starts_with(&canonical_base) {
            return Err("Path traversal detected: final path escapes base directory".to_string());
        }
    } else {
        // File doesn't exist yet, check parent directories
        let mut check_path = final_path.clone();
        while let Some(parent) = check_path.parent() {
            if parent.exists() {
                if let Ok(canonical_parent) = parent.canonicalize() {
                    if !canonical_parent.starts_with(&canonical_base) {
                        return Err("Path traversal detected: parent path escapes base directory".to_string());
                    }
                }
                break;
            }
            check_path = parent.to_path_buf();
        }
    }
    
    Ok(final_path)
}

pub async fn start_download(
    model_id: String,
    filename: String,
    base_path: PathBuf,
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    status_tx: mpsc::UnboundedSender<String>,
    complete_downloads: Arc<Mutex<CompleteDownloads>>,
    expected_sha256: Option<String>,
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
) {
    // Notify user that download is starting
    let _ = status_tx.send(format!("Starting download: {}", filename));
    
    // Validate filename to prevent path traversal
    let sanitized_filename = {
        let parts: Vec<&str> = filename.split('/').collect();
        let mut sanitized_parts = Vec::new();
        for part in parts {
            match sanitize_path_component(part) {
                Some(p) => sanitized_parts.push(p),
                None => {
                    let _ = status_tx.send(format!("Error: Invalid filename component: {}", part));
                    return;
                }
            }
        }
        sanitized_parts.join("/")
    };
    
    let url = format!("https://huggingface.co/{}/resolve/main/{}", model_id, sanitized_filename);
    
    // Create directory if it doesn't exist
    if let Err(e) = tokio::fs::create_dir_all(&base_path).await {
        let _ = status_tx.send(format!("Error: Failed to create directory: {}", e));
        return;
    }
    
    // Canonicalize base path for safety checks
    let canonical_base = match base_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            let _ = status_tx.send(format!("Error: Cannot canonicalize base path: {}", e));
            return;
        }
    };
    
    // Extract just the filename (last part) for local storage
    // This prevents duplicate quantization folders when the remote file is in "Q2_K_L/model.gguf"
    // but we want to store it locally as "base_path/model.gguf" since base_path already includes the quantization folder
    let local_filename = sanitized_filename.rsplit('/').next().unwrap_or(&sanitized_filename);
    let final_path = canonical_base.join(local_filename);
    
    // Ensure final path is still under base directory
    if let Some(parent) = final_path.parent() {
        if let Ok(canonical_final_parent) = parent.canonicalize() {
            if !canonical_final_parent.starts_with(&canonical_base) {
                let _ = status_tx.send("Error: Path traversal detected".to_string());
                return;
            }
        }
    }
    
    // Construct file paths  
    let incomplete_path = final_path.parent()
        .unwrap_or(&canonical_base)
        .join(format!("{}.incomplete", final_path.file_name().unwrap().to_string_lossy()));
    
    // Create parent directories for the file (in case filename contains subdirectories like "Q4_K_M/file.gguf")
    if let Some(parent) = final_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!("Error: Failed to create parent directory: {}", e));
            return;
        }
    }
    if let Some(parent) = incomplete_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!("Error: Failed to create parent directory for incomplete file: {}", e));
            return;
        }
    }
    
    // Check for incomplete downloads and delete them to restart from beginning
    if incomplete_path.exists() {
        let _ = status_tx.send(format!("Found incomplete download for {}, restarting from beginning", filename));
        if let Err(e) = tokio::fs::remove_file(&incomplete_path).await {
            let _ = status_tx.send(format!("Warning: Failed to delete incomplete file: {}", e));
        }
    }
    
    // Also check for the complete file - if it exists, we're done
    if final_path.exists() {
        let _ = status_tx.send(format!("File {} already exists, skipping download", filename));
        
        // Update registry as complete
        let mut registry = registry::load_registry();
        if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
            entry.status = DownloadStatus::Complete;
            let mut complete = complete_downloads.lock().await;
            complete.insert(filename.clone(), entry.clone());
        }
        registry::save_registry(&registry);
        
        let mut prog = progress.lock().await;
        *prog = None;
        return;
    }
    
    let mut retries = DOWNLOAD_CONFIG.max_retries.load(Ordering::Relaxed);
    
    loop {
        match download_chunked(
            &url,
            &incomplete_path,
            &final_path,
            &progress,
            &model_id,
            &filename,
            &status_tx,
            &expected_sha256,
        ).await {
            Ok((final_size, expected_size, verification_item)) => {
                // Verify the download is complete
                if final_size == expected_size && expected_size > 0 {
                    // Update registry: mark as complete
                    let mut registry = registry::load_registry();
                    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                        entry.status = DownloadStatus::Complete;
                        entry.downloaded_size = final_size;
                        
                        // Update in-memory complete downloads map
                        let mut complete = complete_downloads.lock().await;
                        complete.insert(filename.clone(), entry.clone());
                    }
                    registry::save_registry(&registry);
                    
                    // Queue verification if enabled AND hash is available
                    let verification_enabled = DOWNLOAD_CONFIG.enable_verification.load(Ordering::Relaxed);
                    if verification_enabled {
                        if let Some(item) = verification_item {
                            crate::verification::queue_verification(
                                verification_queue,
                                verification_queue_size,
                                item,
                            ).await;
                            let _ = status_tx.send(format!("Download complete, queued for verification: {}", filename));
                        } else {
                            let _ = status_tx.send(format!("Download complete: {} (no hash available)", filename));
                        }
                    } else {
                        let _ = status_tx.send(format!("Download complete: {}", filename));
                    }
                } else {
                    let _ = status_tx.send(format!("Warning: Download may be incomplete: {} (got {} bytes, expected {})", filename, final_size, expected_size));
                }
                break;
            }
            Err(e) if retries > 0 && is_transient_error(&e) => {
                retries -= 1;
                let _ = status_tx.send(format!("Download interrupted: {}. Retrying ({} left)...", e, retries));
                let retry_delay = DOWNLOAD_CONFIG.retry_delay_secs.load(Ordering::Relaxed);
                tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay)).await;
                
                // Delete incomplete file to restart from beginning
                if incomplete_path.exists() {
                    let _ = tokio::fs::remove_file(&incomplete_path).await;
                }
                continue;
            }
            Err(e) => {
                let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));
                
                // Delete incomplete file
                if incomplete_path.exists() {
                    let _ = tokio::fs::remove_file(&incomplete_path).await;
                }
                
                // Update registry with failed state
                let mut registry = registry::load_registry();
                if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                    entry.status = DownloadStatus::Incomplete;
                    entry.downloaded_size = 0;
                }
                registry::save_registry(&registry);
                
                let mut prog = progress.lock().await;
                *prog = None;
                return;
            }
        }
    }
    
    // Clear progress when done
    let mut prog = progress.lock().await;
    *prog = None;
}

fn is_transient_error(e: &Box<dyn std::error::Error + Send + Sync>) -> bool {
    // Check if error is a reqwest error and if it's a timeout or connection error
    if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_timeout() || reqwest_err.is_connect();
    }
    false
}

// Global download configuration (thread-safe, runtime-modifiable)
pub struct DownloadConfig {
    pub concurrent_threads: AtomicUsize,
    pub target_chunks: AtomicUsize,
    pub min_chunk_size: AtomicU64,
    pub max_chunk_size: AtomicU64,
    pub enable_verification: AtomicBool,
    pub max_retries: AtomicU32,
    pub download_timeout_secs: AtomicU64,
    pub retry_delay_secs: AtomicU64,
    pub progress_update_interval_ms: AtomicU64,
}

impl DownloadConfig {
    pub const fn new() -> Self {
        Self {
            concurrent_threads: AtomicUsize::new(8),
            target_chunks: AtomicUsize::new(20),
            min_chunk_size: AtomicU64::new(5 * 1024 * 1024),
            max_chunk_size: AtomicU64::new(100 * 1024 * 1024),
            enable_verification: AtomicBool::new(true),
            max_retries: AtomicU32::new(5),
            download_timeout_secs: AtomicU64::new(300),
            retry_delay_secs: AtomicU64::new(1),
            progress_update_interval_ms: AtomicU64::new(200),
        }
    }
}

// Global static configuration
pub static DOWNLOAD_CONFIG: DownloadConfig = DownloadConfig::new();

fn calculate_chunk_size(file_size: u64) -> usize {
    let target_chunks = DOWNLOAD_CONFIG.target_chunks.load(Ordering::Relaxed) as u64;
    let min_size = DOWNLOAD_CONFIG.min_chunk_size.load(Ordering::Relaxed);
    let max_size = DOWNLOAD_CONFIG.max_chunk_size.load(Ordering::Relaxed);
    let ideal_size = file_size / target_chunks;
    ideal_size.clamp(min_size, max_size) as usize
}

async fn download_chunked(
    url: &str,
    incomplete_path: &PathBuf,
    final_path: &PathBuf,
    progress: &Arc<Mutex<Option<DownloadProgress>>>,
    model_id: &str,
    filename: &str,
    _status_tx: &mpsc::UnboundedSender<String>,
    expected_sha256: &Option<String>,
) -> Result<(u64, u64, Option<VerificationQueueItem>), Box<dyn std::error::Error + Send + Sync>> {
    let local_path_str = final_path.to_string_lossy().to_string();
    let timeout_secs = DOWNLOAD_CONFIG.download_timeout_secs.load(Ordering::Relaxed);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()?;
    
    // Step 1: Get file size using a range request
    let response = client
        .get(url)
        .header("Range", "bytes=0-0")
        .send()
        .await?
        .error_for_status()?;
    
    let total_size = if let Some(content_range) = response.headers().get("content-range") {
        // Parse "bytes 0-0/TOTAL" to get TOTAL
        if let Ok(range_str) = content_range.to_str() {
            if let Some(total_str) = range_str.split('/').nth(1) {
                total_str.parse::<u64>().unwrap_or(0)
            } else {
                return Err("Invalid Content-Range header".into());
            }
        } else {
            return Err("Invalid Content-Range header encoding".into());
        }
    } else {
        // Fallback: try Content-Length
        response.content_length().unwrap_or(0)
    };
    
    if total_size == 0 {
        return Err("Could not determine file size".into());
    }
    
    // Update metadata entry in registry
    let mut registry = registry::load_registry();
    
    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
        entry.total_size = total_size;
        entry.downloaded_size = 0;
    } else {
        registry.downloads.push(DownloadMetadata {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            url: url.to_string(),
            local_path: local_path_str.clone(),
            total_size,
            downloaded_size: 0,
            status: DownloadStatus::Incomplete,
            expected_sha256: expected_sha256.clone(),
        });
    }
    
    registry::save_registry(&registry);
    
    // Calculate dynamic chunk size based on file size
    let chunk_size = calculate_chunk_size(total_size);
    
    // Initialize progress with chunk tracking
    let num_chunks = total_size.div_ceil(chunk_size as u64) as usize;
    
    {
        let mut prog = progress.lock().await;
        *prog = Some(DownloadProgress {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            downloaded: 0,
            total: total_size,
            speed_mbps: 0.0,
            chunks: Vec::new(), // Chunks will be added dynamically as they start
            verifying: false,
        });
    }
    
    // Step 2: Create the file with proper size
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&incomplete_path)
        .await?;
    
    // Pre-allocate file space (optional, helps with fragmentation)
    file.set_len(total_size).await?;
    drop(file); // Close to allow multiple handles
    
    // Step 3: Download chunks in parallel
    let max_concurrent = DOWNLOAD_CONFIG.concurrent_threads.load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut handles = Vec::new();
    
    // Shared progress tracking
    let progress_downloaded = Arc::new(Mutex::new(0u64));
    let start_time = std::time::Instant::now();
    let last_update_time = Arc::new(Mutex::new(start_time));
    let last_downloaded_bytes = Arc::new(Mutex::new(0u64));
    
    for chunk_id in 0..num_chunks {
        let start = chunk_id as u64 * chunk_size as u64;
        let stop = std::cmp::min(start + chunk_size as u64 - 1, total_size - 1);
        let client = client.clone();
        let url = url.to_string();
        let incomplete_path = incomplete_path.clone();
        let semaphore = semaphore.clone();
        let progress_downloaded = progress_downloaded.clone();
        let progress = progress.clone();
        let last_update_time = last_update_time.clone();
        let last_downloaded_bytes = last_downloaded_bytes.clone();
        
        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            
            let chunk_total = stop - start + 1;
            
            // Add this chunk to active chunks
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    p.chunks.push(ChunkProgress {
                        chunk_id,
                        start,
                        end: stop,
                        downloaded: 0,
                        total: chunk_total,
                        speed_mbps: 0.0,
                        is_active: true,
                    });
                }
            }
            
            let chunk_start_time = std::time::Instant::now();
            let mut chunk_last_update = chunk_start_time;
            let mut chunk_last_bytes = 0u64;
            
            // Download this chunk with progress tracking
            let result = download_chunk_with_progress(
                &client,
                &url,
                &incomplete_path,
                start,
                stop,
                chunk_id,
                &progress,
                &mut chunk_last_update,
                &mut chunk_last_bytes,
                &progress_downloaded,
                &last_update_time,
                &last_downloaded_bytes,
            ).await;
            
            let chunk_size = stop - start + 1;
            
            // Remove this chunk from active list (mark as inactive)
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    if let Some(chunk) = p.chunks.iter_mut().find(|c| c.chunk_id == chunk_id) {
                        chunk.is_active = false;
                        chunk.downloaded = chunk_total;
                    }
                }
            }
            
            // Clean up inactive chunks older than 1 second
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    p.chunks.retain(|c| c.is_active);
                }
            }
            
            result?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(chunk_size)
        });
        
        handles.push(handle);
    }
    
    // Wait for all chunks to complete
    for handle in handles {
        handle.await??;
    }
    
    // Final progress update
    {
        let mut prog = progress.lock().await;
        if let Some(p) = prog.as_mut() {
            p.downloaded = total_size;
        }
    }
    
    // Rename to final path immediately after download completes
    tokio::fs::rename(incomplete_path, final_path).await?;
    
    // Prepare verification data if hash is available
    let verification_item = if let Some(expected_hash) = expected_sha256 {
        Some(VerificationQueueItem {
            filename: filename.to_string(),
            local_path: final_path.to_string_lossy().to_string(),
            expected_sha256: expected_hash.clone(),
            total_size,
            is_manual: false,
        })
    } else {
        None
    };
    
    Ok((total_size, total_size, verification_item))
}

#[allow(clippy::too_many_arguments)]
async fn download_chunk_with_progress(
    client: &reqwest::Client,
    url: &str,
    file_path: &PathBuf,
    start: u64,
    stop: u64,
    chunk_id: usize,
    progress: &Arc<Mutex<Option<DownloadProgress>>>,
    last_update: &mut std::time::Instant,
    last_bytes: &mut u64,
    progress_downloaded: &Arc<Mutex<u64>>,
    last_update_time: &Arc<Mutex<std::time::Instant>>,
    last_downloaded_bytes: &Arc<Mutex<u64>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let range = format!("bytes={}-{}", start, stop);
    
    let response = client
        .get(url)
        .header("Range", range)
        .send()
        .await?
        .error_for_status()?;
    
    let mut chunk_downloaded = 0u64;
    
    // Open file for writing at offset
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(file_path)
        .await?;
    
    file.seek(SeekFrom::Start(start)).await?;
    
    // Stream the response and update progress
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    
    while let Some(item) = stream.next().await {
        let bytes = item?;
        file.write_all(&bytes).await?;
        
        let bytes_len = bytes.len() as u64;
        chunk_downloaded += bytes_len;
        
        // Update total downloaded bytes immediately
        {
            let mut downloaded = progress_downloaded.lock().await;
            *downloaded += bytes_len;
        }
        
        // Update chunk progress and total speed at configured interval
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(*last_update).as_secs_f64();
        let interval_secs = DOWNLOAD_CONFIG.progress_update_interval_ms.load(Ordering::Relaxed) as f64 / 1000.0;
        
        if elapsed >= interval_secs {
            let bytes_since_last = chunk_downloaded - *last_bytes;
            let chunk_speed_mbps = (bytes_since_last as f64 / elapsed) / 1_048_576.0;
            
            // Calculate total download speed
            let mut last_update_global = last_update_time.lock().await;
            let elapsed_global = now.duration_since(*last_update_global).as_secs_f64();
            
            let total_speed_mbps = if elapsed_global >= interval_secs {
                let downloaded = progress_downloaded.lock().await;
                let total_downloaded = *downloaded;
                drop(downloaded);
                
                let mut last_bytes_global = last_downloaded_bytes.lock().await;
                let bytes_since_last_global = total_downloaded - *last_bytes_global;
                let speed = (bytes_since_last_global as f64 / elapsed_global) / 1_048_576.0;
                
                *last_bytes_global = total_downloaded;
                *last_update_global = now;
                
                Some((speed, total_downloaded))
            } else {
                None
            };
            drop(last_update_global);
            
            let mut prog = progress.lock().await;
            if let Some(p) = prog.as_mut() {
                if let Some(chunk) = p.chunks.iter_mut().find(|c| c.chunk_id == chunk_id) {
                    chunk.downloaded = chunk_downloaded;
                    chunk.speed_mbps = chunk_speed_mbps;
                }
                
                // Update total speed and downloaded if calculated
                if let Some((speed, total)) = total_speed_mbps {
                    p.speed_mbps = speed;
                    p.downloaded = total;
                }
            }
            
            *last_update = now;
            *last_bytes = chunk_downloaded;
        }
    }
    
    file.flush().await?;
    
    Ok(())
}
