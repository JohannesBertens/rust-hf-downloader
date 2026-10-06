//! Configuration persistence: load/save [`AppOptions`] as TOML at the
//! canonical config path (`crate::paths::config_path()` — writes must
//! always use that path; reads go through
//! `crate::paths::read_config_path()` to honour the legacy layout), plus
//! `apply_options` to push engine tuning into the shared atomics.

use crate::models::AppOptions;
use std::fs;
use std::sync::atomic::Ordering;

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

// ---------------------------------------------------------------------------
// Declarative option→engine-atomics table (M6/C4)
//
// One row per engine-tunable option; `apply_options`, and the test-only
// `EngineGlobalsSnapshot` capture/restore, all iterate this table — adding
// an option is ONE edit here instead of a three-site rewrite (the
// OPTIONS_FIELDS precedent from ui/render/options_popup.rs). Behavior is
// pinned field-by-field by `apply_options_maps_every_field_and_snapshot_round_trips`.
// ---------------------------------------------------------------------------

/// One value of an engine-global atomic, table-typed (the atomics mix
/// `AtomicUsize`/`AtomicU32`/`AtomicU64`/`AtomicBool`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AtomicValue {
    Usize(usize),
    U32(u32),
    U64(u64),
    Bool(bool),
}

/// Type-erased reference to one engine-global atomic (a
/// `DOWNLOAD_CONFIG`/`VERIFICATION_CONFIG` field). Each [`ENGINE_OPTIONS`]
/// row pairs a reader with its atomic, so value and reference variants
/// always line up; a mis-wired row panics here and fails the pin test
/// first.
#[derive(Debug)]
enum EngineAtomic<'a> {
    Usize(&'a std::sync::atomic::AtomicUsize),
    U32(&'a std::sync::atomic::AtomicU32),
    U64(&'a std::sync::atomic::AtomicU64),
    Bool(&'a std::sync::atomic::AtomicBool),
}

impl EngineAtomic<'_> {
    fn store(&self, value: AtomicValue, order: Ordering) {
        match (self, value) {
            (Self::Usize(a), AtomicValue::Usize(v)) => a.store(v, order),
            (Self::U32(a), AtomicValue::U32(v)) => a.store(v, order),
            (Self::U64(a), AtomicValue::U64(v)) => a.store(v, order),
            (Self::Bool(a), AtomicValue::Bool(v)) => a.store(v, order),
            (atomic, value) => {
                panic!("engine option table type mismatch: {atomic:?} vs {value:?}")
            }
        }
    }

    /// Test-side of the table: only `EngineGlobalsSnapshot::capture`
    /// reads values back (production only stores).
    #[cfg(test)]
    fn load(&self, order: Ordering) -> AtomicValue {
        match self {
            Self::Usize(a) => AtomicValue::Usize(a.load(order)),
            Self::U32(a) => AtomicValue::U32(a.load(order)),
            Self::U64(a) => AtomicValue::U64(a.load(order)),
            Self::Bool(a) => AtomicValue::Bool(a.load(order)),
        }
    }
}

/// One row of the table: how to read the option's persisted value from
/// [`AppOptions`], and which engine-global atomic it applies to.
struct EngineOptionRow {
    read: fn(&AppOptions) -> AtomicValue,
    atomic: fn() -> EngineAtomic<'static>,
}

/// The MiB/s→bytes/s conversion of the rate-limit option (the atomic
/// stores bytes; the option and UI carry MiB/s).
fn mbps_to_bytes_per_sec(mbps: f64) -> u64 {
    (mbps * 1_048_576.0) as u64
}

/// All 14 engine-tunable options: download chunking/retries/timeout/cadence
/// (11 rows onto `DOWNLOAD_CONFIG`, including the two rate-limit fields)
/// and verification tuning (3 rows onto `VERIFICATION_CONFIG`).
const ENGINE_OPTIONS: &[EngineOptionRow] = &[
    EngineOptionRow {
        read: |o| AtomicValue::Usize(o.concurrent_threads),
        atomic: || EngineAtomic::Usize(&crate::download::DOWNLOAD_CONFIG.concurrent_threads),
    },
    EngineOptionRow {
        read: |o| AtomicValue::Usize(o.num_chunks),
        atomic: || EngineAtomic::Usize(&crate::download::DOWNLOAD_CONFIG.target_chunks),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(o.min_chunk_size),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.min_chunk_size),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(o.max_chunk_size),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.max_chunk_size),
    },
    EngineOptionRow {
        read: |o| AtomicValue::Bool(o.verification_on_completion),
        atomic: || EngineAtomic::Bool(&crate::download::DOWNLOAD_CONFIG.enable_verification),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U32(o.max_retries),
        atomic: || EngineAtomic::U32(&crate::download::DOWNLOAD_CONFIG.max_retries),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(o.download_timeout_secs),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.download_timeout_secs),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(o.retry_delay_secs),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.retry_delay_secs),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(o.progress_update_interval_ms),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.progress_update_interval_ms),
    },
    EngineOptionRow {
        read: |o| AtomicValue::Bool(o.download_rate_limit_enabled),
        atomic: || EngineAtomic::Bool(&crate::download::DOWNLOAD_CONFIG.rate_limit_enabled),
    },
    EngineOptionRow {
        read: |o| AtomicValue::U64(mbps_to_bytes_per_sec(o.download_rate_limit_mbps)),
        atomic: || EngineAtomic::U64(&crate::download::DOWNLOAD_CONFIG.rate_limit_bytes_per_sec),
    },
    EngineOptionRow {
        read: |o| AtomicValue::Usize(o.concurrent_verifications),
        atomic: || {
            EngineAtomic::Usize(&crate::verification::VERIFICATION_CONFIG.concurrent_verifications)
        },
    },
    EngineOptionRow {
        read: |o| AtomicValue::Usize(o.verification_buffer_size),
        atomic: || EngineAtomic::Usize(&crate::verification::VERIFICATION_CONFIG.buffer_size),
    },
    EngineOptionRow {
        read: |o| AtomicValue::Usize(o.verification_update_interval),
        atomic: || {
            EngineAtomic::Usize(
                &crate::verification::VERIFICATION_CONFIG.update_interval_iterations,
            )
        },
    },
];

