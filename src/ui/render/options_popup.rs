//! The options dialog: 16 editable fields in four categories, with the
//! adaptive vertical layout for short terminals (plan W3.4b split out of
//! `render.rs`; W4.7 moved the field list into the [`OPTIONS_FIELDS`]
//! table shared with `App::modify_option`).

use crate::utils::format_size;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::centered_rect;

/// Identity of one options-dialog row. Never matched by array index:
/// `App::modify_option` and the Enter-edit handler in `ui/app/events.rs`
/// dispatch on these ids, while [`OPTIONS_FIELDS`] owns the display
/// order (W4.7).
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

/// Transient options-dialog UI state (§8.9, moved out of `AppOptions`):
/// the cursor row and the two live-edit flags. Never serialized —
/// `AppOptions` is purely the persisted config schema now; `App` owns
/// one of these. Lives next to [`OPTIONS_FIELDS`] because the cursor
/// bound (`len - 1`) and the two Text fields' edit-mode rendering
/// derive from that table.
#[derive(Debug, Clone, Copy, Default)]
pub struct OptionsDialogState {
    /// Cursor row into [`OPTIONS_FIELDS`] (0..=15; bound enforced by the
    /// j/k handler).
    pub selected_field: usize,
    /// Live edit mode of the Default Directory text field.
    pub editing_directory: bool,
    /// Live edit mode of the HF Token text field.
    pub editing_token: bool,
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
    /// editing flags (§8.9: no longer on `AppOptions`).
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
        value: |options, _, _, _| format_size(options.min_chunk_size),
    },
    OptionsFieldSpec {
        label: "Max Chunk Size:",
        kind: OptionsFieldKind::Number,
        id: OptionsFieldId::MaxChunkSize,
        value: |options, _, _, _| format_size(options.max_chunk_size),
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
        value: |options, _, _, _| format_size(options.verification_buffer_size as u64),
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
// the Enter-edit handler in `ui/app/events.rs` opens an input for. Every
// other row is stepped or toggled by `modify_option` instead.
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

pub fn render_options_popup(
    frame: &mut Frame,
    options: &crate::models::AppOptions,
    dialog: &OptionsDialogState,
    directory_input: &tui_input::Input,
    token_input: &tui_input::Input,
) {
    // Centered popup area. Divergence kept on purpose (W4.8 diff): this is
    // the ONLY popup that also clamps its HEIGHT against terminal - 4.
    let popup_area = centered_rect(
        64,
        31.min(frame.area().height.saturating_sub(4)),
        frame.area(),
    );

    frame.render_widget(Clear, popup_area);

    // border_style (borders only, title unstyled) — NOT the whole-block
    // .style() the other four popups use; a visible difference pinned by
    // the options-popup snapshots.
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Options (ESC to close)")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Render category headers
    let category_offsets = [
        (0, "General"),
        (2, "Download"),
        (10, "Rate Limiting"),
        (12, "Verification"),
    ];

    // Adaptive vertical layout: on short terminals the popup height clamps
    // below the natural content height, and a fixed bottom-anchored help
    // block would overwrite the last fields. Detect that case and drop the
    // decorative spacing (top padding + category gaps) so everything fits.
    let headers_n = category_offsets.len() as u16;
    let fields_n = OPTIONS_FIELDS.len() as u16;
    let spacing_n = headers_n.saturating_sub(1);
    let help_n = 4u16; // every help variant renders 4 lines
    let full_rows = 1 + headers_n + fields_n + spacing_n + help_n + 1;
    let compact = inner.height < full_rows;
    let top_pad: u16 = if compact { 0 } else { 1 };

    let mut y_offset = top_pad;
    let mut field_idx = 0;

    for (cat_idx, (field_start, category_name)) in category_offsets.iter().enumerate() {
        // Render category header
        if cat_idx > 0 && !compact {
            y_offset += 1; // Add spacing before category (except first)
        }

        let separator = format!("─── {} ", category_name);
        let full_width = inner.width.saturating_sub(4) as usize;
        let separator = format!("{:─<width$}", separator, width = full_width);

        let header_area = Rect {
            x: inner.x + 2,
            y: inner.y + y_offset,
            width: inner.width - 4,
            height: 1,
        };

        let header_widget = Paragraph::new(separator).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(header_widget, header_area);

        y_offset += 1;

        // Render fields in this category (order + labels + value strings
        // from the single-source table; W4.7)
        let next_cat_start = category_offsets
            .get(cat_idx + 1)
            .map(|(s, _)| *s)
            .unwrap_or(OPTIONS_FIELDS.len());
        for spec in OPTIONS_FIELDS
            .iter()
            .take(next_cat_start)
            .skip(*field_start)
        {
            let area = Rect {
                x: inner.x + 2,
                y: inner.y + y_offset,
                width: inner.width - 4,
                height: 1,
            };

            let style = if field_idx == dialog.selected_field {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let text = format!(
                "{} {}",
                spec.label,
                (spec.value)(options, dialog, directory_input, token_input)
            );
            let widget = Paragraph::new(text).style(style);
            frame.render_widget(widget, area);

            // Show cursor when editing directory or token
            if dialog.editing_directory && spec.id == OptionsFieldId::DefaultDirectory {
                let cursor_x =
                    area.x + spec.label.len() as u16 + 1 + directory_input.visual_cursor() as u16;
                frame.set_cursor_position((cursor_x, area.y));
            } else if dialog.editing_token && spec.id == OptionsFieldId::HfToken {
                let cursor_x =
                    area.x + spec.label.len() as u16 + 1 + token_input.visual_cursor() as u16;
                frame.set_cursor_position((cursor_x, area.y));
            }

            y_offset += 1;
            field_idx += 1;
        }
    }

    // Controls help (with empty line before). Bottom-anchor when everything
    // fits; otherwise flow directly after the field list so the help block
    // can never overlap the last fields on short terminals.
    let content_rows = y_offset;
    let help_y = if inner.height > content_rows + help_n {
        inner.y + inner.height - help_n - 1
    } else {
        inner.y + content_rows
    };
    let help = if dialog.editing_directory {
        vec![
            "",
            "Type to edit directory path",
            "Enter: Save | ESC: Cancel",
            "",
        ]
    } else if dialog.editing_token {
        vec![
            "",
            "Type to edit HF token (or clear to remove)",
            "Enter: Save | ESC: Cancel",
            "",
        ]
    } else {
        vec![
            "",
            "j/k or ↑/↓: Navigate | Enter: Edit directory",
            "+/- or ←/→: Modify values & toggle verification",
            "ESC: Close",
        ]
    };

    for (i, line) in help.iter().enumerate() {
        let area = Rect {
            x: inner.x + 2,
            y: help_y + i as u16,
            width: inner.width - 4,
            height: 1,
        };
        let widget = Paragraph::new(*line).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(widget, area);
    }
}
