use crate::models::{
    DownloadProgress, FileTreeNode, FocusedPane, InputMode, ModelDisplayMode, ModelInfo,
    ModelMetadata, QuantizationGroup, QuantizationInfo, QueueItemSummary, VerificationProgress,
};
use crate::utils::{format_number, format_size};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tui_input::Input;

/// Parameters for rendering the UI
pub struct RenderParams<'a> {
    pub input: &'a Input,
    pub input_mode: InputMode,
    pub models: &'a [ModelInfo],
    pub list_state: &'a mut ListState,
    pub loading: bool,
    pub quantizations: &'a [QuantizationGroup],
    pub quant_file_list_state: &'a mut ListState,
    pub quant_list_state: &'a mut ListState,
    pub loading_quants: bool,
    pub focused_pane: FocusedPane,
    pub error: &'a Option<String>,
    pub status: &'a str,
    pub selection_info: &'a str,
    pub complete_downloads: &'a HashMap<String, crate::models::DownloadMetadata>,
    // Non-GGUF model support
    pub display_mode: ModelDisplayMode,
    pub model_metadata: &'a Option<ModelMetadata>,
    pub file_tree: &'a Option<FileTreeNode>,
    pub file_tree_state: &'a mut ListState,
    // Filter & Sort
    pub sort_field: crate::models::SortField,
    pub sort_direction: crate::models::SortDirection,
    pub filter_min_downloads: u64,
    pub filter_min_likes: u64,
    pub focused_filter_field: usize,
    // Mouse panel areas (for click/hover detection on panels)
    pub panel_areas: &'a mut Vec<(FocusedPane, Rect)>,
    pub hovered_panel: &'a Option<FocusedPane>,
    // Filter toolbar click areas
    pub filter_areas: &'a mut Vec<(usize, Rect)>,
    // Activity HUD strip height reserved above the status bar (0 = hidden)
    pub hud_height: u16,
}

