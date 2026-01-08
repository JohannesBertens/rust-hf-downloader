//! Headless mode implementation for CLI-only operation
//! 
//! This module provides functions for running the application without a TUI,
//! suitable for CI/CD automation and scripting.

use crate::models::*;
use crate::api;
use crate::config;
use crate::registry;
use std::path::PathBuf;
use std::io::Write;
use tokio::sync::mpsc;

/// Error type for headless operations
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

/// Type for download messages sent to the download manager
pub type DownloadMessage = (
    String,              // model_id
    String,              // filename
    PathBuf,             // output path
    Option<String>,      // sha256
    Option<String>,      // hf_token
    u64,                 // total_size
);

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
            let sha256 = file.lfs.as_ref().map(|l| l.oid.clone());

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

/// Resume incomplete downloads from registry
pub async fn resume_downloads(
    download_tx: mpsc::UnboundedSender<DownloadMessage>,
    progress_tx: mpsc::UnboundedSender<String>,
) -> Result<Vec<DownloadMetadata>, HeadlessError> {
    let registry = registry::load_registry();
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

/// Progress reporter for console output (text and JSON modes)
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
            print!("\r[{}] {}% ({:.2} MB/s) - {}", bar, percent, speed_mbps, filename);
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

    pub fn report_list_quantizations(&self, quantizations: &[QuantizationGroup], metadata: &ModelMetadata) {
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
                println!("  - {} ({} files, {} MB total)",
                    quant.quant_type,
                    quant.files.len(),
                    quant.total_size / 1_048_576
                );
                for file in &quant.files {
                    println!("      - {} ({} MB)",
                        file.filename,
                        file.size / 1_048_576
                    );
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

    pub fn report_resume(&self, resumed: &[DownloadMetadata]) {
        if self.json_mode {
            let json = serde_json::json!({
                "status": "resumed",
                "count": resumed.len(),
                "downloads": resumed
            });
            println!("{}", json);
        } else {
            if resumed.is_empty() {
                self.report_info("No incomplete downloads to resume");
            } else {
                println!("Resumed {} downloads:", resumed.len());
                for download in resumed {
                    println!("  - {}", download.filename);
                }
            }
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
