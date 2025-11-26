use crate::models::{ModelInfo, ModelFile, QuantizationInfo, QuantizationGroup, TrendingResponse, ModelMetadata, RepoFile, FileTreeNode};
use std::collections::HashMap;

pub async fn fetch_trending_models_page(page: u32, token: Option<&String>) -> Result<Vec<ModelInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/models-json?p={}&sort=trending&withCount=true",
        page
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let trending: TrendingResponse = response.json().await?;
    
    Ok(trending.models)
}

pub async fn fetch_trending_models(token: Option<&String>) -> Result<Vec<ModelInfo>, reqwest::Error> {
    // Fetch both page 0 and page 1 to get ~60 trending models
    let page0_future = fetch_trending_models_page(0, token);
    let page1_future = fetch_trending_models_page(1, token);
    
    // Fetch both pages in parallel
    let (page0_result, page1_result) = tokio::join!(page0_future, page1_future);
    
    let mut all_models = page0_result?;
    all_models.extend(page1_result?);
    
    Ok(all_models)
}

#[allow(dead_code)]  // Kept for backward compatibility, use fetch_models_filtered instead
pub async fn fetch_models(query: &str, token: Option<&String>) -> Result<Vec<ModelInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=50&sort=downloads&direction=-1",
        urlencoding::encode(query)
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let models: Vec<ModelInfo> = response.json().await?;
    
    Ok(models)
}

/// Fetch models with sorting and filtering parameters
pub async fn fetch_models_filtered(
    query: &str,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,
    min_likes: u64,
    token: Option<&String>,
) -> Result<Vec<ModelInfo>, reqwest::Error> {
    use crate::models::{SortField, SortDirection};
    
    // Determine if we need client-side sorting
    let needs_client_side_sort = matches!(sort_field, SortField::Name) 
        || matches!(sort_direction, SortDirection::Ascending);
    
    // API only reliably supports descending sort (direction=-1)
    // For name or ascending, we'll fetch descending and sort client-side
    let sort = match sort_field {
        SortField::Downloads => "downloads",
        SortField::Likes => "likes",
        SortField::Modified => "lastModified",
        SortField::Name => "downloads",  // Use downloads for API, sort by name client-side
    };
    
    // Always use descending for API call
    let direction = "-1";
    
    // Request more results (100) since we'll filter client-side
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=100&sort={}&direction={}",
        urlencoding::encode(query),
        sort,
        direction
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let mut models: Vec<ModelInfo> = response.json().await?;
    
    // Client-side filtering (API doesn't support these filters)
    models.retain(|m| {
        m.downloads >= min_downloads && m.likes >= min_likes
    });
    
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
    let url = format!("https://huggingface.co/api/models/{}", model_id);
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let mut metadata: ModelMetadata = response.json().await?;
    
    // Fetch the complete file tree recursively
    let all_files = fetch_recursive_tree(model_id, "", token).await?;
    
    // Convert ModelFile to RepoFile with proper size information
    metadata.siblings = all_files.into_iter().map(|f| RepoFile {
        rfilename: f.path,
        size: Some(f.size),
        lfs: f.lfs,
    }).collect();
    
    Ok(metadata)
}

