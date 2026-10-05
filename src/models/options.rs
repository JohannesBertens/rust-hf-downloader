//! `AppOptions` — the persisted configuration-file schema. Every field and
//! serde attribute here is user-facing config; changes must stay verbatim.

use super::ui::{SortDirection, SortField};
use serde::{Deserialize, Serialize};

// Default value for rate limit (50.0 MB/s)
fn default_rate_limit_mbps() -> f64 {
    50.0
}

/// Application options/settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppOptions {
    // General
    pub default_directory: String,
    pub hf_token: Option<String>,

    // Download Settings
    pub concurrent_threads: usize,
    pub num_chunks: usize,
    pub min_chunk_size: u64,
    pub max_chunk_size: u64,
    pub max_retries: u32,
    pub download_timeout_secs: u64,
    pub retry_delay_secs: u64,
    pub progress_update_interval_ms: u64,

    // Rate Limiting
    #[serde(default)]
    pub download_rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit_mbps")]
    pub download_rate_limit_mbps: f64,

    // Verification Settings
    pub verification_on_completion: bool,
    pub concurrent_verifications: usize,
    pub verification_buffer_size: usize,
    pub verification_update_interval: usize,

    // UI State (not serialized)
    #[serde(skip)]
    pub selected_field: usize,
    #[serde(skip)]
    pub editing_directory: bool,
    #[serde(skip)]
    pub editing_token: bool,

    // Filter & Sort Settings (NEW)
    #[serde(default)]
    pub default_sort_field: SortField,
    #[serde(default)]
    pub default_sort_direction: SortDirection,
    #[serde(default)]
    pub default_min_downloads: u64,
    #[serde(default)]
    pub default_min_likes: u64,
}

impl Default for AppOptions {
    fn default() -> Self {
        let hf_token = std::env::var("HF_TOKEN").ok().filter(|s| !s.is_empty());
        Self {
            default_directory: crate::paths::default_download_dir()
                .to_string_lossy()
                .into_owned(),
            hf_token,
            concurrent_threads: 8,
            num_chunks: 20,
            min_chunk_size: 5 * 1024 * 1024,
            max_chunk_size: 100 * 1024 * 1024,
            max_retries: 5,
            download_timeout_secs: 300,
            retry_delay_secs: 1,
            progress_update_interval_ms: 200,
            download_rate_limit_enabled: false,
            download_rate_limit_mbps: 50.0,
            verification_on_completion: true,
            concurrent_verifications: 4,
            verification_buffer_size: 1024 * 1024,
            verification_update_interval: 100,
            selected_field: 0,
            editing_directory: false,
            editing_token: false,
            // Filter & Sort defaults
            default_sort_field: SortField::Downloads,
            default_sort_direction: SortDirection::Descending,
            default_min_downloads: 0,
            default_min_likes: 0,
        }
    }
}
