//! The HTTP half of [`crate::api`]: every entry point that issues a request
//! against `api_base()`.
//!
//! Moved verbatim from `src/api.rs` (plan W3.2b): search with its client-side
//! filtering/sorting pass, metadata enrichment with the complete recursive
//! tree, revision-to-SHA resolution. Bodies,
//! doc comments and request shapes are byte-identical to the originals; only
//! `api_base` is now imported instead of being a sibling item.
//!
//! Client threading (plan M4/B5): every function takes the run's ONE
//! shared `&reqwest::Client` (built by the frontend bootstrap with the
//! token already installed as its default `Authorization` header) —
//! no function here constructs a client, so the token lives in exactly
//! one place and one TLS pool serves the whole run.

use crate::api::api_base;
use crate::models::{ModelFile, ModelInfo, ModelMetadata, RepoFile};

/// Fetch models with sorting and filtering parameters.
///
/// `limit` is clamped to 1..=500 (the API accepts more, but this is a
/// safety bound); `--min-downloads`/`--min-likes` filtering happens client-side
/// because the API does not support those filters.
pub async fn fetch_models_filtered(
    client: &reqwest::Client,
    query: &str,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,
    min_likes: u64,
    limit: usize,
) -> Result<Vec<ModelInfo>, reqwest::Error> {
    use crate::models::SortField;

    // API only reliably supports descending sort (direction=-1)
    // For name or ascending, we'll fetch descending and sort client-side
    let sort = match sort_field {
        SortField::Downloads => "downloads",
        SortField::Likes => "likes",
        SortField::Modified => "lastModified",
        SortField::Name => "downloads", // Use downloads for API, sort by name client-side
    };

    // Always use descending for API call
    let direction = "-1";

    // Request more results since we'll filter client-side
    // Use full=true to get complete metadata including lastModified
    let url = format!(
        "{}/api/models?search={}&limit={}&sort={}&direction={}&full=true",
        api_base(),
        urlencoding::encode(query),
        limit.clamp(1, 500),
        sort,
        direction
    );

    let response = crate::http_client::get_with_optional_token(client, &url).await?;
    let mut models: Vec<ModelInfo> = response.json().await?;

    // Client-side filtering (API doesn't support these filters)
    filter_models(&mut models, min_downloads, min_likes);

    // Client-side sorting when needed
    if needs_client_side_sort(sort_field, sort_direction) {
        sort_models(&mut models, sort_field, sort_direction);
    }

    Ok(models)
}

/// Whether [`fetch_models_filtered`] has to finish the sorting itself.
///
/// The API only reliably sorts descending, so the request always goes out with
/// `direction=-1`; whatever it cannot express — sorting by name, or any
/// ascending request — is ordered client-side instead. Extracted verbatim from
/// `fetch_models_filtered` (plan W3.3); the pinned order tables in this
/// module's tests cover every field/direction combination.
pub(crate) fn needs_client_side_sort(
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
) -> bool {
    use crate::models::{SortDirection, SortField};

    matches!(sort_field, SortField::Name) || matches!(sort_direction, SortDirection::Ascending)
}

/// Client-side filter pass of [`fetch_models_filtered`]: keeps the models whose
/// `downloads` and `likes` reach the thresholds, because the API has no such
/// filters.
///
/// Extracted verbatim (plan W3.3). Both comparisons are inclusive `>=`, and
/// `retain` keeps the survivors in the incoming (API) order.
pub(crate) fn filter_models(models: &mut Vec<ModelInfo>, min_downloads: u64, min_likes: u64) {
    models.retain(|m| m.downloads >= min_downloads && m.likes >= min_likes);
}

