//! Cross-platform path resolution for config, registry, and the default
//! download directory.
//!
//! Resolution order (highest priority first):
//!
//! 1. Environment variable override
//!    - [`ENV_CONFIG_DIR`] overrides the config directory.
//!    - [`ENV_DATA_DIR`] overrides the data directory (registry + downloads).
//! 2. Portable mode: if `config.toml` sits next to the running executable,
//!    the executable's directory is used for config, and `<exe>/models` is
//!    used for data. This enables running the app from a USB stick or any
//!    user-chosen folder.
//! 3. Platform defaults via the `dirs` crate:
//!    - config: `dirs::config_dir` joined with [`APP_DIR`] (`jreb` —
//!      decision recorded in `plans/cross-platform-paths.md`: the brand
//!      directory is kept on all platforms; do not rename)
//!    - data:   `dirs::home_dir`  joined with `models`
//! 4. Last-resort fallback: [`std::env::temp_dir`] with a subdirectory so
//!    the app never writes directly into the system temp root.
//!
//! All new path decisions must route through this module — never hardcode
//! `HOME` or use `format!` to build filesystem paths (see AGENTS.md).

use std::path::{Path, PathBuf};

/// Environment variable that overrides the config directory.
pub const ENV_CONFIG_DIR: &str = "RUST_HF_DOWNLOADER_CONFIG_DIR";

/// Environment variable that overrides the data directory (registry +
/// default download location).
pub const ENV_DATA_DIR: &str = "RUST_HF_DOWNLOADER_DATA_DIR";

/// File name of the config; also used as the portable-mode marker.
const CONFIG_FILE: &str = "config.toml";

/// File name of the download registry.
const REGISTRY_FILE: &str = "hf-downloads.toml";

/// Subdirectory used under the platform config root.
const APP_DIR: &str = "jreb";

/// Subdirectory under the data root holding model downloads (and the
/// registry, which lives next to the downloads it tracks).
const MODELS_DIR: &str = "models";

/// Pure resolution core. Kept free of process-env/filesystem access so tests
/// can inject overrides directly instead of mutating global state.
fn resolve(
    env_cfg: Option<PathBuf>,
    env_data: Option<PathBuf>,
    portable: Option<PathBuf>,
) -> (PathBuf, PathBuf) {
    let cfg = env_cfg.or_else(|| portable.clone());
    let data = env_data.or_else(|| portable.map(|exe_dir| exe_dir.join(MODELS_DIR)));
    (
        cfg.unwrap_or_else(platform_config_dir),
        data.unwrap_or_else(platform_data_dir),
    )
}

/// Platform-default config directory: `dirs::config_dir()/jreb`, or a
/// namespaced temp directory when `dirs` cannot determine a config root.
fn platform_config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join(APP_DIR))
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIR))
}

/// Platform-default data directory: `dirs::home_dir()/models`, or a
/// namespaced temp directory when no home can be determined.
fn platform_data_dir() -> PathBuf {
    dirs::home_dir()
        .map(|d| d.join(MODELS_DIR))
        .unwrap_or_else(|| std::env::temp_dir().join(MODELS_DIR))
}

/// Reads an env var and returns `Some(PathBuf)` only when it is set and
/// non-empty.
fn env_override(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Returns the directory of the current executable if, and only if, a
/// `config.toml` sits next to it (the portable-mode marker).
fn portable_mode() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?.to_path_buf();
    portable_marker_at(&exe_dir).then_some(exe_dir)
}

/// Whether `dir` carries the portable-mode marker file. Split out for
/// direct testing with temporary directories.
fn portable_marker_at(dir: &Path) -> bool {
    dir.join(CONFIG_FILE).is_file()
}

/// Returns the config directory (see module docs for the resolution order).
pub fn config_dir() -> PathBuf {
    resolve(
        env_override(ENV_CONFIG_DIR),
        env_override(ENV_DATA_DIR),
        portable_mode(),
    )
    .0
}

/// Returns the canonical path to `config.toml`. Writes must always target
/// this path; use [`read_config_path`] for reads.
pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILE)
}

/// Path used when *reading* the config. Falls back to the legacy
/// `~/.config/jreb/config.toml` layout when the canonical file does not
/// exist yet but a legacy one does. This matters on macOS, where pre-v2.6
/// binaries (crates.io builds) read from `~/.config/jreb`; on Linux both
/// locations coincide, and on Windows the legacy layout never worked.
/// The first save writes the canonical path, migrating the file.
pub fn read_config_path() -> PathBuf {
    // An explicit config-dir override means the caller fully owns the
    // location: no legacy-layout fallback (keeps tests and embedding
    // environments hermetic).
    if env_override(ENV_CONFIG_DIR).is_some() {
        return config_path();
    }
    select_read_path(&config_path(), &legacy_config_path())
}

/// Pure selection rule behind [`read_config_path`]: canonical wins when it
/// exists; otherwise a legacy file (if any) is read once so the next save
/// migrates it to the canonical location.
fn select_read_path(canonical: &Path, legacy: &Path) -> PathBuf {
    if canonical.is_file() {
        canonical.to_path_buf()
    } else if legacy.is_file() {
        legacy.to_path_buf()
    } else {
        canonical.to_path_buf()
    }
}

