use crate::models::{
    FileTreeNode, ModelFile, ModelInfo, ModelMetadata, QuantizationGroup, QuantizationInfo,
    RepoFile,
};
use std::collections::HashMap;

/// Base URL for all HuggingFace Hub requests.
///
/// Overridable via the `HF_ENDPOINT` environment variable (same convention
/// as `huggingface_hub`), which enables mirror support (e.g.
/// `HF_ENDPOINT=https://hf-mirror.com`) and hermetic integration tests
/// against a local mock server.
pub fn api_base() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Canonical `resolve` download URL for a repo file. Used by the download
/// engine, the registry bookkeeping in both frontends, and the CLI — keeping
/// one builder guarantees the URLs always match.
pub fn resolve_url(model_id: &str, filename: &str) -> String {
    format!("{}/{}/resolve/main/{}", api_base(), model_id, filename)
}

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

    let response = crate::http_client::get_with_optional_token(&url, token).await?;
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
    token: Option<&String>,
) -> Result<ModelMetadata, reqwest::Error> {
    let url = format!("{}/api/models/{}", api_base(), model_id);

    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let mut metadata: ModelMetadata = response.json().await?;

    // Fetch the complete file tree recursively
    let all_files = fetch_recursive_tree(model_id, "", token).await?;

    // Convert ModelFile to RepoFile with proper size information
    metadata.siblings = all_files
        .into_iter()
        .map(|f| RepoFile {
            rfilename: f.path,
            size: Some(f.size),
            lfs: f.lfs,
        })
        .collect();

    Ok(metadata)
}

/// Recursively fetch all files from a repository, including subdirectories
fn fetch_recursive_tree<'a>(
    model_id: &'a str,
    path: &'a str,
    token: Option<&'a String>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<ModelFile>, reqwest::Error>> + Send + 'a>,
