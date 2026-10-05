//! Configuration persistence: load/save [`AppOptions`] as TOML at the
//! canonical config path (`crate::paths::config_path()` — writes must
//! always use that path; reads go through
//! `crate::paths::read_config_path()` to honour the legacy layout), plus
//! `apply_options` to push engine tuning into the shared atomics.

use crate::models::AppOptions;
use std::fs;

/// Ensure the config directory exists
fn ensure_config_dir() -> Result<(), std::io::Error> {
    let config_path = crate::paths::config_path();
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
    fs::write(crate::paths::config_path(), toml_string)?;

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

/// Test-only snapshot of every engine-global atomic `apply_options`
/// writes, so tests that legitimately drive the full production
/// bootstrap (`run::load_run_config`'s `apply_options` tail, the TUI's
/// `modify_option`→save path) can restore the pre-test values on the way
/// out — cargo runs unit tests as parallel threads of one process, and
/// the globals are shared. Callers must also serialize against tests
/// that mutate these atomics mid-flight (the crate-wide `ENV_MUTEX`
/// convention; see the T1/W-final test-hardening notes in `cli::tests`).
#[cfg(test)]
pub(crate) struct EngineGlobalsSnapshot {
    concurrent_threads: usize,
    target_chunks: usize,
    min_chunk_size: u64,
    max_chunk_size: u64,
    enable_verification: bool,
    max_retries: u32,
    download_timeout_secs: u64,
    retry_delay_secs: u64,
    progress_update_interval_ms: u64,
    rate_limit_enabled: bool,
    rate_limit_bytes_per_sec: u64,
    concurrent_verifications: usize,
    verification_buffer_size: usize,
    verification_update_interval: usize,
}

#[cfg(test)]
impl EngineGlobalsSnapshot {
    pub(crate) fn capture() -> Self {
        use std::sync::atomic::Ordering;
        let d = &crate::download::DOWNLOAD_CONFIG;
        let v = &crate::verification::VERIFICATION_CONFIG;
        Self {
            concurrent_threads: d.concurrent_threads.load(Ordering::Relaxed),
            target_chunks: d.target_chunks.load(Ordering::Relaxed),
            min_chunk_size: d.min_chunk_size.load(Ordering::Relaxed),
            max_chunk_size: d.max_chunk_size.load(Ordering::Relaxed),
            enable_verification: d.enable_verification.load(Ordering::Relaxed),
            max_retries: d.max_retries.load(Ordering::Relaxed),
            download_timeout_secs: d.download_timeout_secs.load(Ordering::Relaxed),
            retry_delay_secs: d.retry_delay_secs.load(Ordering::Relaxed),
            progress_update_interval_ms: d.progress_update_interval_ms.load(Ordering::Relaxed),
            rate_limit_enabled: d.rate_limit_enabled.load(Ordering::Relaxed),
            rate_limit_bytes_per_sec: d.rate_limit_bytes_per_sec.load(Ordering::Relaxed),
            concurrent_verifications: v.concurrent_verifications.load(Ordering::Relaxed),
            verification_buffer_size: v.buffer_size.load(Ordering::Relaxed),
            verification_update_interval: v.update_interval_iterations.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn restore(self) {
        use std::sync::atomic::Ordering;
        let d = &crate::download::DOWNLOAD_CONFIG;
        let v = &crate::verification::VERIFICATION_CONFIG;
        d.concurrent_threads
            .store(self.concurrent_threads, Ordering::Relaxed);
        d.target_chunks.store(self.target_chunks, Ordering::Relaxed);
        d.min_chunk_size
            .store(self.min_chunk_size, Ordering::Relaxed);
        d.max_chunk_size
            .store(self.max_chunk_size, Ordering::Relaxed);
        d.enable_verification
            .store(self.enable_verification, Ordering::Relaxed);
        d.max_retries.store(self.max_retries, Ordering::Relaxed);
        d.download_timeout_secs
            .store(self.download_timeout_secs, Ordering::Relaxed);
        d.retry_delay_secs
            .store(self.retry_delay_secs, Ordering::Relaxed);
        d.progress_update_interval_ms
            .store(self.progress_update_interval_ms, Ordering::Relaxed);
        d.rate_limit_enabled
            .store(self.rate_limit_enabled, Ordering::Relaxed);
        d.rate_limit_bytes_per_sec
            .store(self.rate_limit_bytes_per_sec, Ordering::Relaxed);
        v.concurrent_verifications
            .store(self.concurrent_verifications, Ordering::Relaxed);
        v.buffer_size
            .store(self.verification_buffer_size, Ordering::Relaxed);
        v.update_interval_iterations
            .store(self.verification_update_interval, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_path() {
        let _guard = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let path = crate::paths::config_path();
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
