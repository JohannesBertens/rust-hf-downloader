//! Standard display mode: the model-metadata panel and the repository
//! file tree (plan W3.4b split out of `render.rs`; bodies byte-identical).

use super::{border_style, panel_list, FocusCtx};
use crate::fmt::size_full;
use crate::models::{FileTreeNode, FocusedPane, ModelMetadata};
use crate::ui::tree::{count_tree_files, flatten_tree};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

/// Standard-mode panel inputs (W5.2 group; formerly flat `RenderParams`
/// fields). `loading` is the shared quant-loading flag — Standard mode
/// reuses the GGUF loads' spinner. Focus/hover styling and hit-rect
/// registration travel separately — every panel renderer shares those.
pub struct StandardPanelContext<'a> {
    pub model_metadata: &'a Option<ModelMetadata>,
    pub file_tree: &'a Option<FileTreeNode>,
    pub file_tree_state: &'a mut ListState,
    pub loading: bool,
}

pub(super) fn render_standard_panels(
    frame: &mut Frame,
    chunks: std::rc::Rc<[Rect]>,
    ctx: StandardPanelContext<'_>,
    focus: &FocusCtx,
    panel_areas: &mut Vec<(FocusedPane, Rect)>,
) {
    let StandardPanelContext {
        model_metadata,
        file_tree,
        file_tree_state,
        loading,
    } = ctx;

    // Left side: Model metadata
    let meta_title = if loading {
        "Model Information [Loading...]"
    } else if model_metadata.is_none() {
        "Model Information [Select a model to view]"
    } else {
        "Model Information"
    };

    let metadata_content = if let Some(metadata) = model_metadata {
        let mut lines = vec![Line::from(vec![
            Span::styled("ID: ", Style::default().fg(Color::Yellow)),
            Span::raw(&metadata.model_id),
        ])];

        if let Some(ref lib) = metadata.library_name {
            lines.push(Line::from(vec![
                Span::styled("Library: ", Style::default().fg(Color::Yellow)),
                Span::raw(lib),
            ]));
        }

        if let Some(ref pipeline) = metadata.pipeline_tag {
            lines.push(Line::from(vec![
                Span::styled("Pipeline: ", Style::default().fg(Color::Yellow)),
                Span::raw(pipeline),
            ]));
        }

        if let Some(ref card_data) = metadata.card_data {
            if let Some(ref base) = card_data.base_model {
                lines.push(Line::from(vec![
                    Span::styled("Base Model: ", Style::default().fg(Color::Yellow)),
                    Span::raw(base),
                ]));
            }
            if let Some(ref license) = card_data.license {
                lines.push(Line::from(vec![
                    Span::styled("License: ", Style::default().fg(Color::Yellow)),
                    Span::raw(license),
                ]));
            }
            if let Some(ref languages) = card_data.language {
                lines.push(Line::from(vec![
                    Span::styled("Languages: ", Style::default().fg(Color::Yellow)),
                    Span::raw(languages.join(", ")),
                ]));
            }
        }

        let file_count = metadata.siblings.len();
        let total_size: u64 = metadata.siblings.iter().filter_map(|f| f.size).sum();
        lines.push(Line::from(vec![
            Span::styled("Files: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{} ({})", file_count, size_full(total_size))),
        ]));

        if !metadata.tags.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Tags:",
                Style::default().fg(Color::Yellow),
            )]));
            let tags_str = metadata
                .tags
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(Span::raw(tags_str)));
        }

        lines
    } else {
        vec![Line::from("No model selected")]
    };

    let metadata_widget = Paragraph::new(metadata_content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(meta_title)
                .border_style(border_style(FocusedPane::ModelMetadata, focus)),
        )
        .wrap(Wrap { trim: false });

    // Register panel area for click/hover detection (left before right —
    // registration order is behavior, see `MouseAreas`)
    panel_areas.push((FocusedPane::ModelMetadata, chunks[0]));
    frame.render_widget(metadata_widget, chunks[0]);

    // Right side: File tree
    render_file_tree_panel(
        frame,
        chunks[1],
        file_tree,
        file_tree_state,
        focus,
        panel_areas,
    );
}

fn render_file_tree_panel(
    frame: &mut Frame,
    area: Rect,
    file_tree: &Option<FileTreeNode>,
    file_tree_state: &mut ListState,
    focus: &FocusCtx,
    panel_areas: &mut Vec<(FocusedPane, Rect)>,
) {
    let tree_title = if file_tree.is_none() {
        "Repository Files [Select a model to view]"
    } else {
        "Repository Files"
    };

    let tree_items: Vec<ListItem> = if let Some(tree) = file_tree {
        flatten_tree(tree)
            .into_iter()
            .map(|node| {
                let indent = "  ".repeat(node.depth);
                let icon = if node.is_dir {
                    if node.expanded {
                        "▾ "
                    } else {
                        "▸ "
                    }
                } else {
                    "  "
                };

                let mut spans = vec![
                    Span::raw(indent),
                    Span::styled(icon, Style::default().fg(Color::Cyan)),
                ];

                if node.is_dir {
                    // Directory: show name, size, and file count
                    spans.push(Span::styled(
                        format!("{}/", node.name),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));

                    let size_str = node
                        .size
                        .map(size_full)
                        .unwrap_or_else(|| String::from("-"));
                    let file_count = count_tree_files(&node);

                    spans.push(Span::raw(format!("  {}", size_str)));
                    spans.push(Span::styled(
                        format!(" ({} files)", file_count),
                        Style::default().fg(Color::DarkGray),
                    ));
                } else {
                    // File: show name and size
                    let size_str = node
                        .size
                        .map(size_full)
                        .unwrap_or_else(|| String::from("-"));
                    spans.push(Span::raw(node.name.clone()));
                    spans.push(Span::raw(format!("  {}", size_str)));
                }

                ListItem::new(Line::from(spans))
            })
            .collect()
    } else {
        vec![]
    };

    let tree_list = panel_list(
        tree_items,
        tree_title,
        border_style(FocusedPane::FileTree, focus),
    );

    // Register panel area for click/hover detection
    panel_areas.push((FocusedPane::FileTree, area));
    frame.render_stateful_widget(tree_list, area, file_tree_state);
}
