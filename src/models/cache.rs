//! API cache containers: the per-endpoint map aliases, the search key, the
//! `ApiCache` aggregate that holds them, and its shared get-or-fetch
//! helper (W4.11).

use super::api::{ModelInfo, ModelMetadata, QuantizationGroup};
use super::engine::DownloadMetadata;
use super::ui::{FileTreeNode, SortDirection, SortField};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

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

/// Selects which model-keyed map of [`ApiCache`] a
/// [`ApiCache::get_or_fetch`] call targets, keyed by the map's value type
/// (implementation detail of the helper's signature — W4.11).
pub trait CacheMapFor<V> {
    fn cache_map(&self) -> &HashMap<String, V>;
    fn cache_map_mut(&mut self) -> &mut HashMap<String, V>;
}

impl CacheMapFor<ModelMetadata> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, ModelMetadata> {
        &self.metadata
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, ModelMetadata> {
        &mut self.metadata
    }
}

impl CacheMapFor<FileTreeNode> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, FileTreeNode> {
        &self.file_trees
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, FileTreeNode> {
        &mut self.file_trees
    }
}

impl CacheMapFor<Vec<QuantizationGroup>> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, Vec<QuantizationGroup>> {
        &self.quantizations
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, Vec<QuantizationGroup>> {
        &mut self.quantizations
    }
}

impl ApiCache {
    /// Get-or-fetch over one of the model-keyed maps (metadata / file
    /// trees / quantizations) — one helper for the read-check → fetch →
    /// Entry-API-insert pattern the TUI search/prefetch paths repeated six
    /// times (W4.11). Semantics are the historical ones, exactly:
    ///
    /// - fast path: read lock, clone the cached value if present; the
    ///   lock is dropped BEFORE the fetch (no former site held a cache
    ///   lock across a fetch);
    /// - on a miss the fetch runs unlocked, so two concurrent callers for
    ///   the same key may both fetch — dedup happens at insert;
    /// - a failed fetch is returned as `Err` and NEVER cached — the next
    ///   call retries (both former sites retried);
    /// - the insert goes through the Entry API under the write lock: the
    ///   first completed fetch wins and every caller ends up with the
    ///   map's final value for the key.
    pub async fn get_or_fetch<V, E, F, Fut>(
        cache: &Arc<parking_lot::RwLock<Self>>,
        model_id: &str,
        fetch: F,
    ) -> Result<V, E>
    where
        Self: CacheMapFor<V>,
        V: Clone,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        // Fast path: read lock only, dropped before the fetch.
        if let Some(hit) = Self::cache_map(&*cache.read()).get(model_id).cloned() {
            return Ok(hit);
        }

        let fetched = fetch().await?;

        // Get-or-insert under the write lock: a racing caller may have
        // completed the same fetch — whoever inserted first wins.
        let mut guard = cache.write();
        match Self::cache_map_mut(&mut guard).entry(model_id.to_string()) {
            Entry::Occupied(o) => Ok(o.get().clone()),
            Entry::Vacant(v) => {
                v.insert(fetched.clone());
                Ok(fetched)
            }
        }
    }
}

#[cfg(test)]
mod get_or_fetch_tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A minimal file-tree value whose `name` distinguishes instances.
    fn tree(name: &str) -> FileTreeNode {
        FileTreeNode {
            name: name.to_string(),
            path: String::new(),
            is_dir: false,
            size: None,
            children: Vec::new(),
            expanded: false,
            depth: 0,
        }
    }

    #[tokio::test]
    async fn second_call_hits_the_cache_without_refetching() {
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        let calls = AtomicUsize::new(0);

        let first = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(tree("fetched"))
        })
        .await
        .unwrap();
        let second = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(tree("fetched-again"))
        })
        .await
        .unwrap();

        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "second call must not fetch"
        );
        assert_eq!(first.name, "fetched");
        assert_eq!(second.name, "fetched", "cache hit returns the stored value");
        assert_eq!(cache.read().file_trees.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_duplicate_fetch_inserts_once_and_agrees() {
        // Today's semantics, pinned: the cache lock is NOT held during the
        // fetch, so two concurrent callers for the same key may both run
        // the fetch (asserted: 2 invocations — the barrier holds both
        // inside fetch simultaneously, proving no serialization). The
        // Entry insert dedups: exactly one map entry, and BOTH callers
        // observe the first-inserted value.
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let calls = AtomicUsize::new(0);

        let (b1, c1) = (Arc::clone(&barrier), &calls);
        let f1 = ApiCache::get_or_fetch(&cache, "org/model", move || async move {
            let n = c1.fetch_add(1, Ordering::Relaxed);
            b1.wait().await;
            Ok::<_, Infallible>(tree(&format!("fetcher-{n}")))
        });
        let (b2, c2) = (barrier, &calls);
        let f2 = ApiCache::get_or_fetch(&cache, "org/model", move || async move {
            let n = c2.fetch_add(1, Ordering::Relaxed);
            b2.wait().await;
            Ok::<_, Infallible>(tree(&format!("fetcher-{n}")))
        });

        let (r1, r2) = tokio::join!(f1, f2);
        let (r1, r2) = (r1.unwrap(), r2.unwrap());

        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "duplicate fetch is allowed"
        );
        assert_eq!(r1.name, r2.name, "both callers see the map's final value");
        assert_eq!(
            cache.read().file_trees.len(),
            1,
            "insert happens exactly once"
        );
        assert_eq!(
            cache.read().file_trees["org/model"].name,
            r1.name,
            "the first-inserted fetch wins"
        );
    }

    #[tokio::test]
    async fn failed_fetch_is_not_cached_and_is_retried() {
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        let attempts = AtomicUsize::new(0);

        let err = ApiCache::get_or_fetch(&cache, "org/model", || async {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err::<FileTreeNode, _>("network down".to_string())
        })
        .await;
        assert_eq!(err.unwrap_err(), "network down");
        assert!(
            cache.read().file_trees.is_empty(),
            "failures are never cached"
        );

        let ok = ApiCache::get_or_fetch(&cache, "org/model", || async {
            attempts.fetch_add(1, Ordering::Relaxed);
            Ok::<_, String>(tree("recovered"))
        })
        .await
        .unwrap();
        assert_eq!(ok.name, "recovered");
        assert_eq!(attempts.load(Ordering::Relaxed), 2, "the next call retries");
    }
}
