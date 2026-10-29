//! API cache containers: the per-endpoint map aliases, the search key, the
//! `ApiCache` aggregate that holds them, and its shared get-or-fetch
//! helper (W4.11). The `CompleteDownloads` mirror used to live here too —
//! M5/U6 moved it to `models/engine.rs` next to `DownloadMetadata` (it is
//! an engine-side completion mirror, not an API cache container).

use super::api::{FlareRepoInfo, ModelInfo, ModelMetadata, QuantizationGroup};
use super::ui::{FileTreeNode, SortDirection, SortField};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type QuantizationCache = HashMap<String, Vec<QuantizationGroup>>;

// Additional cache types for comprehensive API caching
pub type MetadataCache = HashMap<String, ModelMetadata>;
pub type FileTreeCache = HashMap<String, FileTreeNode>;
pub type SearchCache = HashMap<SearchKey, Vec<ModelInfo>>;
/// Assembled FLARE scan results, model-keyed (plans/flare-models.md §3.4:
/// refs + per-revision
/// trees + raw config/publisher — one entry per repo, TTL like the rest).
pub type FlareCache = HashMap<String, FlareRepoInfo>;

/// Search cache key that includes all filter parameters
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SearchKey {
    pub query: String,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
}

/// How long a cached entry counts as fresh before
/// [`ApiCache::get_or_fetch`] re-fetches it. A TUI left open for hours
/// must not serve day-old file trees (a since-removed file 404s
/// confusingly against the stale tree), but must not re-hit the API on
/// every redraw either — 10 minutes is the hub-side staleness window the
/// audit picked.
pub(crate) const API_CACHE_TTL: Duration = Duration::from_secs(60 * 10);

/// The freshness decision for one cached entry, factored out as a pure
/// helper so the TTL boundary is unit-testable.
///
/// Boundary choice (documented): an entry whose age EXACTLY equals the
/// TTL is already STALE (`age < TTL` is fresh) — re-validating one tick
/// early is always safe, serving an over-TTL entry is the bug this fixes.
/// `now` before `fetched_at` cannot happen on a monotonic clock, but the
/// saturating subtraction keeps it fresh instead of panicking.
pub(crate) fn entry_is_fresh(fetched_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(fetched_at) < API_CACHE_TTL
}

/// Unified API cache container for all cached data
#[derive(Debug, Default)]
pub struct ApiCache {
    pub metadata: MetadataCache,
    pub quantizations: QuantizationCache,
    pub file_trees: FileTreeCache,
    pub searches: SearchCache,
    /// Assembled FLARE scan results (plans/flare-models.md §3.4),
    /// model-keyed like the other
    /// value maps.
    pub flare: FlareCache,
    /// Fetch timestamps backing the TTL of [`ApiCache::get_or_fetch`]:
    /// one map per model-keyed value map, keyed identically. An entry
    /// present in its value map WITHOUT a timestamp is treated as STALE
    /// (re-fetched) — simple and safe, because the only writer of the
    /// four value maps is `get_or_fetch` itself, which always stamps.
    /// `searches` is deliberately untimed: it is [`SearchKey`]-keyed and
    /// written directly by `ui/app/search.rs` (its inline Entry insert,
    /// outside this module), so a timestamp map here would have no owner;
    /// search results keep their historical process-lifetime cache.
    pub(crate) metadata_at: HashMap<String, Instant>,
    pub(crate) quantizations_at: HashMap<String, Instant>,
    pub(crate) file_trees_at: HashMap<String, Instant>,
    pub(crate) flare_at: HashMap<String, Instant>,
}

/// Selects which model-keyed map of [`ApiCache`] a
/// [`ApiCache::get_or_fetch`] call targets, keyed by the map's value type
/// (implementation detail of the helper's signature — W4.11).
pub trait CacheMapFor<V> {
    fn cache_map(&self) -> &HashMap<String, V>;
    fn cache_map_mut(&mut self) -> &mut HashMap<String, V>;
    /// The fetch-timestamp map paired with [`Self::cache_map`].
    fn stamp_map(&self) -> &HashMap<String, Instant>;
    fn stamp_map_mut(&mut self) -> &mut HashMap<String, Instant>;
}

impl CacheMapFor<ModelMetadata> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, ModelMetadata> {
        &self.metadata
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, ModelMetadata> {
        &mut self.metadata
    }
    fn stamp_map(&self) -> &HashMap<String, Instant> {
        &self.metadata_at
    }
    fn stamp_map_mut(&mut self) -> &mut HashMap<String, Instant> {
        &mut self.metadata_at
    }
}

