//! Results-list item building (plan W3.4b): the per-model `ListItem` spans
//! rendered into the Results pane by `render_ui`.

use crate::fmt::number;
use crate::models::ModelInfo;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

/// plans/flare-models.md §3.4a: the search-list FLARE badge rule — any
/// tag equals `flare` or
/// starts with `flare-` (covers all three flagship repos: two carry the
/// exact `flare` tag, JohannesBertens' carries `flare-runtime-package`).
/// Tag-based only: no extra fetch.
pub(super) fn model_has_flare_tag(tags: &[String]) -> bool {
    tags.iter()
        .any(|tag| tag == "flare" || tag.starts_with("flare-"))
}

/// Build the Results list: one line per model with index, id, author
/// (derived from the id when absent), download/like counts, the
/// last-modified date, the plans/flare-models.md §3.4a `[FLARE]` badge,
/// and up to three tags.
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

            let flare_badge = if model_has_flare_tag(&model.tags) {
                Span::styled(
                    " [FLARE]",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
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
                flare_badge,
                Span::styled(tags_str, Style::default().fg(Color::Yellow)),
            ]);

            ListItem::new(content)
        })
        .collect()
}