/// Apply loaded options to the global engine configuration atomics
/// (download, rate-limit, and verification settings): every row of
/// [`ENGINE_OPTIONS`] in table order, `Relaxed` like the historical
/// hand-written stores.
///
/// Shared by the TUI (`App::sync_options_to_config` delegates here) and the
/// CLI so both frontends tune the engine identically. Must be called from
/// within a tokio runtime (it spawns the rate-limiter update task).
pub fn apply_options(options: &AppOptions) {
    for row in ENGINE_OPTIONS {
        (row.atomic)().store((row.read)(options), Ordering::Relaxed);
    }

    // The rate limiter is updated asynchronously from the same two values
    // the table just stored (enabled flag + the computed bytes/s).
    let rate_limit_enabled = options.download_rate_limit_enabled;
    let bytes_per_sec = mbps_to_bytes_per_sec(options.download_rate_limit_mbps);
    tokio::spawn(async move {
        crate::download::RATE_LIMITER.set_rate(bytes_per_sec).await;
        crate::download::RATE_LIMITER.set_enabled(rate_limit_enabled);
    });
}

/// Test-only snapshot of every engine-global atomic [`ENGINE_OPTIONS`]
/// touches (in table order), so tests that legitimately drive the full
/// production bootstrap (`run::load_run_config`'s `apply_options` tail, the TUI's
/// `modify_option`→save path) can restore the pre-test values on the way
/// out — cargo runs unit tests as parallel threads of one process, and
/// the globals are shared. Callers must also serialize against tests
/// that mutate these atomics mid-flight (the crate-wide `ENV_MUTEX`
/// convention; see the T1/W-final test-hardening notes in
/// `cli::args_tests`).
#[cfg(test)]
pub(crate) struct EngineGlobalsSnapshot {
    /// One captured value per [`ENGINE_OPTIONS`] row, in table order.
    values: Vec<AtomicValue>,
}

