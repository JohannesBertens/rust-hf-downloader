//! Results-list item building (plan W3.4b): the per-model `ListItem` spans
//! rendered into the Results pane by `render_ui`.

use crate::fmt::number;
use crate::models::ModelInfo;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

/// Build the Results list: one line per model with index, id, author
/// (derived from the id when absent), download/like counts, the
/// last-modified date and up to three tags.
pub(super) fn model_list_items(models: &[ModelInfo]) -> Vec<ListItem<'_>> {
    models
        .iter()
        .enumerate()
        .map(|(idx, model)| {
            // Extract author from model.id if not provided (e.g., "unsloth/model" -> "unsloth")
            let author = model
                .author
                .as_deref()
                .or_else(|| model.id.split('/').next())
                .unwrap_or("unknown");
            let downloads = number(model.downloads);
            let likes = number(model.likes);

            let tags_str = if model.tags.is_empty() {
                String::new()
            } else {
                format!(
                    " [{}]",
                    model
                        .tags
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };

            let last_modified_str = if let Some(ref modified) = model.last_modified {
                if !modified.is_empty() {
                    // Parse and format date in short format (YYYY-MM-DD)
                    let date_part: &str = modified.split('T').next().unwrap_or("");
                    if date_part.len() >= 10 {
                        format!(" [{}]", &date_part[..10])
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let content = Line::from(vec![
                Span::styled(
                    format!("{:3}. ", idx + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    &model.id,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" by "),
                Span::styled(author, Style::default().fg(Color::Green)),
                Span::raw(format!(" ↓{} ♥{}", downloads, likes)),
                Span::styled(last_modified_str, Style::default().fg(Color::Cyan)),
                Span::styled(tags_str, Style::default().fg(Color::Yellow)),
            ]);

            ListItem::new(content)
        })
        .collect()
}