/// Pre-v2.6 config location: `~/.config/jreb/config.toml`.
fn legacy_config_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join(APP_DIR).join(CONFIG_FILE))
        .unwrap_or_else(config_path)
}

/// Returns the directory that holds the download registry and, by default,
/// the downloaded model files.
pub fn data_dir() -> PathBuf {
    resolve(
        env_override(ENV_CONFIG_DIR),
        env_override(ENV_DATA_DIR),
        portable_mode(),
    )
    .1
}

/// The default download directory used when the user has not customised
/// `default_directory` in their config.
pub fn default_download_dir() -> PathBuf {
    data_dir()
}

/// Returns the path to `hf-downloads.toml`.
pub fn registry_path() -> PathBuf {
    data_dir().join(REGISTRY_FILE)
}

/// Serialises tests that read or mutate the process env vars used by path
/// resolution (cargo runs tests as parallel threads of one process; ambient
/// env is shared global state).
#[cfg(test)]
pub(crate) static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rhd-paths-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn env_config_override_wins_over_portable() {
        let (cfg, _) = resolve(
            Some(PathBuf::from("/cfg-override")),
            None,
            Some(PathBuf::from("/exe")),
        );
        assert_eq!(cfg, PathBuf::from("/cfg-override"));
    }

    #[test]
    fn env_data_override_is_independent() {
        let (cfg, data) = resolve(
            Some(PathBuf::from("/cfg")),
            Some(PathBuf::from("/data-override")),
            Some(PathBuf::from("/exe")),
        );
        assert_eq!(cfg, PathBuf::from("/cfg"));
        assert_eq!(data, PathBuf::from("/data-override"));
    }

    #[test]
    fn portable_mode_uses_exe_dir_and_models_subdir() {
        let exe = PathBuf::from("/usb/rust-hf-downloader");
        let (cfg, data) = resolve(None, None, Some(exe.clone()));
        assert_eq!(cfg, exe);
        assert_eq!(data, exe.join(MODELS_DIR));
    }

    #[test]
    fn no_overrides_fall_through_to_platform_defaults() {
        let (cfg, data) = resolve(None, None, None);
        assert_eq!(cfg, platform_config_dir());
        assert_eq!(data, platform_data_dir());
        // Temp fallback never yields the bare temp root.
        assert!(cfg != std::env::temp_dir());
        assert!(data != std::env::temp_dir());
    }

    #[test]
    fn file_names_are_stable() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            config_path().file_name().and_then(|s| s.to_str()),
            Some(CONFIG_FILE)
        );
        assert_eq!(
            registry_path().file_name().and_then(|s| s.to_str()),
            Some(REGISTRY_FILE)
        );
        assert_eq!(registry_path().parent(), Some(data_dir().as_path()));
    }

    #[test]
    fn portable_marker_requires_config_file() {
        let dir = tmp("marker");
        assert!(!portable_marker_at(&dir));
        std::fs::write(dir.join(CONFIG_FILE), "# portable").expect("write marker");
        assert!(portable_marker_at(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_path_prefers_canonical_when_it_exists() {
        let dir = tmp("read-canonical");
        let canonical = dir.join("canonical.toml");
        let legacy = dir.join("legacy.toml");
        std::fs::write(&canonical, "").expect("write canonical");
        std::fs::write(&legacy, "").expect("write legacy");
        assert_eq!(select_read_path(&canonical, &legacy), canonical);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_path_falls_back_to_legacy_only_when_missing_canonical() {
        let dir = tmp("read-legacy");
        let legacy = dir.join("legacy.toml");
        let canonical = dir.join("canonical.toml"); // deliberately not created
        std::fs::write(&legacy, "").expect("write legacy");
        assert_eq!(select_read_path(&canonical, &legacy), legacy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_path_returns_canonical_when_neither_exists() {
        let dir = tmp("read-none");
        let canonical = dir.join("canonical.toml");
        let legacy = dir.join("legacy.toml");
        assert_eq!(select_read_path(&canonical, &legacy), canonical);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pre-existing Linux users must see identical paths (no migration).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_paths_unchanged_from_legacy_layout() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        if std::env::var_os(ENV_CONFIG_DIR).is_some()
            || std::env::var_os(ENV_DATA_DIR).is_some()
            || std::env::var_os("XDG_CONFIG_HOME").is_some()
            || portable_mode().is_some()
        {
            return; // redirected environment — comparison not meaningful
        }
        let home = std::env::var_os("HOME").expect("HOME set on Linux");
        let home = PathBuf::from(home);
        assert_eq!(
            read_config_path(),
            home.join(".config").join(APP_DIR).join(CONFIG_FILE)
        );
        assert_eq!(registry_path(), home.join(MODELS_DIR).join(REGISTRY_FILE));
        assert_eq!(default_download_dir(), home.join(MODELS_DIR));
    }
}
