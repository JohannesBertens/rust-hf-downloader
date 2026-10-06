//! GGUF display mode: quantization groups and the files of the selected
//! group (plan W3.4b split out of `render.rs`; bodies byte-identical).

use super::{border_style, panel_list, FocusCtx};
use crate::fmt::size_full;
use crate::models::{FocusedPane, QuantizationGroup, QuantizationInfo};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{ListItem, ListState},
    Frame,
};
use std::collections::HashMap;

/// GGUF-mode panel inputs (W5.2 group; formerly flat `RenderParams`
/// fields). Focus/hover styling and hit-rect registration travel
/// separately — every panel renderer shares those.
pub struct GgufPanelContext<'a> {
    pub quantizations: &'a [QuantizationGroup],
    pub quant_list_state: &'a mut ListState,
    pub quant_file_list_state: &'a mut ListState,
    pub loading_quants: bool,
    pub complete_downloads: &'a HashMap<String, crate::models::DownloadMetadata>,
}

pub(super) fn render_gguf_panels(
    frame: &mut Frame,
    chunks: std::rc::Rc<[Rect]>,
    ctx: GgufPanelContext<'_>,
    focus: &FocusCtx,
    panel_areas: &mut Vec<(FocusedPane, Rect)>,
) {
    let GgufPanelContext {
        quantizations,
        quant_list_state,
        quant_file_list_state,
        loading_quants,
        complete_downloads,
    } = ctx;

    // Left side: Quantization types
    let quant_title = if loading_quants {
        "Quantization Types [Loading...]"
    } else if quantizations.is_empty() {
        "Quantization Types [Select a model to view]"
    } else {
        "Quantization Types"
    };

    let quant_items: Vec<ListItem> = quantizations
        .iter()
        .map(|group| {
            let size_str = size_full(group.total_size);
            let is_downloaded = complete_downloads.contains_key(&group.files[0].filename);

            let mut spans = vec![
                Span::raw(format!("{:>10}  ", size_str)),
                Span::styled(
                    format!("{:<14} ", group.quant_type),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ];

            if is_downloaded {
                spans.push(Span::styled(
                    " [downloaded]",
                    Style::default().fg(Color::Green),
                ));
            } else {
                let file_count = if group.files.len() > 1 {
                    format!(" ({} files)", group.files.len())
                } else {
                    String::new()
                };
                spans.push(Span::styled(
                    file_count,
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let content = Line::from(spans);
            ListItem::new(content)
        })
        .collect();

    let quant_list = panel_list(
        quant_items,
        quant_title,
        border_style(FocusedPane::QuantizationGroups, focus),
    );

    // Register panel area for click/hover detection (left before right —
    // registration order is behavior, see `MouseAreas`)
    panel_areas.push((FocusedPane::QuantizationGroups, chunks[0]));
    frame.render_stateful_widget(quant_list, chunks[0], quant_list_state);

    // Right side: Files for selected quantization
    let selected_quant_idx = quant_list_state.selected();
    let files_for_selected: Vec<QuantizationInfo> = if let Some(idx) = selected_quant_idx {
        if idx < quantizations.len() {
            quantizations[idx].files.clone()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let file_title = if files_for_selected.is_empty() {
        "Files [Select a quantization type]"
    } else {
        "Files"
    };

    let file_items: Vec<ListItem> = files_for_selected
        .iter()
        .map(|file| {
            let size_str = size_full(file.size);
            let is_downloaded = complete_downloads.contains_key(&file.filename);

            // Inner width minus borders (2), highlight-symbol gutter (3) and
            // the right-aligned size column (12). Reserve room for the
            // " [downloaded]" suffix when it will be shown, and truncate with
            // an ellipsis so long shard names are visibly cut, never clipped
            // at the panel border.
            let mut name_budget = (chunks[1].width.saturating_sub(2 + 3 + 12)) as usize;
            if is_downloaded {
                name_budget = name_budget.saturating_sub(" [downloaded]".len());
            }
            let shown_name = crate::fmt::truncate_filename(&file.filename, name_budget);

            let mut spans = vec![Span::raw(format!("{:>10}  ", size_str))];

            if is_downloaded {
                spans.push(Span::styled(shown_name, Style::default().fg(Color::Green)));
                spans.push(Span::styled(
                    " [downloaded]",
                    Style::default().fg(Color::Green),
                ));
            } else {
                spans.push(Span::styled(shown_name, Style::default().fg(Color::White)));
            }

            let content = Line::from(spans);
            ListItem::new(content)
        })
        .collect();

    let file_list = panel_list(
        file_items,
        file_title,
        border_style(FocusedPane::QuantizationFiles, focus),
    );

    // Register panel area for click/hover detection
    panel_areas.push((FocusedPane::QuantizationFiles, chunks[1]));
    frame.render_stateful_widget(file_list, chunks[1], quant_file_list_state);
}
