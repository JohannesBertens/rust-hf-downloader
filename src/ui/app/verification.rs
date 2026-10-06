//! Verification pane: drains verify outcomes into the UI progress views.
use super::state::App;
use crate::models::*;

impl App {
    /// Manually verify a downloaded file's SHA256 hash
    pub async fn verify_downloaded_file(&mut self) {
        let models = self.models.read().clone();
        let quant_groups = self.quantizations.read().clone();
        let complete_downloads = self.engine.complete_downloads.lock().await.clone();

        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();

        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quant_groups.len() {
                let group = &quant_groups[quant_idx];
                let quant = match self.focused_pane {
                    FocusedPane::QuantizationFiles => {
                        let file_idx = self.quant_file_list_state.selected().unwrap_or(0);
                        match group.files.get(file_idx) {
                            Some(quant) => quant,
                            None => return,
                        }
                    }
                    _ => match group.files.first() {
                        Some(quant) => quant,
                        None => return,
                    },
                };

                // Check if file is marked as downloaded
                if !complete_downloads.contains_key(&quant.filename) {
                    *self.status.write() =
                        format!("File {} is not marked as downloaded", quant.filename);
                    return;
                }

                // Get the metadata to find local path and expected hash
                let metadata = match complete_downloads.get(&quant.filename) {
                    Some(m) => m,
                    None => {
                        *self.status.write() =
                            format!("Could not find metadata for {}", quant.filename);
                        return;
                    }
                };

                // Check if we have expected hash
                let expected_hash = match &metadata.expected_sha256 {
                    Some(hash) => hash.clone(),
                    None => {
                        *self.status.write() = format!(
                            "No SHA256 hash available for {}, cannot verify",
                            quant.filename
                        );
                        return;
                    }
                };

                let local_path = std::path::PathBuf::from(&metadata.local_path);

                // Check if file exists
                if !local_path.exists() {
                    *self.status.write() = format!("File not found: {}", local_path.display());
                    *self.error.write() = Some(format!(
                        "File marked as downloaded but not found at {}",
                        local_path.display()
                    ));
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
                    is_manual: true, // Mark as manual
                };

                // Manual verify goes through the hub method — the same
                // queue+counter steps the automatic (post-download) path
                // takes (M3: queue_verification is a VerificationHub
                // method; the engine no longer exposes the raw pair).
                self.engine
                    .verification_hub()
                    .queue_verification(item)
                    .await;

                *self.status.write() = format!("Queued {} for verification", quant.filename);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DownloadMetadata, DownloadStatus, ModelInfo, QuantizationGroup, QuantizationInfo,
    };

    #[tokio::test]
    async fn verify_downloaded_file_shard_selection_in_quant_files_pane() {
        let mut app = App::new();
        let model_id = "author/model";
        let m = ModelInfo {
            id: model_id.to_string(),
            author: Some("author".to_string()),
            downloads: 0,
            likes: 0,
            tags: Vec::new(),
            last_modified: None,
        };
        *app.models.write() = vec![m];
        app.list_state.select(Some(0));

        let f0 = "author/model-Q4_K_M-00001-of-00003.gguf";
        let f1 = "author/model-Q4_K_M-00002-of-00003.gguf";
        let f2 = "author/model-Q4_K_M-00003-of-00003.gguf";

        let tmp = std::env::temp_dir().join(format!("test-verify-b1-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file2_path = tmp.join(f2);
        if let Some(parent) = file2_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file2_path, b"test content for shard 2").unwrap();

        *app.quantizations.write() = vec![QuantizationGroup {
            quant_type: "Q4_K_M".to_string(),
            files: vec![
                QuantizationInfo {
                    filename: f0.to_string(),
                    size: 100,
                    sha256: Some("sha-0".to_string()),
                    quant_type: "Q4_K_M".to_string(),
                },
                QuantizationInfo {
                    filename: f1.to_string(),
                    size: 100,
                    sha256: Some("sha-1".to_string()),
                    quant_type: "Q4_K_M".to_string(),
                },
                QuantizationInfo {
                    filename: f2.to_string(),
                    size: 100,
                    sha256: Some("sha-2".to_string()),
                    quant_type: "Q4_K_M".to_string(),
                },
            ],
            total_size: 300,
        }];
        app.quant_list_state.select(Some(0));

        {
            let mut complete = app.engine.complete_downloads.lock().await;
            complete.insert(
                f2.to_string(),
                DownloadMetadata {
                    model_id: model_id.to_string(),
                    filename: f2.to_string(),
                    url: String::new(),
                    local_path: file2_path.to_string_lossy().to_string(),
                    total_size: 100,
                    downloaded_size: 100,
                    status: DownloadStatus::Complete,
                    expected_sha256: Some("sha-2".to_string()),
                    revision: None,
                },
            );
        }

        app.focused_pane = crate::models::FocusedPane::QuantizationFiles;
        app.quant_file_list_state.select(Some(2));

        app.verify_downloaded_file().await;

        let queue = app.engine.verification_queue.lock().await;
        assert_eq!(
            queue.len(),
            1,
            "Should queue exactly one item for verification"
        );
        assert_eq!(queue[0].filename, f2);
        assert_eq!(queue[0].expected_sha256, "sha-2");
        assert!(queue[0].is_manual);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
