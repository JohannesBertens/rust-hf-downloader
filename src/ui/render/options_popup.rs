//! The options dialog: 16 editable fields in four categories, with the
//! adaptive vertical layout for short terminals (plan W3.4b split out of
//! `render.rs`; body byte-identical).

use crate::utils::format_size;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub fn render_options_popup(
    frame: &mut Frame,
    options: &crate::models::AppOptions,
    directory_input: &tui_input::Input,
    token_input: &tui_input::Input,
) {
    let popup_width = 64.min(frame.area().width.saturating_sub(4));
    let popup_height = 31.min(frame.area().height.saturating_sub(4));
    let popup_area = Rect {
        x: (frame.area().width.saturating_sub(popup_width)) / 2,
        y: (frame.area().height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Options (ESC to close)")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Render 14 fields with category headers
    let fields = vec![
        // General (indices 0-1)
        (
            "Default Directory:",
            if options.editing_directory {
                directory_input.value().to_string()
            } else {
                options.default_directory.clone()
            },
        ),
        (
            "HF Token (optional):",
            if options.editing_token {
                token_input.value().to_string()
            } else if let Some(token) = &options.hf_token {
                if token.is_empty() {
                    "[Not set]".to_string()
                } else {
                    "•".repeat(token.len().min(20))
                }
            } else {
                "[Not set]".to_string()
            },
        ),
        // Download (indices 2-9)
        (
            "Concurrent Threads:",
            options.concurrent_threads.to_string(),
        ),
        ("Target Number of Chunks:", options.num_chunks.to_string()),
        ("Min Chunk Size:", format_size(options.min_chunk_size)),
        ("Max Chunk Size:", format_size(options.max_chunk_size)),
        ("Max Retries:", options.max_retries.to_string()),
        (
            "Download Timeout (sec):",
            options.download_timeout_secs.to_string(),
        ),
        ("Retry Delay (sec):", options.retry_delay_secs.to_string()),
        (
            "Progress Update Interval (ms):",
            options.progress_update_interval_ms.to_string(),
        ),
        // Rate Limiting (indices 10-11)
        (
            "Rate Limit:",
            if options.download_rate_limit_enabled {
                "Enabled".to_string()
            } else {
                "Disabled".to_string()
            },
        ),
        (
            "Max Download Speed (MB/s):",
            format!("{:.1}", options.download_rate_limit_mbps),
        ),
        // Verification (indices 12-15)
        (
            "Enable Verification:",
            if options.verification_on_completion {
                "Enabled".to_string()
            } else {
                "Disabled".to_string()
            },
        ),
        (
            "Concurrent Verifications:",
            options.concurrent_verifications.to_string(),
        ),
        (
            "Verification Buffer Size:",
            format_size(options.verification_buffer_size as u64),
        ),
        (
            "Verification Update Interval:",
            options.verification_update_interval.to_string(),
        ),
    ];

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
    let fields_n = fields.len() as u16;
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

        // Render fields in this category
        let next_cat_start = category_offsets
            .get(cat_idx + 1)
            .map(|(s, _)| *s)
            .unwrap_or(fields.len());
        for (label, value) in fields.iter().take(next_cat_start).skip(*field_start) {
            let area = Rect {
                x: inner.x + 2,
                y: inner.y + y_offset,
                width: inner.width - 4,
                height: 1,
            };

            let style = if field_idx == options.selected_field {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let text = format!("{} {}", label, value);
            let widget = Paragraph::new(text).style(style);
            frame.render_widget(widget, area);

            // Show cursor when editing directory or token
            if options.editing_directory && field_idx == 0 {
                let cursor_x =
                    area.x + label.len() as u16 + 1 + directory_input.visual_cursor() as u16;
                frame.set_cursor_position((cursor_x, area.y));
            } else if options.editing_token && field_idx == 1 {
                let cursor_x = area.x + label.len() as u16 + 1 + token_input.visual_cursor() as u16;
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
    let help = if options.editing_directory {
        vec![
            "",
            "Type to edit directory path",
            "Enter: Save | ESC: Cancel",
            "",
        ]
    } else if options.editing_token {
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
