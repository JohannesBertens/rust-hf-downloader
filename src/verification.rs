use crate::models::{DownloadRegistry, DownloadStatus, VerificationProgress, VerificationQueueItem};
use sha2::{Sha256, Digest};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, mpsc, Semaphore};
use tokio::io::AsyncReadExt;

/// Global verification configuration (thread-safe, runtime-modifiable)
pub struct VerificationConfig {
    pub concurrent_verifications: AtomicUsize,
    pub buffer_size: AtomicUsize,
    pub update_interval_iterations: AtomicUsize,
}

impl VerificationConfig {
    pub const fn new() -> Self {
        Self {
            concurrent_verifications: AtomicUsize::new(2),
            buffer_size: AtomicUsize::new(128 * 1024),
            update_interval_iterations: AtomicUsize::new(100),
        }
    }
}

pub static VERIFICATION_CONFIG: VerificationConfig = VerificationConfig::new();

/// Main verification worker that processes the verification queue
/// Runs continuously in the background, processing items as they arrive
pub async fn verification_worker(
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    status_tx: mpsc::UnboundedSender<String>,
    download_registry: Arc<Mutex<DownloadRegistry>>,
) {
    let max_concurrent = VERIFICATION_CONFIG.concurrent_verifications.load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    
    loop {
        // Check if there's work to do
        let item = {
            let mut queue = verification_queue.lock().await;
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        };
        
        if let Some(item) = item {
            // Decrement queue size
            {
                let mut queue_size = verification_queue_size.lock().await;
                *queue_size = queue_size.saturating_sub(1);
            }
            
            // Spawn verification task
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let verification_progress = verification_progress.clone();
            let status_tx = status_tx.clone();
            let download_registry = download_registry.clone();
            
            tokio::spawn(async move {
                verify_file(item, verification_progress, status_tx, download_registry).await;
                drop(permit);
            });
        } else {
            // No work, sleep briefly
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

/// Verify a single file's SHA256 hash
async fn verify_file(
    item: VerificationQueueItem,
    verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    status_tx: mpsc::UnboundedSender<String>,
    download_registry: Arc<Mutex<DownloadRegistry>>,
) {
    let local_path = PathBuf::from(&item.local_path);
    
    // Check if file exists
    if !local_path.exists() {
        let _ = status_tx.send(format!("Error: Cannot verify {}, file not found", item.filename));
        return;
    }
    
    // Add to active verifications
    {
        let mut progress = verification_progress.lock().await;
        progress.push(VerificationProgress {
            filename: item.filename.clone(),
            local_path: item.local_path.clone(),
            verified_bytes: 0,
            total_bytes: item.total_size,
            speed_mbps: 0.0,
        });
    }
    
    let _ = status_tx.send(format!("Verifying integrity of {}...", item.filename));
    
    // Calculate hash with progress tracking (use filename as identifier)
    match calculate_sha256_with_progress(&local_path, &verification_progress, &item.filename, item.total_size).await {
        Ok(calculated_hash) => {
            if calculated_hash == item.expected_sha256 {
                let _ = status_tx.send(format!("✓ Hash verified for {}", item.filename));
            } else {
                let _ = status_tx.send(format!(
                    "✗ Hash mismatch for {}: expected {}..., got {}...",
                    item.filename,
                    &item.expected_sha256[..16],
                    &calculated_hash[..16]
                ));
                
                // Update registry to HashMismatch
                let mut registry = download_registry.lock().await;
                if let Some(entry) = registry.downloads.iter_mut().find(|d| d.local_path == item.local_path) {
                    entry.status = DownloadStatus::HashMismatch;
                }
                crate::registry::save_registry(&registry);
            }
        }
        Err(e) => {
            let _ = status_tx.send(format!("Warning: Failed to verify {}: {}", item.filename, e));
        }
    }
    
    // Remove from active verifications
    {
        let mut progress = verification_progress.lock().await;
        progress.retain(|p| p.filename != item.filename);
    }
}

/// Calculate SHA256 hash of a file with progress tracking
async fn calculate_sha256_with_progress(
    file_path: &Path,
    verification_progress: &Arc<Mutex<Vec<VerificationProgress>>>,
    filename: &str,
    total_size: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = tokio::fs::File::open(file_path).await?;
    let mut hasher = Sha256::new();
    let buffer_size = VERIFICATION_CONFIG.buffer_size.load(Ordering::Relaxed);
    let mut buffer = vec![0u8; buffer_size];
    
    let mut bytes_verified = 0u64;
    let mut iteration = 0u64;
    let start_time = std::time::Instant::now();
    let mut last_update = start_time;
    let mut last_bytes = 0u64;
    
    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        
        bytes_verified += bytes_read as u64;
        iteration += 1;
        
        // Update progress at configured interval to avoid excessive mutex locks
        let update_interval = VERIFICATION_CONFIG.update_interval_iterations.load(Ordering::Relaxed);
        if iteration % update_interval as u64 == 0 || bytes_verified >= total_size {
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(last_update).as_secs_f64();
            
            if elapsed >= 0.2 {
                let bytes_since_last = bytes_verified - last_bytes;
                let speed = (bytes_since_last as f64 / elapsed) / 1_048_576.0;
                
                // Find and update progress by filename (not index)
                let mut progress = verification_progress.lock().await;
                if let Some(entry) = progress.iter_mut().find(|p| p.filename == filename) {
                    entry.verified_bytes = bytes_verified;
                    entry.speed_mbps = speed;
                }
                
                last_update = now;
                last_bytes = bytes_verified;
            }
        }
    }
    
    // Final progress update to ensure 100%
    {
        let mut progress = verification_progress.lock().await;
        if let Some(entry) = progress.iter_mut().find(|p| p.filename == filename) {
            entry.verified_bytes = total_size;
        }
    }
    
    Ok(hex::encode(hasher.finalize()))
}

/// Queue a file for verification
pub async fn queue_verification(
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    item: VerificationQueueItem,
) {
    let mut queue = verification_queue.lock().await;
    queue.push(item);
    
    let mut queue_size = verification_queue_size.lock().await;
    *queue_size += 1;
}
