//! Options-dialog interaction table (G2, test-hardening pass): the 16
//! `OPTIONS_FIELDS` rows × {+1, −1} through the PRODUCTION
//! `App::modify_option`, with literal post-values; the cursor bounds;
//! and the edit-mode lifecycle (Enter opens edit ONLY on the two text
//! fields, Enter saves through `save_config`, Esc discards without
//! writing).
//!
//! Containment: `modify_option` runs the full production tail —
//! `sync_options_to_config` (→ `config::apply_options`, which
//! `tokio::spawn`s, hence `#[tokio::test]`, and writes the shared engine
//! atomics, hence the `EngineGlobalsSnapshot` restore) and
//! `save_config` (isolated through `RUST_HF_DOWNLOADER_CONFIG_DIR` into
//! a per-test temp dir). Every env-touching test here holds
//! `ENV_MUTEX`, serializing against the other env/atomics mutators.

use super::state::App;
use crate::models::SortField;
use crate::paths::{ENV_CONFIG_DIR, ENV_MUTEX};
use crate::ui::render::OptionsFieldId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Isolated config dir + unset HF_TOKEN for one options test; returns
/// the temp dir (cleaned up on drop) and the env guards.
struct ConfigDirGuard {
    tmp: std::path::PathBuf,
    _g1: VarGuard,
    _g2: VarGuard,
}

struct VarGuard {
    key: &'static str,
    saved: Option<std::ffi::OsString>,
}

impl VarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let saved = std::env::var_os(key);
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        Self { key, saved }
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

impl Drop for ConfigDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn isolated_config(tag: &str) -> ConfigDirGuard {
    let tmp = std::env::temp_dir().join(format!("rhd-options-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp config dir");
    ConfigDirGuard {
        _g1: VarGuard::set(ENV_CONFIG_DIR, tmp.to_str()),
        tmp,
        _g2: VarGuard::set("HF_TOKEN", None),
    }
}

/// The 16-row × {+1,−1} interaction table. Field identity comes from the
/// OPTIONS_FIELDS table via modify_option itself; the expected values are
/// LITERAL (computed by hand from the documented clamps), so any change
/// to a clamp bound, step, or arm mapping fails here.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn modify_option_all_sixteen_fields_plus_minus_literal_table() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("table");
    let globals = crate::config::EngineGlobalsSnapshot::capture();

