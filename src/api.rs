use crate::models::{ModelInfo, ModelFile, QuantizationInfo};
use std::collections::HashMap;

pub async fn fetch_models(query: &str) -> Result<Vec<ModelInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=50&sort=downloads&direction=-1",
        urlencoding::encode(query)
    );
    
    let response = reqwest::get(&url).await?;
    let models: Vec<ModelInfo> = response.json().await?;
    
    Ok(models)
}

pub async fn fetch_model_files(model_id: &str) -> Result<Vec<QuantizationInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = reqwest::get(&url).await?;
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
                multi_part_groups.entry(base_name).or_insert_with(Vec::new).push(file.clone());
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
        else if file.file_type == "directory" {
            if is_quantization_directory(&file.path) {
                // Fetch files from this subdirectory
                let subdir_url = format!(
                    "https://huggingface.co/api/models/{}/tree/main/{}",
                    model_id, file.path
                );
                
                if let Ok(subdir_response) = reqwest::get(&subdir_url).await {
                    if let Ok(subdir_files) = subdir_response.json::<Vec<ModelFile>>().await {
                        // Calculate total size of all GGUF files in this directory
                        // Match both .gguf and .gguf.partNofM patterns
                        let total_size: u64 = subdir_files
                            .iter()
                            .filter(|f| f.file_type == "file" && 
                                       (f.path.ends_with(".gguf") || f.path.contains(".gguf.part")))
                            .map(|f| f.size)
                            .sum();
                        
                        if total_size > 0 {
                            // Get first GGUF file as representative filename and its SHA256
                            let first_file = subdir_files
                                .iter()
                                .find(|f| f.file_type == "file" && 
                                         (f.path.ends_with(".gguf") || f.path.contains(".gguf.part")));
                            
                            let filename = first_file
                                .map(|f| f.path.clone())
                                .unwrap_or_else(|| format!("{}/model.gguf", file.path));
                            
                            // Extract SHA256 from first file's lfs.oid
                            let sha256 = first_file
                                .and_then(|f| f.lfs.as_ref())
                                .map(|lfs| lfs.oid.clone());
                            
                            quantizations.push(QuantizationInfo {
                                quant_type: extract_quantization_type_from_dirname(&file.path),
                                filename,
                                size: total_size,
                                sha256,
                            });
                        }
                    }
                }
            }
        }
    }
    
    // Process multi-part groups
    // Note: Multi-part files are separate complete files, each with their own SHA256
    // They are NOT downloaded as chunks and concatenated
    for (base_name, parts) in multi_part_groups {
        let total_size: u64 = parts.iter().map(|f| f.size).sum();
        if let Some(quant_type) = extract_quantization_type(&base_name) {
            // Use the first part's filename as representative
            let first_part = parts.first();
            let filename = first_part.map(|f| f.path.clone()).unwrap_or(base_name);
            
            // Extract SHA256 from first part's lfs.oid
            // Each part is a separate file with its own hash
            let sha256 = first_part
                .and_then(|f| f.lfs.as_ref())
                .map(|lfs| lfs.oid.clone());
            
            quantizations.push(QuantizationInfo {
                quant_type,
                filename,
                size: total_size,
                sha256,
            });
        }
    }
    
    // Sort by file size (largest first)
    quantizations.sort_by(|a, b| b.size.cmp(&a.size));
    
    Ok(quantizations)
}

/// Fetch SHA256 hashes for multiple files in a single API call
/// Returns a HashMap mapping filename to its SHA256 hash (if available)
pub async fn fetch_multipart_sha256s(
    model_id: &str,
    filenames: &[String],
) -> Result<HashMap<String, Option<String>>, reqwest::Error> {
    // Single API call to get all files
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = reqwest::get(&url).await?;
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
        if last_part.starts_with('Q') && last_part.len() > 1 && last_part.chars().nth(1).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        // IQ followed by digit (IQ4, IQ3, etc.)
        if last_part.starts_with("IQ") && last_part.len() > 2 && last_part.chars().nth(2).map_or(false, |c| c.is_ascii_digit()) {
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
        if upper.starts_with('Q') && upper.len() > 1 && upper.chars().nth(1).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        // IQ followed by digit (IQ4_XS, IQ3_M, etc.)
        if upper.starts_with("IQ") && upper.len() > 2 && upper.chars().nth(2).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        // MXFP followed by digit (MXFP4, MXFP6, MXFP8, etc.)
        // But not MXFP4_MOE (that should be split to MXFP4)
        if upper.starts_with("MXFP") && upper.len() > 4 && upper.chars().nth(4).map_or(false, |c| c.is_ascii_digit()) {
            // Make sure there's no underscore with additional suffix
            if !upper.contains('_') || upper.chars().nth(5).map_or(false, |c| c == '_' && upper.len() == 6) {
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
