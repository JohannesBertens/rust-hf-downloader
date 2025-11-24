use crate::models::AppOptions;
use std::fs;
use std::path::PathBuf;

/// Get the path to the configuration file
pub fn get_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(format!("{}/.config/jreb/config.toml", home))
}

/// Ensure the config directory exists
fn ensure_config_dir() -> Result<(), std::io::Error> {
    let config_path = get_config_path();
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Load configuration from disk, or return defaults if not found
pub fn load_config() -> AppOptions {
    let path = get_config_path();
    
    if !path.exists() {
        return AppOptions::default();
    }
    
    match fs::read_to_string(&path) {
        Ok(contents) => {
            match toml::from_str::<AppOptions>(&contents) {
                Ok(options) => options,
                Err(e) => {
                    eprintln!("Warning: Failed to parse config file: {}. Using defaults.", e);
                    AppOptions::default()
                }
            }
        }
        Err(e) => {
            eprintln!("Warning: Failed to read config file: {}. Using defaults.", e);
            AppOptions::default()
        }
    }
}

/// Save configuration to disk
pub fn save_config(options: &AppOptions) -> Result<(), Box<dyn std::error::Error>> {
    ensure_config_dir()?;
    
    let toml_string = toml::to_string_pretty(options)?;
    fs::write(get_config_path(), toml_string)?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_path() {
        let path = get_config_path();
        assert!(path.to_string_lossy().contains(".config/jreb/config.toml"));
    }

    #[test]
    fn test_load_nonexistent_config() {
        // Should return defaults without panicking
        let options = load_config();
        assert_eq!(options.concurrent_threads, 8);
    }
}