    for field in 0..16 {
        for delta in [1i32, -1] {
            let mut app = App::new(); // empty temp config → default options
                                      // The temp config is SHARED across cells and modify_option
                                      // persists after every call — re-base to the compiled-in
                                      // defaults so every cell starts from the same state.
            app.options = crate::models::AppOptions::default();
            app.options_dialog.selected_field = field;
            app.modify_option(delta);

            let o = &app.options;
            match crate::ui::render::OPTIONS_FIELDS[field].id {
                // Text fields: +/- is a no-op (Enter edits instead).
                OptionsFieldId::DefaultDirectory => {
                    assert_eq!(
                        o.default_directory,
                        app_default_directory(),
                        "field {field}"
                    );
                    assert!(o.hf_token.is_none(), "field {field}");
                }
                OptionsFieldId::HfToken => assert!(o.hf_token.is_none(), "field {field}"),
                OptionsFieldId::ConcurrentThreads => {
                    assert_eq!(
                        o.concurrent_threads,
                        if delta > 0 { 9 } else { 7 },
                        "field {field}"
                    );
                }
                OptionsFieldId::NumChunks => {
                    assert_eq!(
                        o.num_chunks,
                        if delta > 0 { 21 } else { 19 },
                        "field {field}"
                    );
                }
                OptionsFieldId::MinChunkSize => {
                    assert_eq!(
                        o.min_chunk_size,
                        if delta > 0 {
                            6 * 1024 * 1024
                        } else {
                            4 * 1024 * 1024
                        },
                        "field {field}"
                    );
                }
                OptionsFieldId::MaxChunkSize => {
                    assert_eq!(
                        o.max_chunk_size,
                        if delta > 0 {
                            110 * 1024 * 1024
                        } else {
                            90 * 1024 * 1024
                        },
                        "field {field}"
                    );
                }
                OptionsFieldId::MaxRetries => {
                    assert_eq!(
                        o.max_retries,
                        if delta > 0 { 6 } else { 4 },
                        "field {field}"
                    );
                }
                OptionsFieldId::DownloadTimeoutSecs => {
                    assert_eq!(
                        o.download_timeout_secs,
                        if delta > 0 { 330 } else { 270 },
                        "field {field}"
                    );
                }
                OptionsFieldId::RetryDelaySecs => {
                    // default 1 sits ON the min clamp: −1 stays at 1.
                    assert_eq!(
                        o.retry_delay_secs,
                        if delta > 0 { 2 } else { 1 },
                        "field {field}"
                    );
                }
                OptionsFieldId::ProgressUpdateIntervalMs => {
                    assert_eq!(
                        o.progress_update_interval_ms,
                        if delta > 0 { 250 } else { 150 },
                        "field {field}"
                    );
                }
                // Toggles flip on EITHER sign (historical +/- behavior).
                OptionsFieldId::RateLimitEnabled => {
                    assert!(o.download_rate_limit_enabled, "field {field}");
                }
                OptionsFieldId::VerificationEnabled => {
                    assert!(!o.verification_on_completion, "field {field}");
                }
                OptionsFieldId::RateLimitMbps => {
                    assert_eq!(
                        o.download_rate_limit_mbps,
                        if delta > 0 { 50.5 } else { 49.5 },
                        "field {field}"
                    );
                }
                OptionsFieldId::ConcurrentVerifications => {
                    assert_eq!(
                        o.concurrent_verifications,
                        if delta > 0 { 5 } else { 3 },
                        "field {field}"
                    );
                }
                OptionsFieldId::VerificationBufferSize => {
                    // The dialog's clamp range is 64KB..=512KB while the
                    // DEFAULT (1 MiB) sits ABOVE the max — so both deltas
                    // clamp DOWN to 512 KiB. Pinned literally.
                    assert_eq!(o.verification_buffer_size, 512 * 1024, "field {field}");
                }
                OptionsFieldId::VerificationUpdateInterval => {
                    assert_eq!(
                        o.verification_update_interval,
                        if delta > 0 { 150 } else { 50 },
                        "field {field}"
                    );
                }
            }
        }
    }

    globals.restore();
}

/// The default_directory an empty-config `App::new` resolves: derived
/// from the (isolated) data-dir resolution — not asserted literally,
/// only used to prove +/- left it untouched.
fn app_default_directory() -> String {
    crate::paths::default_download_dir()
        .to_string_lossy()
        .into_owned()
}

/// Boundary clamps with out-of-range PRE-states (the interesting part is
/// what the i32/u64 casts do to values the config file could carry):
///
/// - `max_retries = u32::MAX` → `u32::MAX as i32` is −1, so BOTH deltas
///   land at 0 after the `.clamp(0, 10)` — the cast truncation is
///   behavior, pinned here.
/// - `num_chunks = 1` (below the 10..100 range) → both deltas clamp up
///   to 10.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn modify_option_boundary_clamps_from_out_of_range_states() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("bounds");
    let globals = crate::config::EngineGlobalsSnapshot::capture();

    for delta in [1i32, -1] {
        let mut app = App::new();
        app.options.max_retries = u32::MAX;
        app.options_dialog.selected_field = 6; // MaxRetries
        app.modify_option(delta);
        assert_eq!(
            app.options.max_retries, 0,
            "max_retries from u32::MAX, delta {delta}"
        );

        let mut app = App::new();
        app.options.num_chunks = 1;
        app.options_dialog.selected_field = 3; // NumChunks
        app.modify_option(delta);
        assert_eq!(
            app.options.num_chunks, 10,
            "num_chunks from 1, delta {delta}"
        );
    }

    globals.restore();
}

/// Cursor bounds through the production key handler: Down at the last
/// row (15) stays 15, Up at row 0 stays 0 — and the interior moves work.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn options_cursor_bounds_and_moves() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("cursor");
    let mut app = App::new();
    app.popup_mode = crate::models::PopupMode::Options;

    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);

    app.options_dialog.selected_field = 0;
    app.on_key_event(down).await;
    assert_eq!(app.options_dialog.selected_field, 1, "Down 0 → 1");
    app.on_key_event(up).await;
    assert_eq!(app.options_dialog.selected_field, 0, "Up 1 → 0");
    app.on_key_event(up).await;
    assert_eq!(app.options_dialog.selected_field, 0, "Up at 0 stays 0");

    app.options_dialog.selected_field = 15;
    app.on_key_event(down).await;
    assert_eq!(app.options_dialog.selected_field, 15, "Down at 15 stays 15");
    app.on_key_event(up).await;
    assert_eq!(app.options_dialog.selected_field, 14, "Up 15 → 14");
}