impl CacheMapFor<FileTreeNode> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, FileTreeNode> {
        &self.file_trees
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, FileTreeNode> {
        &mut self.file_trees
    }
    fn stamp_map(&self) -> &HashMap<String, Instant> {
        &self.file_trees_at
    }
    fn stamp_map_mut(&mut self) -> &mut HashMap<String, Instant> {
        &mut self.file_trees_at
    }
}

impl CacheMapFor<Vec<QuantizationGroup>> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, Vec<QuantizationGroup>> {
        &self.quantizations
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, Vec<QuantizationGroup>> {
        &mut self.quantizations
    }
    fn stamp_map(&self) -> &HashMap<String, Instant> {
        &self.quantizations_at
    }
    fn stamp_map_mut(&mut self) -> &mut HashMap<String, Instant> {
        &mut self.quantizations_at
    }
}

impl CacheMapFor<FlareRepoInfo> for ApiCache {
    fn cache_map(&self) -> &HashMap<String, FlareRepoInfo> {
        &self.flare
    }
    fn cache_map_mut(&mut self) -> &mut HashMap<String, FlareRepoInfo> {
        &mut self.flare
    }
    fn stamp_map(&self) -> &HashMap<String, Instant> {
        &self.flare_at
    }
    fn stamp_map_mut(&mut self) -> &mut HashMap<String, Instant> {
        &mut self.flare_at
    }
}

impl ApiCache {
    /// Get-or-fetch over one of the model-keyed maps (metadata / file
    /// trees / quantizations) — one helper for the read-check → fetch →
    /// Entry-API-insert pattern the TUI search/prefetch paths repeated six
    /// times (W4.11). Semantics are the historical ones, exactly:
    ///
    /// - fast path: read lock, clone the cached value if present AND
    ///   fresh ([`entry_is_fresh`] on the map's paired timestamp; a
    ///   missing timestamp counts as stale); the lock is dropped BEFORE
    ///   the fetch (no former site held a cache lock across a fetch);
    /// - on a miss (absent, stale, or un-stamped) the fetch runs
    ///   unlocked, so two concurrent callers for the same key may both
    ///   fetch — dedup happens at the write-lock re-check;
    /// - a failed fetch is returned as `Err` and NEVER cached — the next
    ///   call retries (both former sites retried);
    /// - the commit runs under the write lock and re-checks freshness
    ///   first: a racing fetch that already installed a FRESH value wins
    ///   (the historical first-completed-fetch-wins semantics, now
    ///   re-established under the lock); otherwise the fresh fetch
    ///   replaces the stale/absent entry and the TTL clock starts here
    ///   (`Instant::now()`).
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
        // Fast path: read lock only, dropped before the fetch. An entry
        // without a timestamp is stale (see the `ApiCache` field docs).
        let hit = {
            let guard = cache.read();
            Self::cache_map(&guard)
                .get(model_id)
                .zip(Self::stamp_map(&guard).get(model_id))
                .filter(|(_, fetched_at)| entry_is_fresh(**fetched_at, Instant::now()))
                .map(|(value, _)| value.clone())
        };
        if let Some(hit) = hit {
            return Ok(hit);
        }

        let fetched = fetch().await?;

        // Re-check under the write lock: a racing get_or_fetch may have
        // refreshed the key since the fast-path read — a fresh racing
        // value wins (dedup at the commit, as before).
        let mut guard = cache.write();
        let now = Instant::now();
        let raced = Self::cache_map(&guard)
            .get(model_id)
            .zip(Self::stamp_map(&guard).get(model_id))
            .filter(|(_, fetched_at)| entry_is_fresh(**fetched_at, now))
            .map(|(value, _)| value.clone());
        if let Some(raced) = raced {
            return Ok(raced);
        }

        // Absent, stale, or un-stamped: the fresh fetch replaces the
        // entry outright and the TTL clock restarts.
        Self::cache_map_mut(&mut guard).insert(model_id.to_string(), fetched.clone());
        Self::stamp_map_mut(&mut guard).insert(model_id.to_string(), now);
        Ok(fetched)
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

    // ---- TTL (audit fix: a long-open TUI must not serve stale trees) ----

