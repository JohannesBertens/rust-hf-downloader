//! The HTTP half of [`crate::api`]: every entry point that issues a request
//! against `api_base()`.
//!
//! Moved verbatim from `src/api.rs` (plan W3.2b): search with its client-side
//! filtering/sorting pass, metadata enrichment with the complete recursive
//! tree, revision-to-SHA resolution, and the multipart SHA256 lookup. Bodies,
//! doc comments and request shapes are byte-identical to the originals; only
//! `api_base` is now imported instead of being a sibling item.

use crate::api::api_base;
use crate::models::{ModelFile, ModelInfo, ModelMetadata, RepoFile};
use std::collections::HashMap;

/// Fetch models with sorting and filtering parameters.
///
/// `limit` is clamped to 1..=500 (the API accepts more, but this is a
/// safety bound); `--min-downloads`/`--min-likes` filtering happens client-side
/// because the API does not support those filters.
pub async fn fetch_models_filtered(
    query: &str,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,
    min_likes: u64,
    limit: usize,
    token: Option<&String>,
) -> Result<Vec<ModelInfo>, reqwest::Error> {
    use crate::models::{SortDirection, SortField};

    // Determine if we need client-side sorting
    let needs_client_side_sort =
        matches!(sort_field, SortField::Name) || matches!(sort_direction, SortDirection::Ascending);

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

    let response =
        crate::http_client::get_with_optional_token(&url, token.map(String::as_str)).await?;
    let mut models: Vec<ModelInfo> = response.json().await?;

    // Client-side filtering (API doesn't support these filters)
    models.retain(|m| m.downloads >= min_downloads && m.likes >= min_likes);

    // Client-side sorting when needed
    if needs_client_side_sort {
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

    Ok(models)
}

/// Fetch detailed model metadata from /api/models/{model_id}
pub async fn fetch_model_metadata(
    model_id: &str,
    revision: &str,
    token: Option<&String>,
) -> Result<ModelMetadata, reqwest::Error> {
    let url = format!("{}/api/models/{}", api_base(), model_id);

    let response =
        crate::http_client::get_with_optional_token(&url, token.map(String::as_str)).await?;
    // Surface HTTP errors (unknown repo, auth) as status errors so callers
    // can distinguish not_found/auth from decode failures.
    let response = response.error_for_status()?;
    let mut metadata: ModelMetadata = response.json().await?;

    // Fetch the complete file tree recursively
    let all_files = fetch_recursive_tree(model_id, "", revision, token).await?;

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
    model_id: &str,
    revision: &str,
    token: Option<&String>,
) -> Result<String, reqwest::Error> {
    let url = revision_url(model_id, revision);
    let response =
        crate::http_client::get_with_optional_token(&url, token.map(String::as_str)).await?;
    // Unknown revision → 404 → not_found for the caller (issue #28).
    let response = response.error_for_status()?;
    let info: RevisionInfo = response.json().await?;
    Ok(info.sha)
}

/// Recursively fetch all files from a repository, including subdirectories
fn fetch_recursive_tree<'a>(
    model_id: &'a str,
    path: &'a str,
    revision: &'a str,
    token: Option<&'a String>,
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

        let response =
            crate::http_client::get_with_optional_token(&tree_url, token.map(String::as_str))
                .await?;
        // Unknown revision → 404 → not_found for the caller (issue #28).
        let response = response.error_for_status()?;
        let items: Vec<ModelFile> = response.json().await?;

        let mut all_files = Vec::new();

        for item in items {
            if item.file_type == "directory" {
                // Recursively fetch contents of this directory
                if let Ok(subdir_files) =
                    fetch_recursive_tree(model_id, &item.path, revision, token).await
                {
                    all_files.extend(subdir_files);
                }
            } else {
                // It's a file, add it to the list
                all_files.push(item);
            }
        }

        Ok(all_files)
    })
}

/// Fetch SHA256 hashes for multiple files in a single API call
/// Returns a HashMap mapping filename to its SHA256 hash (if available)
pub async fn fetch_multipart_sha256s(
    model_id: &str,
    revision: &str,
    filenames: &[String],
    token: Option<&String>,
) -> Result<HashMap<String, Option<String>>, reqwest::Error> {
    // Single API call to get all files
    let url = format!("{}/api/models/{}/tree/{}", api_base(), model_id, revision);

    let response =
        crate::http_client::get_with_optional_token(&url, token.map(String::as_str)).await?;
    let files: Vec<ModelFile> = response.json().await?;

    // Create lookup map for fast matching
    let mut sha256_map = HashMap::new();

    for filename in filenames {
        let sha256 = files
            .iter()
            .find(|f| &f.path == filename && f.file_type == "file")
            .and_then(|f| f.lfs.as_ref())
            .map(|lfs| lfs.oid.clone());

        sha256_map.insert(filename.clone(), sha256);
    }

    Ok(sha256_map)
}

#[cfg(test)]
mod tests {
    // URL-shape tests for the private endpoint builders. No network
    // access: the futures are never polled.
    use super::*;
    // `DEFAULT_REVISION` lives in the facade module (`crate::api`).
    use crate::api::DEFAULT_REVISION;

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
}
