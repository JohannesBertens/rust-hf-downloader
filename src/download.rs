use crate::models::{CompleteDownloads, DownloadMetadata, DownloadProgress, DownloadStatus};
use crate::registry;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

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
) {
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
    
    let final_path = canonical_base.join(&sanitized_filename);
    
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
    
    // Load registry to check existing metadata
    let mut registry = registry::load_registry();
    
    // Find or create metadata entry
    let metadata_entry = registry.downloads.iter_mut()
        .find(|d| d.url == url);
    
    let resume_from = if let Some(entry) = metadata_entry {
        // Check if file actually exists and size matches
        if incomplete_path.exists() {
            if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                let size = metadata.len();
                entry.downloaded_size = size;
                let _ = status_tx.send(format!("Resuming {} from {} bytes", filename, size));
                size
            } else {
                0
            }
        } else {
            entry.downloaded_size
        }
    } else {
        0
    };
    
    const MAX_RETRIES: u32 = 5;
    let mut retries = MAX_RETRIES;
    let mut current_resume_from = resume_from;
    
    loop {
        match download_with_resume(
            &url,
            &incomplete_path,
            &final_path,
            current_resume_from,
            &progress,
            &model_id,
            &filename,
            &status_tx,
        ).await {
            Ok((final_size, expected_size)) => {
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
                    let _ = status_tx.send(format!("Download complete: {} ({} bytes)", filename, final_size));
                } else {
                    let _ = status_tx.send(format!("Warning: Download may be incomplete: {} (got {} bytes, expected {})", filename, final_size, expected_size));
                }
                break;
            }
            Err(e) if retries > 0 && is_transient_error(&e) => {
                retries -= 1;
                let _ = status_tx.send(format!("Download interrupted: {}. Retrying ({} left)...", e, retries));
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                
                // Update current position from incomplete file and save to registry
                if incomplete_path.exists() {
                    if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                        current_resume_from = metadata.len();
                        
                        // Update registry
                        let mut registry = registry::load_registry();
                        if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                            entry.downloaded_size = current_resume_from;
                        }
                        registry::save_registry(&registry);
                    }
                }
                continue;
            }
            Err(e) => {
                let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));
                
                // Update registry with current state
                let mut registry = registry::load_registry();
                if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                    entry.status = DownloadStatus::Incomplete;
                    if incomplete_path.exists() {
                        if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                            entry.downloaded_size = metadata.len();
                        }
                    }
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

async fn download_with_resume(
    url: &str,
    incomplete_path: &PathBuf,
    final_path: &PathBuf,
    resume_from: u64,
    progress: &Arc<Mutex<Option<DownloadProgress>>>,
    model_id: &str,
    filename: &str,
    _status_tx: &mpsc::UnboundedSender<String>,
) -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let local_path_str = final_path.to_string_lossy().to_string();
    let client = reqwest::Client::new();
    
    // Build request with Range header if resuming
    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header("Range", format!("bytes={}-", resume_from));
    }
    
    let response = request.send().await?;
    
    // Get total size from Content-Length or Content-Range
    let total_size = if let Some(content_range) = response.headers().get("content-range") {
        // Parse "bytes X-Y/Z" to get Z (total size)
        if let Ok(range_str) = content_range.to_str() {
            if let Some(total_str) = range_str.split('/').nth(1) {
                total_str.parse::<u64>().unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        }
    } else {
        response.content_length().unwrap_or(0) + resume_from
    };
    
    // Update metadata entry in registry with total_size
    let mut registry = registry::load_registry();
    
    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
        // Update existing entry with total size
        entry.total_size = total_size;
        entry.downloaded_size = resume_from;
    } else {
        // Create new entry if it doesn't exist (shouldn't happen but be defensive)
        registry.downloads.push(DownloadMetadata {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            url: url.to_string(),
            local_path: local_path_str.clone(),
            total_size,
            downloaded_size: resume_from,
            status: DownloadStatus::Incomplete,
        });
    }
    
    registry::save_registry(&registry);
    
    // Initialize progress
    {
        let mut prog = progress.lock().await;
        *prog = Some(DownloadProgress {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            downloaded: resume_from,
            total: total_size,
            speed_mbps: 0.0,
        });
    }
    
    // Open file in append mode
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&incomplete_path)
        .await?;
    
    let mut downloaded: u64 = resume_from;
    let mut stream = response.bytes_stream();
    
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    
    // For speed calculation
    let start_time = std::time::Instant::now();
    let mut last_update = start_time;
    let mut last_downloaded = resume_from;
    
    while let Some(item) = stream.next().await {
        let chunk = item?;
        
        file.write_all(&chunk).await?;
        
        downloaded += chunk.len() as u64;
        
        // Update progress and calculate speed every 500ms
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_update).as_secs_f64();
        
        if elapsed >= 0.5 {
            let bytes_since_last = downloaded - last_downloaded;
            let speed_mbps = (bytes_since_last as f64 / elapsed) / 1_048_576.0; // Convert to MB/s
            
            let mut prog = progress.lock().await;
            if let Some(p) = prog.as_mut() {
                p.downloaded = downloaded;
                p.speed_mbps = speed_mbps;
            }
            
            last_update = now;
            last_downloaded = downloaded;
        }
    }
    
    // Flush and sync
    file.flush().await?;
    file.sync_all().await?;
    
    // Rename to final path on successful completion
    tokio::fs::rename(incomplete_path, final_path).await?;
    
    Ok((downloaded, total_size))
}
