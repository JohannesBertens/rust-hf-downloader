//! Modal popups other than the options dialog: resume list, search input,
//! download-path chooser, auth-error steps (plan W3.4b split out of
//! `render.rs`; bodies byte-identical).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tui_input::Input;

use super::{centered_rect, popup_shell};

pub fn render_resume_popup(
    frame: &mut Frame,
    incomplete_downloads: &[crate::models::DownloadMetadata],
) {
    // Centered popup area (width clamped to terminal - 4 by centered_rect)
    let popup_area = centered_rect(
        70,
        10 + incomplete_downloads.len().min(5) as u16,
        frame.area(),
    );

    // Clear the popup area first to remove any underlying content
    popup_shell(
        frame,
        popup_area,
        "Resume Incomplete Downloads?",
        Style::default().fg(Color::Yellow).bg(Color::Black),
    );

    // Render message
    let message_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(4),
        height: 2,
    };

    let message = Paragraph::new(format!(
        "Found {} incomplete download(s):\n",
        incomplete_downloads.len()
    ))
    .style(Style::default().fg(Color::White));

    frame.render_widget(message, message_area);

    // Render list of incomplete files (up to 5)
    let list_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 3,
        width: popup_area.width.saturating_sub(4),
        height: incomplete_downloads.len().min(5) as u16,
    };

    let file_lines: Vec<Line> = incomplete_downloads
        .iter()
        .take(5)
        .map(|metadata| {
            let progress_pct = if metadata.total_size > 0 {
                (metadata.downloaded_size as f64 / metadata.total_size as f64 * 100.0) as u64
            } else {
                0
            };
            Line::from(vec![
                Span::raw("  • "),
                Span::styled(&metadata.filename, Style::default().fg(Color::Cyan)),
                Span::raw(format!(" ({}%)", progress_pct)),
            ])
        })
        .collect();

    let files_widget = Paragraph::new(file_lines).style(Style::default().fg(Color::White));

    frame.render_widget(files_widget, list_area);

    // Show "and X more..." if there are more than 5
    if incomplete_downloads.len() > 5 {
        let more_area = Rect {
            x: popup_area.x + 2,
            y: list_area.y + list_area.height,
            width: popup_area.width.saturating_sub(4),
            height: 1,
        };

        let more_text =
            Paragraph::new(format!("  ... and {} more", incomplete_downloads.len() - 5))
                .style(Style::default().fg(Color::DarkGray));

        frame.render_widget(more_text, more_area);
    }

    // Render instructions
    let instructions_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + popup_area.height.saturating_sub(3),
        width: popup_area.width.saturating_sub(4),
        height: 2,
    };

    let instructions = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to resume all  |  "),
            Span::styled(
                "N",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to skip  |  "),
            Span::styled(
                "D",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to delete and skip"),
        ]),
    ])
    .style(Style::default().fg(Color::White));

    frame.render_widget(instructions, instructions_area);
}

/// Render search popup dialog
pub fn render_search_popup(frame: &mut Frame, input: &Input) {
    let area = centered_rect(60, 8, frame.area());

    let inner = popup_shell(
        frame,
        area,
        " Search HuggingFace Models ",
        Style::default().fg(Color::Cyan),
    );

    // Input field
    let input_area = Rect {
        x: inner.x + 2,
        y: inner.y + 1,
        width: inner.width - 4,
        height: 1,
    };

    let input_widget = Paragraph::new(input.value()).style(Style::default().fg(Color::Yellow));
    frame.render_widget(input_widget, input_area);

    // Show cursor
    frame.set_cursor_position((input_area.x + input.visual_cursor() as u16, input_area.y));

    // Help text
    let help = [
        "",
        "Enter search query and press Enter to search",
        "ESC: Cancel",
    ];

    for (i, line) in help.iter().enumerate() {
        let area = Rect {
            x: inner.x + 2,
            y: inner.y + 3 + i as u16,
            width: inner.width - 4,
            height: 1,
        };
        let widget = Paragraph::new(*line).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(widget, area);
    }
}

pub fn render_download_path_popup(frame: &mut Frame, download_path_input: &Input) {
    // Centered popup area (width clamped to terminal - 4 by centered_rect)
    let popup_area = centered_rect(60, 7, frame.area());

    // Clear the popup area first to remove any underlying content
    popup_shell(
        frame,
        popup_area,
        "Download Model",
        Style::default().fg(Color::White).bg(Color::Black),
    );

    // Render input label
    let label_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };

    let label = Paragraph::new("Download path:").style(Style::default().fg(Color::White));

    frame.render_widget(label, label_area);

    // Render input field
    let input_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 2,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };

    let width = input_area.width.max(3) as usize;
    let scroll = download_path_input.visual_scroll(width);

    let input_widget = Paragraph::new(download_path_input.value())
        .style(Style::default().fg(Color::Yellow))
        .scroll((0, scroll as u16));

    frame.render_widget(input_widget, input_area);

    // Set cursor position
    frame.set_cursor_position((
        input_area.x + ((download_path_input.visual_cursor()).max(scroll) - scroll) as u16,
        input_area.y,
    ));

    // Render instructions
    let instructions_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 4,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };

    let instructions = Paragraph::new("Press Enter to confirm, ESC to cancel")
        .style(Style::default().fg(Color::DarkGray));

    frame.render_widget(instructions, instructions_area);
}

pub fn render_auth_error_popup(frame: &mut Frame, model_url: &str, has_token: bool) {
    // Centered popup area (width clamped to terminal - 4 by centered_rect)
    let popup_area = centered_rect(70, if has_token { 13 } else { 17 }, frame.area());

    // Clear the popup area first to remove any underlying content
    popup_shell(
        frame,
        popup_area,
        "Authentication Required",
        Style::default().fg(Color::Yellow).bg(Color::Black),
    );

    // Render message
    let message_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(4),
        height: popup_area.height.saturating_sub(3),
    };

    let mut lines = vec![
        Line::from(Span::styled(
            "This model requires authentication to download.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Steps to access this model:",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("1. ", Style::default().fg(Color::Yellow)),
            Span::raw("Visit: "),
            Span::styled(model_url, Style::default().fg(Color::Blue)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("2. ", Style::default().fg(Color::Yellow)),
            Span::raw("Sign the model usage agreement/waiver"),
        ]),
        Line::from(""),
    ];

    if has_token {
        lines.push(Line::from(vec![
            Span::styled("3. ", Style::default().fg(Color::Yellow)),
            Span::raw("Ensure your token has access to this model"),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("3. ", Style::default().fg(Color::Yellow)),
            Span::raw("Create a HuggingFace token at:"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                "https://huggingface.co/settings/tokens",
                Style::default().fg(Color::Blue),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("4. ", Style::default().fg(Color::Yellow)),
            Span::raw("Press "),
            Span::styled(
                "'o'",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" and add token in Options"),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press ESC or Enter to dismiss",
        Style::default().fg(Color::DarkGray),
    )));

    let message = Paragraph::new(lines)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });

    frame.render_widget(message, message_area);
}