    #[test]
    fn entry_is_fresh_boundary_is_pure_and_documented() {
        let base = Instant::now();
        let at = |secs: u64| {
            base.checked_add(Duration::from_secs(secs))
                .expect("instant arithmetic")
        };
        assert!(entry_is_fresh(base, base), "age 0 is fresh");
        assert!(
            entry_is_fresh(base, at(API_CACHE_TTL.as_secs() - 1)),
            "one tick under the TTL is fresh"
        );
        assert!(
            !entry_is_fresh(base, at(API_CACHE_TTL.as_secs())),
            "exactly at the TTL is stale (documented boundary: re-validate early)"
        );
        assert!(
            !entry_is_fresh(base, at(API_CACHE_TTL.as_secs() + 60)),
            "past the TTL is stale"
        );
        // Monotonic-clock paranoia: `now` before `fetched_at` stays fresh
        // instead of panicking.
        assert!(entry_is_fresh(at(60), base), "clock skew saturates");
    }

    #[tokio::test]
    async fn stale_entry_is_refetched_and_restamped() {
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        // Seed a value map entry WITHOUT a timestamp — the documented
        // stale rule (only get_or_fetch writes the maps, and it always
        // stamps; a timestamp-less entry must not be served).
        {
            let mut guard = cache.write();
            guard
                .file_trees
                .insert("org/model".to_string(), tree("stale-value"));
        }

        let calls = AtomicUsize::new(0);
        let value = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(tree("refetched"))
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1, "stale entry refetches");
        assert_eq!(value.name, "refetched");
        assert_eq!(cache.read().file_trees["org/model"].name, "refetched");
        assert!(
            cache.read().file_trees_at.contains_key("org/model"),
            "the refetch stamps the entry"
        );

        // Fresh now → the next call is a cheap hit.
        let second = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(tree("should-not-appear"))
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1, "fresh entry is a hit");
        assert_eq!(second.name, "refetched");
    }

    #[tokio::test]
    async fn expired_timestamp_is_treated_as_stale() {
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        // An explicitly expired stamp must trigger a refetch. The
        // past-Instant build can fail on a machine that booted less than
        // one TTL ago; then this case adds nothing over
        // `stale_entry_is_refetched_and_restamped`, so it skips.
        let Some(expired) = Instant::now().checked_sub(API_CACHE_TTL + Duration::from_secs(1))
        else {
            return;
        };
        {
            let mut guard = cache.write();
            guard
                .file_trees
                .insert("org/model".to_string(), tree("stale-value"));
            guard.file_trees_at.insert("org/model".to_string(), expired);
        }

        let calls = AtomicUsize::new(0);
        let value = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(tree("refetched"))
        })
        .await
        .unwrap();
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "expired timestamp refetches"
        );
        assert_eq!(value.name, "refetched");
    }

    // ---- FLARE value map (plans/flare-models.md §3.4: same get-or-fetch
    // pattern) ----

    /// Minimal FLARE scan value whose revision count distinguishes
    /// instances.
    fn flare(revisions: usize) -> FlareRepoInfo {
        FlareRepoInfo {
            revisions: (0..revisions)
                .map(|i| crate::models::FlareRevision {
                    revision: format!("gb{i}"),
                    aliases: Vec::new(),
                    kind: crate::models::RefKind::Branch,
                    target_commit: format!("{i:040}"),
                    package: crate::models::FlarePackage::Packed {
                        file: crate::models::RepoFile {
                            rfilename: format!("pkg{i}.safetensors"),
                            size: Some(1),
                            oid: None,
                            lfs: None,
                        },
                    },
                    parsed: crate::models::ParsedPackageTokens::default(),
                })
                .collect(),
            config: crate::models::FlareConfigFacts::default(),
            publisher: None,
            dropped_refs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn flare_map_round_trips_through_get_or_fetch() {
        let cache = Arc::new(parking_lot::RwLock::new(ApiCache::default()));
        let calls = AtomicUsize::new(0);

        let first = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(flare(2))
        })
        .await
        .unwrap();
        let second = ApiCache::get_or_fetch(&cache, "org/model", || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(flare(3))
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::Relaxed), 1, "second call is a hit");
        assert_eq!(first.revisions.len(), 2);
        assert_eq!(second.revisions.len(), 2, "hit returns the stored value");
        assert_eq!(cache.read().flare.len(), 1);
        assert!(cache.read().flare_at.contains_key("org/model"));
    }
}