/// Recursively fetch all files from a repository, including subdirectories
fn fetch_recursive_tree<'a>(
    model_id: &'a str,
    path: &'a str,
    token: Option<&'a String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<ModelFile>, reqwest::Error>> + 'a>> {
    Box::pin(async move {
        let tree_url = if path.is_empty() {
            format!("https://huggingface.co/api/models/{}/tree/main", model_id)
        } else {
            format!("https://huggingface.co/api/models/{}/tree/main/{}", model_id, path)
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
pub fn has_gguf_files(metadata: &ModelMetadata) -> bool {
    metadata.siblings.iter().any(|file| {
        file.rfilename.ends_with(".gguf") || file.rfilename.contains(".gguf.part")
    })
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
        let total: u64 = node.children.iter_mut()
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
    let child_pos = node.children.iter().position(|child| child.name == current_part);
    
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

pub async fn fetch_model_files(model_id: &str, token: Option<&String>) -> Result<Vec<QuantizationGroup>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let files: Vec<ModelFile> = response.json().await?;
    
    let mut quantizations = Vec::new();
    let mut multi_part_groups: HashMap<String, Vec<ModelFile>> = HashMap::new();
    
    for file in &files {
        // Handle GGUF files in root directory
        // Match both .gguf and .gguf.partNofM patterns
        let is_gguf_file = file.file_type == "file" && 
                          (file.path.ends_with(".gguf") || file.path.contains(".gguf.part"));
        
        if is_gguf_file {
            // Extract SHA256 from lfs.oid (available for all files)
            let sha256 = file.lfs.as_ref().map(|lfs| lfs.oid.clone());
            
            // Check if this is a multi-part file
            if let Some((_, _)) = parse_multipart_filename(&file.path) {
                // Group multi-part files by their base name
                let base_name = get_multipart_base_name(&file.path);
                multi_part_groups.entry(base_name).or_default().push(file.clone());
            } else {
                // Single file
                if let Some(quant_type) = extract_quantization_type(&file.path) {
                    quantizations.push(QuantizationInfo {
                        quant_type,
                        filename: file.path.clone(),
                        size: file.size,
                        sha256,
                    });
                }
            }
        }
        // Handle subdirectories named by quantization type (e.g., Q4_K_M/, Q8_0/)
        else if file.file_type == "directory"
            && is_quantization_directory(&file.path) {
                // Fetch files from this subdirectory
                let subdir_url = format!(
                    "https://huggingface.co/api/models/{}/tree/main/{}",
                    model_id, file.path
                );
                
                if let Ok(subdir_response) = crate::http_client::get_with_optional_token(&subdir_url, token).await {
                    if let Ok(subdir_files) = subdir_response.json::<Vec<ModelFile>>().await {
                        let quant_type = extract_quantization_type_from_dirname(&file.path);
                        
                        // Add each individual file in the directory as a separate QuantizationInfo
                        for subdir_file in subdir_files {
                            if subdir_file.file_type == "file" && 
                               (subdir_file.path.ends_with(".gguf") || subdir_file.path.contains(".gguf.part")) {
                                
                                let sha256 = subdir_file.lfs.as_ref().map(|lfs| lfs.oid.clone());
                                
                                quantizations.push(QuantizationInfo {
                                    quant_type: quant_type.clone(),
                                    filename: subdir_file.path.clone(),
                                    size: subdir_file.size,
                                    sha256,
                                });
                            }
                        }
                    }
                }
            }
    }
    
    // Process multi-part groups - keep all individual files
    // Note: Multi-part files are separate complete files, each with their own SHA256
    // They are NOT downloaded as chunks and concatenated
    for (base_name, parts) in multi_part_groups {
        if let Some(quant_type) = extract_quantization_type(&base_name) {
            // Add each individual part as a separate QuantizationInfo
            for part in parts {
                let sha256 = part.lfs.as_ref().map(|lfs| lfs.oid.clone());
                quantizations.push(QuantizationInfo {
                    quant_type: quant_type.clone(),
                    filename: part.path.clone(),
                    size: part.size,
                    sha256,
                });
            }
        }
    }
    
    // Group quantizations by type
    let mut grouped: HashMap<String, Vec<QuantizationInfo>> = HashMap::new();
    
    for quant in quantizations {
        grouped
            .entry(quant.quant_type.clone())
            .or_default()
            .push(quant);
    }
    
    // Convert to QuantizationGroups and sort by total size (largest first)
    let mut quantization_groups: Vec<QuantizationGroup> = grouped
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
    
    quantization_groups.sort_by(|a, b| b.total_size.cmp(&a.total_size));
    
    Ok(quantization_groups)
}

/// Fetch SHA256 hashes for multiple files in a single API call
/// Returns a HashMap mapping filename to its SHA256 hash (if available)
pub async fn fetch_multipart_sha256s(
    model_id: &str,
    filenames: &[String],
    token: Option<&String>,
) -> Result<HashMap<String, Option<String>>, reqwest::Error> {
    // Single API call to get all files
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let files: Vec<ModelFile> = response.json().await?;
    
    // Create lookup map for fast matching
    let mut sha256_map = HashMap::new();
    
    for filename in filenames {
        let sha256 = files.iter()
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
                return format!("{}{}", &filename[..part_start], &filename[filename.rfind(".gguf").unwrap_or(filename.len())..]);
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
            if part_num.chars().all(|c| c.is_ascii_digit()) && total_num.chars().all(|c| c.is_ascii_digit()) {
                // Return filename without the .partNofM suffix
                return filename[..part_pos].to_string();
            }
        }
    }
    
    filename.to_string()
}

pub fn is_quantization_directory(dirname: &str) -> bool {
    // Check if directory name looks like a quantization type
    // Examples: Q4_K_M, Q8_0, Q5_K_S, IQ4_XS, BF16, etc.
    // Also handles patterns like: cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0
    let upper = dirname.to_uppercase();
    
    // First check if it starts with a known quantization pattern (original behavior)
    if upper.starts_with('Q') || upper.starts_with("IQ") || upper == "BF16" || upper == "FP16" {
        return true;
    }
    
    // Extract the last component after the last hyphen
    // This handles cases like "cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0" -> "Q8_0"
    let parts: Vec<&str> = upper.split('-').collect();
    if let Some(&last_part) = parts.last() {
        // Check if last part looks like a quantization type
        // Q followed by digit (Q4, Q5, Q8, etc.)
        if last_part.starts_with('Q') && last_part.len() > 1 && last_part.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        // IQ followed by digit (IQ4, IQ3, etc.)
        if last_part.starts_with("IQ") && last_part.len() > 2 && last_part.chars().nth(2).is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        // Special formats
        if last_part == "BF16" || last_part == "FP16" || last_part == "FP32" {
            return true;
        }
    }
    
    false
}

pub fn extract_quantization_type_from_dirname(dirname: &str) -> String {
    // Extract just the quantization type from a directory name
    // Examples:
    //   "Q4_K_M" -> "Q4_K_M"
    //   "cerebras_MiniMax-M2-REAP-139B-A10B-Q8_0" -> "Q8_0"
    let upper = dirname.to_uppercase();
    
    // If it already starts with a quantization pattern, return as-is
    if upper.starts_with('Q') || upper.starts_with("IQ") || upper == "BF16" || upper == "FP16" {
        return upper;
    }
    
    // Extract the last component after the last hyphen
    let parts: Vec<&str> = upper.split('-').collect();
    if let Some(&last_part) = parts.last() {
        if last_part.starts_with('Q') || last_part.starts_with("IQ") || last_part == "BF16" || last_part == "FP16" || last_part == "FP32" {
            return last_part.to_string();
        }
    }
    
    // Fallback: return the original name uppercased
    upper
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
    
    // Helper function to check if a string looks like a quantization type
    let is_quant_type = |s: &str| -> bool {
        let upper = s.to_uppercase();
        // Check for common quantization patterns
        // Q followed by digit (Q4, Q5, Q8, etc.)
        if upper.starts_with('Q') && upper.len() > 1 && upper.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        // IQ followed by digit (IQ4_XS, IQ3_M, etc.)
        if upper.starts_with("IQ") && upper.len() > 2 && upper.chars().nth(2).is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        // MXFP followed by digit (MXFP4, MXFP6, MXFP8, etc.)
        // But not MXFP4_MOE (that should be split to MXFP4)
        if upper.starts_with("MXFP") && upper.len() > 4 && upper.chars().nth(4).is_some_and(|c| c.is_ascii_digit()) {
            // Make sure there's no underscore with additional suffix
            if !upper.contains('_') || upper.chars().nth(5).is_some_and(|c| c == '_' && upper.len() == 6) {
                return true;
            }
        }
        // Special formats
        if upper == "BF16" || upper == "FP16" || upper == "FP32" {
            return true;
        }
        false
    };
    
    // Try splitting by '.' first (handles model.Q4_K_M.gguf)
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() > 1 {
        if let Some(last_part) = parts.last() {
            if is_quant_type(last_part) {
                return Some(last_part.to_uppercase());
            }
        }
    }
    
    // If no dots, try splitting by '-' (handles Qwen3-VL-30B-Q8_K_XL.gguf and IQ4_XS)
    let parts: Vec<&str> = name.split('-').collect();
    for part in parts.iter().rev() {
        // First check if the whole part is a valid quant type (e.g., Q4_K_M, IQ4_XS)
        if is_quant_type(part) {
            return Some(part.to_uppercase());
        }
        // If not, check if it contains an underscore and the prefix is a quant type
        // This handles cases like "MXFP4_MOE" where MXFP4_MOE is not recognized as a whole,
        // but MXFP4 is the actual quantization type
        if part.contains('_') {
            let subparts: Vec<&str> = part.split('_').collect();
            if let Some(first) = subparts.first() {
                if is_quant_type(first) {
                    // Only use the prefix if it's different from checking the whole part
                    // This prevents Q4_K_M from becoming just Q4
                    return Some(first.to_uppercase());
                }
            }
        }
    }
    
    None
}

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
