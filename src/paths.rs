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
//!
//! The HuggingFace **hub** cache directory (used by the `hf-cache`
//! commands) is a deliberate exception to the override chain above:
//! [`hf_hub_cache`] follows `huggingface_hub` conventions (`--cache-dir` /
//! `HF_HUB_CACHE` / `HF_HOME`) so the cache we write is interchangeable
//! with the Python client's, regardless of where this app keeps its own
//! config and data.
//!
//! Path *security* policy — sanitizing user-supplied path components and
//! the traversal-checked final-path builder — lives in the [`sanitize`]
//! submodule.

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

// ---------------------------------------------------------------------------
// HuggingFace hub cache
//
// Resolution here mirrors the `huggingface_hub` Python package, NOT the
// app-specific override chain at the top of this file: a shared cache must
// land on exactly the same directory `hf download` and transformers use,
// so users can point `HF_HUB_CACHE` at one location and mix clients.
//

/// Environment variable holding the primary HuggingFace hub cache override
/// (`HF_HUB_CACHE`). Highest-priority environment source.
pub const ENV_HF_HUB_CACHE: &str = "HF_HUB_CACHE";

/// Environment variable holding the legacy, deprecated HuggingFace hub
/// cache override (`HUGGINGFACE_HUB_CACHE`). Honored for parity with old
/// `huggingface_hub` deployments; wins only when [`ENV_HF_HUB_CACHE`] is
/// unset, and emits a one-line warning.
pub const ENV_HUGGINGFACE_HUB_CACHE: &str = "HUGGINGFACE_HUB_CACHE";

/// Environment variable holding the HuggingFace home directory (`HF_HOME`).
/// When set, the hub cache defaults to `$HF_HOME/hub`.
pub const ENV_HF_HOME: &str = "HF_HOME";

/// Directory `huggingface_hub` places under the platform cache root.
const HF_CACHE_DIR: &str = "huggingface";

/// Hub cache leaf directory: the cache root is always `<base>/huggingface/hub`.
const HF_HUB_SUBDIR: &str = "hub";

/// Pure resolution core behind [`hf_hub_cache`]: takes already-read env
/// values so tests can inject combinations directly instead of mutating
/// process state. Returns the resolved directory plus whether the
/// deprecated [`ENV_HUGGINGFACE_HUB_CACHE`] variable decided the outcome
/// (the caller turns that flag into a one-line warning).
fn resolve_hf_hub_cache(
    flag: Option<&str>,
    hf_hub_cache: Option<PathBuf>,
    legacy_hub_cache: Option<PathBuf>,
    hf_home: Option<PathBuf>,
) -> (PathBuf, bool) {
    if let Some(flag) = flag.filter(|f| !f.is_empty()) {
        return (PathBuf::from(flag), false);
    }
    if let Some(dir) = hf_hub_cache {
        return (dir, false);
    }
    if let Some(dir) = legacy_hub_cache {
        return (dir, true);
    }
    if let Some(home) = hf_home {
        return (home.join(HF_HUB_SUBDIR), false);
    }
    (platform_hf_hub_cache_dir(), false)
}

/// Platform-default hub cache: `dirs::cache_dir()/huggingface/hub`
/// (`~/.cache/huggingface/hub` on Linux), or the same namespaced path under
/// [`std::env::temp_dir`] when no cache root can be determined. Matches the
/// `huggingface_hub` constants module.
fn platform_hf_hub_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join(HF_CACHE_DIR).join(HF_HUB_SUBDIR))
        .unwrap_or_else(|| std::env::temp_dir().join(HF_CACHE_DIR).join(HF_HUB_SUBDIR))
}

