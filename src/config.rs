use crate::models::AppOptions;
use std::fs;
use std::path::PathBuf;

/// Get the canonical path to the configuration file.
/// Writes must always use this path; reads go through
/// [`crate::paths::read_config_path`] to honour the legacy layout.
pub fn get_config_path() -> PathBuf {
    crate::paths::config_path()
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
    let path = crate::paths::read_config_path();

    if !path.exists() {
        return AppOptions::default();
    }

    match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<AppOptions>(&contents) {
            Ok(options) => options,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to parse config file: {}. Using defaults.",
                    e
                );
                AppOptions::default()
            }
        },
        Err(e) => {
            eprintln!(
                "Warning: Failed to read config file: {}. Using defaults.",
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
    fs::write(get_config_path(), toml_string)?;

    Ok(())
}

/// Apply loaded options to the global engine configuration atomics
/// (download, rate-limit, and verification settings).
///
/// Shared by the TUI (`App::sync_options_to_config` delegates here) and the
/// CLI so both frontends tune the engine identically. Must be called from
/// within a tokio runtime (it spawns the rate-limiter update task).
pub fn apply_options(options: &AppOptions) {
    use std::sync::atomic::Ordering;

    // Download config
    crate::download::DOWNLOAD_CONFIG
        .concurrent_threads
        .store(options.concurrent_threads, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .target_chunks
        .store(options.num_chunks, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .min_chunk_size
        .store(options.min_chunk_size, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .max_chunk_size
        .store(options.max_chunk_size, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .enable_verification
        .store(options.verification_on_completion, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .max_retries
        .store(options.max_retries, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .download_timeout_secs
        .store(options.download_timeout_secs, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .retry_delay_secs
        .store(options.retry_delay_secs, Ordering::Relaxed);
    crate::download::DOWNLOAD_CONFIG
        .progress_update_interval_ms
        .store(options.progress_update_interval_ms, Ordering::Relaxed);

    // Rate limiting config
    let rate_limit_enabled = options.download_rate_limit_enabled;
    crate::download::DOWNLOAD_CONFIG
        .rate_limit_enabled
        .store(rate_limit_enabled, Ordering::Relaxed);
    let bytes_per_sec = (options.download_rate_limit_mbps * 1_048_576.0) as u64;
    crate::download::DOWNLOAD_CONFIG
        .rate_limit_bytes_per_sec
        .store(bytes_per_sec, Ordering::Relaxed);

    // Update rate limiter asynchronously
    tokio::spawn(async move {
        crate::download::RATE_LIMITER.set_rate(bytes_per_sec).await;
        crate::download::RATE_LIMITER.set_enabled(rate_limit_enabled);
    });

    // Verification config
    crate::verification::VERIFICATION_CONFIG
        .concurrent_verifications
        .store(options.concurrent_verifications, Ordering::Relaxed);
    crate::verification::VERIFICATION_CONFIG
        .buffer_size
        .store(options.verification_buffer_size, Ordering::Relaxed);
    crate::verification::VERIFICATION_CONFIG
        .update_interval_iterations
        .store(options.verification_update_interval, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_path() {
        let _guard = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let path = get_config_path();
        // Separator-agnostic assertions (would fail on Windows if built
        // with hardcoded '/' separators).
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("config.toml")
        );
        assert!(path
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|d| d.to_str() == Some("jreb")));
    }

    #[test]
    fn test_load_nonexistent_config() {
        // Isolate the config dir so the developer's real config file cannot
        // leak into this test (it asserts defaults, which only hold when no
        // config exists). Env override also disables the legacy-layout
        // fallback in paths::read_config_path.
        let _guard = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "rust-hf-downloader-test-config-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("failed to create temp config dir");

        let key = crate::paths::ENV_CONFIG_DIR;
        let original = std::env::var_os(key);
        std::env::set_var(key, &tmp);

        let options = load_config();

        match original {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let _ = std::fs::remove_dir_all(&tmp);

        // Should return defaults without panicking
        assert_eq!(options.concurrent_threads, 8);
    }
}
