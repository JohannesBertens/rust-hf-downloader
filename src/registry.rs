use crate::models::{DownloadRegistry, DownloadStatus};
use std::path::PathBuf;
use std::fs;
use std::io::Write;

pub fn get_registry_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(format!("{}/models/hf-downloads.toml", home))
}

pub fn load_registry() -> DownloadRegistry {
    let path = get_registry_path();
    if !path.exists() {
        return DownloadRegistry::default();
    }
    
    match fs::read_to_string(&path) {
        Ok(content) => {
            toml::from_str(&content).unwrap_or_default()
        }
        Err(_) => DownloadRegistry::default(),
    }
}

pub fn save_registry(registry: &DownloadRegistry) {
    let path = get_registry_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    if let Ok(toml_string) = toml::to_string_pretty(registry) {
        if let Ok(mut file) = fs::File::create(&path) {
            let _ = file.write_all(toml_string.as_bytes());
        }
    }
}

pub fn get_incomplete_downloads(registry: &DownloadRegistry) -> Vec<crate::models::DownloadMetadata> {
    registry.downloads.iter()
        .filter(|d| d.status == DownloadStatus::Incomplete)
        .cloned()
        .collect()
}

pub fn get_complete_downloads(registry: &DownloadRegistry) -> std::collections::HashMap<String, crate::models::DownloadMetadata> {
    registry.downloads.iter()
        .filter(|d| d.status == DownloadStatus::Complete)
        .map(|d| (d.filename.clone(), d.clone()))
        .collect()
}