/// Resolves the HuggingFace hub cache directory used by the `hf-cache`
/// commands (`sync` writes `models--<org>--<name>/…` into it).
///
/// Precedence, in `huggingface_hub` order (highest first):
///
/// 1. explicit `--cache-dir` flag argument,
/// 2. [`ENV_HF_HUB_CACHE`] (`HF_HUB_CACHE`),
/// 3. [`ENV_HUGGINGFACE_HUB_CACHE`] (`HUGGINGFACE_HUB_CACHE`, deprecated:
///    honored only when `HF_HUB_CACHE` is unset, with a one-line warning),
/// 4. [`ENV_HF_HOME`] (`HF_HOME`) joined with `hub`,
/// 5. platform default `dirs::cache_dir()/huggingface/hub`
///    (e.g. `~/.cache/huggingface/hub` on Linux).
///
/// Environment values that are set but empty count as unset (same rule as
/// [`env_override`]). No canonicalization: the caller's `--cache-dir` is
/// taken verbatim, like the Python client takes it.
pub fn hf_hub_cache(flag: Option<&str>) -> PathBuf {
    let (dir, deprecated) = resolve_hf_hub_cache(
        flag,
        env_override(ENV_HF_HUB_CACHE),
        env_override(ENV_HUGGINGFACE_HUB_CACHE),
        env_override(ENV_HF_HOME),
    );
    if deprecated {
        eprintln!(
            "warning: ${ENV_HUGGINGFACE_HUB_CACHE} is deprecated by huggingface_hub; \
             set ${ENV_HF_HUB_CACHE} instead (using cache at {})",
            dir.display()
        );
    }
    dir
}

/// Name of the `CACHEDIR.TAG` marker file placed at the root of the hub
/// cache directory.
const CACHEDIR_TAG_FILE: &str = "CACHEDIR.TAG";

/// Exact contents of [`CACHEDIR_TAG_FILE`], per the cache directory tagging
/// standard (<https://bford.info/cachedir/spec.html>). The first line — the
/// 43-byte signature plus trailing newline — is fixed by the spec and must
/// stay byte-identical; backup tools scan for it and ignore the tag
/// otherwise. The comment block below it is ours.
const CACHEDIR_TAG_CONTENT: &[u8] = br#"Signature: 8a477f597d28d172789f0688a068862f
#
# This file is a cache directory tag created by rust-hf-downloader.
# This directory is a huggingface-hub-compatible model cache written by
# `hf-cache sync` (models--<org>--<name>/{refs,blobs,snapshots}).
# For information about cache directory tags, see:
# http://www.brynosaurus.com/cachedir/
"#;

/// Writes the `CACHEDIR.TAG` marker into `cache_root` so backup tools skip
/// the hub cache (spec: <https://bford.info/cachedir/spec.html>). Missing
/// parent directories are created. Idempotent: an already-present tag file
/// is never touched, whatever wrote it.
pub fn write_cachedir_tag(cache_root: &Path) -> std::io::Result<()> {
    let tag = cache_root.join(CACHEDIR_TAG_FILE);
    if tag.is_file() {
        return Ok(());
    }
    if let Some(parent) = tag.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tag, CACHEDIR_TAG_CONTENT)
}

// ---------------------------------------------------------------------------
// Path-security policy
//

/// Path-security policy for everything a user or the Hub can influence in
/// a filesystem path: per-component sanitization
/// ([`sanitize_path_component`], including Windows reserved-name and
/// illegal-character rejection) and the containment-checked
/// [`validate_and_sanitize_path`] driver built on it. Every download path
/// is constructed through this module; its [`sanitize::PathError`]
/// messages are user-visible (status messages, CLI error events) and must
/// stay byte-identical — the `Display` strings are pinned by golden
/// tests.
pub mod sanitize {
    use std::fmt;
    use std::path::PathBuf;

    /// Typed validation error for [`validate_and_sanitize_path`]. The
    /// `Display` strings are the exact historical `String` messages
    /// (user-visible in TUI status/error lines and CLI `InvalidPath`
    /// events) — pinned verbatim by the golden tests below; changing a
    /// variant's wording is an observable-behavior change.
    #[derive(Debug)]
    pub enum PathError {
        /// The current working directory could not be resolved for a
        /// relative base path.
        CurrentDirectory(std::io::Error),
        /// The (existing) base path could not be canonicalized.
        InvalidBase(std::io::Error),
        /// Model ID is not exactly `author/model-name`.
        InvalidModelId(String),
        /// The author component of the model ID failed sanitization.
        InvalidAuthor(String),
        /// The model-name component of the model ID failed sanitization.
        InvalidModelName(String),
        /// A `/`-separated component of the filename failed sanitization.
        InvalidFilenameComponent(String),
        /// The sanitized final path resolves outside the base directory.
        TraversalFinalEscape,
        /// The first existing ancestor of the final path resolves outside
        /// the base directory (and is not an ancestor of it).
        TraversalParentEscape,
    }

