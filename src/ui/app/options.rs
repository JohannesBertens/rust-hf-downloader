//! The options dialog's state and field table (M5/U1 ownership
//! inversion, moved out of `ui/render/options_popup.rs`: `ui::app` owns
//! the dialog's transient state — cursor row, live-edit flags, and the
//! two text-edit buffers — and the 16-row [`OPTIONS_FIELDS`] table shared
//! with `App::modify_option`; `ui::render` is a pure consumer that
//! imports from here).

use crate::fmt::size_full;

/// Identity of one options-dialog row. Never matched by array index:
/// `App::modify_option` and the Enter-edit handler in
/// `ui/app/events/keys.rs` dispatch on these ids, while [`OPTIONS_FIELDS`]
/// owns the display order (W4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsFieldId {
    DefaultDirectory,
    HfToken,
    ConcurrentThreads,
    NumChunks,
    MinChunkSize,
    MaxChunkSize,
    MaxRetries,
    DownloadTimeoutSecs,
    RetryDelaySecs,
    ProgressUpdateIntervalMs,
    RateLimitEnabled,
    RateLimitMbps,
    VerificationEnabled,
    ConcurrentVerifications,
    VerificationBufferSize,
    VerificationUpdateInterval,
}

/// Interaction class of an options field. Documentation of the dialog's
/// three behaviors: text fields are edited via Enter (no +/−), numbers
/// step with per-field step/clamp (kept in `App::modify_option`), and
/// toggles flip on any +/−.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsFieldKind {
    Text,
    Number,
    Toggle,
}

/// Transient options-dialog UI state
/// (docs/DEFERRED.md#options-dialog-transient-state, moved out of
/// `AppOptions`): the cursor row, the two live-edit flags, and — since
/// M5/U1 — the two text-edit buffers (formerly loose `App` fields
/// `options_directory_input`/`options_token_input`). Never serialized —
/// `AppOptions` is purely the persisted config schema now; `App` owns
/// one of these. Lives next to [`OPTIONS_FIELDS`] because the cursor
/// bound (`len - 1`) and the two Text fields' edit-mode rendering
/// derive from that table.
#[derive(Debug, Clone, Default)]
pub struct OptionsDialogState {
    /// Cursor row into [`OPTIONS_FIELDS`] (0..=15; bound enforced by the
    /// j/k handler).
    pub selected_field: usize,
    /// Live edit mode of the Default Directory text field.
    pub editing_directory: bool,
    /// Live edit mode of the HF Token text field.
    pub editing_token: bool,
    /// Live edit buffer of the Default Directory field (seeded from
    /// `options.default_directory` when Enter opens edit mode; committed
    /// on Enter, discarded on Esc).
    pub directory_input: tui_input::Input,
    /// Live edit buffer of the HF Token field (seeded from
    /// `options.hf_token`; an empty buffer commits `None`).
    pub token_input: tui_input::Input,
}

/// One row of the options dialog: label, interaction kind, identity and
/// the value-string accessor. The per-field +/− step/clamp/toggle bodies
/// deliberately stay in `App::modify_option` — they differ per field.
#[derive(Debug, Clone, Copy)]
pub struct OptionsFieldSpec {
    pub label: &'static str,
    pub kind: OptionsFieldKind,
    pub id: OptionsFieldId,
    /// Renders the field's current value; the `Input` arguments are the
    /// live directory/token edit buffers (read by the Text fields while
    /// editing, ignored by the rest), and the dialog state carries the
    /// editing flags (docs/DEFERRED.md#options-dialog-transient-state: no
    /// longer on `AppOptions`).
    pub value: fn(
        &crate::models::AppOptions,
        &OptionsDialogState,
        &tui_input::Input,
        &tui_input::Input,
    ) -> String,
}