/// Enter opens edit mode ONLY on the two text fields (0/1); every other
/// row's Enter is a no-op; Esc leaves edit mode without saving.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn options_enter_opens_edit_only_on_text_fields() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("enter");
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    for (field, expect_dir, expect_token) in [
        (0usize, true, false),
        (1, false, true),
        (2, false, false),
        (15, false, false),
    ] {
        let mut app = App::new();
        app.popup_mode = crate::models::PopupMode::Options;
        app.options_dialog.selected_field = field;
        app.on_key_event(enter).await;
        assert_eq!(
            app.options_dialog.editing_directory, expect_dir,
            "Enter on field {field}: editing_directory"
        );
        assert_eq!(
            app.options_dialog.editing_token, expect_token,
            "Enter on field {field}: editing_token"
        );

        // Esc leaves edit mode (and, for non-text fields, is a plain
        // no-op on the flags).
        app.on_key_event(esc).await;
        assert!(!app.options_dialog.editing_directory, "Esc field {field}");
        assert!(!app.options_dialog.editing_token, "Esc field {field}");
    }
}

/// Enter-in-edit SAVES: option updated AND `save_config` wrote it to the
/// (temp) config dir. Esc-in-edit DISCARDS: option unchanged and no
/// config file written at all.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn options_enter_saves_and_esc_discards() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("save");
    let globals = crate::config::EngineGlobalsSnapshot::capture();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let config_file = crate::paths::config_path();

    // --- Esc discards, and never writes ---
    let mut app = App::new();
    app.popup_mode = crate::models::PopupMode::Options;
    app.options_dialog.selected_field = 0;
    app.on_key_event(enter).await;
    assert!(app.options_dialog.editing_directory);
    let original_dir = app.options.default_directory.clone();
    app.options_directory_input = tui_input::Input::new("/tmp/typed-dir".to_string());
    app.on_key_event(esc).await;
    assert!(!app.options_dialog.editing_directory, "Esc left edit mode");
    assert_eq!(
        app.options.default_directory, original_dir,
        "Esc must not commit the edit buffer"
    );
    assert!(
        !config_file.exists(),
        "Esc path must not write the config file"
    );

    // --- Enter saves (directory field) ---
    app.on_key_event(enter).await;
    app.options_directory_input = tui_input::Input::new("/tmp/typed-dir".to_string());
    app.on_key_event(enter).await;
    assert!(
        !app.options_dialog.editing_directory,
        "Enter left edit mode"
    );
    assert_eq!(
        app.options.default_directory, "/tmp/typed-dir",
        "Enter commits the edit buffer"
    );
    let on_disk = std::fs::read_to_string(&config_file).expect("Enter saved the config");
    assert!(
        on_disk.contains("default_directory = \"/tmp/typed-dir\""),
        "config on disk: {on_disk}"
    );

    // --- Enter saves (token field), empty string → None ---
    app.options_dialog.selected_field = 1;
    app.on_key_event(enter).await;
    assert!(app.options_dialog.editing_token);
    app.options.hf_token = Some("stale".to_string());
    app.options_token_input = tui_input::Input::default(); // empty buffer
    app.on_key_event(enter).await;
    assert!(!app.options_dialog.editing_token);
    assert_eq!(
        app.options.hf_token, None,
        "empty token edit clears the token"
    );
    let on_disk = std::fs::read_to_string(&config_file).expect("token save");
    // serde omits None-valued keys in TOML: a cleared token drops the
    // `hf_token` line entirely rather than writing `hf_token = ""`.
    assert!(
        !on_disk.contains("hf_token"),
        "cleared token must omit the key: {on_disk}"
    );

    globals.restore();
}

/// Sanity for the table test's fixture assumption: an empty isolated
/// config yields the compiled-in default filter seeds (the table's
/// no-op assertions for fields 0/1 depend on a deterministic base).
#[test]
fn isolated_app_seeds_default_filters() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = isolated_config("seed");
    let app = App::new();
    assert_eq!(app.filters.sort_field, SortField::Downloads);
    assert_eq!(app.filters.min_downloads, 0);
    assert_eq!(app.filters.min_likes, 0);
}