> {
    Box::pin(async move {
        let tree_url = if path.is_empty() {
            format!("{}/api/models/{}/tree/main", api_base(), model_id)
        } else {
            format!("{}/api/models/{}/tree/main/{}", api_base(), model_id, path)
        };

        let response = crate::http_client::get_with_optional_token(&tree_url, token).await?;
        let items: Vec<ModelFile> = response.json().await?;

        let mut all_files = Vec::new();

        for item in items {
            if item.file_type == "directory" {
                // Recursively fetch contents of this directory
                if let Ok(subdir_files) = fetch_recursive_tree(model_id, &item.path, token).await {
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

/// Check if model has GGUF files
#[cfg_attr(not(test), allow(dead_code))]
pub fn has_gguf_files(metadata: &ModelMetadata) -> bool {
    metadata
        .siblings
        .iter()
        .any(|file| file.rfilename.ends_with(".gguf") || file.rfilename.contains(".gguf.part"))
}

/// Build tree structure from flat file list
pub fn build_file_tree(files: Vec<RepoFile>) -> FileTreeNode {
    let mut root = FileTreeNode {
        name: String::new(),
        path: String::new(),
        is_dir: true,
        size: None,
        children: Vec::new(),
        expanded: true, // Root is always expanded
        depth: 0,
    };

    for file in files {
        let parts: Vec<&str> = file.rfilename.split('/').collect();
        insert_into_tree(&mut root, &parts, 0, &file);
    }

    // Sort children at each level (directories first, then alphabetically)
    sort_tree_recursive(&mut root);

    // Calculate directory sizes (sum of all files within)
    calculate_directory_sizes(&mut root);

    root
}

/// Calculate total size for each directory recursively
fn calculate_directory_sizes(node: &mut FileTreeNode) -> u64 {
    if node.is_dir {
        let total: u64 = node
            .children
            .iter_mut()
            .map(calculate_directory_sizes)
            .sum();
        node.size = Some(total);
        total
    } else {
        node.size.unwrap_or(0)
    }
}

fn insert_into_tree(node: &mut FileTreeNode, parts: &[&str], depth: usize, file: &RepoFile) {
    if parts.is_empty() {
        return;
    }

    let current_part = parts[0];
    let is_last = parts.len() == 1;

    // Find or create child node
    let child_pos = node
        .children
        .iter()
        .position(|child| child.name == current_part);

    let child = if let Some(pos) = child_pos {
        &mut node.children[pos]
    } else {
        let new_node = FileTreeNode {
            name: current_part.to_string(),
            path: if node.path.is_empty() {
                current_part.to_string()
            } else {
                format!("{}/{}", node.path, current_part)
            },
            is_dir: !is_last,
            size: if is_last { file.size } else { None },
            children: Vec::new(),
            expanded: false,
            depth: depth + 1,
        };
        node.children.push(new_node);
        node.children.last_mut().unwrap()
    };

    if !is_last {
        insert_into_tree(child, &parts[1..], depth + 1, file);
    }
}

fn sort_tree_recursive(node: &mut FileTreeNode) {
    node.children.sort_by(|a, b| {
        // Directories before files
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    for child in &mut node.children {
        sort_tree_recursive(child);
    }
}

/// Fetch and classify a model's GGUF files into quantization groups.
///
/// Compat wrapper: frontends that already hold `ModelMetadata` should call
/// [`classify_quantizations`] directly over `metadata.siblings` to avoid the
/// extra API round-trip.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn fetch_model_files(
    model_id: &str,
    token: Option<&String>,
) -> Result<Vec<QuantizationGroup>, reqwest::Error> {
    // Thin wrapper kept for API compatibility: classification is a pure
    // function over the full recursive tree (see `classify_quantizations`),
    // so a single metadata fetch is all we need.
    let metadata = fetch_model_metadata(model_id, token).await?;
    Ok(classify_quantizations(&metadata.siblings))
}

/// Quantization group for GGUF files whose layout we don't recognize.
/// These files used to be silently dropped (issue #25); they now stay
/// visible and downloadable. Always sorted last.
pub const OTHER_QUANT_TYPE: &str = "OTHER";

/// Quantization group prefix for multimodal projector files (`mmproj-*`),
/// which are companions to (not part of) the model weights and must never
/// be mixed into a weight quantization group (issue #25).
pub const MMPROJ_QUANT_TYPE: &str = "MMPROJ";

/// Is this repo path a GGUF-family file (single or multipart)?
fn is_gguf_path(path: &str) -> bool {
    path.ends_with(".gguf") || path.contains(".gguf.part")
}

/// Basename with the `.gguf` extension and any multipart suffix removed —
/// the string quantization hints live in.
/// `Dynamic/model.Q4_K_M-00001-of-00002.gguf` -> `model.Q4_K_M`.
fn classification_stem(path: &str) -> String {
    let basename = path.rsplit('/').next().unwrap_or(path);
    let base = get_multipart_base_name(basename);
    base.strip_suffix(".gguf").unwrap_or(&base).to_string()
}

/// Does the (multipart-stripped) basename identify a multimodal projector
/// file? Token match on `mmproj` so names like `model-mmproj-Q8_0.gguf`,
/// `mmproj-F32.gguf`, and `mmproj-Qwen3.5-….gguf` all match.
fn is_mmproj_name(stem: &str) -> bool {
    stem.split(['.', '-', '_'])
        .any(|token| token.eq_ignore_ascii_case("mmproj"))
}

/// Quantization type for one repo file, applying the issue #25 rules:
/// 1. mmproj files get their own `MMPROJ`/`MMPROJ-<quant>` group
/// 2. a quant hint in the filename wins (`model.Q4_K_M.gguf`)
/// 3. otherwise inherit from a quant-named ancestor directory (`Q4_K_M/…`)
/// 4. otherwise land in the `OTHER` group instead of being dropped
fn classify_file_quant_type(path: &str) -> String {
    let stem = classification_stem(path);

    if is_mmproj_name(&stem) {
        return match extract_quantization_type(&stem) {
            Some(quant) => format!("{}-{}", MMPROJ_QUANT_TYPE, quant),
            None => MMPROJ_QUANT_TYPE.to_string(),
        };
    }

    if let Some(quant) = extract_quantization_type(&stem) {
        return quant;
    }

    if let Some(quant) = quant_type_from_directory(path) {
        return quant;
    }

    OTHER_QUANT_TYPE.to_string()
}

/// Strictly extract a quantization type from a directory name.
/// Unlike `extract_quantization_type_from_dirname` this never falls back to
/// the raw (uppercased) name — inheritance only fires for recognized
/// patterns, so `Dynamic/` never becomes a quant group.
fn quant_type_from_dirname_strict(dirname: &str) -> Option<String> {
    let upper = dirname.to_uppercase();
    if looks_like_quant_type(&upper) {
        return Some(upper);
    }
    // Model-name-suffixed dirs: `cerebras_…-Q8_0` -> `Q8_0`
    if let Some(last) = upper.rsplit('-').next() {
        if looks_like_quant_type(last) {
            return Some(last.to_string());
        }
    }
    None
}

/// First quant-named ancestor directory of `path` (root scanned first, so
/// the top-level quant layout wins over deeper coincidences).
fn quant_type_from_directory(path: &str) -> Option<String> {
    let mut components = path.split('/');
    let mut current = components.next()?;
    for next in components {
        if let Some(quant) = quant_type_from_dirname_strict(current) {
            return Some(quant);
        }
        current = next;
    }
    None
}

/// Classify a repo's files into quantization groups.
///
/// Pure function over the complete recursive file listing
/// (`ModelMetadata::siblings`) — no I/O, exhaustively unit-testable.
/// Replaces the old root-only tree walk which missed GGUFs stored in
/// arbitrarily named subdirectories (issue #25).
///
/// Only GGUF-family files are grouped; other files (safetensors, configs…)
/// stay for the Standard-mode file tree.
pub fn classify_quantizations(files: &[RepoFile]) -> Vec<QuantizationGroup> {
    let mut grouped: HashMap<String, Vec<QuantizationInfo>> = HashMap::new();

    for file in files {
        let path = &file.rfilename;
        // Defensive: siblings may contain directory entries (trailing '/')
        if path.ends_with('/') || !is_gguf_path(path) {
            continue;
        }

        let quant_type = classify_file_quant_type(path);
        grouped.entry(quant_type.clone()).or_default().push(QuantizationInfo {
            quant_type,
            filename: path.clone(),
            size: file.size.unwrap_or(0),
            sha256: file.lfs.as_ref().map(|lfs| lfs.oid.clone()),
        });
    }

    let mut groups: Vec<QuantizationGroup> = grouped
        .into_iter()
        .map(|(quant_type, files)| {
            let total_size: u64 = files.iter().map(|f| f.size).sum();
            QuantizationGroup {
                quant_type,
                files,
                total_size,
            }
        })
        .collect();

    // Largest group first (previous behavior); `OTHER` always pinned last so
    // recognized quants stay at the top of the list.
    groups.sort_by(|a, b| {
        let is_other = |g: &QuantizationGroup| g.quant_type == OTHER_QUANT_TYPE;
        match (is_other(a), is_other(b)) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => b.total_size.cmp(&a.total_size),
        }
    });

    groups
}

/// Fetch SHA256 hashes for multiple files in a single API call
/// Returns a HashMap mapping filename to its SHA256 hash (if available)
pub async fn fetch_multipart_sha256s(
    model_id: &str,
    filenames: &[String],
    token: Option<&String>,
) -> Result<HashMap<String, Option<String>>, reqwest::Error> {
    // Single API call to get all files
    let url = format!("{}/api/models/{}/tree/main", api_base(), model_id);

    let response = crate::http_client::get_with_optional_token(&url, token).await?;
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

pub fn get_multipart_base_name(filename: &str) -> String {
    // Extract base name from multi-part filename
    // E.g., "model-Q6_K-00003-of-00009.gguf" -> "model-Q6_K.gguf"
    // E.g., "model.Q4_K_M.gguf.part1of2" -> "model.Q4_K_M.gguf"

    // Handle 5-digit format: -00003-of-00009
    if let Some(multi_part_pos) = filename.rfind("-of-") {
        if let Some(part_start) = filename[..multi_part_pos].rfind('-') {
            let part_num = &filename[part_start + 1..multi_part_pos];
            if part_num.len() == 5 && part_num.chars().all(|c| c.is_ascii_digit()) {
                return format!(
                    "{}{}",
                    &filename[..part_start],
                    &filename[filename.rfind(".gguf").unwrap_or(filename.len())..]
                );
            }
        }
    }

    // Handle partNofM format: .part1of2, .part2of3, etc.
    if let Some(part_pos) = filename.rfind(".part") {
        // Check if it's followed by digits+of+digits
        let suffix = &filename[part_pos + 5..]; // Skip ".part"
        if let Some(of_pos) = suffix.find("of") {
            let part_num = &suffix[..of_pos];
            let total_num = &suffix[of_pos + 2..];
            if part_num.chars().all(|c| c.is_ascii_digit())
                && total_num.chars().all(|c| c.is_ascii_digit())
            {
                // Return filename without the .partNofM suffix
                return filename[..part_pos].to_string();
            }
        }
    }

    filename.to_string()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn is_quantization_directory(dirname: &str) -> bool {
    // Check if directory name looks like a quantization type
    // Examples: Q4_K_M, Q8_0, Q5_K_S, IQ4_XS, TQ1_0, MXFP4, BF16, etc.
    // Also handles patterns like: cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0
    //
    // Delegates to the shared predicate (via `quant_type_from_dirname_strict`)
    // so filename and directory heuristics can never drift apart again
    // (issue #25: MXFP was missing here while filenames knew it).
    quant_type_from_dirname_strict(dirname).is_some()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn extract_quantization_type_from_dirname(dirname: &str) -> String {
    // Extract just the quantization type from a directory name
    // Examples:
    //   "Q4_K_M" -> "Q4_K_M"
    //   "cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0" -> "Q8_0"
    //
    // Delegates to the shared predicate; keeps the historical uppercased
    // fallback for unrecognized names (classification uses the strict
    // `quant_type_from_dirname_strict` instead).
    if let Some(quant) = quant_type_from_dirname_strict(dirname) {
        return quant;
    }
    dirname.to_uppercase()
}


pub fn extract_quantization_type(filename: &str) -> Option<String> {
    // Extract quantization type from filenames like:
    // "model.Q4_K_M.gguf" or "llama-2-7b.Q5_0.gguf" or "Qwen3-VL-30B-Q8_K_XL.gguf"
    // "Qwen3-VL-4B-Thinking-1M-IQ4_XS.gguf" or "model-BF16.gguf"
    // "cerebras.MiniMax-M2-REAP-172B-A10B.Q6_K-00003-of-00009.gguf" (multi-part)
    // "MiniMax-M2-REAP-162B-A10B.Q4_K_M.gguf.part1of2" (multi-part)
    let name = filename;

    // Remove .partNofM suffix if present (must do this BEFORE removing .gguf)
    let name = if let Some(part_pos) = name.rfind(".part") {
        let suffix = &name[part_pos + 5..];
        if let Some(of_pos) = suffix.find("of") {
            let part_num = &suffix[..of_pos];
            if part_num.chars().all(|c| c.is_ascii_digit()) {
                &name[..part_pos]
            } else {
                name
            }
        } else {
            name
        }
    } else {
        name
    };

    // Now remove .gguf extension
    let mut name = name.trim_end_matches(".gguf");

    // Remove multi-part suffix if present (e.g., "-00003-of-00009")
    if let Some(multi_part_pos) = name.rfind("-of-") {
        // Find the start of the part number (should be format: -NNNNN-of-NNNNN)
        if let Some(part_start) = name[..multi_part_pos].rfind('-') {
            // Verify it looks like a part number (5 digits)
            let part_num = &name[part_start + 1..multi_part_pos];
            if part_num.len() == 5 && part_num.chars().all(|c| c.is_ascii_digit()) {
                // Remove the multi-part suffix
                name = &name[..part_start];
            }
        }
    }

    // Try splitting by '.' first (handles model.Q4_K_M.gguf)
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() > 1 {
        if let Some(last_part) = parts.last() {
            if looks_like_quant_type(last_part) {
                return Some(last_part.to_uppercase());
            }
            // Underscore-prefix fallback: `mxfp4_moe` -> `MXFP4`. Only fires
            // when the whole part is NOT a quant type, so `Q4_K_M` stays
            // intact (issue #25).
            if let Some(prefix) = last_part.split('_').next() {
                if looks_like_quant_type(prefix) {
                    return Some(prefix.to_uppercase());
                }
            }
        }
    }

    // If no dots, try splitting by '-' (handles Qwen3-VL-30B-Q8_K_XL.gguf and IQ4_XS)
    let parts: Vec<&str> = name.split('-').collect();
    for part in parts.iter().rev() {
        // First check if the whole part is a valid quant type (e.g., Q4_K_M, IQ4_XS)
        if looks_like_quant_type(part) {
            return Some(part.to_uppercase());
        }
        // If not, check if it contains an underscore and the prefix is a quant type
        // This handles cases like "MXFP4_MOE" where MXFP4_MOE is not recognized as a whole,
        // but MXFP4 is the actual quantization type
        if part.contains('_') {
            let subparts: Vec<&str> = part.split('_').collect();
            if let Some(first) = subparts.first() {
                if looks_like_quant_type(first) {
                    // Only use the prefix if it's different from checking the whole part
                    // This prevents Q4_K_M from becoming just Q4
                    return Some(first.to_uppercase());
                }
            }
        }
    }

    None
}

/// Does `s` look like a quantization type? The ONE predicate shared by
/// filename, directory, and classification logic (the three drifted copies
/// are how MXFP went missing from the directory heuristics — issue #25).
///
/// Recognizes: `Q<digit>…` (Q4, Q4_K_M), `IQ<digit>…`, `TQ<digit>…`,
/// `MXFP<digit>` (MXFP4, but not MXFP4_MOE), and the exact formats
/// BF16 / F16 / FP16 / FP32.
pub fn looks_like_quant_type(s: &str) -> bool {
    let upper = s.to_uppercase();
    // Q followed by digit (Q4, Q5, Q8, etc.)
    if upper.starts_with('Q')
        && upper.len() > 1
        && upper.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
    {
        return true;
    }
    // IQ followed by digit (IQ4_XS, IQ3_M, etc.)
    if upper.starts_with("IQ")
        && upper.len() > 2
        && upper.chars().nth(2).is_some_and(|c| c.is_ascii_digit())
    {
        return true;
    }
    // TQ followed by digit (TQ1_0, TQ2_0, etc.) - ternary packing for TriLMs/BitNet
    if upper.starts_with("TQ")
        && upper.len() > 2
        && upper.chars().nth(2).is_some_and(|c| c.is_ascii_digit())
    {
        return true;
    }
    // MXFP followed by digit (MXFP4, MXFP6, MXFP8, etc.)
    // But not MXFP4_MOE (that should be split to MXFP4)
    if upper.starts_with("MXFP")
        && upper.len() > 4
        && upper.chars().nth(4).is_some_and(|c| c.is_ascii_digit())
    {
        // Make sure there's no underscore with additional suffix
        if !upper.contains('_')
            || upper
                .chars()
                .nth(5)
                .is_some_and(|c| c == '_' && upper.len() == 6)
        {
            return true;
        }
    }
    // Special formats
    if upper == "BF16" || upper == "F16" || upper == "FP16" || upper == "FP32" {
        return true;
    }
    false
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_multipart_filename(filename: &str) -> Option<(u32, u32)> {
    // Parse filenames like:
    // "Q2_K/MiniMax-M2-Q2_K-00001-of-00002.gguf" (5-digit format)
    // "MiniMax-M2-REAP-162B-A10B.Q4_K_M.gguf.part1of2" (partNofM format)
    // Returns (current_part, total_parts) if this is a multi-part file
    use regex::Regex;

    // Try 5-digit format first: 00001-of-00002
    if let Ok(re) = Regex::new(r"(\d{5})-of-(\d{5})") {
        if let Some(caps) = re.captures(filename) {
            let current_part = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let total_parts = caps.get(2)?.as_str().parse::<u32>().ok()?;

            if total_parts > 1 && current_part <= total_parts {
                return Some((current_part, total_parts));
            }
        }
    }

    // Try partNofM format: part1of2, part2of3, etc.
    if let Ok(re) = Regex::new(r"part(\d+)of(\d+)") {
        if let Some(caps) = re.captures(filename) {
            let current_part = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let total_parts = caps.get(2)?.as_str().parse::<u32>().ok()?;

            if total_parts > 1 && current_part <= total_parts {
                return Some((current_part, total_parts));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    // Unit tests for pure helper functions. No network access: fixtures are
    // constructed directly and no fetch_* function is called.
    use super::*;

    fn repo_file(path: &str, size: u64) -> RepoFile {
        RepoFile {
            rfilename: path.to_string(),
            size: Some(size),
            lfs: None,
        }
    }

    fn repo_file_lfs(path: &str, size: u64, oid: &str) -> RepoFile {
        RepoFile {
            rfilename: path.to_string(),
            size: Some(size),
            lfs: Some(crate::models::LfsInfo {
                oid: oid.to_string(),
                size,
                pointer_size: 136,
            }),
        }
    }

    fn group_types(groups: &[QuantizationGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.quant_type.as_str()).collect()
    }

    fn group_files<'a>(groups: &'a [QuantizationGroup], quant: &str) -> Vec<&'a str> {
        groups
            .iter()
            .find(|g| g.quant_type == quant)
            .map(|g| g.files.iter().map(|f| f.filename.as_str()).collect())
            .unwrap_or_default()
    }

    fn make_metadata(siblings: &[&str]) -> ModelMetadata {
        ModelMetadata {
            model_id: "test/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: siblings.iter().map(|s| repo_file(s, 1)).collect(),
            tags: Vec::new(),
        }
    }

    // ---- extract_quantization_type ----

    #[test]
    fn quant_type_from_dotted_filename() {
        assert_eq!(
            extract_quantization_type("model.Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
        assert_eq!(
            extract_quantization_type("llama-2-7b.Q5_0.gguf"),
            Some("Q5_0".to_string())
        );
    }

    #[test]
    fn quant_type_from_dashed_filename() {
        assert_eq!(
            extract_quantization_type("Qwen3-VL-30B-Q8_K_XL.gguf"),
            Some("Q8_K_XL".to_string())
        );
        assert_eq!(
            extract_quantization_type("Qwen3-VL-4B-Thinking-1M-IQ4_XS.gguf"),
            Some("IQ4_XS".to_string())
        );
    }

    #[test]
    fn quant_type_normalizes_lowercase_to_uppercase() {
        // Documents actual behavior: the quant token is matched
        // case-insensitively and normalized to uppercase in the result.
        assert_eq!(
            extract_quantization_type("model.q4_k_m.gguf"),
            Some("Q4_K_M".to_string())
        );
        assert_eq!(
            extract_quantization_type("Model.Bf16.gguf"),
            Some("BF16".to_string())
        );
    }

    #[test]
    fn quant_type_none_for_non_gguf_or_plain_names() {
        // No quant token anywhere -> None.
        assert_eq!(extract_quantization_type("model.gguf"), None);
        assert_eq!(extract_quantization_type("model.txt"), None);
        assert_eq!(extract_quantization_type("readme.bin"), None);
    }

    #[test]
    fn quant_type_from_multipart_names() {
        // 5-digit multi-part suffix is stripped before extraction.
        assert_eq!(
            extract_quantization_type("model.Q4_K_M-00002-of-00005.gguf"),
            Some("Q4_K_M".to_string())
        );
        // partNofM suffix is stripped before extraction.
        assert_eq!(
            extract_quantization_type("model.Q4_K_M.gguf.part1of2"),
            Some("Q4_K_M".to_string())
        );
        // Multi-part name without a recognizable quant token yields None.
        assert_eq!(extract_quantization_type("model-00002-of-00005.gguf"), None);
    }

    #[test]
    fn quant_type_uppercase_gguf_extension_is_not_recognized() {
        // Documents actual behavior: only a lowercase ".gguf" suffix is
        // trimmed, so an uppercase ".GGUF" extension prevents extraction.
        assert_eq!(extract_quantization_type("model.Q4_K_M.GGUF"), None);
    }

    // ---- parse_multipart_filename ----

    #[test]
    fn parse_multipart_five_digit_format() {
        assert_eq!(
            parse_multipart_filename("name-00002-of-00005.gguf"),
            Some((2, 5))
        );
        assert_eq!(parse_multipart_filename("a-00003-of-00004"), Some((3, 4)));
    }

    #[test]
    fn parse_multipart_partnofm_format() {
        assert_eq!(
            parse_multipart_filename("model.Q4_K_M.gguf.part1of2"),
            Some((1, 2))
        );
    }

    #[test]
    fn parse_multipart_rejects_invalid_names() {
        assert_eq!(parse_multipart_filename("model.gguf"), None);
        // Single part (total of 1) is not multi-part.
        assert_eq!(parse_multipart_filename("model-00001-of-00001.gguf"), None);
        // Current part greater than total is rejected.
        assert_eq!(parse_multipart_filename("model-00006-of-00005.gguf"), None);
    }

    // ---- get_multipart_base_name ----

    #[test]
    fn base_name_strips_five_digit_suffix() {
        assert_eq!(
            get_multipart_base_name("model-Q6_K-00003-of-00009.gguf"),
            "model-Q6_K.gguf"
        );
    }

    #[test]
    fn base_name_strips_partnofm_suffix() {
        assert_eq!(
            get_multipart_base_name("model.Q4_K_M.gguf.part1of2"),
            "model.Q4_K_M.gguf"
        );
    }

    #[test]
    fn base_name_leaves_plain_filenames_alone() {
        assert_eq!(get_multipart_base_name("model.gguf"), "model.gguf");
    }

    // ---- is_quantization_directory ----

    #[test]
    fn quant_dir_recognizes_common_types() {
        assert!(is_quantization_directory("Q4_K_M"));
        assert!(is_quantization_directory("Q8_0"));
        assert!(is_quantization_directory("IQ4_XS"));
        assert!(is_quantization_directory("TQ1_0"));
        assert!(is_quantization_directory("BF16"));
        assert!(is_quantization_directory("F16"));
        assert!(is_quantization_directory("FP16"));
        assert!(is_quantization_directory("FP32"));
    }

    #[test]
    fn quant_dir_is_case_insensitive() {
        assert!(is_quantization_directory("q4_k_m"));
        assert!(is_quantization_directory("iq4_xs"));
    }

    #[test]
    fn quant_dir_rejects_random_names() {
        assert!(!is_quantization_directory("random"));
        assert!(!is_quantization_directory("models"));
        assert!(!is_quantization_directory(""));
    }

    #[test]
    fn quant_dir_matches_suffix_of_model_dirnames() {
        assert!(is_quantization_directory(
            "cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0"
        ));
    }

    #[test]
    fn quant_dir_strict_q_prefix_matching() {
        // Tightened with the unified predicate (issue #25): a bare 'Q' prefix
        // without a digit is NOT a quantization directory — this used to
        // swallow arbitrary dirs like "QuickCheck" or "Qwen-backup" into
        // junk quant groups.
        assert!(!is_quantization_directory("QuickCheck"));
        assert!(!is_quantization_directory("Qwen-backup"));
        // Real quant dirs still match
        assert!(is_quantization_directory("Q4_K_M"));
        assert!(is_quantization_directory("Q8_0"));
    }

    #[test]
    fn quant_dir_mxfp_recognized() {
        // MXFP was known to filename heuristics but missing from directory
        // heuristics (drifted copies) — issue #25
        assert!(is_quantization_directory("MXFP4"));
        assert!(is_quantization_directory("mxfp4"));
        assert!(!is_quantization_directory("MXFP4_MOE"));
        assert_eq!(
            extract_quantization_type_from_dirname("MXFP4"),
            "MXFP4"
        );
    }

    // ---- extract_quantization_type_from_dirname ----

    #[test]
    fn dirname_type_plain() {
        assert_eq!(extract_quantization_type_from_dirname("Q4_K_M"), "Q4_K_M");
        assert_eq!(extract_quantization_type_from_dirname("q8_0"), "Q8_0");
    }

    #[test]
    fn dirname_type_from_model_dirname() {
        assert_eq!(
            extract_quantization_type_from_dirname("cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0"),
            "Q8_0"
        );
        assert_eq!(
            extract_quantization_type_from_dirname("my_model-BF16"),
            "BF16"
        );
    }

    #[test]
    fn dirname_type_fallback_uppercases_whole_name() {
        // Documents actual behavior: unrecognized directory names are returned
        // uppercased rather than rejected.
        assert_eq!(extract_quantization_type_from_dirname("random"), "RANDOM");
    }

    // ---- build_file_tree ----

    #[test]
    fn build_file_tree_nests_and_sorts() {
        let files = vec![
            repo_file("a/b.gguf", 100),
            repo_file("a/c.txt", 50),
            repo_file("d.bin", 30),
            repo_file("e/f/g.gguf", 7),
        ];

        let root = build_file_tree(files);

        assert_eq!(root.name, "");
        assert!(root.is_dir);
        assert_eq!(root.depth, 0);
        // Directories first ("a", "e"), then files ("d.bin").
        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "e", "d.bin"]);
        assert_eq!(root.size, Some(187));

        let dir_a = &root.children[0];
        assert!(dir_a.is_dir);
        assert_eq!(dir_a.path, "a");
        assert_eq!(dir_a.size, Some(150));
        let a_names: Vec<&str> = dir_a.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(a_names, vec!["b.gguf", "c.txt"]);

        let b = &dir_a.children[0];
        assert!(!b.is_dir);
        assert_eq!(b.path, "a/b.gguf");
        assert_eq!(b.size, Some(100));
        assert_eq!(b.depth, 2);

        // Deeply nested branch: e/f/g.gguf
        let dir_e = &root.children[1];
        assert_eq!(dir_e.size, Some(7));
        let dir_f = &dir_e.children[0];
        assert_eq!(dir_f.name, "f");
        let g = &dir_f.children[0];
        assert_eq!(g.name, "g.gguf");
        assert_eq!(g.path, "e/f/g.gguf");
        assert_eq!(g.depth, 3);

        // Plain file at root level.
        let d = &root.children[2];
        assert!(!d.is_dir);
        assert_eq!(d.path, "d.bin");
        assert_eq!(d.size, Some(30));
    }

    // ---- has_gguf_files ----

    #[test]
    fn has_gguf_files_true_for_gguf_sibling() {
        assert!(has_gguf_files(&make_metadata(&[
            "config.json",
            "model.Q4_K_M.gguf"
        ])));
    }

    #[test]
    fn has_gguf_files_true_for_multipart_gguf() {
        // ".gguf.partNofM" files also count as GGUF.
        assert!(has_gguf_files(&make_metadata(&["model.gguf.part1of2"])));
    }

    #[test]
    fn has_gguf_files_false_without_gguf() {
        assert!(!has_gguf_files(&make_metadata(&[
            "config.json",
            "weights.safetensors"
        ])));
        assert!(!has_gguf_files(&make_metadata(&[])));
    }

    // ---- classify_quantizations: issue #25 regression fixtures ----
    // Layouts recorded from the live HuggingFace API (2026-09-25) for the
    // four repos named in the issue. Any classifier change must keep these
    // green.

    #[test]
    fn classify_root_layout_unchanged() {
        // Baseline: classic mradermacher-style root layout keeps working.
        let groups = classify_quantizations(&[
            repo_file("model.Q4_K_M.gguf", 100),
            repo_file("model.Q8_0.gguf", 50),
            repo_file("README.md", 1),
        ]);
        assert_eq!(group_types(&groups), vec!["Q4_K_M", "Q8_0"]);
        assert_eq!(group_files(&groups, "Q4_K_M"), vec!["model.Q4_K_M.gguf"]);
    }

    #[test]
    fn classify_ex0bit_nested_dynamic_directory() {
        // Issue #25 case B: all GGUFs live in a non-quant-named `Dynamic/`
        // directory. Previously: zero groups (dead-end UI). Now: visible.
        let groups = classify_quantizations(&[
            repo_file(".gitattributes", 1746),
            repo_file("README.md", 7167),
            repo_file("Dynamic/Qwen3.5-122B-A10B-PRISM-LITE-Dynamic.gguf", 61_970_228_480),
            repo_file("Dynamic/imatrix.dat", 358_906_272),
            repo_file("Dynamic/mmproj-Qwen3.5-122B-A10B-PRISM-LITE.gguf", 912_263_520),
        ]);
        assert_eq!(group_types(&groups), vec!["MMPROJ", "OTHER"]);
        assert_eq!(
            group_files(&groups, "OTHER"),
            vec!["Dynamic/Qwen3.5-122B-A10B-PRISM-LITE-Dynamic.gguf"]
        );
        assert_eq!(
            group_files(&groups, "MMPROJ"),
            vec!["Dynamic/mmproj-Qwen3.5-122B-A10B-PRISM-LITE.gguf"]
        );
        // imatrix.dat is not GGUF: stays out of quant groups
    }

    #[test]
    fn classify_sabomako_mxfp4_moe_and_mmproj_f32() {
        // Issue #25 case D: `mxfp4_moe` multiparts were silently dropped and
        // `mmproj-F32.gguf` was dropped (F32 not a known quant).
        let groups = classify_quantizations(&[
            repo_file("Qwen3.5-122B-A10B-heretic.BF16-00001-of-00006.gguf", 48_656_272_000),
            repo_file("Qwen3.5-122B-A10B-heretic.mxfp4_moe-00001-of-00002.gguf", 39_636_333_056),
            repo_file("Qwen3.5-122B-A10B-heretic.mxfp4_moe-00002-of-00002.gguf", 28_629_663_936),
            repo_file("mmproj-F32.gguf", 1_805_183_712),
            repo_file("README.md", 122),
        ]);
        assert_eq!(group_types(&groups), vec!["MXFP4", "BF16", "MMPROJ"]);
        assert_eq!(group_files(&groups, "MXFP4").len(), 2);
        assert_eq!(group_files(&groups, "MMPROJ"), vec!["mmproj-F32.gguf"]);
    }

    #[test]
    fn classify_mradermacher_mmproj_not_mixed_into_weights() {
        // Issue #25 case C: `*.mmproj-Q8_0.gguf` used to land in the Q8_0
        // weight group; it gets its own MMPROJ-Q8_0 group now.
        let groups = classify_quantizations(&[
            repo_file("Qwen3.5-27B-heretic.Q8_0.gguf", 1000),
            repo_file("Qwen3.5-27B-heretic.mmproj-Q8_0.gguf", 100),
        ]);
        assert_eq!(group_types(&groups), vec!["Q8_0", "MMPROJ-Q8_0"]);
        assert_eq!(group_files(&groups, "Q8_0"), vec!["Qwen3.5-27B-heretic.Q8_0.gguf"]);
        assert_eq!(
            group_files(&groups, "MMPROJ-Q8_0"),
            vec!["Qwen3.5-27B-heretic.mmproj-Q8_0.gguf"]
        );
    }

    #[test]
    fn classify_quant_named_directory_inheritance() {
        // `Q4_K_M/model.gguf` inherits the quant type from the directory
        // (the old root-only walk's one subdir case, now generalized).
        let groups = classify_quantizations(&[
            repo_file("Q4_K_M/model.gguf", 100),
            repo_file("Q8_0/model.gguf", 80),
            repo_file("MXFP4/model.gguf", 60),
        ]);
        assert_eq!(group_types(&groups), vec!["Q4_K_M", "Q8_0", "MXFP4"]);
        assert_eq!(group_files(&groups, "Q4_K_M"), vec!["Q4_K_M/model.gguf"]);
    }

    #[test]
    fn classify_deeply_nested_quant_dir_and_multipart() {
        // Deeply nested branch: e/f/g.gguf — dir hint wins even when the
        // filename itself has no quant info; multipart parts stay together.
        let groups = classify_quantizations(&[
            repo_file("Q4_K_M/model.Q4_K_M-00001-of-00002.gguf", 10),
            repo_file("Q4_K_M/model.Q4_K_M-00002-of-00002.gguf", 10),
        ]);
        assert_eq!(group_types(&groups), vec!["Q4_K_M"]);
        assert_eq!(group_files(&groups, "Q4_K_M").len(), 2);
    }

    #[test]
    fn classify_non_gguf_repo_yields_no_groups() {
        // Issue #25 case A (stepfun-ai Int4): safetensors-only repos produce
        // no quant groups — the TUI falls back to the Standard file tree.
        let groups = classify_quantizations(&[
            repo_file("config.json", 100),
            repo_file("model-00001-of-00002.safetensors", 1000),
            repo_file("tokenizer.json", 10),
        ]);
        assert!(groups.is_empty());
    }

    #[test]
    fn classify_sha256_and_size_carried_through() {
        // Nested files keep LFS oid + size so verification still works.
        let groups = classify_quantizations(&[repo_file_lfs(
            "Dynamic/model-Q4_K.gguf",
            42,
            "deadbeef",
        )]);
        let file = &groups[0].files[0];
        assert_eq!(file.sha256.as_deref(), Some("deadbeef"));
        assert_eq!(file.size, 42);
        assert_eq!(groups[0].total_size, 42);
    }

    #[test]
    fn classify_other_group_sorted_last() {
        let groups = classify_quantizations(&[
            repo_file("mystery.gguf", 999),
            repo_file("model.Q2_K.gguf", 10),
        ]);
        assert_eq!(group_types(&groups), vec!["Q2_K", "OTHER"]);
    }

    #[test]
    fn classify_skips_directory_entries() {
        // Siblings listings sometimes include trailing-slash directory rows.
        let groups = classify_quantizations(&[
            repo_file("Dynamic/", 0),
            repo_file("Dynamic/model.gguf", 5),
        ]);
        assert_eq!(group_types(&groups), vec!["OTHER"]);
    }

    // ---- mmproj helpers ----

    #[test]
    fn mmproj_detection_variants() {
        assert!(is_mmproj_name("mmproj-Q8_0"));
        assert!(is_mmproj_name("model.mmproj-Q8_0"));
        assert!(is_mmproj_name("mmproj-F32"));
        assert!(is_mmproj_name("mmproj-Qwen3.5-122B"));
        assert!(!is_mmproj_name("model-Q8_0"));
        assert!(!is_mmproj_name("mmprojector-adjacent-false-positive-check"));
    }
}