/// Client-side sorting pass of [`fetch_models_filtered`]: orders `models` by
/// `sort_field`/`sort_direction`.
///
/// Extracted verbatim (plan W3.3). The sort is STABLE — `sort_by`, never
/// `sort_unstable_by` — so models that compare equal keep their incoming order;
/// `equal_keys_keep_their_incoming_order` and the pinned tables assert exactly
/// that (and were mutation-checked against `sort_unstable_by`). Call it only
/// behind [`needs_client_side_sort`]: descending responses that the API already
/// ordered are deliberately left untouched.
pub(crate) fn sort_models(
    models: &mut [ModelInfo],
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
) {
    use crate::models::{SortDirection, SortField};

    models.sort_by(|a, b| {
        let cmp = match sort_field {
            SortField::Name => a.id.to_lowercase().cmp(&b.id.to_lowercase()),
            SortField::Downloads => a.downloads.cmp(&b.downloads),
            SortField::Likes => a.likes.cmp(&b.likes),
            SortField::Modified => a.last_modified.as_ref().cmp(&b.last_modified.as_ref()),
        };

        match sort_direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

/// Fetch detailed model metadata from /api/models/{model_id}
pub async fn fetch_model_metadata(
    client: &reqwest::Client,
    model_id: &str,
    revision: &str,
) -> Result<ModelMetadata, reqwest::Error> {
    let url = format!("{}/api/models/{}", api_base(), model_id);

    let response = crate::http_client::get_with_optional_token(client, &url).await?;
    // Surface HTTP errors (unknown repo, auth) as status errors so callers
    // can distinguish not_found/auth from decode failures.
    let response = response.error_for_status()?;
    let mut metadata: ModelMetadata = response.json().await?;

    // Fetch the complete file tree recursively
    let all_files = fetch_recursive_tree(client, model_id, "", revision).await?;

    // Convert ModelFile to RepoFile with proper size information
    metadata.siblings = all_files
        .into_iter()
        .map(|f| RepoFile {
            rfilename: f.path,
            size: Some(f.size),
            oid: f.oid,
            lfs: f.lfs,
        })
        .collect();

    Ok(metadata)
}

/// Minimal shape of a `GET /api/models/{id}/revision/{rev}` response. Only
/// the top-level `sha` is consumed; every other field is ignored by serde.
#[derive(serde::Deserialize)]
struct RevisionInfo {
    sha: String,
}

/// Revision info endpoint URL: resolves any branch/tag/commit name to the
/// commit SHA it currently points at. Sibling of
/// [`crate::api::resolve_url`] under the same single-builder convention.
fn revision_url(model_id: &str, revision: &str) -> String {
    format!(
        "{}/api/models/{}/revision/{}",
        api_base(),
        model_id,
        revision
    )
}

/// Resolve a branch/tag/commit name to its commit SHA via
/// `GET {api_base()}/api/models/{model_id}/revision/{revision}`.
///
/// `fetch_model_metadata`'s info call stays revision-less, so this is the
/// authoritative SHA source for pinning snapshots and refs (hf-cache sync).
/// Unknown repos or revisions surface as HTTP 404 status errors so callers
/// can distinguish not_found/auth from decode failures (issue #28).
pub async fn resolve_revision_sha(
    client: &reqwest::Client,
    model_id: &str,
    revision: &str,
) -> Result<String, reqwest::Error> {
    let url = revision_url(model_id, revision);
    let response = crate::http_client::get_with_optional_token(client, &url).await?;
    // Unknown revision → 404 → not_found for the caller (issue #28).
    let response = response.error_for_status()?;
    let info: RevisionInfo = response.json().await?;
    Ok(info.sha)
}

/// Recursively fetch all files from a repository, including subdirectories
fn fetch_recursive_tree<'a>(
    client: &'a reqwest::Client,
    model_id: &'a str,
    path: &'a str,
    revision: &'a str,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<ModelFile>, reqwest::Error>> + Send + 'a>,
> {
    Box::pin(async move {
        let tree_url = if path.is_empty() {
            format!("{}/api/models/{}/tree/{}", api_base(), model_id, revision)
        } else {
            format!(
                "{}/api/models/{}/tree/{}/{}",
                api_base(),
                model_id,
                revision,
                path
            )
        };

        let response = crate::http_client::get_with_optional_token(client, &tree_url).await?;
        // Unknown revision → 404 → not_found for the caller (issue #28).
        let response = response.error_for_status()?;
        let items: Vec<ModelFile> = response.json().await?;

        let mut all_files = Vec::new();

        for item in items {
            if item.file_type == "directory" {
                // Recursively fetch contents of this directory. A failed
                // subdirectory fetch fails the WHOLE tree fetch (plan
                // M4/B3): swallowing it here produced silently truncated
                // trees that later died as misleading selection errors —
                // the caller must see the underlying transport error.
                let subdir_files =
                    fetch_recursive_tree(client, model_id, &item.path, revision).await?;
                all_files.extend(subdir_files);
            } else {
                // It's a file, add it to the list
                all_files.push(item);
            }
        }

        Ok(all_files)
    })
}

#[cfg(test)]
mod tests {
    // URL-shape tests for the private endpoint builders plus the
    // characterization tables for the client-side filter/sort pass. No
    // network access: the futures are never polled.
    use super::*;
    // `DEFAULT_REVISION` lives in the facade module (`crate::api`).
    use crate::api::DEFAULT_REVISION;
    use crate::models::{SortDirection, SortField};

    #[test]
    fn revision_url_targets_revision_info_endpoint() {
        // Same convention as resolve_url tests: expectations are built from
        // api_base() so the test stays correct under HF_ENDPOINT overrides.
        let base = api_base();
        assert_eq!(
            revision_url("a/b", DEFAULT_REVISION),
            format!("{base}/api/models/a/b/revision/main")
        );
        assert_eq!(
            revision_url("a/b", "2.0bpw"),
            format!("{base}/api/models/a/b/revision/2.0bpw")
        );
        // commit SHA revisions use the same shape
        assert_eq!(
            revision_url("a/b", "0123456789abcdef"),
            format!("{base}/api/models/a/b/revision/0123456789abcdef")
        );
    }

    #[test]
    fn revision_info_parses_top_level_sha_and_ignores_the_rest() {
        // Representative (truncated) revision-endpoint payload: serde keeps
        // only the top-level sha; all other fields are ignored.
        let payload = r#"{
            "id": "a/b",
            "sha": "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234",
            "private": false,
            "gated": false,
            "downloads": 1234,
            "likes": 42,
            "tags": ["text-generation"],
            "siblings": [{"rfilename": "config.json"}]
        }"#;
        let info: RevisionInfo = serde_json::from_str(payload).unwrap();
        assert_eq!(info.sha, "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234");
    }

    // ---- client-side filter/sort pass (W3.3) ----
    //
    // Characterization-first (plan H1): `legacy_filter_sort::apply` is the
    // pre-W3.3 body of `fetch_models_filtered` — its
    // `needs_client_side_sort` decision, its `retain` and its `sort_by` —
    // kept verbatim as the test oracle. The pinned tables below are the exact
    // output ORDER that pass produces on `corpus()`, ties included;
    // `filter_and_sort` is the seam that points at whatever implements the
    // pass today, so the same table test re-runs unchanged after the
    // extraction and any divergence shows up as a red assertion.
    //
    // Mutation-checked: swapping the oracle's `sort_by` for `sort_unstable_by`
    // turns both the pinned table and `equal_keys_keep_their_incoming_order`
    // red, so the fixtures pin stability instead of restating it.
    //
    // W3.3 then landed: `filter_and_sort` runs the extracted free functions and
    // the tables re-ran unchanged; the oracle stays as the permanent
    // differential counterpart (same role as `quant::tests::legacy_multipart`),
    // with `extracted_pass_matches_the_legacy_oracle_on_the_matrix` closing the
    // loop over the full field/direction/threshold matrix.

    /// Pre-W3.3 filter+sort pass, verbatim, as the behavioral oracle.
    mod legacy_filter_sort {
        use crate::models::{ModelInfo, SortDirection, SortField};

        pub fn apply(
            models: &mut Vec<ModelInfo>,
            sort_field: SortField,
            sort_direction: SortDirection,
            min_downloads: u64,
            min_likes: u64,
        ) {
            // Determine if we need client-side sorting
            let needs_client_side_sort = matches!(sort_field, SortField::Name)
                || matches!(sort_direction, SortDirection::Ascending);

            // Client-side filtering (API doesn't support these filters)
            models.retain(|m| m.downloads >= min_downloads && m.likes >= min_likes);

            // Client-side sorting when needed
            if needs_client_side_sort {
                models.sort_by(|a, b| {
                    let cmp = match sort_field {
                        SortField::Name => a.id.to_lowercase().cmp(&b.id.to_lowercase()),
                        SortField::Downloads => a.downloads.cmp(&b.downloads),
                        SortField::Likes => a.likes.cmp(&b.likes),
                        SortField::Modified => {
                            a.last_modified.as_ref().cmp(&b.last_modified.as_ref())
                        }
                    };

                    match sort_direction {
                        SortDirection::Ascending => cmp,
                        SortDirection::Descending => cmp.reverse(),
                    }
                });
            }
        }
    }

    /// The seam under test: whatever implements the client-side filter/sort
    /// step of `fetch_models_filtered` right now — since W3.3 the extracted
    /// `filter_models` + `needs_client_side_sort` + `sort_models` triple,
    /// called in the same order and behind the same gate as the real caller.
    /// In W3.3a these tables ran against `legacy_filter_sort::apply` instead
    /// and did not change.
    fn filter_and_sort(
        models: &mut Vec<ModelInfo>,
        sort_field: SortField,
        sort_direction: SortDirection,
        min_downloads: u64,
        min_likes: u64,
    ) {
        filter_models(models, min_downloads, min_likes);
        if needs_client_side_sort(sort_field, sort_direction) {
            sort_models(models, sort_field, sort_direction);
        }
    }

    /// Fixed 26-entry search-result fixture for the pinned order tables.
    ///
    /// Deliberately loaded with TIES, because which of two equal models comes
    /// first is exactly what a stable sort promises and what an extraction must
    /// not change: three rows identical on all three sort keys (`zeta/Alpha`,
    /// `alpha/Beta`, `sierra/k`), more pairs matching on
    /// downloads+likes+modified (`delta/x`/`kilo/s`, `charlie/AAA`/`romeo/l`,
    /// `Foxtrot/z`/`Uniform/i`), large 900/500/400/75 downloads groups, equal
    /// and absent `last_modified` values, and ids that tie under
    /// `to_lowercase()` (`alpha/Beta` vs `Alpha/beta`). 26 entries on purpose:
    /// below ~20 elements `sort_unstable_by` agrees with `sort_by`, which would
    /// make the tie assertions a tautology.
    fn corpus() -> Vec<ModelInfo> {
        [
            ("zeta/Alpha", 500u64, 10u64, Some("2026-01-05")),
            ("alpha/Beta", 500, 10, Some("2026-01-05")),
            ("Alpha/beta", 500, 10, None),
            ("mike/Q4", 500, 40, None),
            ("bravo/two", 400, 10, Some("2026-02-01")),
            ("charlie/AAA", 75, 99, Some("2025-12-31")),
            ("delta/x", 75, 10, Some("2026-03-01")),
            ("echo/y", 75, 10, None),
            ("Foxtrot/z", 900, 0, Some("2026-04-01")),
            ("golf/w", 900, 5, Some("2026-04-01")),
            ("hotel/v", 10, 1, Some("2024-01-01")),
            ("india/u", 0, 0, None),
            ("juliet/t", 500, 10, Some("2026-01-01")),
            ("kilo/s", 75, 10, Some("2026-03-01")),
            ("lima/r", 400, 10, None),
            ("mike/q", 900, 10, Some("2026-04-01")),
            ("november/p", 500, 40, Some("2025-01-01")),
            ("oscar/o", 400, 99, Some("2026-02-01")),
            ("papa/n", 5, 5, Some("2023-06-15")),
            ("quebec/m", 900, 10, None),
            ("romeo/l", 75, 99, Some("2025-12-31")),
            ("sierra/k", 500, 10, Some("2026-01-05")),
            ("tango/j", 400, 10, Some("2026-02-01")),
            ("Uniform/i", 900, 0, Some("2026-04-01")),
            ("victor/h", 10, 1, None),
            ("whiskey/g", 500, 40, Some("2025-01-01")),
        ]
        .into_iter()
        .map(|(id, downloads, likes, last_modified)| ModelInfo {
            id: id.to_string(),
            author: None,
            downloads,
            likes,
            tags: Vec::new(),
            last_modified: last_modified.map(str::to_string),
        })
        .collect()
    }

    /// Minimal single-purpose fixture for the threshold tests.
    fn model(id: &str, downloads: u64, likes: u64) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            author: None,
            downloads,
            likes,
            tags: Vec::new(),
            last_modified: Some("2026-01-01".to_string()),
        }
    }

    fn ids(models: &[ModelInfo]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    /// Pinned output ORDER — ties included — of the client-side filter/sort
    /// pass, recorded from the pre-W3.3 inline implementation running on
    /// `corpus()`.
    ///
    /// Each row states the complete id sequence, so a change in the filter
    /// thresholds, in the `needs_client_side_sort` rule, in any comparator, or
    /// in tie handling fails here. The `* / Descending / 0 / 0` rows are the
    /// historical contract for API-sortable fields: the pass leaves the
    /// incoming (API) order completely untouched — re-sorting it client-side
    /// would be a behavior change.
    #[test]
    fn filter_sort_pass_order_is_pinned_including_ties() {
        // (sort_field, sort_direction, min_downloads, min_likes, expected ids)
        let cases: &[(SortField, SortDirection, u64, u64, &[&str])] = &[
            (
                SortField::Downloads,
                SortDirection::Descending,
                0,
                0,
                &[
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "mike/Q4",
                    "bravo/two",
                    "charlie/AAA",
                    "delta/x",
                    "echo/y",
                    "Foxtrot/z",
                    "golf/w",
                    "hotel/v",
                    "india/u",
                    "juliet/t",
                    "kilo/s",
                    "lima/r",
                    "mike/q",
                    "november/p",
                    "oscar/o",
                    "papa/n",
                    "quebec/m",
                    "romeo/l",
                    "sierra/k",
                    "tango/j",
                    "Uniform/i",
                    "victor/h",
                    "whiskey/g",
                ],
            ),
            (
                SortField::Downloads,
                SortDirection::Ascending,
                0,
                0,
                &[
                    "india/u",
                    "papa/n",
                    "hotel/v",
                    "victor/h",
                    "charlie/AAA",
                    "delta/x",
                    "echo/y",
                    "kilo/s",
                    "romeo/l",
                    "bravo/two",
                    "lima/r",
                    "oscar/o",
                    "tango/j",
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "mike/Q4",
                    "juliet/t",
                    "november/p",
                    "sierra/k",
                    "whiskey/g",
                    "Foxtrot/z",
                    "golf/w",
                    "mike/q",
                    "quebec/m",
                    "Uniform/i",
                ],
            ),
            (
                SortField::Likes,
                SortDirection::Descending,
                0,
                0,
                &[
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "mike/Q4",
                    "bravo/two",
                    "charlie/AAA",
                    "delta/x",
                    "echo/y",
                    "Foxtrot/z",
                    "golf/w",
                    "hotel/v",
                    "india/u",
                    "juliet/t",
                    "kilo/s",
                    "lima/r",
                    "mike/q",
                    "november/p",
                    "oscar/o",
                    "papa/n",
                    "quebec/m",
                    "romeo/l",
                    "sierra/k",
                    "tango/j",
                    "Uniform/i",
                    "victor/h",
                    "whiskey/g",
                ],
            ),
            (
                SortField::Likes,
                SortDirection::Ascending,
                0,
                0,
                &[
                    "Foxtrot/z",
                    "india/u",
                    "Uniform/i",
                    "hotel/v",
                    "victor/h",
                    "golf/w",
                    "papa/n",
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "bravo/two",
                    "delta/x",
                    "echo/y",
                    "juliet/t",
                    "kilo/s",
                    "lima/r",
                    "mike/q",
                    "quebec/m",
                    "sierra/k",
                    "tango/j",
                    "mike/Q4",
                    "november/p",
                    "whiskey/g",
                    "charlie/AAA",
                    "oscar/o",
                    "romeo/l",
                ],
            ),
            (
                SortField::Modified,
                SortDirection::Descending,
                0,
                0,
                &[
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "mike/Q4",
                    "bravo/two",
                    "charlie/AAA",
                    "delta/x",
                    "echo/y",
                    "Foxtrot/z",
                    "golf/w",
                    "hotel/v",
                    "india/u",
                    "juliet/t",
                    "kilo/s",
                    "lima/r",
                    "mike/q",
                    "november/p",
                    "oscar/o",
                    "papa/n",
                    "quebec/m",
                    "romeo/l",
                    "sierra/k",
                    "tango/j",
                    "Uniform/i",
                    "victor/h",
                    "whiskey/g",
                ],
            ),
            (
                SortField::Modified,
                SortDirection::Ascending,
                0,
                0,
                &[
                    "Alpha/beta",
                    "mike/Q4",
                    "echo/y",
                    "india/u",
                    "lima/r",
                    "quebec/m",
                    "victor/h",
                    "papa/n",
                    "hotel/v",
                    "november/p",
                    "whiskey/g",
                    "charlie/AAA",
                    "romeo/l",
                    "juliet/t",
                    "zeta/Alpha",
                    "alpha/Beta",
                    "sierra/k",
                    "bravo/two",
                    "oscar/o",
                    "tango/j",
                    "delta/x",
                    "kilo/s",
                    "Foxtrot/z",
                    "golf/w",
                    "mike/q",
                    "Uniform/i",
                ],
            ),
            (
                SortField::Name,
                SortDirection::Descending,
                0,
                0,
                &[
                    "zeta/Alpha",
                    "whiskey/g",
                    "victor/h",
                    "Uniform/i",
                    "tango/j",
                    "sierra/k",
                    "romeo/l",
                    "quebec/m",
                    "papa/n",
                    "oscar/o",
                    "november/p",
                    "mike/Q4",
                    "mike/q",
                    "lima/r",
                    "kilo/s",
                    "juliet/t",
                    "india/u",
                    "hotel/v",
                    "golf/w",
                    "Foxtrot/z",
                    "echo/y",
                    "delta/x",
                    "charlie/AAA",
                    "bravo/two",
                    "alpha/Beta",
                    "Alpha/beta",
                ],
            ),
            (
                SortField::Name,
                SortDirection::Ascending,
                0,
                0,
                &[
                    "alpha/Beta",
                    "Alpha/beta",
                    "bravo/two",
                    "charlie/AAA",
                    "delta/x",
                    "echo/y",
                    "Foxtrot/z",
                    "golf/w",
                    "hotel/v",
                    "india/u",
                    "juliet/t",
                    "kilo/s",
                    "lima/r",
                    "mike/q",
                    "mike/Q4",
                    "november/p",
                    "oscar/o",
                    "papa/n",
                    "quebec/m",
                    "romeo/l",
                    "sierra/k",
                    "tango/j",
                    "Uniform/i",
                    "victor/h",
                    "whiskey/g",
                    "zeta/Alpha",
                ],
            ),
            (
                SortField::Downloads,
                SortDirection::Descending,
                500,
                10,
                &[
                    "zeta/Alpha",
                    "alpha/Beta",
                    "Alpha/beta",
                    "mike/Q4",
                    "juliet/t",
                    "mike/q",
                    "november/p",
                    "quebec/m",
                    "sierra/k",
                    "whiskey/g",
                ],
            ),
            (
                SortField::Name,
                SortDirection::Ascending,
                400,
                10,
                &[
                    "alpha/Beta",
                    "Alpha/beta",
                    "bravo/two",
                    "juliet/t",
                    "lima/r",
                    "mike/q",
                    "mike/Q4",
                    "november/p",
                    "oscar/o",
                    "quebec/m",
                    "sierra/k",
                    "tango/j",
                    "whiskey/g",
                    "zeta/Alpha",
                ],
            ),
            (
                SortField::Likes,
                SortDirection::Ascending,
                0,
                99,
                &["charlie/AAA", "oscar/o", "romeo/l"],
            ),
            (
                SortField::Modified,
                SortDirection::Ascending,
                1000,
                1000,
                &[],
            ),
        ];
        for (field, dir, min_downloads, min_likes, expected) in cases {
            let mut models = corpus();
            filter_and_sort(&mut models, *field, *dir, *min_downloads, *min_likes);
            assert_eq!(
                ids(&models).as_slice(),
                *expected,
                "order diverged for {field:?}/{dir:?} min_downloads={min_downloads} \
                 min_likes={min_likes}"
            );
        }
    }

    /// The tie guarantee the tables above depend on: models that compare equal
    /// keep their incoming order, because the pass sorts with `sort_by` and
    /// never with `sort_unstable_by`.
    #[test]
    fn equal_keys_keep_their_incoming_order() {
        // Interleaved ties: `downloads` cycles 0..=3 and `likes` cycles 0..=1
        // over 26 entries, so every sorted group is a scramble of the incoming
        // order and the stable result is fully determined. Keys are cycled
        // rather than identical on purpose — pdqsort leaves an all-equal (or
        // already-sorted) slice untouched, so a naive fixture would pass even
        // with an unstable sort; this fixture was verified to fail when the
        // oracle's `sort_by` is swapped for `sort_unstable_by`.
        let models: Vec<ModelInfo> = (0..26)
            .map(|i| model(&format!("m{i:02}"), (i % 4) as u64, (i % 2) as u64))
            .collect();

        // Fully pinned: ascending by downloads, each key group in incoming order.
        let mut downloads = models.clone();
        filter_and_sort(
            &mut downloads,
            SortField::Downloads,
            SortDirection::Ascending,
            0,
            0,
        );
        assert_eq!(
            ids(&downloads),
            [
                "m00", "m04", "m08", "m12", "m16", "m20", "m24", // downloads 0
                "m01", "m05", "m09", "m13", "m17", "m21", "m25", // downloads 1
                "m02", "m06", "m10", "m14", "m18", "m22", // downloads 2
                "m03", "m07", "m11", "m15", "m19", "m23", // downloads 3
            ]
        );

        // Shape rule for every key: non-decreasing key, and strictly
        // increasing id (= incoming order, ids encode the index) inside a tie.
        for field in [SortField::Downloads, SortField::Likes, SortField::Modified] {
            let mut sorted = models.clone();
            filter_and_sort(&mut sorted, field, SortDirection::Ascending, 0, 0);
            let mut prev: Option<(u64, &str)> = None;
            for m in &sorted {
                let key = match field {
                    SortField::Downloads => m.downloads,
                    SortField::Likes => m.likes,
                    // All entries share the fixture's timestamp: every pair is
                    // a tie, so this is the pure stability probe.
                    _ => 0,
                };
                if let Some((prev_key, prev_id)) = prev {
                    assert!(
                        prev_key < key || (prev_key == key && prev_id < m.id.as_str()),
                        "{field:?} ascending broke tie order: {prev_id} before {}",
                        m.id
                    );
                }
                prev = Some((key, &m.id));
            }
        }
    }

    /// Differential test (plan H1): the extracted triple reproduces the
    /// verbatim oracle exactly on the whole (SortField x SortDirection) matrix
    /// crossed with eight threshold pairs, over the tie-loaded corpus, a
    /// partially pre-filtered corpus, a single-element corpus and the empty one
    /// — so the pinned tables are not the only thing standing between a future
    /// edit and a silent order change.
    #[test]
    fn extracted_pass_matches_the_legacy_oracle_on_the_matrix() {
        let fixtures: Vec<Vec<ModelInfo>> = vec![
            Vec::new(),
            vec![model("solo/one", 3, 1)],
            corpus(),
            corpus().into_iter().filter(|m| m.likes >= 10).collect(),
        ];
        let thresholds: [(u64, u64); 8] = [
            (0, 0),
            (10, 1),
            (75, 0),
            (100, 10),
            (400, 10),
            (500, 10),
            (900, 10),
            (1000, 1000),
        ];

        for base in &fixtures {
            for field in [
                SortField::Downloads,
                SortField::Likes,
                SortField::Modified,
                SortField::Name,
            ] {
                for dir in [SortDirection::Ascending, SortDirection::Descending] {
                    for (min_downloads, min_likes) in thresholds {
                        let mut legacy = base.clone();
                        legacy_filter_sort::apply(
                            &mut legacy,
                            field,
                            dir,
                            min_downloads,
                            min_likes,
                        );

                        let mut extracted = base.clone();
                        filter_models(&mut extracted, min_downloads, min_likes);
                        if needs_client_side_sort(field, dir) {
                            sort_models(&mut extracted, field, dir);
                        }

                        let fixture_len = base.len();
                        assert_eq!(
                            ids(&extracted),
                            ids(&legacy),
                            "extracted pass diverged for {field:?}/{dir:?} \
                             min_downloads={min_downloads} min_likes={min_likes} \
                             on a fixture of {fixture_len} entries"
                        );
                    }
                }
            }
        }
    }

    /// Threshold semantics, kept separate from the big table so a drift in the
    /// comparison direction is named explicitly.
    #[test]
    fn thresholds_are_inclusive_on_both_fields() {
        // `downloads >= min_downloads && likes >= min_likes`: a model exactly
        // on both thresholds survives; one short on either side does not.
        let mut models = vec![
            model("on/both", 10, 5),
            model("short/on/likes", 10, 4),
            model("short/on/downloads", 9, 5),
        ];
        filter_and_sort(
            &mut models,
            SortField::Name,
            SortDirection::Descending,
            10,
            5,
        );
        assert_eq!(ids(&models), ["on/both"]);
    }
}
