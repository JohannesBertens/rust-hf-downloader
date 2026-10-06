//! The options dialog renderer: 16 editable fields in four categories,
//! with the adaptive vertical layout for short terminals (plan W3.4b
//! split out of `render.rs`). Since M5/U1 this file is a PURE CONSUMER:
//! the dialog state ([`OptionsDialogState`]) and the field table
//! ([`OPTIONS_FIELDS`]) live in `ui/app/options.rs` — the app layer owns
//! the dialog, the renderer only draws it.

use crate::ui::app::options::{OptionsDialogState, OptionsFieldId, OPTIONS_FIELDS};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::centered_rect;

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
