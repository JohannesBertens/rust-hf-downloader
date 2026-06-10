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
        Ok(contents) => match toml::from_str::<AppOptions>(&contents) {
            Ok(options) => options,
            Err(e) => {
                eprintln!(
                    "Warning: Configuration file at '{}' is malformed and could not be \
                     parsed: {}. Falling back to default configuration.",
                    path.display(),
                    e
                );
                AppOptions::default()
            }
        },
        Err(e) => {
            eprintln!(
                "Warning: Unable to read configuration file at '{}': {}. \
                 Falling back to default configuration.",
                path.display(),
                e
            );
            AppOptions::default()
        }
    }
}

/// Save configuration to disk
pub fn save_config(options: &AppOptions) -> Result<(), Box<dyn std::error::Error>> {
    ensure_config_dir()?;

    let toml_string = toml::to_string_pretty(options)?;
    let config_path = get_config_path();
    fs::write(&config_path, toml_string)?;

    // Set restrictive permissions to protect sensitive token data stored in config
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&config_path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&config_path, perms)?;
    }

    Ok(())
}

/// Check if the config file has permissions that allow group or other access.
/// Prints a warning if the file is readable by anyone other than the owner.
pub fn check_config_permissions() {
    let path = get_config_path();
    if !path.exists() {
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(&path) {
            Ok(metadata) => {
                let mode = metadata.permissions().mode();
                // Check if any group (0o070) or other (0o007) permission bits are set
                if mode & 0o077 != 0 {
                    eprintln!(
                        "Warning: Configuration file at '{}' has permissions {:#o}. \
                         It is recommended to restrict access using 'chmod 600 {}' to \
                         protect your HuggingFace token.",
                        path.display(),
                        mode & 0o777,
                        path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not check permissions of configuration file at '{}': {}",
                    path.display(),
                    e
                );
            }
        }
    }
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
