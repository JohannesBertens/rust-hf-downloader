//! API cache containers: the per-endpoint map aliases, the search key, and
//! the `ApiCache` aggregate that holds them.

use super::api::{ModelInfo, ModelMetadata, QuantizationGroup};
use super::engine::DownloadMetadata;
use super::ui::{FileTreeNode, SortDirection, SortField};
use std::collections::HashMap;

pub type QuantizationCache = HashMap<String, Vec<QuantizationGroup>>;
pub type CompleteDownloads = HashMap<String, DownloadMetadata>;

// Additional cache types for comprehensive API caching
pub type MetadataCache = HashMap<String, ModelMetadata>;
pub type FileTreeCache = HashMap<String, FileTreeNode>;
pub type SearchCache = HashMap<SearchKey, Vec<ModelInfo>>;

/// Search cache key that includes all filter parameters
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SearchKey {
    pub query: String,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
}

/// Unified API cache container for all cached data
#[derive(Debug, Default)]
pub struct ApiCache {
    pub metadata: MetadataCache,
    pub quantizations: QuantizationCache,
    pub file_trees: FileTreeCache,
    pub searches: SearchCache,
}
