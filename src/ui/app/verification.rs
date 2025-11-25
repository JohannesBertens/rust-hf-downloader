use super::state::App;
use crate::models::*;

impl App {
    /// Manually verify a downloaded file's SHA256 hash
    pub async fn verify_downloaded_file(&mut self) {
        let models = self.models.lock().await.clone();
        let quantizations = self.quantizations.lock().await.clone();
        let complete_downloads = self.complete_downloads.lock().await.clone();
        
        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();
        
        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quantizations.len() {
                let quant = &quantizations[quant_idx];
                
                // Check if file is marked as downloaded
                if !complete_downloads.contains_key(&quant.filename) {
                    self.status = format!("File {} is not marked as downloaded", quant.filename);
                    return;
                }
                
                // Get the metadata to find local path and expected hash
                let metadata = match complete_downloads.get(&quant.filename) {
                    Some(m) => m,
                    None => {
                        self.status = format!("Could not find metadata for {}", quant.filename);
                        return;
                    }
                };
                
                // Check if we have expected hash
                let expected_hash = match &metadata.expected_sha256 {
                    Some(hash) => hash.clone(),
                    None => {
                        self.status = format!("No SHA256 hash available for {}, cannot verify", quant.filename);
                        return;
                    }
                };
                
                let local_path = std::path::PathBuf::from(&metadata.local_path);
                
                // Check if file exists
                if !local_path.exists() {
                    self.status = format!("File not found: {}", local_path.display());
                    self.error = Some(format!("File marked as downloaded but not found at {}", local_path.display()));
                    return;
                }
                
                // Get file size for progress tracking
                let file_size = match tokio::fs::metadata(&local_path).await {
                    Ok(metadata) => metadata.len(),
                    Err(_) => 0,
                };
                
                // Queue verification item (ALWAYS queue, ignoring ENABLE_DOWNLOAD_VERIFICATION)
                let item = VerificationQueueItem {
                    filename: quant.filename.clone(),
                    local_path: local_path.to_string_lossy().to_string(),
                    expected_sha256: expected_hash,
                    total_size: file_size,
                    is_manual: true,  // Mark as manual
                };
                
                crate::verification::queue_verification(
                    self.verification_queue.clone(),
                    self.verification_queue_size.clone(),
                    item,
                ).await;
                
                self.status = format!("Queued {} for verification", quant.filename);
            }
        }
    }
}
