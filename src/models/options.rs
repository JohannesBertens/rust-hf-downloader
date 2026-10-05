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
            // Filter & Sort defaults
            default_sort_field: SortField::Downloads,
            default_sort_direction: SortDirection::Descending,
            default_min_downloads: 0,
            default_min_likes: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config TOML golden (§8.9): pins the exact persisted schema — every
    /// config key, its type/format, and the field order — so removing the
    /// serde-skipped transient UI state (selected_field / editing_* —
    /// never serialized) and any future schema change are both reviewable
    /// diffs. Values are chosen non-default so value formatting is pinned
    /// too (note `f64` always renders with a decimal point).
    #[test]
    fn appoptions_toml_golden_round_trip() {
        let options = AppOptions {
            default_directory: "/data/models".to_string(),
            hf_token: Some("tok-123".to_string()),
            concurrent_threads: 3,
            num_chunks: 12,
            min_chunk_size: 1024,
            max_chunk_size: 2048,
            max_retries: 2,
            download_timeout_secs: 60,
            retry_delay_secs: 5,
            progress_update_interval_ms: 250,
            download_rate_limit_enabled: true,
            download_rate_limit_mbps: 12.5,
            verification_on_completion: false,
            concurrent_verifications: 2,
            verification_buffer_size: 4096,
            verification_update_interval: 7,
            default_sort_field: SortField::Likes,
            default_sort_direction: SortDirection::Ascending,
            default_min_downloads: 100,
            default_min_likes: 10,
        };

        let toml_string = toml::to_string_pretty(&options).expect("serialize");
        assert_eq!(
            toml_string,
            "\
default_directory = \"/data/models\"\n\
hf_token = \"tok-123\"\n\
concurrent_threads = 3\n\
num_chunks = 12\n\
min_chunk_size = 1024\n\
max_chunk_size = 2048\n\
max_retries = 2\n\
download_timeout_secs = 60\n\
retry_delay_secs = 5\n\
progress_update_interval_ms = 250\n\
download_rate_limit_enabled = true\n\
download_rate_limit_mbps = 12.5\n\
verification_on_completion = false\n\
concurrent_verifications = 2\n\
verification_buffer_size = 4096\n\
verification_update_interval = 7\n\
default_sort_field = \"Likes\"\n\
default_sort_direction = \"Ascending\"\n\
default_min_downloads = 100\n\
default_min_likes = 10\n\
"
        );

        // Round trip: the golden must parse back to the same options.
        let back: AppOptions = toml::from_str(&toml_string).expect("deserialize");
        assert_eq!(back.default_directory, options.default_directory);
        assert_eq!(back.hf_token, options.hf_token);
        assert_eq!(back.default_sort_field, options.default_sort_field);
        assert_eq!(back.default_sort_direction, options.default_sort_direction);
        assert_eq!(
            back.download_rate_limit_mbps,
            options.download_rate_limit_mbps
        );
        assert!(!back.verification_on_completion);
    }
}