/// The options dialog's 16 fields in display order — the single source
/// (W4.7). The renderer iterates this table, `App::modify_option` matches
/// the [`OptionsFieldId`]s, and the popup cursor bound derives from the
/// table length (16 entries → last index 15, exactly the historical
/// `< 15` clamp in `handle_options_popup_input`).
pub const OPTIONS_FIELDS: &[OptionsFieldSpec] = &[
    // General (indices 0-1)
    OptionsFieldSpec {
        label: "Default Directory:",
        kind: OptionsFieldKind::Text,
        id: OptionsFieldId::DefaultDirectory,
        value: |options, dialog, directory_input, _| {
            if dialog.editing_directory {
                directory_input.value().to_string()
            } else {
                options.default_directory.clone()
            }
        },
    },
    OptionsFieldSpec {
        label: "HF Token (optional):",
        kind: OptionsFieldKind::Text,
        id: OptionsFieldId::HfToken,
        value: |options, dialog, _, token_input| {
            if dialog.editing_token {
                token_input.value().to_string()
            } else if let Some(token) = &options.hf_token {
                if token.is_empty() {
                    "[Not set]".to_string()
                } else {
                    "•".repeat(token.len().min(20))
                }
            } else {
                "[Not set]".to_string()
            }
        },
    },
    // Download (indices 2-9)
    OptionsFieldSpec {
        label: "Concurrent Threads:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::ConcurrentThreads,
        value: |options, _, _, _| options.concurrent_threads.to_string(),
    },
    OptionsFieldSpec {
        label: "Target Number of Chunks:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::NumChunks,
        value: |options, _, _, _| options.num_chunks.to_string(),
    },
    OptionsFieldSpec {
        label: "Min Chunk Size:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::MinChunkSize,
        value: |options, _, _, _| size_full(options.min_chunk_size),
    },
    OptionsFieldSpec {
        label: "Max Chunk Size:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::MaxChunkSize,
        value: |options, _, _, _| size_full(options.max_chunk_size),
    },
    OptionsFieldSpec {
        label: "Max Retries:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::MaxRetries,
        value: |options, _, _, _| options.max_retries.to_string(),
    },
    OptionsFieldSpec {
        label: "Download Timeout (sec):",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::DownloadTimeoutSecs,
        value: |options, _, _, _| options.download_timeout_secs.to_string(),
    },
    OptionsFieldSpec {
        label: "Retry Delay (sec):",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::RetryDelaySecs,
        value: |options, _, _, _| options.retry_delay_secs.to_string(),
    },
    OptionsFieldSpec {
        label: "Progress Update Interval (ms):",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::ProgressUpdateIntervalMs,
        value: |options, _, _, _| options.progress_update_interval_ms.to_string(),
    },
    // Rate Limiting (indices 10-11)
    OptionsFieldSpec {
        label: "Rate Limit:",
        kind: OptionsFieldKind::Toggle,
        id: OptionsFieldId::RateLimitEnabled,
        value: |options, _, _, _| {
            (if options.download_rate_limit_enabled {
                "Enabled"
            } else {
                "Disabled"
            })
            .to_string()
        },
    },
    OptionsFieldSpec {
        label: "Max Download Speed (MB/s):",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::RateLimitMbps,
        value: |options, _, _, _| format!("{:.1}", options.download_rate_limit_mbps),
    },
    // Verification (indices 12-15)
    OptionsFieldSpec {
        label: "Enable Verification:",
        kind: OptionsFieldKind::Toggle,
        id: OptionsFieldId::VerificationEnabled,
        value: |options, _, _, _| {
            (if options.verification_on_completion {
                "Enabled"
            } else {
                "Disabled"
            })
            .to_string()
        },
    },
    OptionsFieldSpec {
        label: "Concurrent Verifications:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::ConcurrentVerifications,
        value: |options, _, _, _| options.concurrent_verifications.to_string(),
    },
    OptionsFieldSpec {
        label: "Verification Buffer Size:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::VerificationBufferSize,
        value: |options, _, _, _| size_full(options.verification_buffer_size as u64),
    },
    OptionsFieldSpec {
        label: "Verification Update Interval:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::VerificationUpdateInterval,
        value: |options, _, _, _| options.verification_update_interval.to_string(),
    },
];

// Compile-time structural invariants of the table (W4.7): the dialog has
// 16 rows, and its two Text rows are exactly rows 0-1 — the only fields
// the Enter-edit handler in `ui/app/events/keys.rs` opens an input for.
// Every other row is stepped or toggled by `modify_option` instead.
const _: () = {
    assert!(OPTIONS_FIELDS.len() == 16, "options dialog has 16 fields");
    assert!(matches!(
        OPTIONS_FIELDS[0].id,
        OptionsFieldId::DefaultDirectory
    ));
    assert!(matches!(OPTIONS_FIELDS[1].id, OptionsFieldId::HfToken));
    let mut i = 0;
    while i < OPTIONS_FIELDS.len() {
        let is_text = matches!(OPTIONS_FIELDS[i].kind, OptionsFieldKind::Text);
        assert!(
            is_text == (i == 0 || i == 1),
            "Text rows must be exactly 0-1"
        );
        i += 1;
    }
};