    impl fmt::Display for PathError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PathError::CurrentDirectory(e) => {
                    write!(f, "Cannot determine current directory: {}", e)
                }
                PathError::InvalidBase(e) => write!(f, "Invalid base path: {}", e),
                PathError::InvalidModelId(id) => write!(f, "Invalid model ID format: {}", id),
                PathError::InvalidAuthor(author) => {
                    write!(f, "Invalid author in model ID: {}", author)
                }
                PathError::InvalidModelName(name) => {
                    write!(f, "Invalid model name in model ID: {}", name)
                }
                PathError::InvalidFilenameComponent(part) => {
                    write!(f, "Invalid filename component: {}", part)
                }
                PathError::TraversalFinalEscape => {
                    write!(
                        f,
                        "Path traversal detected: final path escapes base directory"
                    )
                }
                PathError::TraversalParentEscape => {
                    write!(
                        f,
                        "Path traversal detected: parent path escapes base directory"
                    )
                }
            }
        }
    }

    impl std::error::Error for PathError {}

    pub fn sanitize_path_component(component: &str) -> Option<String> {
        // Reject path components that contain path traversal or are invalid
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.contains('/')
            || component.contains('\\')
            || component.contains('\0')
        {
            return None;
        }

        // Reject any ASCII control character (0x00-0x1F, 0x7F) and the
        // Windows-illegal characters `< > : " | ? *`. Rejecting these on all
        // platforms keeps behaviour consistent across Unix and Windows; they
        // never occur in real HuggingFace file names.
        if component
            .chars()
            .any(|c| c.is_ascii_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return None;
        }

        // Remove leading/trailing whitespace, but preserve leading dots (for dotfiles like .gitattributes)
        // Only trim trailing dots (can cause issues on Windows)
        let trimmed = component.trim().trim_end_matches('.');

        if trimmed.is_empty() {
            return None;
        }

        // Reject Windows reserved device names, case-insensitive, with or
        // without a file extension (e.g. `CON`, `con.txt`, `LPT3.gguf` are all
        // reserved). Opening such names on Windows can target a device or hang
        // legacy I/O; rejecting them on every platform is safe — they are not
        // used by real HuggingFace model files.
        const RESERVED: &[&str] = &[
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ];
        let stem = trimmed
            .split('.')
            .next()
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return None;
        }

        Some(trimmed.to_string())
    }

    /// Canonicalized nearest existing ancestor of `path` (path itself excluded).
    /// Used to keep containment checks symlink-consistent when the base
    /// directory does not exist yet (e.g. a first download into ~/models).
    fn nearest_existing_ancestor(path: &std::path::Path) -> Option<PathBuf> {
        let mut current = path;
        loop {
            let parent = current.parent()?;
            if parent.as_os_str().is_empty() {
                return None;
            }
            if let Ok(canonical) = parent.canonicalize() {
                return Some(canonical);
            }
            current = parent;
        }
    }

    pub fn validate_and_sanitize_path(
        base_path: &str,
        model_id: &str,
        filename: &str,
    ) -> Result<PathBuf, PathError> {
        // Validate base path (relative paths resolve against the current dir,
        // preserving the previous behavior)
        let mut base = PathBuf::from(base_path);
        if !base.is_absolute() {
            base = std::env::current_dir()
                .map_err(PathError::CurrentDirectory)?
                .join(&base);
        }

        // Canonicalize base path if it exists; otherwise resolve to its nearest
        // existing ancestor so the containment checks below stay correct (and
        // symlink-consistent) for a not-yet-created base directory.
        let canonical_base = if base.exists() {
            base.canonicalize().map_err(PathError::InvalidBase)?
        } else {
            match nearest_existing_ancestor(&base) {
                Some(ancestor) => ancestor,
                None => base.clone(),
            }
        };

        // Validate and sanitize model_id (format: "author/model-name")
        let model_parts: Vec<&str> = model_id.split('/').collect();
        if model_parts.len() != 2 {
            return Err(PathError::InvalidModelId(model_id.to_string()));
        }

        let author = sanitize_path_component(model_parts[0])
            .ok_or_else(|| PathError::InvalidAuthor(model_parts[0].to_string()))?;
        let model_name = sanitize_path_component(model_parts[1])
            .ok_or_else(|| PathError::InvalidModelName(model_parts[1].to_string()))?;

        // Validate and sanitize filename - may contain subdirectory (e.g., "Q4_K_M/file.gguf")
        let filename_parts: Vec<&str> = filename.split('/').collect();
        let mut sanitized_filename_parts = Vec::new();

        for part in filename_parts {
            let sanitized = sanitize_path_component(part)
                .ok_or_else(|| PathError::InvalidFilenameComponent(part.to_string()))?;
            sanitized_filename_parts.push(sanitized);
        }

        // Build the final path: base/author/model_name/[subdir/]filename.
        // Built from the *original* base, not the canonical anchor, so a
        // not-yet-created base directory keeps its place in the result.
        let mut final_path = base.join(&author).join(&model_name);
        for part in sanitized_filename_parts {
            final_path = final_path.join(&part);
        }

        // Final safety check: ensure the resulting path is still under the base directory
        if let Ok(canonical_final) = final_path.canonicalize() {
            if !canonical_final.starts_with(&canonical_base) {
                return Err(PathError::TraversalFinalEscape);
            }
        } else {
            // File doesn't exist yet, check parent directories. The first
            // existing ancestor of the final path must be under the base — or an
            // *ancestor of* the base, which is the normal first-download case
            // where the base directory has not been created yet (no symlink can
            // exist under a nonexistent path, so containment still holds).
            let mut check_path = final_path.clone();
            while let Some(parent) = check_path.parent() {
                if parent.exists() {
                    if let Ok(canonical_parent) = parent.canonicalize() {
                        if !canonical_parent.starts_with(&canonical_base)
                            && !canonical_base.starts_with(&canonical_parent)
                        {
                            return Err(PathError::TraversalParentEscape);
                        }
                    }
                    break;
                }
                check_path = parent.to_path_buf();
            }
        }

        Ok(final_path)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sanitize_accepts_normal_components() {
            assert_eq!(
                sanitize_path_component("Qwen3.5-27B-Q8_0.gguf"),
                Some("Qwen3.5-27B-Q8_0.gguf".to_string())
            );
            assert_eq!(
                sanitize_path_component(".gitattributes"),
                Some(".gitattributes".to_string())
            );
            assert_eq!(
                sanitize_path_component("dir v2"),
                Some("dir v2".to_string())
            );
        }

        #[test]
        fn sanitize_rejects_windows_illegal_characters() {
            for bad in ["a<b", "a>b", "a:b", "a\"b", "a|b", "a?b", "a*b"] {
                assert!(sanitize_path_component(bad).is_none(), "accepted {bad:?}");
            }
        }

        #[test]
        fn sanitize_rejects_control_characters() {
            for bad in ["a\nb", "a\tb", "a\u{1b}b", "a\u{7f}"] {
                assert!(sanitize_path_component(bad).is_none(), "accepted {bad:?}");
            }
        }

        #[test]
        fn sanitize_rejects_windows_reserved_device_names() {
            for bad in [
                "CON",
                "con",
                "con.txt",
                "LPT3.gguf",
                "aux",
                "NUL",
                "com7.safetensors",
            ] {
                assert!(sanitize_path_component(bad).is_none(), "accepted {bad:?}");
            }
            // Stem-based check must not reject similar-but-fine names.
            assert!(sanitize_path_component("config.json").is_some());
            assert!(sanitize_path_component("console.log").is_some());
            assert!(sanitize_path_component("nul-pre-check.json").is_some());
        }

        #[test]
        fn sanitize_rejects_traversal_and_empty() {
            for bad in ["", ".", "..", "a/b", "a\\\\b", "a\0b", "   ", "..."] {
                assert!(sanitize_path_component(bad).is_none(), "accepted {bad:?}");
            }
        }

        #[test]
        fn validate_accepts_not_yet_created_base_directory() {
            // Regression: a first-ever download into a fresh base directory was
            // falsely rejected as path traversal (the parent-walk found an
            // ancestor OF the base, which is normal when the base doesn't exist).
            let home = std::env::temp_dir().join(format!("validate-test-{}", std::process::id()));
            let base = home.join("models"); // deliberately not created
            let path = validate_and_sanitize_path(base.to_str().unwrap(), "a/b", "x.gguf");
            assert!(path.is_ok(), "fresh base rejected: {:?}", path);
            assert_eq!(path.unwrap(), base.join("a").join("b").join("x.gguf"));
            let _ = std::fs::remove_dir_all(&home);
        }

        #[test]
        fn validate_accepts_existing_base_directory() {
            let home =
                std::env::temp_dir().join(format!("validate-test-exists-{}", std::process::id()));
            let base = home.join("models");
            std::fs::create_dir_all(base.join("a/b")).unwrap();
            let path = validate_and_sanitize_path(base.to_str().unwrap(), "a/b", "sub/dir/x.gguf");
            assert!(path.is_ok());
            assert_eq!(
                path.unwrap(),
                base.join("a")
                    .join("b")
                    .join("sub")
                    .join("dir")
                    .join("x.gguf")
            );
            let _ = std::fs::remove_dir_all(&home);
        }

        #[test]
        fn validate_rejects_traversal_and_bad_model_ids() {
            let home =
                std::env::temp_dir().join(format!("validate-test-bad-{}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            let base = home.to_str().unwrap().to_string();

            // traversal in filename
            assert!(validate_and_sanitize_path(&base, "a/b", "../escape.gguf").is_err());
            assert!(validate_and_sanitize_path(&base, "a/b", "sub/../../escape.gguf").is_err());
            // bad model ids
            assert!(validate_and_sanitize_path(&base, "nodash", "x.gguf").is_err());
            assert!(validate_and_sanitize_path(&base, "a/b/c", "x.gguf").is_err());
            let _ = std::fs::remove_dir_all(&home);
        }

        #[test]
        fn path_error_display_strings_are_pinned_verbatim() {
            // Golden tests: one assert per variant. These strings are
            // user-visible (TUI error lines, CLI InvalidPath events) and
            // must stay byte-identical to the pre-typed-error messages.
            let io = || std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
            assert_eq!(
                PathError::CurrentDirectory(io()).to_string(),
                "Cannot determine current directory: no such file"
            );
            assert_eq!(
                PathError::InvalidBase(io()).to_string(),
                "Invalid base path: no such file"
            );
            assert_eq!(
                PathError::InvalidModelId("a/b/c".to_string()).to_string(),
                "Invalid model ID format: a/b/c"
            );
            assert_eq!(
                PathError::InvalidAuthor("bad author".to_string()).to_string(),
                "Invalid author in model ID: bad author"
            );
            assert_eq!(
                PathError::InvalidModelName("..".to_string()).to_string(),
                "Invalid model name in model ID: .."
            );
            assert_eq!(
                PathError::InvalidFilenameComponent("a/b".to_string()).to_string(),
                "Invalid filename component: a/b"
            );
            assert_eq!(
                PathError::TraversalFinalEscape.to_string(),
                "Path traversal detected: final path escapes base directory"
            );
            assert_eq!(
                PathError::TraversalParentEscape.to_string(),
                "Path traversal detected: parent path escapes base directory"
            );
        }

        #[test]
        fn validate_returns_the_typed_variant_for_each_failure() {
            let home =
                std::env::temp_dir().join(format!("validate-test-vars-{}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            let base = home.to_str().unwrap().to_string();

            // traversal in a filename component -> InvalidFilenameComponent
            assert!(matches!(
                validate_and_sanitize_path(&base, "a/b", "../escape.gguf"),
                Err(PathError::InvalidFilenameComponent(part)) if part == ".."
            ));
            // model id without exactly one slash -> InvalidModelId
            assert!(matches!(
                validate_and_sanitize_path(&base, "nodash", "x.gguf"),
                Err(PathError::InvalidModelId(id)) if id == "nodash"
            ));
            // reserved device name as author -> InvalidAuthor
            assert!(matches!(
                validate_and_sanitize_path(&base, "CON/model", "x.gguf"),
                Err(PathError::InvalidAuthor(a)) if a == "CON"
            ));
            // reserved device name as model name -> InvalidModelName
            assert!(matches!(
                validate_and_sanitize_path(&base, "a/aux", "x.gguf"),
                Err(PathError::InvalidModelName(n)) if n == "aux"
            ));
            let _ = std::fs::remove_dir_all(&home);
        }
    }
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

    /// RAII guard that restores one environment variable on drop, so tests
    /// can flip hub-cache overrides without leaking state into siblings
    /// (same pattern as the `EnvGuard` in `engine.rs` tests).
    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn hf_hub_cache_flag_beats_every_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::set(ENV_HF_HUB_CACHE, "/env-hub-cache");
        let _legacy = EnvGuard::set(ENV_HUGGINGFACE_HUB_CACHE, "/env-legacy-cache");
        let _home = EnvGuard::set(ENV_HF_HOME, "/env-hf-home");
        assert_eq!(
            hf_hub_cache(Some("/flag-cache")),
            PathBuf::from("/flag-cache")
        );
    }

    #[test]
    fn hf_hub_cache_env_beats_deprecated_and_home() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::set(ENV_HF_HUB_CACHE, "/hub-cache");
        let _legacy = EnvGuard::set(ENV_HUGGINGFACE_HUB_CACHE, "/legacy-cache");
        let _home = EnvGuard::set(ENV_HF_HOME, "/hf-home");
        assert_eq!(hf_hub_cache(None), PathBuf::from("/hub-cache"));
    }

    #[test]
    fn hf_hub_cache_deprecated_var_wins_and_flags_warning() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::unset(ENV_HF_HUB_CACHE);
        let _legacy = EnvGuard::set(ENV_HUGGINGFACE_HUB_CACHE, "/legacy-cache");
        let _home = EnvGuard::set(ENV_HF_HOME, "/hf-home");
        // The public fn resolves to the deprecated path (it also eprintlns
        // the one-line warning; stderr content is asserted via the core).
        assert_eq!(hf_hub_cache(None), PathBuf::from("/legacy-cache"));
        // Pure core: the deprecated var is what decided it.
        let (dir, deprecated) =
            resolve_hf_hub_cache(None, None, Some(PathBuf::from("/legacy-cache")), None);
        assert_eq!(dir, PathBuf::from("/legacy-cache"));
        assert!(deprecated);
    }

    #[test]
    fn hf_hub_cache_hf_home_gets_hub_appended() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::unset(ENV_HF_HUB_CACHE);
        let _legacy = EnvGuard::unset(ENV_HUGGINGFACE_HUB_CACHE);
        let _home = EnvGuard::set(ENV_HF_HOME, "/hf-home");
        assert_eq!(
            hf_hub_cache(None),
            PathBuf::from("/hf-home").join(HF_HUB_SUBDIR)
        );
    }

    #[test]
    fn hf_hub_cache_defaults_to_platform_cache_dir() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::unset(ENV_HF_HUB_CACHE);
        let _legacy = EnvGuard::unset(ENV_HUGGINGFACE_HUB_CACHE);
        let _home = EnvGuard::unset(ENV_HF_HOME);
        assert_eq!(hf_hub_cache(None), platform_hf_hub_cache_dir());
        // Concrete layout check, independent of the implementation consts:
        // `<platform cache>/huggingface/hub`, never the bare cache root.
        if let Some(cache) = dirs::cache_dir() {
            assert_eq!(hf_hub_cache(None), cache.join("huggingface").join("hub"));
        }
    }

    #[test]
    fn hf_hub_cache_ignores_empty_values() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _hub = EnvGuard::set(ENV_HF_HUB_CACHE, "");
        let _legacy = EnvGuard::set(ENV_HUGGINGFACE_HUB_CACHE, "");
        let _home = EnvGuard::set(ENV_HF_HOME, "/hf-home");
        // Empty flag and empty env vars count as unset.
        assert_eq!(
            hf_hub_cache(Some("")),
            PathBuf::from("/hf-home").join(HF_HUB_SUBDIR)
        );
    }

    #[test]
    fn cachedir_tag_signature_line_is_byte_exact() {
        let dir = tmp("cachedir-exact");
        write_cachedir_tag(&dir).expect("write tag");
        let bytes = std::fs::read(dir.join(CACHEDIR_TAG_FILE)).expect("read tag");
        // First 43 bytes: the spec's fixed signature line, then a newline.
        assert_eq!(&bytes[..43], b"Signature: 8a477f597d28d172789f0688a068862f");
        assert_eq!(bytes[43], b'\n');
        // Remainder is our comment block, marking hub compatibility.
        let tail = String::from_utf8_lossy(&bytes[44..]);
        assert!(tail.contains("huggingface-hub"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cachedir_tag_is_idempotent() {
        let dir = tmp("cachedir-idempotent");
        write_cachedir_tag(&dir).expect("initial write");
        // Replace the tag with foreign content: a re-run must leave any
        // existing tag file untouched rather than rewrite it.
        let tag = dir.join(CACHEDIR_TAG_FILE);
        std::fs::write(&tag, "Signature: existing\n").expect("replace tag");
        write_cachedir_tag(&dir).expect("second write");
        assert_eq!(
            std::fs::read_to_string(&tag).expect("read tag"),
            "Signature: existing\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cachedir_tag_creates_missing_root() {
        let base = tmp("cachedir-mkroot");
        let root = base.join("nested").join("hub");
        write_cachedir_tag(&root).expect("write with missing root");
        assert!(root.join(CACHEDIR_TAG_FILE).is_file());
        let _ = std::fs::remove_dir_all(&base);
    }
}