#[cfg(test)]
impl EngineGlobalsSnapshot {
    pub(crate) fn capture() -> Self {
        Self {
            values: ENGINE_OPTIONS
                .iter()
                .map(|row| (row.atomic)().load(Ordering::Relaxed))
                .collect(),
        }
    }

    pub(crate) fn restore(self) {
        for (row, value) in ENGINE_OPTIONS.iter().zip(self.values) {
            (row.atomic)().store(value, Ordering::Relaxed);
        }
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

    /// C4 pin (validation-first, written before the declarative-table
    /// refactor): `apply_options` must map EVERY engine-tunable option onto
    /// its global atomic — each asserted by name with a distinct non-default
    /// value, so a dropped or mis-wired table row fails here — and
    /// `EngineGlobalsSnapshot::capture`/`restore` must round-trip the full
    /// applied state over a clobbering re-apply of defaults.
    /// Serializes on `ENV_MUTEX` like every atomics-mutating test; the
    /// rate-limiter spawn needs a tokio runtime (hence `#[tokio::test]`).
    #[tokio::test]
    async fn apply_options_maps_every_field_and_snapshot_round_trips() {
        use std::sync::atomic::Ordering::Relaxed;
        let _env = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let globals = EngineGlobalsSnapshot::capture();

        // Distinct non-default values for all 14 engine-tunable fields
        // (defaults: 8/20/5MiB/100MiB/true/5/300/1/200/false/50.0/4/1MiB/100).
        let options = crate::models::AppOptions {
            concurrent_threads: 3,
            num_chunks: 11,
            min_chunk_size: 1 << 20,
            max_chunk_size: 7 << 20,
            verification_on_completion: false,
            max_retries: 2,
            download_timeout_secs: 33,
            retry_delay_secs: 4,
            progress_update_interval_ms: 250,
            download_rate_limit_enabled: true,
            download_rate_limit_mbps: 3.0,
            concurrent_verifications: 5,
            verification_buffer_size: 65_536,
            verification_update_interval: 42,
            ..Default::default()
        };
        apply_options(&options);

        let d = &crate::download::DOWNLOAD_CONFIG;
        let v = &crate::verification::VERIFICATION_CONFIG;
        let assert_applied = |ctx: &str| {
            assert_eq!(d.concurrent_threads.load(Relaxed), 3, "{ctx}");
            assert_eq!(d.target_chunks.load(Relaxed), 11, "{ctx}");
            assert_eq!(d.min_chunk_size.load(Relaxed), 1 << 20, "{ctx}");
            assert_eq!(d.max_chunk_size.load(Relaxed), 7 << 20, "{ctx}");
            assert!(!d.enable_verification.load(Relaxed), "{ctx}");
            assert_eq!(d.max_retries.load(Relaxed), 2, "{ctx}");
            assert_eq!(d.download_timeout_secs.load(Relaxed), 33, "{ctx}");
            assert_eq!(d.retry_delay_secs.load(Relaxed), 4, "{ctx}");
            assert_eq!(d.progress_update_interval_ms.load(Relaxed), 250, "{ctx}");
            assert!(d.rate_limit_enabled.load(Relaxed), "{ctx}");
            assert_eq!(
                d.rate_limit_bytes_per_sec.load(Relaxed),
                (3.0 * 1_048_576.0) as u64,
                "{ctx}"
            );
            assert_eq!(v.concurrent_verifications.load(Relaxed), 5, "{ctx}");
            assert_eq!(v.buffer_size.load(Relaxed), 65_536, "{ctx}");
            assert_eq!(v.update_interval_iterations.load(Relaxed), 42, "{ctx}");
        };
        assert_applied("apply_options direct");

        // Snapshot round-trip: capture the applied state, clobber it with
        // the defaults, restore — every atomic must come back.
        let snapshot = EngineGlobalsSnapshot::capture();
        apply_options(&crate::models::AppOptions::default());
        assert_ne!(d.concurrent_threads.load(Relaxed), 3, "clobber failed");
        snapshot.restore();
        assert_applied("after restore");

        globals.restore();
    }
}