pub fn render_ui(frame: &mut Frame, params: RenderParams) {
    let RenderParams {
        input,
        input_mode,
        models,
        list_state,
        loading,
        quantizations,
        quant_file_list_state,
        quant_list_state,
        loading_quants,
        focused_pane,
        error,
        status,
        selection_info,
        complete_downloads,
        display_mode,
        model_metadata,
        file_tree,
        file_tree_state,
        sort_field,
        sort_direction,
        filter_min_downloads,
        filter_min_likes,
        focused_filter_field,
        panel_areas,
        hovered_panel,
        filter_areas,
        hud_height,
    } = params;

    // Clear previous panel and filter areas
    panel_areas.clear();
    filter_areas.clear();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),          // Filter toolbar
            Constraint::Min(10),            // Main content (models list)
            Constraint::Length(12),         // Bottom panels
            Constraint::Length(hud_height), // Activity HUD (Option 3 matrix)
            Constraint::Length(4),          // Status bar
        ])
        .split(frame.area());

    // Render filter toolbar
    render_filter_toolbar(
        frame,
        chunks[0],
        sort_field,
        sort_direction,
        filter_min_downloads,
        filter_min_likes,
        focused_filter_field,
        filter_areas,
    );

    // Helper to determine border style based on focus and hover state
    let get_border_style = |pane: FocusedPane| -> Style {
        if input_mode == InputMode::Normal && focused_pane == pane {
            Style::default().fg(Color::Yellow)
        } else if hovered_panel.as_ref() == Some(&pane) {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        }
    };

    // Results list (chunks[1])
    let items: Vec<ListItem> = models
        .iter()
        .enumerate()
        .map(|(idx, model)| {
            // Extract author from model.id if not provided (e.g., "unsloth/model" -> "unsloth")
            let author = model
                .author
                .as_deref()
                .or_else(|| model.id.split('/').next())
                .unwrap_or("unknown");
            let downloads = format_number(model.downloads);
            let likes = format_number(model.likes);

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
        .collect();

    let list_title = if loading {
        "Results [Loading...]"
    } else if models.is_empty() && !input.value().is_empty() {
        "Results [No models found]"
    } else if models.is_empty() {
        "Results [Enter a search query]"
    } else {
        "Results"
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(list_title)
                .border_style(get_border_style(FocusedPane::Models)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Store panel area for click/hover detection
    panel_areas.push((FocusedPane::Models, chunks[1]));
    frame.render_stateful_widget(list, chunks[1], list_state);

    // Split bottom panel into left and right sections
    let bottom_panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    // Render based on display mode
    match display_mode {
        ModelDisplayMode::Gguf => {
            render_gguf_panels(
                frame,
                bottom_panel_chunks,
                GgufPanelContext {
                    quantizations,
                    quant_list_state,
                    quant_file_list_state,
                    loading_quants,
                    input_mode,
                    focused_pane,
                    complete_downloads,
                    hovered_panel,
                    panel_areas,
                },
            );
        }
        ModelDisplayMode::Standard => {
            render_standard_panels(
                frame,
                bottom_panel_chunks,
                StandardPanelContext {
                    model_metadata,
                    file_tree,
                    file_tree_state,
                    loading: loading_quants,
                    input_mode,
                    focused_pane,
                    hovered_panel,
                    panel_areas,
                },
            );
        }
    }

    // Status bar with 2 lines: selection_info and status message
    let line1 = if !selection_info.is_empty() {
        selection_info.to_string()
    } else if let Some(selected) = list_state.selected() {
        if selected < models.len() {
            let model = &models[selected];
            format!(
                "Selected: {} | URL: https://huggingface.co/{}",
                model.id, model.id
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Check if any filters are non-default
    let has_filters = filter_min_downloads > 0
        || filter_min_likes > 0
        || sort_field != crate::models::SortField::Downloads
        || sort_direction != crate::models::SortDirection::Descending;

    let base_line2 = if let Some(err) = error {
        format!("Error: {}", err)
    } else {
        status.to_string()
    };

    let line2 = if has_filters {
        format!("{} [Filters Active]", base_line2)
    } else {
        base_line2
    };

    let status_text = if !line1.is_empty() {
        format!("{}\n{}", line1, line2)
    } else {
        line2
    };

    let status_widget = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .style(if error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        })
        .wrap(Wrap { trim: true });

    frame.render_widget(status_widget, chunks[4]);
}

struct StandardPanelContext<'a> {
    model_metadata: &'a Option<ModelMetadata>,
    file_tree: &'a Option<FileTreeNode>,
    file_tree_state: &'a mut ListState,
    loading: bool,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    hovered_panel: &'a Option<FocusedPane>,
    panel_areas: &'a mut Vec<(FocusedPane, Rect)>,
}

fn render_standard_panels(
    frame: &mut Frame,
    chunks: std::rc::Rc<[Rect]>,
    ctx: StandardPanelContext,
) {
    let StandardPanelContext {
        model_metadata,
        file_tree,
        file_tree_state,
        loading,
        input_mode,
        focused_pane,
        hovered_panel,
        panel_areas,
    } = ctx;

    // Helper to determine border style based on focus and hover state
    let get_border_style = |pane: FocusedPane| -> Style {
        if input_mode == InputMode::Normal && focused_pane == pane {
            Style::default().fg(Color::Yellow)
        } else if hovered_panel.as_ref() == Some(&pane) {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        }
    };
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
            Span::raw(format!("{} ({})", file_count, format_size(total_size))),
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
                .border_style(get_border_style(FocusedPane::ModelMetadata)),
        )
        .wrap(Wrap { trim: false });

    // Store panel area for click/hover detection
    panel_areas.push((FocusedPane::ModelMetadata, chunks[0]));
    frame.render_widget(metadata_widget, chunks[0]);

    // Right side: File tree
    render_file_tree_panel(
        frame,
        chunks[1],
        file_tree,
        file_tree_state,
        input_mode,
        focused_pane,
        hovered_panel,
        panel_areas,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_file_tree_panel(
    frame: &mut Frame,
    area: Rect,
    file_tree: &Option<FileTreeNode>,
    file_tree_state: &mut ListState,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    hovered_panel: &Option<FocusedPane>,
    panel_areas: &mut Vec<(FocusedPane, Rect)>,
) {
    // Helper to determine border style based on focus and hover state
    let get_border_style = |pane: FocusedPane| -> Style {
        if input_mode == InputMode::Normal && focused_pane == pane {
            Style::default().fg(Color::Yellow)
        } else if hovered_panel.as_ref() == Some(&pane) {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        }
    };
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
                        .map(format_size)
                        .unwrap_or_else(|| String::from("-"));
                    let file_count = count_files(&node);

                    spans.push(Span::raw(format!("  {}", size_str)));
                    spans.push(Span::styled(
                        format!(" ({} files)", file_count),
                        Style::default().fg(Color::DarkGray),
                    ));
                } else {
                    // File: show name and size
                    let size_str = node
                        .size
                        .map(format_size)
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

    let tree_list = List::new(tree_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(tree_title)
                .border_style(get_border_style(FocusedPane::FileTree)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Store panel area for click/hover detection
    panel_areas.push((FocusedPane::FileTree, area));
    frame.render_stateful_widget(tree_list, area, file_tree_state);
}

/// Count total number of files within a node (recursive)
fn count_files(node: &FileTreeNode) -> usize {
    if node.is_dir {
        node.children.iter().map(count_files).sum()
    } else {
        1
    }
}

/// Flatten tree into a list for rendering
fn flatten_tree(node: &FileTreeNode) -> Vec<FileTreeNode> {
    let mut result = Vec::new();
    flatten_tree_recursive(node, &mut result);
    result
}

fn flatten_tree_recursive(node: &FileTreeNode, result: &mut Vec<FileTreeNode>) {
    for child in &node.children {
        result.push(child.clone());
        if child.is_dir && child.expanded {
            flatten_tree_recursive(child, result);
        }
    }
}

/// Public helper for flattening tree (used by events.rs for navigation)
pub fn flatten_tree_for_navigation(node: &FileTreeNode) -> Vec<FileTreeNode> {
    flatten_tree(node)
}

struct GgufPanelContext<'a> {
    quantizations: &'a [QuantizationGroup],
    quant_list_state: &'a mut ListState,
    quant_file_list_state: &'a mut ListState,
    loading_quants: bool,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    complete_downloads: &'a HashMap<String, crate::models::DownloadMetadata>,
    hovered_panel: &'a Option<FocusedPane>,
    panel_areas: &'a mut Vec<(FocusedPane, Rect)>,
}

fn render_gguf_panels(frame: &mut Frame, chunks: std::rc::Rc<[Rect]>, ctx: GgufPanelContext) {
    let GgufPanelContext {
        quantizations,
        quant_list_state,
        quant_file_list_state,
        loading_quants,
        input_mode,
        focused_pane,
        complete_downloads,
        hovered_panel,
        panel_areas,
    } = ctx;

    // Helper to determine border style based on focus and hover state
    let get_border_style = |pane: FocusedPane| -> Style {
        if input_mode == InputMode::Normal && focused_pane == pane {
            Style::default().fg(Color::Yellow)
        } else if hovered_panel.as_ref() == Some(&pane) {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        }
    };
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
            let size_str = format_size(group.total_size);
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

    let quant_list = List::new(quant_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(quant_title)
                .border_style(get_border_style(FocusedPane::QuantizationGroups)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Store panel area for click/hover detection
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
            let size_str = format_size(file.size);
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
            let shown_name = truncate_filename(&file.filename, name_budget);

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

    let file_list = List::new(file_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(file_title)
                .border_style(get_border_style(FocusedPane::QuantizationFiles)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Store panel area for click/hover detection
    panel_areas.push((FocusedPane::QuantizationFiles, chunks[1]));
    frame.render_stateful_widget(file_list, chunks[1], quant_file_list_state);
}

/// Truncate a filename to at most `max_chars` using a middle ellipsis, so
/// the tail (shard index, extension) stays visible — the identifying part
/// of multipart names like `model-00002-of-00003.gguf`. Names that already
/// fit are returned unchanged.
fn truncate_filename(name: &str, max_chars: usize) -> String {
    let count = name.chars().count();
    if max_chars == 0 {
        return String::new();
    }
    if count <= max_chars {
        return name.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let tail_len = (max_chars - 1) / 3;
    let head_len = max_chars - 1 - tail_len;
    let head: String = name.chars().take(head_len).collect();
    let tail: String = name.chars().skip(count - tail_len).collect();
    format!("{}…{}", head, tail)
}

/// Format bytes as GB, rounding up. Returns empty string for 0 bytes.
fn format_remaining_gb(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    if bytes == 0 {
        String::new()
    } else if bytes < GB {
        "<1GB".to_string()
    } else {
        let gb = (bytes as f64 / GB as f64).ceil() as u64;
        format!("{}GB", gb)
    }
}

// ============================================================================
// Activity HUD (design Option 3: compact matrix — PLANS/design-options.md)
//
// One fixed-column line per pipeline item, rendered as a strip above the
// status bar. State glyphs (DL/VF/Q) sort rows into pipeline stages without
// boxes; numeric columns are right-aligned for vertical scan paths; row
// count and row height are fixed while active (zero reflow).
// ============================================================================

/// Data bundle for the activity HUD.
pub struct ActivityHudData<'a> {
    pub download_progress: &'a Option<DownloadProgress>,
    pub queue_size: usize,
    pub queue_bytes: u64,
    pub queue_items: &'a [QueueItemSummary],
    pub verification_progress: &'a [VerificationProgress],
    pub verification_queue_size: usize,
    pub verification_queue_bytes: u64,
    pub verified_ok: usize,
    pub verified_fail: usize,
}

/// Maximum item rows before truncation (footer always renders as row +1).
const HUD_MAX_ITEM_ROWS: usize = 8;
/// Queue rows shown individually before collapsing into "+N more".
const HUD_MAX_QUEUE_ROWS: usize = 3;

/// Height of the reserved HUD strip (item rows + 1 footer row + 2 border
/// rows), 0 when idle.
pub fn activity_hud_height(data: &ActivityHudData) -> u16 {
    let rows = hud_item_row_count(data);
    if rows == 0 {
        0
    } else {
        rows.min(HUD_MAX_ITEM_ROWS) as u16 + 1 + 2
    }
}

fn hud_item_row_count(data: &ActivityHudData) -> usize {
    let mut rows = 0usize;
    if data.download_progress.is_some() {
        rows += 1;
    }
    rows += data.verification_progress.len();
    let queue_len = data.queue_items.len();
    rows += queue_len.min(HUD_MAX_QUEUE_ROWS);
    if queue_len > HUD_MAX_QUEUE_ROWS {
        rows += 1; // "+N more" row
    }
    rows
}

/// Render the HUD into `area` (the strip reserved above the status bar).
pub fn render_activity_hud(frame: &mut Frame, area: Rect, data: &ActivityHudData) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    // --- Download row ---
    if let Some(p) = data.download_progress {
        lines.push(download_hud_line(p, data.queue_bytes, w));
    }

    // --- Verification rows ---
    for ver in data.verification_progress {
        lines.push(verification_hud_line(ver, w));
    }

    // --- Queue rows ---
    let queue_len = data.queue_items.len();
    for item in data.queue_items.iter().take(HUD_MAX_QUEUE_ROWS) {
        lines.push(queue_hud_line(
            &item.filename,
            Some(item.total_size),
            w,
            false,
        ));
    }
    if queue_len > HUD_MAX_QUEUE_ROWS {
        let more = queue_len - HUD_MAX_QUEUE_ROWS;
        let more_bytes: u64 = data
            .queue_items
            .iter()
            .skip(HUD_MAX_QUEUE_ROWS)
            .map(|i| i.total_size)
            .sum();
        lines.push(queue_hud_line(
            &format!("+{more} more"),
            Some(more_bytes),
            w,
            true,
        ));
    }

    // Truncate to budget (footer aggregates what was cut)
    lines.truncate(HUD_MAX_ITEM_ROWS);

    // --- Footer aggregate ---
    lines.push(hud_footer_line(data, w));

    let widget =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Activity"));
    frame.render_widget(widget, area);
}

/// Column layout shared by all HUD rows, degraded on narrow terminals.
struct HudColumns {
    name_w: usize,
    pct_w: usize,
    speed_w: usize,
    eta_w: usize,
}

impl HudColumns {
    fn for_width(w: usize) -> Self {
        let (name_w, speed_w, eta_w) = if w >= 100 {
            (24, 9, 7)
        } else if w >= 84 {
            (16, 9, 0)
        } else if w >= 70 {
            (10, 9, 0)
        } else {
            (6, 0, 0)
        };
        HudColumns {
            name_w,
            pct_w: 4,
            speed_w,
            eta_w,
        }
    }

    /// Visible width of the right-aligned block (pct + speed + eta + gaps).
    fn right_w(&self) -> usize {
        let mut w = self.pct_w;
        if self.speed_w > 0 {
            w += 1 + self.speed_w;
        }
        if self.eta_w > 0 {
            w += 1 + self.eta_w;
        }
        w
    }

    /// Middle space available for bar (+ chunk map on the DL row).
    fn middle_w(&self, total_w: usize) -> usize {
        total_w.saturating_sub(4 + self.name_w + 1 + 1 + self.right_w() + 2) // borders + paddings
    }
}

fn download_hud_line(p: &DownloadProgress, queue_bytes: u64, w: usize) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    let pct = if p.total > 0 {
        (p.downloaded as f64 / p.total as f64 * 100.0) as u16
    } else {
        0
    };

    // Speed + ETA over current file plus the rest of the queue
    let current_remaining = p.total.saturating_sub(p.downloaded);
    let total_remaining = current_remaining + queue_bytes;
    let speed_str = if p.speed_mbps > 0.0 {
        format_speed_hud(p.speed_mbps)
    } else {
        "--".to_string()
    };
    let eta_str = if p.speed_mbps > 0.0 {
        let secs = total_remaining as f64 / (p.speed_mbps * 1_048_576.0);
        format_eta_hud(secs as u64)
    } else {
        "--".to_string()
    };

    let mut spans = vec!["DL ".into_cyan(), Span::raw("  ")];
    spans.push(Span::raw(truncate_name_middle(&p.filename, cols.name_w)));
    spans.push(Span::raw(" "));

    // Bar + (if room) "ch n/total" chunk map filling the remaining space
    let middle = cols.middle_w(w);
    let label = format!(
        "ch {}/{}",
        p.chunk_completed.iter().filter(|b| **b).count(),
        p.num_chunks
    );
    let active_ids: Vec<usize> = p
        .chunks
        .iter()
        .filter(|c| c.is_active)
        .map(|c| c.chunk_id)
        .collect();
    let label_w = label.chars().count() + 1; // + leading gap
    let mut bar_w = middle;
    let mut map_cells = 0;
    if p.num_chunks > 0 && middle >= 24 + label_w + 4 {
        bar_w = 24;
        map_cells = middle - bar_w - label_w;
        if map_cells > p.num_chunks {
            map_cells = p.num_chunks;
            bar_w = middle - map_cells - label_w;
        }
    }
    if bar_w > 0 {
        let (filled, bar) = bar_spans(pct, bar_w, Color::Cyan);
        spans.push(bar);
        let _ = filled;
    }
    if map_cells > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
        spans.push(Span::raw(" "));
        spans.extend(chunk_map_spans(&p.chunk_completed, &active_ids, map_cells));
    }

    spans.push(right_block_spans(
        &cols,
        Some(pct),
        Some(&speed_str),
        Some(&eta_str),
    ));
    Line::from(spans)
}

fn verification_hud_line(ver: &VerificationProgress, w: usize) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    // Ordering::Relaxed is safe here: eventual consistency is fine for display
    let verified = ver.verified_bytes.load(Ordering::Relaxed);
    let pct = if ver.total_bytes > 0 {
        (verified as f64 / ver.total_bytes as f64 * 100.0) as u16
    } else {
        0
    };
    let speed_str = if ver.speed_mbps > 0.0 {
        format_speed_hud(ver.speed_mbps)
    } else {
        "--".to_string()
    };
    let eta_str = if ver.speed_mbps > 0.0 && ver.total_bytes > verified {
        let remaining = (ver.total_bytes - verified) as f64 / (ver.speed_mbps * 1_048_576.0);
        format_eta_hud(remaining as u64)
    } else {
        "--".to_string()
    };

    let mut spans = vec!["VF ".into_green(), Span::raw("  ")];
    spans.push(Span::raw(truncate_name_middle(&ver.filename, cols.name_w)));
    spans.push(Span::raw(" "));
    let middle = cols.middle_w(w);
    if middle >= 8 {
        spans.push(bar_spans(pct, middle, Color::Green).1);
    }
    spans.push(right_block_spans(
        &cols,
        Some(pct),
        Some(&speed_str),
        Some(&eta_str),
    ));
    Line::from(spans)
}

fn queue_hud_line(name: &str, size: Option<u64>, w: usize, dim: bool) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    let size_str = size
        .map(format_bytes_hud)
        .unwrap_or_else(|| "--".to_string());
    let name_span = if dim {
        Span::styled(
            truncate_name_middle(name, cols.name_w),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::raw(truncate_name_middle(name, cols.name_w))
    };

    let mut spans = vec!["Q  ".into_gray(), Span::raw("  ")];
    spans.push(name_span);
    spans.push(Span::raw(" "));
    // Dim dotted rail fills the middle so the right block stays aligned
    let middle = cols.middle_w(w);
    if middle > 0 {
        let mut rail = String::new();
        while rail.chars().count() + 5 <= middle {
            rail.push_str("\u{b7}    ");
        }
        let used = rail.chars().count();
        rail.push_str(&" ".repeat(middle - used));
        spans.push(Span::styled(rail, Style::default().fg(Color::DarkGray)));
    }
    spans.push(right_block_spans(
        &cols,
        None,
        Some(&size_str),
        Some("wait"),
    ));
    Line::from(spans)
}

fn hud_footer_line(data: &ActivityHudData<'_>, w: usize) -> Line<'static> {
    let mut parts: Vec<String> = Vec::new();
    if data.verified_ok > 0 || data.verified_fail > 0 {
        parts.push(format!(
            "hash ✓{} ✗{}",
            data.verified_ok, data.verified_fail
        ));
    }
    if data.verification_queue_size > 0 {
        parts.push(format!(
            "verify q {} ({})",
            data.verification_queue_size,
            format_bytes_hud(data.verification_queue_bytes)
        ));
    }
    if data.queue_size > 0 {
        parts.push(format!(
            "dl q {} ({})",
            data.queue_size,
            format_bytes_hud(data.queue_bytes)
        ));
    }
    if let Some(p) = data.download_progress {
        if p.speed_mbps > 0.0 {
            let total_remaining = p.total.saturating_sub(p.downloaded) + data.queue_bytes;
            let secs = total_remaining as f64 / (p.speed_mbps * 1_048_576.0);
            parts.push(format!(
                "remaining {} {}",
                format_remaining_gb(total_remaining),
                format_eta_hud(secs as u64)
            ));
        }
    }

    let inner_w = w.saturating_sub(2); // borders
    let text = if parts.is_empty() {
        "idle".to_string()
    } else {
        parts.join(" · ")
    };
    let used = text.chars().count() + 4; // "── " prefix + " " suffix
    let fill = inner_w.saturating_sub(used);
    let mut s = format!("── {text} ");
    s.push_str(&"─".repeat(fill));
    Line::from(Span::styled(s, Style::default().fg(Color::DarkGray)))
}

/// Filled/empty bar spans; `filled` count is returned for potential reuse.
fn bar_spans(pct: u16, width: usize, color: Color) -> (usize, Span<'static>) {
    let filled = (width as f64 * pct as f64 / 100.0).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    (
        filled,
        Span::styled(
            format!("{}{}", "█".repeat(filled), "░".repeat(empty)),
            Style::default().fg(color),
        ),
    )
}

/// Map each of `cells` positions to its chunk bucket and pick a state char.
fn chunk_map_spans(completed: &[bool], active_ids: &[usize], cells: usize) -> Vec<Span<'static>> {
    let total = completed.len();
    if total == 0 || cells == 0 {
        return Vec::new();
    }
    let mut spans = Vec::with_capacity(cells);
    for i in 0..cells {
        let bucket = i * total / cells;
        let (ch, color) = if completed[bucket] {
            ('█', Color::Cyan) // done
        } else if active_ids.contains(&bucket) {
            ('▓', Color::Yellow) // active
        } else {
            ('░', Color::DarkGray) // pending
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }
    spans
}

/// Right-aligned pct / speed / eta block.
fn right_block_spans(
    cols: &HudColumns,
    pct: Option<u16>,
    speed: Option<&str>,
    eta: Option<&str>,
) -> Span<'static> {
    let mut s = String::new();
    match pct {
        Some(p) => s.push_str(&format!("{:>3}%", p)),
        None => s.push_str("  --"),
    }
    if cols.speed_w > 0 {
        let sp = speed.unwrap_or("");
        s.push(' ');
        s.push_str(&format!("{:>width$}", sp, width = cols.speed_w));
    }
    if cols.eta_w > 0 {
        let e = eta.unwrap_or("");
        s.push(' ');
        s.push_str(&format!("{:>width$}", e, width = cols.eta_w));
    }
    Span::raw(s)
}

/// Compact byte size for HUD columns, e.g. "38.2GB", "512MB".
fn format_bytes_hud(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    const GB: f64 = 1_073_741_824.0;
    let b = bytes as f64;
    if bytes >= 1 << 30 {
        format!("{:.1}GB", b / GB)
    } else if bytes >= 1 << 20 {
        format!("{:.0}MB", b / MB)
    } else if bytes >= 1024 {
        format!("{:.0}KB", b / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

/// Compact throughput, e.g. "32.8MB/s" or "1.9GB/s".
fn format_speed_hud(mbps: f64) -> String {
    if mbps >= 1024.0 {
        format!("{:.1}GB/s", mbps / 1024.0)
    } else {
        format!("{:.1}MB/s", mbps)
    }
}

/// Compact ETA bounded to the HUD eta column (7 cells), e.g. "~1h05m",
/// "~12m21s", "~42s".
fn format_eta_hud(secs: u64) -> String {
    if secs >= 3600 {
        format!("~{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("~{}m{}s", secs / 60, secs % 60)
    } else {
        format!("~{}s", secs)
    }
}

/// Middle-truncate a name to `max` chars (char-based, UTF-8 safe), keeping
/// head and tail with a `~` marker: "model~.gguf".
fn truncate_name_middle(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 3 {
        return name.chars().take(max).collect();
    }
    let tail_w = (max - 1) / 2;
    let head_w = max - 1 - tail_w;
    let head: String = chars[..head_w].iter().collect();
    let tail: String = chars[chars.len() - tail_w..].iter().collect();
    format!("{head}~{tail}")
}

trait StateGlyph {
    fn into_cyan(self) -> Span<'static>;
    fn into_green(self) -> Span<'static>;
    fn into_gray(self) -> Span<'static>;
}

impl StateGlyph for &str {
    fn into_cyan(self) -> Span<'static> {
        Span::styled(
            format!("{:<2}", self),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }
    fn into_green(self) -> Span<'static> {
        Span::styled(
            format!("{:<2}", self),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    }
    fn into_gray(self) -> Span<'static> {
        Span::styled(format!("{:<2}", self), Style::default().fg(Color::DarkGray))
    }
}

pub fn render_resume_popup(
    frame: &mut Frame,
    incomplete_downloads: &[crate::models::DownloadMetadata],
) {
    // Calculate centered popup area
    let popup_width = 70.min(frame.area().width.saturating_sub(4));
    let popup_height = 10 + incomplete_downloads.len().min(5) as u16;
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the popup area first to remove any underlying content
    frame.render_widget(Clear, popup_area);

    // Render popup background
    let popup_block = Block::default()
        .borders(Borders::ALL)
        .title("Resume Incomplete Downloads?")
        .style(Style::default().fg(Color::Yellow).bg(Color::Black));

    frame.render_widget(popup_block, popup_area);

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
    let popup_width = 60.min(frame.area().width.saturating_sub(4));
    let popup_height = 8;
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    let area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the area
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Search HuggingFace Models ")
        .style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

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
    // Calculate centered popup area
    let popup_width = 60.min(frame.area().width.saturating_sub(4));
    let popup_height = 7;
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the popup area first to remove any underlying content
    frame.render_widget(Clear, popup_area);

    // Render popup background
    let popup_block = Block::default()
        .borders(Borders::ALL)
        .title("Download Model")
        .style(Style::default().fg(Color::White).bg(Color::Black));

    frame.render_widget(popup_block, popup_area);

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
    // Calculate centered popup area
    let popup_width = 70.min(frame.area().width.saturating_sub(4));
    let popup_height = if has_token { 13 } else { 17 };
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the popup area first to remove any underlying content
    frame.render_widget(Clear, popup_area);

    // Render popup background
    let popup_block = Block::default()
        .borders(Borders::ALL)
        .title("Authentication Required")
        .style(Style::default().fg(Color::Yellow).bg(Color::Black));

    frame.render_widget(popup_block, popup_area);

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

/// Render filter and sort toolbar
#[allow(clippy::too_many_arguments)]
pub fn render_filter_toolbar(
    frame: &mut Frame,
    area: Rect,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,
    min_likes: u64,
    focused_field: usize,
    filter_areas: &mut Vec<(usize, Rect)>,
) {
    use crate::models::{SortDirection, SortField};

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Filters  [Click to cycle | 1-4: Presets | r: Reset | Ctrl+S: Save]")
        .style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Sort arrow
    let sort_arrow = match sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };

    // Sort name
    let sort_name = match sort_field {
        SortField::Downloads => "Downloads",
        SortField::Likes => "Likes",
        SortField::Modified => "Modified",
        SortField::Name => "Name",
    };

    // Build display line with highlighting for focused field
    let sort_style = if focused_field == 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    let downloads_style = if focused_field == 1 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };

    let likes_style = if focused_field == 2 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };

    // Detect which preset is active (if any)
    let preset_name = if sort_field == SortField::Modified
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 0
    {
        Some("Recent")
    } else if sort_field == SortField::Likes
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 1_000
    {
        Some("Highly Rated")
    } else if sort_field == SortField::Downloads
        && sort_direction == SortDirection::Descending
        && min_downloads == 10_000
        && min_likes == 100
    {
        Some("Popular")
    } else if sort_field == SortField::Downloads
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 0
    {
        Some("No Filters")
    } else {
        None
    };

    // Calculate text segments for click detection
    // Format: "Sort: {value}  |  Min Downloads: {value}  |  Min Likes: {value}"
    let sort_label = "Sort: ";
    let sort_value = format!("{} {}", sort_name, sort_arrow);
    let separator1 = "  |  ";
    let downloads_label = "Min Downloads: ";
    let downloads_value = crate::utils::format_number(min_downloads);
    let separator2 = "  |  ";
    let likes_label = "Min Likes: ";
    let likes_value = crate::utils::format_number(min_likes);

    // Calculate x positions for each clickable area
    let mut x = inner.x;

    // Sort area: includes label and value
    let sort_start = x;
    x += sort_label.len() as u16 + sort_value.len() as u16;
    let sort_area = Rect {
        x: sort_start,
        y: inner.y,
        width: x - sort_start,
        height: 1,
    };
    filter_areas.push((0, sort_area));

    x += separator1.len() as u16;

    // Downloads area: includes label and value
    let downloads_start = x;
    x += downloads_label.len() as u16 + downloads_value.len() as u16;
    let downloads_area = Rect {
        x: downloads_start,
        y: inner.y,
        width: x - downloads_start,
        height: 1,
    };
    filter_areas.push((1, downloads_area));

    x += separator2.len() as u16;

    // Likes area: includes label and value
    let likes_start = x;
    x += likes_label.len() as u16 + likes_value.len() as u16;
    let likes_area = Rect {
        x: likes_start,
        y: inner.y,
        width: x - likes_start,
        height: 1,
    };
    filter_areas.push((2, likes_area));

    let mut line_parts = vec![
        Span::styled(sort_label, Style::default().fg(Color::DarkGray)),
        Span::styled(sort_value, sort_style),
        Span::raw(separator1),
        Span::styled(downloads_label, Style::default().fg(Color::DarkGray)),
        Span::styled(downloads_value, downloads_style),
        Span::raw(separator2),
        Span::styled(likes_label, Style::default().fg(Color::DarkGray)),
        Span::styled(likes_value, likes_style),
    ];

    // Add preset indicator if a preset is active
    if let Some(preset) = preset_name {
        line_parts.push(Span::raw("  |  "));
        line_parts.push(Span::styled(
            format!("[{}]", preset),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let line = Line::from(line_parts);

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, inner);
}

// =====================================================================
// Snapshot tests (insta)
//
// This is a binary crate, so the test module must live inline here to
// access the render functions. All snapshots use a fixed 100x30
// TestBackend terminal and literal fixture constants so the output is
// fully deterministic (no time, randomness, or environment reads).
// =====================================================================

#[cfg(test)]
mod hud_tests {
    use super::*;
    use crate::models::{ChunkProgress, QueueItemSummary};
    use ratatui::Terminal;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    fn dl_progress(done: usize, total_chunks: usize) -> DownloadProgress {
        let mut chunk_completed = vec![false; total_chunks];
        for (i, c) in chunk_completed.iter_mut().enumerate().take(done) {
            *c = true;
            let _ = i;
        }
        DownloadProgress {
            model_id: "a/b".to_string(),
            filename: "model.gguf".to_string(),
            downloaded: 5,
            total: 10,
            speed_mbps: 0.0,
            chunks: Vec::new(),
            verifying: false,
            num_chunks: total_chunks,
            chunk_completed,
        }
    }

    #[test]
    fn hud_height_is_zero_when_idle() {
        let none: Option<DownloadProgress> = None;
        let data = ActivityHudData {
            download_progress: &none,
            queue_size: 0,
            queue_bytes: 0,
            queue_items: &[],
            verification_progress: &[],
            verification_queue_size: 0,
            verification_queue_bytes: 0,
            verified_ok: 0,
            verified_fail: 0,
        };
        assert_eq!(activity_hud_height(&data), 0);
    }

    #[test]
    fn hud_height_counts_rows_plus_footer() {
        let dl = dl_progress(2, 4);
        let v1 = VerificationProgress {
            filename: "a".to_string(),
            verified_bytes: Arc::new(AtomicU64::new(0)),
            total_bytes: 1,
            speed_mbps: 0.0,
        };
        let items: Vec<QueueItemSummary> = (0..5)
            .map(|i| QueueItemSummary {
                filename: format!("f{i}"),
                total_size: 1,
            })
            .collect();
        let none: Option<DownloadProgress> = None;
        // 1 DL + 1 VF + 3 Q + 1 "+more" = 6 rows + footer = 7
        let data = ActivityHudData {
            download_progress: &Some(dl),
            queue_size: 5,
            queue_bytes: 5,
            queue_items: &items,
            verification_progress: &[v1],
            verification_queue_size: 0,
            verification_queue_bytes: 0,
            verified_ok: 0,
            verified_fail: 0,
        };
        assert_eq!(activity_hud_height(&data), 9); // 6 rows + footer + borders
        let _ = none;
    }

    #[test]
    fn hud_height_caps_at_budget() {
        let dl = dl_progress(0, 2);
        let vfs: Vec<VerificationProgress> = (0..4)
            .map(|i| VerificationProgress {
                filename: format!("v{i}"),
                verified_bytes: Arc::new(AtomicU64::new(0)),
                total_bytes: 1,
                speed_mbps: 0.0,
            })
            .collect();
        let items: Vec<QueueItemSummary> = (0..4)
            .map(|i| QueueItemSummary {
                filename: format!("f{i}"),
                total_size: 1,
            })
            .collect();
        // 1 + 4 + 3 + 1 = 9 rows uncapped -> capped to 8 + footer + borders
        let data = ActivityHudData {
            download_progress: &Some(dl),
            queue_size: 4,
            queue_bytes: 4,
            queue_items: &items,
            verification_progress: &vfs,
            verification_queue_size: 0,
            verification_queue_bytes: 0,
            verified_ok: 0,
            verified_fail: 0,
        };
        assert_eq!(activity_hud_height(&data), 11);
    }

    #[test]
    fn chunk_map_marks_done_active_pending() {
        // 4 chunks: 0 done, 1 done, 2 active, 3 pending -> 4 cells 1:1
        let completed = vec![true, true, false, false];
        let map = chunk_map_spans(&completed, &[2], 4);
        assert_eq!(map.len(), 4);
        let chars: String = map.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(chars, "\u{2588}\u{2588}\u{2593}\u{2591}");
    }

    #[test]
    fn chunk_map_scales_down_without_gaps() {
        // 20 chunks (first 10 done), 10 cells -> each cell = 2 chunks
        let completed: Vec<bool> = (0..20).map(|i| i < 10).collect();
        let map = chunk_map_spans(&completed, &[], 10);
        let chars: String = map.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            chars,
            "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}"
        );
    }

    #[test]
    fn truncate_name_keeps_head_and_tail() {
        assert_eq!(
            truncate_name_middle("model-Q4_K_M.gguf", 24),
            "model-Q4_K_M.gguf"
        );
        assert_eq!(
            truncate_name_middle("shard-00001.safetensors", 12),
            "shard-~nsors"
        );
        assert_eq!(truncate_name_middle("ab", 5), "ab");
        // UTF-8 safe
        assert_eq!(truncate_name_middle("模　型　名　称.gguf", 7), "模　型~guf");
    }

    #[test]
    fn format_bytes_and_speed_compact() {
        assert_eq!(format_bytes_hud(0), "0B");
        assert_eq!(format_bytes_hud(2048), "2KB");
        assert_eq!(format_bytes_hud(5 * 1_048_576), "5MB");
        assert_eq!(format_bytes_hud(6_442_450_944), "6.0GB");
        assert_eq!(format_speed_hud(32.84), "32.8MB/s");
        assert_eq!(format_speed_hud(2048.0), "2.0GB/s");
    }

    #[test]
    fn snapshot_activity_hud_renders_matrix() {
        let dl = DownloadProgress {
            model_id: "unsloth/Mistral".to_string(),
            filename: "model-Q4_K_M.gguf".to_string(),
            downloaded: 4_700_000_000,
            total: 10_240_000_000,
            speed_mbps: 32.8,
            chunks: vec![ChunkProgress {
                chunk_id: 12,
                start: 0,
                end: 0,
                downloaded: 1,
                total: 2,
                speed_mbps: 4.0,
                is_active: true,
            }],
            verifying: false,
            num_chunks: 20,
            chunk_completed: (0..20).map(|i| i < 11).collect(),
        };
        let vb = |n: u64| VerificationProgress {
            filename: format!("shard-0000{n}.safetensors"),
            verified_bytes: Arc::new(AtomicU64::new(n * 1_000_000_000)),
            total_bytes: 5_000_000_000,
            speed_mbps: 2000.0,
        };
        let vfs = vec![vb(4), vb(3), vb(2), vb(1)];
        let items: Vec<QueueItemSummary> = vec![
            QueueItemSummary {
                filename: "shard-5.gguf".to_string(),
                total_size: 6_657_199_915,
            },
            QueueItemSummary {
                filename: "shard-6.gguf".to_string(),
                total_size: 6_657_199_915,
            },
            QueueItemSummary {
                filename: "shard-7.gguf".to_string(),
                total_size: 6_657_199_915,
            },
            QueueItemSummary {
                filename: "tokenizer.json".to_string(),
                total_size: 1_048_576,
            },
        ];
        let data = ActivityHudData {
            download_progress: &Some(dl),
            queue_size: 4,
            queue_bytes: 19_972_000_000,
            queue_items: &items,
            verification_progress: &vfs,
            verification_queue_size: 9,
            verification_queue_bytes: 12_884_901_888,
            verified_ok: 2,
            verified_fail: 0,
        };
        let height = activity_hud_height(&data);
        assert_eq!(height, 11); // 8 capped rows + footer + borders

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
        terminal
            .draw(|frame| {
                let area = Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height,
                };
                render_activity_hud(frame, area, &data);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::models::{AppOptions, DownloadMetadata, DownloadStatus, SortDirection, SortField};
    use ratatui::{backend::TestBackend, Terminal};

    const TERMINAL_WIDTH: u16 = 100;
    const TERMINAL_HEIGHT: u16 = 30;

    fn test_terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(TERMINAL_WIDTH, TERMINAL_HEIGHT))
            .expect("failed to create test terminal")
    }

    fn model_fixture(id: &str, downloads: u64, likes: u64) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            author: None,
            downloads,
            likes,
            tags: Vec::new(),
            last_modified: None,
        }
    }

    /// Three realistic model entries covering: derived author, explicit
    /// author, tags, and last_modified rendering.
    fn three_model_fixtures() -> Vec<ModelInfo> {
        let mut first = model_fixture("meta-llama/Llama-3.1-8B", 1_234_567, 12_345);
        first.tags = vec!["text-generation".to_string(), "llama".to_string()];
        first.last_modified = Some("2024-07-03T09:15:00Z".to_string());
        let mut second = model_fixture("mistralai/Mistral-7B-v0.3", 987_654, 8_900);
        second.author = Some("mistralai".to_string());
        second.tags = vec!["text-generation".to_string()];
        let mut third = model_fixture("Qwen/Qwen2.5-Coder-32B-Instruct", 45_678, 678);
        third.last_modified = Some("2024-11-01T00:00:00Z".to_string());
        vec![first, second, third]
    }

    /// Two quantization groups: Q4_K_M with 2 files, Q8_0 with 1 file.
    fn quantization_fixtures() -> Vec<QuantizationGroup> {
        vec![
            QuantizationGroup {
                quant_type: "Q4_K_M".to_string(),
                files: vec![
                    QuantizationInfo {
                        quant_type: "Q4_K_M".to_string(),
                        filename: "Llama-3.1-8B-Q4_K_M.gguf".to_string(),
                        size: 4_921_860_096,
                        sha256: None,
                    },
                    QuantizationInfo {
                        quant_type: "Q4_K_M".to_string(),
                        filename: "Llama-3.1-8B-Q4_K_M-00002-of-00002.gguf".to_string(),
                        size: 1_000_000_000,
                        sha256: None,
                    },
                ],
                total_size: 5_921_860_096,
            },
            QuantizationGroup {
                quant_type: "Q8_0".to_string(),
                files: vec![QuantizationInfo {
                    quant_type: "Q8_0".to_string(),
                    filename: "Llama-3.1-8B-Q8_0.gguf".to_string(),
                    size: 8_500_000_000,
                    sha256: None,
                }],
                total_size: 8_500_000_000,
            },
        ]
    }

    /// Draw `render_ui` on the terminal with fixed defaults for every
    /// RenderParams field not worth varying between tests (all filters at
    /// defaults, no error, no hovered panel, Gguf display mode).
    #[allow(clippy::too_many_arguments)]
    fn draw_render_ui(
        terminal: &mut Terminal<TestBackend>,
        input: &Input,
        models: &[ModelInfo],
        list_state: &mut ListState,
        quantizations: &[QuantizationGroup],
        quant_list_state: &mut ListState,
        quant_file_list_state: &mut ListState,
        focused_pane: FocusedPane,
        status: &str,
        selection_info: &str,
    ) {
        let error: Option<String> = None;
        let model_metadata: Option<ModelMetadata> = None;
        let file_tree: Option<FileTreeNode> = None;
        let mut file_tree_state = ListState::default();
        let complete_downloads: HashMap<String, DownloadMetadata> = HashMap::new();
        let mut panel_areas = Vec::new();
        let hovered_panel: Option<FocusedPane> = None;
        let mut filter_areas = Vec::new();

        terminal
            .draw(|frame| {
                render_ui(
                    frame,
                    RenderParams {
                        input,
                        input_mode: InputMode::Normal,
                        models,
                        list_state,
                        loading: false,
                        quantizations,
                        quant_file_list_state,
                        quant_list_state,
                        loading_quants: false,
                        focused_pane,
                        error: &error,
                        status,
                        selection_info,
                        complete_downloads: &complete_downloads,
                        display_mode: ModelDisplayMode::Gguf,
                        model_metadata: &model_metadata,
                        file_tree: &file_tree,
                        file_tree_state: &mut file_tree_state,
                        sort_field: SortField::Downloads,
                        sort_direction: SortDirection::Descending,
                        filter_min_downloads: 0,
                        filter_min_likes: 0,
                        focused_filter_field: 5,
                        panel_areas: &mut panel_areas,
                        hovered_panel: &hovered_panel,
                        filter_areas: &mut filter_areas,
                        hud_height: 0,
                    },
                );
            })
            .expect("failed to draw UI");
    }

    #[test]
    fn snapshot_render_ui_empty_state() {
        let input = Input::default();
        let models: Vec<ModelInfo> = Vec::new();
        let mut list_state = ListState::default();
        let quantizations: Vec<QuantizationGroup> = Vec::new();
        let mut quant_list_state = ListState::default();
        let mut quant_file_list_state = ListState::default();

        let mut terminal = test_terminal();
        draw_render_ui(
            &mut terminal,
            &input,
            &models,
            &mut list_state,
            &quantizations,
            &mut quant_list_state,
            &mut quant_file_list_state,
            FocusedPane::Models,
            "Press / to search",
            "",
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_render_ui_model_list_selection() {
        let input = Input::new("llama".to_string());
        let models = three_model_fixtures();
        let mut list_state = ListState::default();
        list_state.select(Some(1));
        let quantizations: Vec<QuantizationGroup> = Vec::new();
        let mut quant_list_state = ListState::default();
        let mut quant_file_list_state = ListState::default();

        let mut terminal = test_terminal();
        draw_render_ui(
            &mut terminal,
            &input,
            &models,
            &mut list_state,
            &quantizations,
            &mut quant_list_state,
            &mut quant_file_list_state,
            FocusedPane::Models,
            "Press / to search",
            "Selection: 2 of 3",
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_render_ui_quantization_panels() {
        let input = Input::default();
        let models = three_model_fixtures();
        let mut list_state = ListState::default();
        let quantizations = quantization_fixtures();
        let mut quant_list_state = ListState::default();
        quant_list_state.select(Some(0));
        let mut quant_file_list_state = ListState::default();
        quant_file_list_state.select(Some(0));

        let mut terminal = test_terminal();
        draw_render_ui(
            &mut terminal,
            &input,
            &models,
            &mut list_state,
            &quantizations,
            &mut quant_list_state,
            &mut quant_file_list_state,
            FocusedPane::QuantizationGroups,
            "2 quantization groups available",
            "",
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_search_popup() {
        let input = Input::new("llama".to_string());
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| render_search_popup(frame, &input))
            .expect("failed to draw search popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_search_popup_over_populated_ui() {
        // Mirrors the app run loop: main UI first, popup last, both in the
        // SAME draw closure (sequential draws start from a reset buffer).
        // Verifies the popup's Clear widget wipes the underlying UI inside
        // the popup area instead of blending with it, and that the popup
        // remains centered over real content.
        let input = Input::new("llama".to_string());
        let models = three_model_fixtures();
        let mut list_state = ListState::default();
        list_state.select(Some(1));
        let quantizations: Vec<QuantizationGroup> = Vec::new();
        let mut quant_list_state = ListState::default();
        let mut quant_file_list_state = ListState::default();

        let error: Option<String> = None;
        let model_metadata: Option<ModelMetadata> = None;
        let file_tree: Option<FileTreeNode> = None;
        let mut file_tree_state = ListState::default();
        let complete_downloads: HashMap<String, DownloadMetadata> = HashMap::new();
        let mut panel_areas = Vec::new();
        let hovered_panel: Option<FocusedPane> = None;
        let mut filter_areas = Vec::new();

        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_ui(
                    frame,
                    RenderParams {
                        input: &input,
                        input_mode: InputMode::Normal,
                        models: &models,
                        list_state: &mut list_state,
                        loading: false,
                        quantizations: &quantizations,
                        quant_file_list_state: &mut quant_file_list_state,
                        quant_list_state: &mut quant_list_state,
                        loading_quants: false,
                        focused_pane: FocusedPane::Models,
                        error: &error,
                        status: "Press / to search",
                        selection_info: "Selection: 2 of 3",
                        complete_downloads: &complete_downloads,
                        display_mode: ModelDisplayMode::Gguf,
                        model_metadata: &model_metadata,
                        file_tree: &file_tree,
                        file_tree_state: &mut file_tree_state,
                        sort_field: SortField::Downloads,
                        sort_direction: SortDirection::Descending,
                        filter_min_downloads: 0,
                        filter_min_likes: 0,
                        focused_filter_field: 5,
                        panel_areas: &mut panel_areas,
                        hovered_panel: &hovered_panel,
                        filter_areas: &mut filter_areas,
                        hud_height: 0,
                    },
                );
                render_search_popup(frame, &input);
            })
            .expect("failed to draw UI + popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_download_path_popup() {
        let input = Input::new("/models/output".to_string());
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| render_download_path_popup(frame, &input))
            .expect("failed to draw download path popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_auth_error_popup_no_token() {
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_auth_error_popup(
                    frame,
                    "https://huggingface.co/meta-llama/Llama-3.1-8B",
                    false,
                );
            })
            .expect("failed to draw auth error popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_auth_error_popup_with_token() {
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_auth_error_popup(
                    frame,
                    "https://huggingface.co/meta-llama/Llama-3.1-8B",
                    true,
                );
            })
            .expect("failed to draw auth error popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_resume_popup() {
        let incomplete = vec![
            DownloadMetadata {
                model_id: "meta-llama/Llama-3.1-8B".to_string(),
                filename: "Llama-3.1-8B-Q4_K_M.gguf".to_string(),
                url: "https://huggingface.co/meta-llama/Llama-3.1-8B/resolve/main/Llama-3.1-8B-Q4_K_M.gguf"
                    .to_string(),
                local_path:
                    "/home/user/models/meta-llama/Llama-3.1-8B/Llama-3.1-8B-Q4_K_M.gguf"
                        .to_string(),
                total_size: 4_921_860_096,
                downloaded_size: 1_230_465_024,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
            },
            DownloadMetadata {
                model_id: "Qwen/Qwen2.5-7B".to_string(),
                filename: "Qwen2.5-7B-Q8_0.gguf".to_string(),
                url: "https://huggingface.co/Qwen/Qwen2.5-7B/resolve/main/Qwen2.5-7B-Q8_0.gguf"
                    .to_string(),
                local_path:
                    "/home/user/models/Qwen/Qwen2.5-7B/Qwen2.5-7B-Q8_0.gguf".to_string(),
                total_size: 8_500_000_000,
                downloaded_size: 4_250_000_000,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
            },
        ];
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| render_resume_popup(frame, &incomplete))
            .expect("failed to draw resume popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_options_popup() {
        // AppOptions::default() reads HOME/HF_TOKEN from the environment;
        // pin those two fields so the snapshot stays deterministic.
        let options = AppOptions {
            default_directory: "/home/testuser/models".to_string(),
            hf_token: None,
            ..AppOptions::default()
        };
        let directory_input = Input::default();
        let token_input = Input::default();
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_options_popup(frame, &options, &directory_input, &token_input);
            })
            .expect("failed to draw options popup");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_filter_toolbar_unfocused() {
        // focused_field 5 is out of range (valid: 0=sort, 1=downloads,
        // 2=likes), so no field is highlighted. Default values also
        // trigger the "[No Filters]" preset indicator.
        let mut filter_areas = Vec::new();
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_filter_toolbar(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: TERMINAL_WIDTH,
                        height: 3,
                    },
                    SortField::Downloads,
                    SortDirection::Descending,
                    0,
                    0,
                    5,
                    &mut filter_areas,
                );
            })
            .expect("failed to draw filter toolbar");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_filter_toolbar_sort_focused() {
        let mut filter_areas = Vec::new();
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_filter_toolbar(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: TERMINAL_WIDTH,
                        height: 3,
                    },
                    SortField::Likes,
                    SortDirection::Ascending,
                    2_500,
                    300,
                    0,
                    &mut filter_areas,
                );
            })
            .expect("failed to draw filter toolbar");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_filter_toolbar_downloads_focused() {
        let mut filter_areas = Vec::new();
        let mut terminal = test_terminal();
        terminal
            .draw(|frame| {
                render_filter_toolbar(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: TERMINAL_WIDTH,
                        height: 3,
                    },
                    SortField::Downloads,
                    SortDirection::Descending,
                    10_000,
                    100,
                    1,
                    &mut filter_areas,
                );
            })
            .expect("failed to draw filter toolbar");
        insta::assert_snapshot!(terminal.backend());
    }
}
