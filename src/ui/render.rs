use crate::models::{FocusedPane, InputMode, ModelInfo, QuantizationInfo, QuantizationGroup, DownloadProgress, VerificationProgress, ModelDisplayMode, ModelMetadata, FileTreeNode, ValidationStatus, CanvasContent, AppOptions};
use crate::ui::app::state::ChartType;
use crate::ui::app::DownloadRecord;
use crate::utils::{format_number, format_size};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::Line as CanvasLine;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap, Gauge, Clear,
        canvas::{
            Canvas, Circle, Rectangle,
            Context
        }
    },
};
use std::collections::HashMap;
use tui_input::Input;

/// Enhanced canvas state for popup enhancements
#[derive(Debug, Clone, Copy)]
pub struct CanvasPreferences {
    pub enable_animations: bool,
    pub preferred_marker: Marker,
    pub animation_fps: u8,
    pub enable_mouse_interaction: bool,
    pub visual_feedback_level: FeedbackLevel,
}

#[derive(Debug, Clone, Copy)]
pub enum FeedbackLevel {
    Minimal,
    Standard,
    Enhanced,
}

impl Default for CanvasPreferences {
    fn default() -> Self {
        Self {
            enable_animations: true,
            preferred_marker: Marker::Braille,
            animation_fps: 30,
            enable_mouse_interaction: true,
            visual_feedback_level: FeedbackLevel::Standard,
        }
    }
}

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
    // Canvas features
    pub popup_mode: crate::models::PopupMode,
    pub download_progress: &'a Option<DownloadProgress>,
    pub verification_progress: &'a [VerificationProgress],
    pub advanced_canvas_state: &'a super::app::state::AdvancedCanvasState,
    pub canvas_animation_frame: u64,
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
        ref popup_mode,
        download_progress,
        verification_progress,
        advanced_canvas_state,
        canvas_animation_frame,
    } = params;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(12),
            Constraint::Length(4),  // Changed from 3 to 4 for 2-line status
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
    );

    // Results list
    let items: Vec<ListItem> = models
        .iter()
        .enumerate()
        .map(|(idx, model)| {
            // Extract author from model.id if not provided (e.g., "unsloth/model" -> "unsloth")
            let author = model.author.as_deref()
                .or_else(|| model.id.split('/').next())
                .unwrap_or("unknown");
            let downloads = format_number(model.downloads);
            let likes = format_number(model.likes);
            
            let tags_str = if model.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", model.tags.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
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
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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
                .border_style(
                    if input_mode == InputMode::Normal 
                        && focused_pane == FocusedPane::Models {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                ),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, chunks[1], list_state);

    // Split bottom panel into left and right sections
    let bottom_panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[2]);

    // Render based on display mode
    match display_mode {
        ModelDisplayMode::Gguf => {
            render_gguf_panels(frame, bottom_panel_chunks, GgufPanelContext {
                quantizations,
                quant_list_state,
                quant_file_list_state,
                loading_quants,
                input_mode,
                focused_pane,
                complete_downloads,
            });
        }
        ModelDisplayMode::Standard => {
            render_standard_panels(frame, bottom_panel_chunks, StandardPanelContext {
                model_metadata,
                file_tree,
                file_tree_state,
                loading: loading_quants,
                input_mode,
                focused_pane,
            });
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
            Style::default().fg(Color::White)
        })
        .wrap(Wrap { trim: true });

    frame.render_widget(status_widget, chunks[3]);
    
    // Render canvas popups if active
    render_canvas_popups(frame, params);
}

/// Render canvas-based popups for advanced features
fn render_canvas_popups(frame: &mut Frame, params: RenderParams) {
    let RenderParams {
        ref popup_mode,
        models,
        download_progress,
        verification_progress,
        advanced_canvas_state,
        canvas_animation_frame,
        list_state,
        ..
    } = params;

    match popup_mode {
        crate::models::PopupMode::ModelVisualization => {
            if let Some(selected_idx) = list_state.selected() {
                if selected_idx < models.len() {
                    render_model_architecture_popup(frame, &models[selected_idx], canvas_animation_frame);
                }
            }
        }
        crate::models::PopupMode::ModelComparison => {
            render_model_comparison_popup(frame, models, &advanced_canvas_state.model_visualization.selected_models);
        }
        crate::models::PopupMode::NetworkActivity => {
            if let Some(progress) = download_progress {
                render_network_activity_popup(frame, progress, canvas_animation_frame);
            }
        }
        crate::models::PopupMode::PerformanceAnalytics => {
            render_performance_analytics_popup(frame, &advanced_canvas_state.performance_analytics.history_data, advanced_canvas_state.performance_analytics.chart_type);
        }
        crate::models::PopupMode::EnhancedVerification => {
            render_enhanced_verification_popup(frame, verification_progress, canvas_animation_frame);
        }
        _ => {}
    }
}

/// Render model architecture visualization popup
fn render_model_architecture_popup(frame: &mut Frame, model: &ModelInfo, animation_frame: u64) {
    let popup_width = 100.min(frame.area().width.saturating_sub(4));
    let popup_height = 60.min(frame.area().height.saturating_sub(4));
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    frame.render_widget(Clear, popup_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(format!(" Model Architecture: {} ", model.id))
            .style(Style::default().fg(Color::Cyan)))
        .marker(ratatui::symbols::Marker::Braille)
        .x_bounds([0.0, popup_area.width as f64])
        .y_bounds([0.0, popup_area.height as f64])
        .paint(move |ctx| {
            render_model_architecture_visualization(ctx, model, popup_area);
        });
    
    frame.render_widget(canvas, popup_area);
}

/// Render model comparison popup
fn render_model_comparison_popup(frame: &mut Frame, models: &[ModelInfo], selected_models: &[usize]) {
    let popup_width = 120.min(frame.area().width.saturating_sub(4));
    let popup_height = 50.min(frame.area().height.saturating_sub(4));
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    frame.render_widget(Clear, popup_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(format!(" Model Comparison ({} models) ", selected_models.len()))
            .style(Style::default().fg(Color::Yellow)))
        .marker(ratatui::symbols::Marker::Braille)
        .x_bounds([0.0, popup_area.width as f64])
        .y_bounds([0.0, popup_area.height as f64])
        .paint(move |ctx| {
            render_model_comparison_canvas(ctx, models, selected_models, popup_area);
        });
    
    frame.render_widget(canvas, popup_area);
}

/// Render network activity popup
fn render_network_activity_popup(frame: &mut Frame, progress: &DownloadProgress, animation_frame: u64) {
    let popup_width = 80.min(frame.area().width.saturating_sub(4));
    let popup_height = 40.min(frame.area().height.saturating_sub(4));
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    frame.render_widget(Clear, popup_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(" Network Activity ")
            .style(Style::default().fg(Color::Green)))
        .marker(ratatui::symbols::Marker::Braille)
        .x_bounds([0.0, popup_area.width as f64])
        .y_bounds([0.0, popup_area.height as f64])
        .paint(move |ctx| {
            render_network_activity_canvas(ctx, progress, popup_area);
        });
    
    frame.render_widget(canvas, popup_area);
}

/// Render performance analytics popup
fn render_performance_analytics_popup(frame: &mut Frame, history_data: &[DownloadRecord], chart_type: ChartType) {
    let popup_width = 90.min(frame.area().width.saturating_sub(4));
    let popup_height = 45.min(frame.area().height.saturating_sub(4));
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    frame.render_widget(Clear, popup_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(format!(" Performance Analytics ({:?}) ", chart_type))
            .style(Style::default().fg(Color::Magenta)))
        .marker(ratatui::symbols::Marker::Braille)
        .x_bounds([0.0, popup_area.width as f64])
        .y_bounds([0.0, popup_area.height as f64])
        .paint(move |ctx| {
            render_performance_analytics_canvas(ctx, history_data, popup_area);
        });
    
    frame.render_widget(canvas, popup_area);
}

/// Render enhanced verification popup
fn render_enhanced_verification_popup(frame: &mut Frame, verification_progress: &[VerificationProgress], animation_frame: u64) {
    if verification_progress.is_empty() {
        return;
    }
    
    let popup_width = 85.min(frame.area().width.saturating_sub(4));
    let popup_height = 35.min(frame.area().height.saturating_sub(4));
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    frame.render_widget(Clear, popup_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(" Verification Progress ")
            .style(Style::default().fg(Color::Green)))
        .marker(ratatui::symbols::Marker::Braille)
        .x_bounds([0.0, popup_area.width as f64])
        .y_bounds([0.0, popup_area.height as f64])
        .paint(move |ctx| {
            render_verification_progress_chart(ctx, verification_progress, popup_area);
        });
    
    frame.render_widget(canvas, popup_area);
}

struct StandardPanelContext<'a> {
    model_metadata: &'a Option<ModelMetadata>,
    file_tree: &'a Option<FileTreeNode>,
    file_tree_state: &'a mut ListState,
    loading: bool,
    input_mode: InputMode,
    focused_pane: FocusedPane,
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
        let mut lines = vec![
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Yellow)),
                Span::raw(&metadata.model_id),
            ]),
        ];

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
            lines.push(Line::from(vec![
                Span::styled("Tags:", Style::default().fg(Color::Yellow)),
            ]));
            let tags_str = metadata.tags.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
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
                .border_style(
                    if input_mode == InputMode::Normal 
                        && focused_pane == FocusedPane::ModelMetadata {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                ),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(metadata_widget, chunks[0]);

    // Right side: File tree
    render_file_tree_panel(frame, chunks[1], file_tree, file_tree_state, input_mode, focused_pane);
}

fn render_file_tree_panel(
    frame: &mut Frame,
    area: Rect,
    file_tree: &Option<FileTreeNode>,
    file_tree_state: &mut ListState,
    input_mode: InputMode,
    focused_pane: FocusedPane,
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
                    if node.expanded { "▾ " } else { "▸ " }
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
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ));
                    
                    let size_str = node.size.map(format_size).unwrap_or_else(|| String::from("-"));
                    let file_count = count_files(&node);
                    
                    spans.push(Span::raw(format!("  {}", size_str)));
                    spans.push(Span::styled(
                        format!(" ({} files)", file_count),
                        Style::default().fg(Color::DarkGray),
                    ));
                } else {
                    // File: show name and size
                    let size_str = node.size.map(format_size).unwrap_or_else(|| String::from("-"));
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
                .border_style(
                    if input_mode == InputMode::Normal 
                        && focused_pane == FocusedPane::FileTree {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                ),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

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
}

fn render_gguf_panels(
    frame: &mut Frame,
    chunks: std::rc::Rc<[Rect]>,
    ctx: GgufPanelContext,
) {
    let GgufPanelContext {
        quantizations,
        quant_list_state,
        quant_file_list_state,
        loading_quants,
        input_mode,
        focused_pane,
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
            let size_str = format_size(group.total_size);
            let is_downloaded = complete_downloads.contains_key(&group.files[0].filename);
            
            let mut spans = vec![
                Span::raw(format!("{:>10}  ", size_str)),
                Span::styled(
                    format!("{:<14} ", group.quant_type),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
            ];
            
            if is_downloaded {
                spans.push(Span::styled(" [downloaded]", Style::default().fg(Color::Green)));
            } else {
                let file_count = if group.files.len() > 1 {
                    format!(" ({} files)", group.files.len())
                } else {
                    String::new()
                };
                spans.push(Span::styled(file_count, Style::default().fg(Color::DarkGray)));
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
                .border_style(
                    if input_mode == InputMode::Normal 
                        && focused_pane == FocusedPane::QuantizationGroups {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                ),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

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
            
            let mut spans = vec![
                Span::raw(format!("{:>10}  ", size_str)),
            ];
            
            if is_downloaded {
                spans.push(Span::styled(&file.filename, Style::default().fg(Color::Green)));
                spans.push(Span::styled(" [downloaded]", Style::default().fg(Color::Green)));
            } else {
                spans.push(Span::styled(&file.filename, Style::default().fg(Color::White)));
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
                .border_style(
                    if input_mode == InputMode::Normal 
                        && focused_pane == FocusedPane::QuantizationFiles {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                ),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(file_list, chunks[1], quant_file_list_state);
}

/// Render both download and verification progress bars
pub fn render_progress_bars(
    frame: &mut Frame,
    download_progress: &Option<DownloadProgress>,
    download_queue_size: usize,
    verification_progress: &[VerificationProgress],
    verification_queue_size: usize,
) {
    // Render download progress (top-right) if active
    if let Some(progress) = download_progress {
        render_download_progress(frame, progress, download_queue_size);
    }
    
    // Render verification progress (bottom-right) if active
    if !verification_progress.is_empty() || verification_queue_size > 0 {
        render_verification_progress(frame, verification_progress, verification_queue_size);
    }
}

/// Render download progress bar in top-right corner
fn render_download_progress(
    frame: &mut Frame,
    progress: &DownloadProgress,
    queue_size: usize,
) {
    // Filter active chunks
    let active_chunks: Vec<_> = progress.chunks.iter()
        .filter(|c| c.is_active)
        .collect();
    
    // Calculate height
    let num_active = active_chunks.len();
    let total_height = if num_active > 0 {
        3 + num_active as u16 + 2
    } else {
        3
    };
    
    // Position: top-right
    let progress_area = Rect {
        x: frame.area().width.saturating_sub(52),
        y: 0,
        width: 52.min(frame.area().width),
        height: total_height.min(frame.area().height),
    };
    
    frame.render_widget(Clear, progress_area);
    
    let percentage = if progress.total > 0 {
        (progress.downloaded as f64 / progress.total as f64 * 100.0) as u16
    } else {
        0
    };
    
    // Title with queue info (no more verifying logic)
    let title = if queue_size > 0 {
        format!("Downloading ({} queued)", queue_size)
    } else {
        "Downloading".to_string()
    };
    
    // Label with speed
    let label = if progress.speed_mbps > 0.0 {
        format!("{}% - {:.2} MB/s", percentage, progress.speed_mbps)
    } else {
        format!("{}%", percentage)
    };
    
    // Overall progress gauge
    let overall_area = Rect {
        x: progress_area.x,
        y: progress_area.y,
        width: progress_area.width,
        height: 3,
    };
    
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .percent(percentage)
        .label(label);
    
    frame.render_widget(gauge, overall_area);
    
    // Render active chunk progress
    if !active_chunks.is_empty() {
        let chunks_area = Rect {
            x: progress_area.x,
            y: progress_area.y + 3,
            width: progress_area.width,
            height: num_active as u16 + 2,
        };
        
        let chunks_block = Block::default()
            .borders(Borders::ALL)
            .title("Active Chunks");
        
        let inner_area = chunks_block.inner(chunks_area);
        frame.render_widget(chunks_block, chunks_area);
        
        for (y_offset, chunk) in active_chunks.into_iter().enumerate() {
            let chunk_area = Rect {
                x: inner_area.x,
                y: inner_area.y + y_offset as u16,
                width: inner_area.width,
                height: 1,
            };
            
            let chunk_pct = if chunk.total > 0 {
                (chunk.downloaded as f64 / chunk.total as f64 * 100.0) as u16
            } else {
                0
            };
            
            let bar_width = chunk_area.width.saturating_sub(20) as usize;
            let filled = (bar_width as f64 * chunk_pct as f64 / 100.0) as usize;
            let empty = bar_width.saturating_sub(filled);
            
            let bar = format!(
                "#{:<2}[{}{}] {:>6.2} MB/s",
                chunk.chunk_id + 1,
                "=".repeat(filled),
                " ".repeat(empty),
                chunk.speed_mbps
            );
            
            let chunk_widget = Paragraph::new(bar)
                .style(Style::default().fg(Color::Yellow));
            
            frame.render_widget(chunk_widget, chunk_area);
        }
    }
}

/// Render verification progress bar in bottom-right corner
fn render_verification_progress(
    frame: &mut Frame,
    verifications: &[VerificationProgress],
    queue_size: usize,
) {
    if verifications.is_empty() && queue_size == 0 {
        return;
    }
    
    // Calculate height: each verification gets 3 lines
    let height = 3 + (verifications.len() as u16 * 3);
    
    // Position: bottom-right
    let area = Rect {
        x: frame.area().width.saturating_sub(52),
        y: frame.area().height.saturating_sub(height.min(frame.area().height)),
        width: 52.min(frame.area().width),
        height: height.min(frame.area().height),
    };
    
    frame.render_widget(Clear, area);
    
    // Title with queue info
    let title = if queue_size > 0 {
        format!("Verifying ({} queued)", queue_size)
    } else {
        "Verifying".to_string()
    };
    
    // Main container block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Green));
    
    let inner = block.inner(area);
    frame.render_widget(block, area);
    
    // Render each active verification as a progress bar
    for (i, ver) in verifications.iter().enumerate() {
        let ver_area = Rect {
            x: inner.x,
            y: inner.y + (i as u16 * 3),
            width: inner.width,
            height: 3.min(inner.height.saturating_sub(i as u16 * 3)),
        };
        
        if ver_area.height == 0 {
            break; // No more room
        }
        
        let percentage = if ver.total_bytes > 0 {
            (ver.verified_bytes as f64 / ver.total_bytes as f64 * 100.0) as u16
        } else {
            0
        };
        
        // Truncate filename to fit (show end of filename)
        let display_name = if ver.filename.len() > 35 {
            format!("...{}", &ver.filename[ver.filename.len()-32..])
        } else {
            ver.filename.clone()
        };
        
        let label = format!("{}%", percentage);
        
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(display_name))
            .gauge_style(Style::default().fg(Color::Green).bg(Color::Black))
            .percent(percentage)
            .label(label);
        
        frame.render_widget(gauge, ver_area);
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
    
    let files_widget = Paragraph::new(file_lines)
        .style(Style::default().fg(Color::White));
    
    frame.render_widget(files_widget, list_area);
    
    // Show "and X more..." if there are more than 5
    if incomplete_downloads.len() > 5 {
        let more_area = Rect {
            x: popup_area.x + 2,
            y: list_area.y + list_area.height,
            width: popup_area.width.saturating_sub(4),
            height: 1,
        };
        
        let more_text = Paragraph::new(format!("  ... and {} more", incomplete_downloads.len() - 5))
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
            Span::styled("Y", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" to resume all  |  "),
            Span::styled("N", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" to skip  |  "),
            Span::styled("D", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" to delete and skip"),
        ]),
    ])
    .style(Style::default().fg(Color::White));
    
    frame.render_widget(instructions, instructions_area);
}

/// Render search popup dialog
pub fn render_search_popup(
    frame: &mut Frame, 
    input: &Input,
    suggestions: &[String],
    selected_index: usize,
    animation_frame: u64,
    canvas_enabled: bool,
) {
    let popup_width = 60.min(frame.area().width.saturating_sub(4));
    let popup_height = (8 + suggestions.len().min(5)) as u16;
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    let area = Rect { x: popup_x, y: popup_y, width: popup_width, height: popup_height };
    
    // Progressive enhancement: canvas or text rendering
    if canvas_enabled {
        render_canvas_search_popup(frame, area, suggestions, selected_index, animation_frame);
    }
    
    // Always render text-based UI as fallback
    render_text_search_popup(frame, area, input, suggestions, selected_index);
}

/// Canvas-based search popup rendering
fn render_canvas_search_popup(
    frame: &mut Frame,
    area: Rect,
    suggestions: &[String],
    selected_index: usize,
    animation_frame: u64,
) {
    let canvas_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.min(8 + suggestions.len().min(5) as u16),
    };
    
    frame.render_widget(Clear, canvas_area);
    
    let canvas = Canvas::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .title(" Search Models ")
            .style(Style::default().fg(Color::Cyan)))
        .marker(Marker::Braille)
        .x_bounds([0.0, canvas_area.width as f64])
        .y_bounds([0.0, canvas_area.height as f64])
        .paint(move |ctx| {
            // Animated search indicator
            render_canvas_search_indicator(ctx, canvas_area.width.into(), canvas_area.height.into(), animation_frame);
            
            // Search suggestions preview
            render_canvas_search_preview(ctx, suggestions, selected_index, canvas_area);
        });
    
    frame.render_widget(canvas, canvas_area);
}

/// Text-based search popup rendering (fallback)
fn render_text_search_popup(
    frame: &mut Frame,
    area: Rect,
    input: &Input,
    suggestions: &[String],
    selected_index: usize,
) {
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
    
    let input_widget = Paragraph::new(input.value())
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(input_widget, input_area);
    
    // Show cursor
    frame.set_cursor_position((
        input_area.x + input.visual_cursor() as u16,
        input_area.y,
    ));
    
    // Search suggestions preview
    render_search_suggestions_preview(frame, suggestions, selected_index, inner);
    
    // Help text
    let help_start_y = inner.y + 3 + suggestions.len().min(5) as u16;
    let help = [
        "Enter search query and press Enter to search",
        "ESC: Cancel",
    ];
    
    for (i, line) in help.iter().enumerate() {
        let area = Rect {
            x: inner.x + 2,
            y: help_start_y + i as u16,
            width: inner.width - 4,
            height: 1,
        };
        let widget = Paragraph::new(*line)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(widget, area);
    }
}

/// Canvas rendering: Animated search indicator
fn render_canvas_search_indicator(
    ctx: &mut Context,
    width: f64,
    height: f64,
    animation_frame: u64,
) {
    let center_x = width / 2.0;
    let center_y = height / 2.0 - 8.0; // Position above suggestions
    
    let progress = (animation_frame % 12) as f64 / 12.0;
    
    // Draw spinning indicator
    for i in 0..12 {
        let angle = (i as f64 / 12.0) * std::f64::consts::PI * 2.0 + progress;
        let x = center_x + 10.0 * angle.cos();
        let y = center_y + 10.0 * angle.sin();
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 1.5,
            color: if (i as f64) < progress * 12.0 { Color::Cyan } else { Color::DarkGray },
        });
    }
}

/// Canvas rendering: Search suggestions preview
fn render_canvas_search_preview(
    ctx: &mut Context,
    suggestions: &[String],
    selected_index: usize,
    area: Rect,
) {
    use ratatui::widgets::canvas::Rectangle;
    
    let line_height = 2.0;
    let start_y = area.y as f64 + 4.0;
    
    suggestions.iter().enumerate().take(5).for_each(|(i, suggestion)| {
        let y = start_y + (i as f64 * line_height);
        let is_selected = i == selected_index;
        
        // Background rectangle for selection
        ctx.draw(&Rectangle {
            x: 2.0,
            y,
            width: (area.width as f64 - 4.0),
            height: line_height,
            color: if is_selected { Color::Blue } else { Color::Black },
        });
        
        // Selection indicator
        if is_selected {
            // Draw right-pointing triangle
            ctx.draw(&Circle {
                x: 1.5,
                y: y + 1.0,
                radius: 0.8,
                color: Color::Yellow,
            });
        }
        
        // Truncated suggestion text
        let text = suggestion.chars().take(20).collect::<String>();
        if !text.is_empty() {
            // Note: Full text rendering would need font support
            // For now, just show a placeholder
        }
    });
}

/// Text-based suggestions preview
fn render_search_suggestions_preview(
    frame: &mut Frame,
    suggestions: &[String],
    selected_index: usize,
    area: Rect,
) {
    if suggestions.is_empty() {
        return;
    }
    
    // Title
    let title_area = Rect {
        x: area.x + 2,
        y: area.y + 3,
        width: area.width - 4,
        height: 1,
    };
    
    let title = Paragraph::new(format!("Suggestions ({}):", suggestions.len()))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(title, title_area);
    
    // Suggestions list
    let list_start_y = area.y + 4;
    let max_items = suggestions.len().min(5);
    
    for (i, suggestion) in suggestions.iter().enumerate().take(max_items) {
        let item_area = Rect {
            x: area.x + 2,
            y: list_start_y + i as u16,
            width: area.width - 4,
            height: 1,
        };
        
        let is_selected = i == selected_index;
        let style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        
        // Truncate suggestion text
        let text = if suggestion.len() > 30 {
            format!("{}...", &suggestion[..27])
        } else {
            suggestion.clone()
        };
        
        let widget = Paragraph::new(format!("> {}", text)).style(style);
        frame.render_widget(widget, item_area);
    }
}

pub fn render_download_path_popup(
    frame: &mut Frame,
    download_path_input: &Input,
    path_components: &[String],
    _current_index: usize,
    _validation_status: ValidationStatus,
    _animation_frame: u64,
    _canvas_enabled: bool,
) {
    // Calculate centered popup area
    let popup_width = 60.min(frame.area().width.saturating_sub(4));
    let popup_height = (7 + path_components.len().min(3)) as u16;
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
    
    let label = Paragraph::new("Download path:")
        .style(Style::default().fg(Color::White));
    
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

/// Canvas rendering: Directory tree visualization
fn render_canvas_directory_tree(
    ctx: &mut Context,
    path_components: &[String],
    current_index: usize,
    area: Rect,
) {
    use ratatui::widgets::canvas::{Rectangle, Circle};
    
    let indent_width = 15.0;
    let line_height = 2.0;
    let start_y = area.y as f64 + 3.0;
    
    path_components.iter().enumerate().take(3).for_each(| (i, _component) | {
        let y = start_y + (i as f64 * line_height);
        let x = 2.0 + (i as f64 * indent_width);
        let is_current = i == current_index;
        
        // Draw folder icon
        ctx.draw(&Rectangle {
            x,
            y,
            width: 12.0,
            height: 8.0,
            color: if is_current { Color::Yellow } else { Color::Blue },
        });
        
        // Path indicator line
        if i < path_components.len() - 1 {
            ctx.draw(&Circle {
                x: x + 6.0,
                y: y + 8.0,
                radius: 1.0,
                color: Color::DarkGray,
            });
        }
        
        // Note: Full text rendering would need font support
    });
}

impl Default for ValidationStatus {
    fn default() -> Self {
        ValidationStatus::Pending
    }
}

pub fn render_auth_error_popup(
    frame: &mut Frame,
    model_url: &str,
    has_token: bool,
) {
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
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("Steps to access this model:", Style::default().fg(Color::Cyan))),
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
            Span::styled("https://huggingface.co/settings/tokens", Style::default().fg(Color::Blue)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("4. ", Style::default().fg(Color::Yellow)),
            Span::raw("Press "),
            Span::styled("'o'", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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
    _animation_frame: u64,
    _canvas_preferences: CanvasPreferences,
) {
    let popup_width = 64.min(frame.area().width.saturating_sub(4));
    let popup_height = 27;
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
        ("Default Directory:", if options.editing_directory { 
            directory_input.value().to_string() 
        } else { 
            options.default_directory.clone() 
        }),
        ("HF Token (optional):", if options.editing_token {
            token_input.value().to_string()
        } else if let Some(token) = &options.hf_token {
            if token.is_empty() {
                "[Not set]".to_string()
            } else {
                "•".repeat(token.len().min(20))
            }
        } else {
            "[Not set]".to_string()
        }),
        // Download (indices 2-9)
        ("Concurrent Threads:", options.concurrent_threads.to_string()),
        ("Target Number of Chunks:", options.num_chunks.to_string()),
        ("Min Chunk Size:", format_size(options.min_chunk_size)),
        ("Max Chunk Size:", format_size(options.max_chunk_size)),
        ("Max Retries:", options.max_retries.to_string()),
        ("Download Timeout (sec):", options.download_timeout_secs.to_string()),
        ("Retry Delay (sec):", options.retry_delay_secs.to_string()),
        ("Progress Update Interval (ms):", options.progress_update_interval_ms.to_string()),
        // Verification (indices 10-13)
        ("Enable Verification:", if options.verification_on_completion { "Enabled".to_string() } else { "Disabled".to_string() }),
        ("Concurrent Verifications:", options.concurrent_verifications.to_string()),
        ("Verification Buffer Size:", format_size(options.verification_buffer_size as u64)),
        ("Verification Update Interval:", options.verification_update_interval.to_string()),
    ];
    
    // Render category headers
    let category_offsets = [
        (0, "General"),
        (2, "Download"),
        (10, "Verification"),
    ];
    
    let mut y_offset = 1u16;
    let mut field_idx = 0;
    
    for (cat_idx, (field_start, category_name)) in category_offsets.iter().enumerate() {
        // Render category header
        if cat_idx > 0 {
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
        
        let header_widget = Paragraph::new(separator)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(header_widget, header_area);
        
        y_offset += 1;
        
        // Render fields in this category
        let next_cat_start = category_offsets.get(cat_idx + 1).map(|(s, _)| *s).unwrap_or(fields.len());
        for (label, value) in fields.iter().take(next_cat_start).skip(*field_start) {
            
            let area = Rect { 
                x: inner.x + 2, 
                y: inner.y + y_offset, 
                width: inner.width - 4, 
                height: 1 
            };
            
            let style = if field_idx == options.selected_field {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            
            let text = format!("{} {}", label, value);
            let widget = Paragraph::new(text).style(style);
            frame.render_widget(widget, area);
            
            // Show cursor when editing directory or token
            if options.editing_directory && field_idx == 0 {
                let cursor_x = area.x + label.len() as u16 + 1 + directory_input.visual_cursor() as u16;
                frame.set_cursor_position((cursor_x, area.y));
            } else if options.editing_token && field_idx == 1 {
                let cursor_x = area.x + label.len() as u16 + 1 + token_input.visual_cursor() as u16;
                frame.set_cursor_position((cursor_x, area.y));
            }
            
            y_offset += 1;
            field_idx += 1;
        }
    }
    
    // Controls help (with empty line before)
    let help_y = inner.y + inner.height - 5;
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
            height: 1 
        };
        let widget = Paragraph::new(*line)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(widget, area);
    }
    pub fn render_canvas_popup(
        frame: &mut Frame,
        _area: Rect,
        title: &str,
        #[allow(unused_variables)] content: CanvasContent,
        marker: Marker,
    ) {
        // Calculate centered popup area
        let popup_width = 80.min(frame.area().width.saturating_sub(4));
        let popup_height = 30;
        let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
        let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };
        
        // Clear the popup area
        frame.render_widget(Clear, popup_area);
        
        // Create canvas widget
        let canvas_widget = Canvas::default()
            .block(Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().fg(Color::Cyan)))
            .marker(marker)
            .paint(move |ctx| {
                // Canvas drawing logic here
                ctx.draw(&Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: popup_area.width as f64,
                    height: popup_area.height as f64,
                    color: Color::Black,
                });
            })
            .x_bounds([0.0, f64::from(popup_area.width)])
            .y_bounds([0.0, f64::from(popup_area.height)]);
        
        frame.render_widget(canvas_widget, popup_area);
    }
}

/// Render model architecture visualization canvas
pub fn render_model_architecture_visualization(
    ctx: &mut Context,
    model_info: &ModelInfo,
    area: Rect,
) {
    let model_type = detect_model_type(&model_info.id);
    
    match model_type {
        ModelType::Transformer => render_transformer_architecture(ctx, area),
        ModelType::CNN => render_cnn_architecture(ctx, area),
        ModelType::GPT => render_gpt_architecture(ctx, area),
        ModelType::LSTM => render_lstm_architecture(ctx, area),
        ModelType::Unknown => render_unknown_architecture(ctx, area),
    }
    
    // Add model statistics
    render_model_statistics(ctx, model_info, area);
}

/// Render model statistics alongside architecture
fn render_model_statistics(ctx: &mut Context, model_info: &ModelInfo, area: Rect) {
    let stats_x = area.x as f64 + 5.0;
    let stats_y = area.y as f64 + area.height as f64 - 25.0;
    
    // Background for stats
    ctx.draw(&Rectangle {
        x: stats_x,
        y: stats_y,
        width: area.width as f64 - 10.0,
        height: 20.0,
        color: Color::Black,
    });
    
    // Model type
    let model_type = detect_model_type(&model_info.id);
    let _type_str = match model_type {
        ModelType::Transformer => "Transformer",
        ModelType::CNN => "CNN",
        ModelType::GPT => "GPT",
        ModelType::LSTM => "LSTM",
        ModelType::Unknown => "Unknown",
    };
    
    // Downloads and likes stats (simplified visualization)
    let downloads_width = (model_info.downloads as f64 / 1_000_000.0).min(1.0) * 30.0;
    let likes_width = (model_info.likes as f64 / 100_000.0).min(1.0) * 30.0;
    
    // Downloads bar
    ctx.draw(&Rectangle {
        x: stats_x + 5.0,
        y: stats_y + 5.0,
        width: downloads_width,
        height: 5.0,
        color: Color::Cyan,
    });
    
    // Likes bar  
    ctx.draw(&Rectangle {
        x: stats_x + 5.0,
        y: stats_y + 12.0,
        width: likes_width,
        height: 5.0,
        color: Color::Yellow,
    });
}

fn detect_model_type(model_id: &str) -> ModelType {
    let id_lower = model_id.to_lowercase();
    if id_lower.contains("gpt") {
        ModelType::GPT
    } else if id_lower.contains("bert") || id_lower.contains("transformer") {
        ModelType::Transformer
    } else if id_lower.contains("cnn") || id_lower.contains("conv") || id_lower.contains("resnet") || id_lower.contains("vgg") {
        ModelType::CNN
    } else if id_lower.contains("lstm") || id_lower.contains("rnn") || id_lower.contains("gru") {
        ModelType::LSTM
    } else {
        ModelType::Unknown
    }
}

#[derive(Debug, Clone, Copy)]
enum ModelType {
    Transformer,
    CNN,
    GPT,
    LSTM,
    Unknown,
}

fn render_transformer_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    let layer_spacing = 18.0;
    let num_layers = 6;
    
    // Input embedding layer
    ctx.draw(&Rectangle {
        x: center_x - 45.0,
        y: center_y - (num_layers as f64 * layer_spacing) / 2.0 - 30.0,
        width: 90.0,
        height: 15.0,
        color: Color::Blue,
    });
    
    // Positional encoding indicator (small dots above input)
    for i in 0..8 {
        let x = center_x - 35.0 + (i as f64 * 10.0);
        ctx.draw(&Circle {
            x,
            y: center_y - (num_layers as f64 * layer_spacing) / 2.0 - 40.0,
            radius: 2.0,
            color: Color::LightBlue,
        });
    }
    
    // Transformer layers with attention heads
    for i in 0..num_layers {
        let y = center_y - (num_layers as f64 * layer_spacing) / 2.0 + (i as f64 * layer_spacing);
        
        // Multi-head attention block
        ctx.draw(&Rectangle {
            x: center_x - 40.0,
            y,
            width: 80.0,
            height: 8.0,
            color: Color::Cyan,
        });
        
        // Attention heads visualization (small vertical lines)
        for head in 0..8 {
            let head_x = center_x - 35.0 + (head as f64 * 8.75);
            ctx.draw(&CanvasLine {
                x1: head_x,
                y1: y + 2.0,
                x2: head_x,
                y2: y + 6.0,
                color: Color::Cyan,
            });
        }
        
        // Feed-forward network
        ctx.draw(&Rectangle {
            x: center_x - 35.0,
            y: center_y + 10.0,
            width: 70.0,
            height: 6.0,
            color: Color::LightGreen,
        });
        
        // Layer normalization (small rectangles)
        ctx.draw(&Rectangle {
            x: center_x - 38.0,
            y: center_y + 8.0,
            width: 76.0,
            height: 2.0,
            color: Color::Yellow,
        });
        
        // Residual connections (diagonal lines)
        if i > 0 {
            let prev_y = center_y - (num_layers as f64 * layer_spacing) / 2.0 + ((i-1) as f64 * layer_spacing);
            ctx.draw(&CanvasLine {
                x1: center_x - 45.0,
                y1: prev_y + 16.0,
                x2: center_x - 40.0,
                y2: y,
                color: Color::Gray,
            });
            ctx.draw(&CanvasLine {
                x1: center_x + 45.0,
                y1: prev_y + 16.0,
                x2: center_x + 40.0,
                y2: y,
                color: Color::Gray,
            });
        }
    }
    
    // Output layer
    let output_y = center_y + (num_layers as f64 * layer_spacing) / 2.0 + 30.0;
    ctx.draw(&Rectangle {
        x: center_x - 35.0,
        y: output_y,
        width: 70.0,
        height: 12.0,
        color: Color::Green,
    });
    
    // Final layer norm
    ctx.draw(&Rectangle {
        x: center_x - 30.0,
        y: output_y + 15.0,
        width: 60.0,
        height: 3.0,
        color: Color::Yellow,
    });
}

fn render_cnn_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Input layer (3D cube representation for RGB image)
    let input_size = 35.0;
    let input_x = center_x - 80.0;
    let input_y = center_y - input_size / 2.0;
    
    // Front face
    ctx.draw(&Rectangle {
        x: input_x,
        y: input_y,
        width: input_size,
        height: input_size,
        color: Color::Blue,
    });
    
    // Top face (3D effect)
    for i in 0..5 {
        let offset = i as f64 * 2.0;
        ctx.draw(&CanvasLine {
            x1: input_x + offset,
            y1: input_y - offset,
            x2: input_x + input_size + offset,
            y2: input_y - offset,
            color: Color::LightBlue,
        });
    }
    
    // Side face (3D effect)
    for i in 0..5 {
        let offset = i as f64 * 2.0;
        ctx.draw(&CanvasLine {
            x1: input_x + input_size + offset,
            y1: input_y - offset,
            x2: input_x + input_size + offset,
            y2: input_y + input_size - offset,
            color: Color::Blue,
        });
    }
    
    // Conv+Pool blocks with feature map size indicators
    let layers = [
        (center_x - 20.0, center_y - 25.0, 28.0, 50.0, "Conv1", Color::Cyan, 64),
        (center_x + 35.0, center_y - 20.0, 22.0, 40.0, "Pool1", Color::LightCyan, 64),
        (center_x + 80.0, center_y - 15.0, 18.0, 32.0, "Conv2", Color::Magenta, 128),
        (center_x + 115.0, center_y - 12.0, 14.0, 25.0, "Pool2", Color::LightMagenta, 128),
        (center_x + 145.0, center_y - 10.0, 10.0, 20.0, "Conv3", Color::Yellow, 256),
    ];
    
    for (i, (x, y, width, height, label, color, channels)) in layers.iter().enumerate() {
        // Main feature map
        ctx.draw(&Rectangle {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
            color: *color,
        });
        
        // Channel indicators (small dots showing depth)
        let dots_per_row = 4;
        let dots_per_col = (channels / dots_per_row).min(4);
        for row in 0..dots_per_col {
            for col in 0..dots_per_row {
                if row * dots_per_row + col < *channels {
                    let dot_x = x + 2.0 + (col as f64 * 3.0);
                    let dot_y = y + 2.0 + (row as f64 * 3.0);
                    ctx.draw(&Circle {
                        x: dot_x,
                        y: dot_y,
                        radius: 0.8,
                        color: Color::White,
                    });
                }
            }
        }
        
        // Connection lines showing data flow
        if i > 0 {
            let (prev_x, prev_y, prev_width, prev_height, _, _, _) = layers[i - 1];
            // Multiple connections to show feature propagation
            for j in 0..3 {
                let y_offset = (j as f64 - 1.0) * 8.0;
                ctx.draw(&CanvasLine {
                    x1: prev_x + prev_width,
                    y1: prev_y + prev_height / 2.0 + y_offset,
                    x2: *x,
                    y2: *y + *height / 2.0 + y_offset * 0.7,
                    color: Color::Gray,
                });
            }
        }
        
        // Pooling indicators (every other layer)
        if label.starts_with("Pool") {
            // Draw 2x2 pooling grid
            let cell_width = width / 2.0;
            let cell_height = height / 2.0;
            for row in 0..2 {
                for col in 0..2 {
                    ctx.draw(&Rectangle {
                        x: x + col as f64 * cell_width,
                        y: y + row as f64 * cell_height,
                        width: cell_width - 1.0,
                        height: cell_height - 1.0,
                        color: Color::Black,
                    });
                }
            }
        }
    }
    
    // Flatten operation
    let flatten_x = center_x + 175.0;
    ctx.draw(&CanvasLine {
        x1: layers.last().unwrap().0 + layers.last().unwrap().2,
        y1: center_y,
        x2: flatten_x,
        y2: center_y,
        color: Color::Gray,
    });
    
    // Show flattening as expanding width
    for i in 0..5 {
        let y_offset = (i as f64 - 2.0) * 3.0;
        ctx.draw(&CanvasLine {
            x1: flatten_x + i as f64 * 3.0,
            y1: center_y + y_offset,
            x2: flatten_x + (i + 1) as f64 * 3.0,
            y2: center_y + y_offset,
            color: Color::Gray,
        });
    }
    
    // Fully connected layers
    let fc_layers = [
        (center_x + 200.0, center_y - 15.0, 12.0, 30.0, "FC1", Color::Green),
        (center_x + 220.0, center_y - 8.0, 8.0, 16.0, "FC2", Color::LightGreen),
        (center_x + 235.0, center_y - 4.0, 6.0, 8.0, "FC3", Color::Yellow),
    ];
    
    for (i, (x, y, width, height, label, color)) in fc_layers.iter().enumerate() {
        ctx.draw(&Rectangle {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
            color: *color,
        });
        
        // Neuron visualization (small circles)
        let neurons_per_col = (height / 3.0) as usize;
        for row in 0..neurons_per_col.min(4) {
            for col in 0..3 {
                let neuron_x = x + 1.0 + col as f64 * (width - 2.0) / 2.0;
                let neuron_y = y + 1.0 + row as f64 * (height - 2.0) / (neurons_per_col as f64);
                ctx.draw(&Circle {
                    x: neuron_x,
                    y: neuron_y,
                    radius: 0.5,
                    color: Color::White,
                });
            }
        }
        
        // Connections
        if i > 0 {
            let (prev_x, prev_y, prev_width, prev_height, _, _) = fc_layers[i - 1];
            for j in 0..3 {
                let y_offset = (j as f64 - 1.0) * 6.0;
                ctx.draw(&CanvasLine {
                    x1: prev_x + prev_width,
                    y1: prev_y + prev_height / 2.0 + y_offset,
                    x2: *x,
                    y2: *y + *height / 2.0 + y_offset * 0.5,
                    color: Color::Gray,
                });
            }
        } else {
            // Connection from flatten to first FC layer
            ctx.draw(&CanvasLine {
                x1: flatten_x + 15.0,
                y1: center_y,
                x2: *x,
                y2: *y + *height / 2.0,
                color: Color::Gray,
            });
        }
    }
    
    // Output classification
    let output_x = center_x + 250.0;
    for i in 0..3 {
        let class_y = center_y - 6.0 + (i as f64 * 6.0);
        ctx.draw(&Circle {
            x: output_x,
            y: class_y,
            radius: 3.0,
            color: Color::Red,
        });
        
        // Connection from last FC to output
        ctx.draw(&CanvasLine {
            x1: fc_layers.last().unwrap().0 + fc_layers.last().unwrap().2,
            y1: center_y,
            x2: output_x - 3.0,
            y2: class_y,
            color: Color::Gray,
        });
    }
}

fn render_lstm_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Input sequence with embeddings
    let seq_length = 6;
    for i in 0..seq_length {
        let x = center_x - 90.0 + (i as f64 * 20.0);
        let y = center_y - 30.0;
        
        // Input token
        ctx.draw(&Rectangle {
            x,
            y,
            width: 15.0,
            height: 15.0,
            color: Color::Blue,
        });
        
        // Embedding vector
        ctx.draw(&CanvasLine {
            x1: x + 7.5,
            y1: y + 15.0,
            x2: x + 7.5,
            y2: y + 25.0,
            color: Color::LightBlue,
        });
        
        // Time step indicator
        ctx.draw(&Circle {
            x: x + 7.5,
            y: y - 5.0,
            radius: 2.0,
            color: Color::Yellow,
        });
        
        // Sequence connections
        if i < seq_length - 1 {
            ctx.draw(&CanvasLine {
                x1: x + 15.0,
                y1: y + 7.5,
                x2: x + 20.0,
                y2: y + 7.5,
                color: Color::Gray,
            });
        }
    }
    
    // LSTM cells with detailed gate visualization
    let num_cells = 4;
    let cell_width = 35.0;
    let cell_height = 50.0;
    let cell_spacing = 20.0;
    
    for i in 0..num_cells {
        let x = center_x - 50.0 + (i as f64 * (cell_width + cell_spacing));
        let y = center_y - cell_height / 2.0;
        
        // Main LSTM cell body
        ctx.draw(&Rectangle {
            x,
            y,
            width: cell_width,
            height: cell_height,
            color: Color::Cyan,
        });
        
        // Gate visualization
        let gates = [
            ("Input", Color::Magenta, y + 5.0),
            ("Forget", Color::Yellow, y + 18.0),
            ("Output", Color::Green, y + 31.0),
        ];
        
        for (gate_name, gate_color, gate_y) in &gates {
            // Gate activation rectangle
            ctx.draw(&Rectangle {
                x: x + 2.0,
                y: *gate_y,
                width: cell_width - 4.0,
                height: 10.0,
                color: *gate_color,
            });
            
            // Sigmoid activation indicator
            for j in 0..5 {
                let sig_x = x + 5.0 + (j as f64 * 6.0);
                let sig_y = *gate_y + 5.0;
                ctx.draw(&Circle {
                    x: sig_x,
                    y: sig_y,
                    radius: 1.5,
                    color: Color::White,
                });
            }
        }
        
        // Cell state indicator (horizontal line through middle)
        ctx.draw(&CanvasLine {
            x1: x,
            y1: center_y,
            x2: x + cell_width,
            y2: center_y,
            color: Color::Red,
            // Dashed effect
        });
        
        // Hidden state indicator (vertical line)
        ctx.draw(&CanvasLine {
            x1: x + cell_width / 2.0,
            y1: y,
            x2: x + cell_width / 2.0,
            y2: y + cell_height,
            color: Color::LightGreen,
        });
        
        // Connections between cells
        if i > 0 {
            let prev_x = x - cell_spacing;
            
            // Cell state connection (top line)
            ctx.draw(&CanvasLine {
                x1: prev_x,
                y1: center_y - 5.0,
                x2: x,
                y2: center_y - 5.0,
                color: Color::Red,
            });
            
            // Hidden state connection (bottom line)
            ctx.draw(&CanvasLine {
                x1: prev_x,
                y1: center_y + 5.0,
                x2: x,
                y2: center_y + 5.0,
                color: Color::LightGreen,
            });
        }
    }
    
    // Input connections to first LSTM
    let first_lstm_x = center_x - 50.0;
    for i in 0..seq_length {
        let input_x = center_x - 90.0 + (i as f64 * 20.0);
        let input_y = center_y - 15.0;
        
        ctx.draw(&CanvasLine {
            x1: input_x + 15.0,
            y1: input_y + 15.0,
            x2: first_lstm_x + 5.0,
            y2: input_y + 15.0,
            color: Color::Gray,
        });
    }
    
    // Output sequence
    let output_start_x = center_x + 50.0 + (cell_spacing * (num_cells - 1) as f64);
    for i in 0..3 {
        let x = output_start_x + 20.0 + (i as f64 * 18.0);
        
        // Output token
        ctx.draw(&Rectangle {
            x,
            y: center_y - 10.0,
            width: 15.0,
            height: 15.0,
            color: Color::Red,
        });
        
        // Output embedding
        ctx.draw(&CanvasLine {
            x1: x + 7.5,
            y1: center_y - 25.0,
            x2: x + 7.5,
            y2: center_y - 10.0,
            color: Color::LightRed,
        });
        
        // Connection from last LSTM
        if i == 0 {
            ctx.draw(&CanvasLine {
                x1: output_start_x,
                y1: center_y,
                x2: x,
                y2: center_y - 2.5,
                color: Color::LightGreen,
            });
        }
        
        // Output sequence connections
        if i < 2 {
            ctx.draw(&CanvasLine {
                x1: x + 15.0,
                y1: center_y - 2.5,
                x2: x + 18.0,
                y2: center_y - 2.5,
                color: Color::Gray,
            });
        }
    }
    
    // Gradient flow indicators (dotted lines)
    for i in 0..3 {
        let grad_y = center_y + 40.0 + (i as f64 * 5.0);
        ctx.draw(&CanvasLine {
            x1: center_x - 80.0,
            y1: grad_y,
            x2: output_start_x + 60.0,
            y2: grad_y,
            color: Color::DarkGray,
        });
    }
}

fn render_gpt_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Input token embeddings
    let num_tokens = 8;
    for i in 0..num_tokens {
        let x = center_x - 60.0 + (i as f64 * 12.0);
        let y = center_y - 50.0;
        
        // Token representation
        ctx.draw(&Rectangle {
            x,
            y,
            width: 10.0,
            height: 10.0,
            color: Color::Blue,
        });
        
        // Token embedding vector (vertical line)
        ctx.draw(&CanvasLine {
            x1: x + 5.0,
            y1: y + 10.0,
            x2: x + 5.0,
            y2: y + 20.0,
            color: Color::LightBlue,
        });
        
        // Positional encoding (small circle above token)
        ctx.draw(&Circle {
            x: x + 5.0,
            y: y - 5.0,
            radius: 2.0,
            color: Color::Yellow,
        });
    }
    
    // Causal attention mask visualization
    let mask_y = center_y - 20.0;
    for i in 0..num_tokens {
        for j in 0..=i {
            let x = center_x - 60.0 + (j as f64 * 12.0);
            let y = mask_y + (i as f64 * 3.0);
            
            ctx.draw(&Rectangle {
                x,
                y,
                width: 10.0,
                height: 2.0,
                color: Color::Green,
            });
        }
    }
    
    // GPT-style decoder layers
    let num_layers = 6;
    let layer_height = 12.0;
    let layer_spacing = 10.0;
    
    for layer in 0..num_layers {
        let y = center_y + 10.0 + (layer as f64 * (layer_height + layer_spacing));
        
        // Multi-head self-attention block
        ctx.draw(&Rectangle {
            x: center_x - 50.0,
            y,
            width: 100.0,
            height: layer_height / 2.0,
            color: Color::Magenta,
        });
        
        // Attention heads
        for head in 0..12 {
            let head_x = center_x - 45.0 + (head as f64 * 7.5);
            ctx.draw(&Rectangle {
                x: head_x,
                y: y + 1.0,
                width: 5.0,
                height: layer_height / 2.0 - 2.0,
                color: Color::Magenta,
            });
        }
        
        // Feed-forward network (larger than in standard transformers)
        ctx.draw(&Rectangle {
            x: center_x - 45.0,
            y: y + layer_height / 2.0 + 2.0,
            width: 90.0,
            height: layer_height / 2.0 - 2.0,
            color: Color::Cyan,
        });
        
        // Layer normalization
        ctx.draw(&Rectangle {
            x: center_x - 48.0,
            y: y + layer_height,
            width: 96.0,
            height: 2.0,
            color: Color::Yellow,
        });
        
        // Residual connections
        if layer > 0 {
            let prev_y = center_y + 10.0 + ((layer - 1) as f64 * (layer_height + layer_spacing));
            ctx.draw(&CanvasLine {
                x1: center_x - 55.0,
                y1: prev_y + layer_height + 2.0,
                x2: center_x - 50.0,
                y2: y,
                color: Color::Gray,
            });
            ctx.draw(&CanvasLine {
                x1: center_x + 55.0,
                y1: prev_y + layer_height + 2.0,
                x2: center_x + 50.0,
                y2: y,
                color: Color::Gray,
            });
        }
    }
    
    // Final layer norm
    let final_y = center_y + 10.0 + (num_layers as f64 * (layer_height + layer_spacing));
    ctx.draw(&Rectangle {
        x: center_x - 40.0,
        y: final_y + 5.0,
        width: 80.0,
        height: 4.0,
        color: Color::Yellow,
    });
    
    // Output head (language model)
    ctx.draw(&Rectangle {
        x: center_x - 30.0,
        y: final_y + 15.0,
        width: 60.0,
        height: 15.0,
        color: Color::Red,
    });
    
    // Vocabulary projection indicators
    for i in 0..10 {
        let x = center_x - 25.0 + (i as f64 * 5.0);
        ctx.draw(&CanvasLine {
            x1: x,
            y1: final_y + 30.0,
            x2: x,
            y2: final_y + 35.0,
            color: Color::LightRed,
        });
    }
}

fn render_unknown_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Simple box representation
    ctx.draw(&Rectangle {
        x: center_x - 50.0,
        y: center_y - 30.0,
        width: 100.0,
        height: 60.0,
        color: Color::Gray,
    });
}

/// Render model comparison canvas
pub fn render_model_comparison_canvas(
    ctx: &mut Context,
    models: &[ModelInfo],
    selected_models: &[usize],
    area: Rect,
) {
    if selected_models.is_empty() {
        // Show "no models selected" message
        let center_x = area.width as f64 / 2.0;
        let center_y = area.height as f64 / 2.0;
        
        ctx.draw(&Rectangle {
            x: center_x - 60.0,
            y: center_y - 15.0,
            width: 120.0,
            height: 30.0,
            color: Color::DarkGray,
        });
        return;
    }
    
    let comparison_width = area.width as f64 / selected_models.len() as f64;
    let max_downloads = models.iter()
        .filter(|m| selected_models.contains(&models.iter().position(|mm| mm.id == m.id).unwrap_or(0)))
        .map(|m| m.downloads)
        .max()
        .unwrap_or(1);
    let max_likes = models.iter()
        .filter(|m| selected_models.contains(&models.iter().position(|mm| mm.id == m.id).unwrap_or(0)))
        .map(|m| m.likes)
        .max()
        .unwrap_or(1);
    
    for (i, &model_idx) in selected_models.iter().enumerate() {
        if model_idx >= models.len() {
            continue;
        }
        
        let model = &models[model_idx];
        let x_offset = i as f64 * comparison_width;
        
        // Model container with border
        ctx.draw(&Rectangle {
            x: x_offset + 2.0,
            y: 2.0,
            width: comparison_width - 4.0,
            height: area.height as f64 - 4.0,
            color: Color::Black,
        });
        
        // Model header with model type color coding
        let model_type = detect_model_type(&model.id);
        let header_color = match model_type {
            ModelType::Transformer => Color::Cyan,
            ModelType::CNN => Color::Magenta,
            ModelType::GPT => Color::Yellow,
            ModelType::LSTM => Color::Green,
            ModelType::Unknown => Color::Gray,
        };
        
        ctx.draw(&Rectangle {
            x: x_offset + 2.0,
            y: 2.0,
            width: comparison_width - 4.0,
            height: 20.0,
            color: header_color,
        });
        
        // Statistics bars with improved scaling
        render_enhanced_model_stats_bars(ctx, model, x_offset, area.height as f64, max_downloads, max_likes, comparison_width);
    }
}

fn render_model_stats_bars(ctx: &mut Context, model: &ModelInfo, x_offset: f64, _height: f64) {
    let stats_y_start = 20.0;
    let bar_width = 40.0;
    
    // Downloads bar
    let downloads_percent = (model.downloads as f64 / 100000.0).min(1.0) * 100.0;
    ctx.draw(&Rectangle {
        x: x_offset + 5.0,
        y: stats_y_start,
        width: bar_width * downloads_percent / 100.0,
        height: 8.0,
        color: Color::Cyan,
    });
    
    // Likes bar
    let likes_percent = (model.likes as f64 / 10000.0).min(1.0) * 100.0;
    ctx.draw(&Rectangle {
        x: x_offset + 5.0,
        y: stats_y_start + 12.0,
        width: bar_width * likes_percent / 100.0,
        height: 8.0,
        color: Color::Yellow,
    });
}

/// Enhanced model statistics bars with relative scaling
fn render_enhanced_model_stats_bars(
    ctx: &mut Context, 
    model: &ModelInfo, 
    x_offset: f64, 
    _height: f64,
    max_downloads: u64,
    max_likes: u64,
    comparison_width: f64,
) {
    let stats_y_start = 25.0;
    let bar_width = (comparison_width - 20.0).max(30.0);
    let bar_height = 10.0;
    
    // Downloads bar with relative scaling
    let downloads_width = if max_downloads > 0 {
        (model.downloads as f64 / max_downloads as f64) * bar_width
    } else {
        0.0
    };
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start,
        width: bar_width,
        height: bar_height,
        color: Color::DarkGray,
    });
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start,
        width: downloads_width,
        height: bar_height,
        color: Color::Cyan,
    });
    
    // Likes bar with relative scaling
    let likes_width = if max_likes > 0 {
        (model.likes as f64 / max_likes as f64) * bar_width
    } else {
        0.0
    };
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start + bar_height + 5.0,
        width: bar_width,
        height: bar_height,
        color: Color::DarkGray,
    });
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start + bar_height + 5.0,
        width: likes_width,
        height: bar_height,
        color: Color::Yellow,
    });
    
    // Tags indicator
    let tag_count = model.tags.len();
    let tag_width = if tag_count > 0 {
        (tag_count as f64 / 20.0).min(1.0) * bar_width
    } else {
        0.0
    };
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start + (bar_height + 5.0) * 2.0,
        width: bar_width,
        height: bar_height - 2.0,
        color: Color::DarkGray,
    });
    
    ctx.draw(&Rectangle {
        x: x_offset + 10.0,
        y: stats_y_start + (bar_height + 5.0) * 2.0,
        width: tag_width,
        height: bar_height - 2.0,
        color: Color::Magenta,
    });
}

/// Render network activity canvas
pub fn render_network_activity_canvas(
    ctx: &mut Context,
    download_progress: &DownloadProgress,
    area: Rect,
) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Render enhanced speed gauge with arc
    render_enhanced_speed_gauge(ctx, download_progress.speed_mbps, center_x, center_y, 30.0);
    
    // Render active connections with animation
    let active_connections: Vec<_> = download_progress.chunks.iter()
        .filter(|chunk| chunk.is_active)
        .collect();
    
    // Connection quality indicator based on speed
    let quality_color = if download_progress.speed_mbps > 10.0 {
        Color::Green
    } else if download_progress.speed_mbps > 5.0 {
        Color::Yellow
    } else if download_progress.speed_mbps > 0.0 {
        Color::Red
    } else {
        Color::DarkGray
    };
    
    // Center hub with quality indicator
    ctx.draw(&Circle {
        x: center_x,
        y: center_y,
        radius: 12.0,
        color: quality_color,
    });
    
    // Render active connections in a radial pattern
    for (i, chunk) in active_connections.iter().enumerate() {
        let angle = (i as f64 / active_connections.len() as f64) * std::f64::consts::PI * 2.0;
        let radius = 35.0 + (i as f64 % 3.0) * 5.0; // Vary radius for visual interest
        let x = center_x + radius * angle.cos();
        let y = center_y + radius * angle.sin();
        
        let progress = (chunk.downloaded as f64 / chunk.total as f64 * 100.0) as u16;
        let connection_color = if progress < 50 { 
            Color::Red 
        } else if progress < 90 { 
            Color::Yellow 
        } else { 
            Color::Green 
        };
        
        // Connection node with progress-based size
        let node_radius = 6.0 + (progress as f64 / 100.0) * 4.0;
        ctx.draw(&Circle {
            x,
            y,
            radius: node_radius,
            color: connection_color,
        });
        
        // Connection line with quality indication
        ctx.draw(&CanvasLine {
            x1: center_x,
            y1: center_y,
            x2: x,
            y2: y,
            color: quality_color,
        });
        
        // Progress indicator ring around each node
        if progress > 0 && progress < 100 {
            let _progress_angle = (progress as f64 / 100.0) * std::f64::consts::PI * 2.0;
            for step in 0..(progress as i32) {
                let angle = (step as f64 / 100.0) * std::f64::consts::PI * 2.0;
                let ring_x = x + (node_radius + 3.0) * angle.cos();
                let ring_y = y + (node_radius + 3.0) * angle.sin();
                
                ctx.draw(&Circle {
                    x: ring_x,
                    y: ring_y,
                    radius: 1.0,
                    color: Color::Cyan,
                });
            }
        }
    }
    
    // Render network performance metrics
    render_network_metrics(ctx, download_progress, area);
}

fn render_speed_gauge(ctx: &mut Context, speed_mbps: f64, center_x: f64, center_y: f64, radius: f64) {
    // Speed gauge background
    ctx.draw(&Circle {
        x: center_x,
        y: center_y,
        radius,
        color: Color::Black,
    });
    
    // Speed indicator arc
    let max_speed = 100.0;
    let speed_percent = (speed_mbps / max_speed).min(1.0);
    
    // Draw gauge arc
    for i in 0..(speed_percent * 100.0) as i32 {
        let angle = (i as f64 / 100.0) * std::f64::consts::PI * 1.5 - std::f64::consts::PI;
        let x = center_x + (radius - 2.0) * angle.cos();
        let y = center_y + (radius - 2.0) * angle.sin();
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 1.5,
            color: Color::Cyan,
        });
    }
}



/// Render live config preview
pub fn render_live_config_preview(
    ctx: &mut Context,
    current_config: &AppOptions,
    temp_config: &AppOptions,
    area: Rect,
) {
    let panel_width = area.width as f64 / 2.0;
    
    // Current config panel
    render_config_panel(ctx, current_config, 0.0, area.height as f64, Color::DarkGray);
    
    // Temp config panel
    render_config_panel(ctx, temp_config, panel_width, area.height as f64, Color::Cyan);
}

fn render_config_panel(
    ctx: &mut Context,
    _config: &AppOptions,
    x_offset: f64,
    height: f64,
    color: Color,
) {
    // Panel header
    ctx.draw(&Rectangle {
        x: x_offset + 2.0,
        y: 2.0,
        width: height / 2.0 - 4.0,
        height: 15.0,
        color,
    });
}

/// Render performance analytics canvas
pub fn render_performance_analytics_canvas(
    ctx: &mut Context,
    download_history: &[DownloadRecord],
    area: Rect,
) {
    if download_history.len() < 2 {
        return;
    }
    
    let speeds: Vec<f64> = download_history.iter()
        .map(|record| record.speed_mbps)
        .collect();
    
    render_speed_chart(ctx, &speeds, area);
}

fn render_speed_chart(ctx: &mut Context, speeds: &[f64], area: Rect) {
    let chart_width = area.width as f64 - 20.0;
    let chart_height = area.height as f64 - 40.0;
    let start_x = 10.0;
    let start_y = 10.0;
    
    let max_speed = speeds.iter().copied().fold(0.0, f64::max);
    
    for (i, &speed) in speeds.iter().enumerate() {
        let x = start_x + (i as f64 * chart_width / speeds.len() as f64);
        let bar_height = (speed / max_speed) * chart_height;
        let y = start_y + chart_height - bar_height;
        
        // Speed bar
        ctx.draw(&Rectangle {
            x,
            y,
            width: chart_width / speeds.len() as f64 - 2.0,
            height: bar_height,
            color: Color::Blue,
        });
    }
}

/// Render filter and sort toolbar
pub fn render_filter_toolbar(
    frame: &mut Frame,
    area: Rect,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,
    min_likes: u64,
    focused_field: usize,
) {
    use crate::models::{SortField, SortDirection};
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Filters  [/: Search | 1-4: Presets | f: Focus | +/-: Modify | r: Reset | Ctrl+S: Save]")
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
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    };
    
    let downloads_style = if focused_field == 1 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };
    
    let likes_style = if focused_field == 2 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };
    
    // Detect which preset is active (if any)
    let preset_name = if sort_field == SortField::Modified 
        && sort_direction == SortDirection::Descending 
        && min_downloads == 0 
        && min_likes == 0 {
        Some("Recent")
    } else if sort_field == SortField::Likes 
        && sort_direction == SortDirection::Descending 
        && min_downloads == 0 
        && min_likes == 1_000 {
        Some("Highly Rated")
    } else if sort_field == SortField::Downloads 
        && sort_direction == SortDirection::Descending 
        && min_downloads == 10_000 
        && min_likes == 100 {
        Some("Popular")
    } else if sort_field == SortField::Downloads 
        && sort_direction == SortDirection::Descending 
        && min_downloads == 0 
        && min_likes == 0 {
        Some("No Filters")
    } else {
        None
    };
    
    let mut line_parts = vec![
        Span::styled("Sort: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} {}", sort_name, sort_arrow), sort_style),
        Span::raw("  |  "),
        Span::styled("Min Downloads: ", Style::default().fg(Color::DarkGray)),
        Span::styled(crate::utils::format_number(min_downloads), downloads_style),
        Span::raw("  |  "),
        Span::styled("Min Likes: ", Style::default().fg(Color::DarkGray)),
        Span::styled(crate::utils::format_number(min_likes), likes_style),
    ];
    
    // Add preset indicator if a preset is active
    if let Some(preset) = preset_name {
        line_parts.push(Span::raw("  |  "));
        line_parts.push(Span::styled(
            format!("[{}]", preset),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        ));
    }
    
    let line = Line::from(line_parts);
    
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, inner);
}

/// Enhanced speed gauge with circular arc rendering
fn render_enhanced_speed_gauge(ctx: &mut Context, speed_mbps: f64, center_x: f64, center_y: f64, radius: f64) {
    // Outer ring
    ctx.draw(&Circle {
        x: center_x,
        y: center_y,
        radius,
        color: Color::DarkGray,
    });
    
    // Inner background
    ctx.draw(&Circle {
        x: center_x,
        y: center_y,
        radius: radius - 3.0,
        color: Color::Black,
    });
    
    // Speed arc with color gradient based on speed
    let max_speed = 100.0;
    let speed_percent = (speed_mbps / max_speed).min(1.0);
    let arc_color = if speed_mbps > 50.0 {
        Color::Green
    } else if speed_mbps > 20.0 {
        Color::Yellow
    } else if speed_mbps > 0.0 {
        Color::Red
    } else {
        Color::DarkGray
    };
    
    // Draw speed arc (270 degrees: from -135 to +135 degrees)
    let arc_points = (speed_percent * 270.0) as i32;
    for i in 0..arc_points {
        let angle = (i as f64 - 135.0) * std::f64::consts::PI / 180.0;
        let x = center_x + (radius - 1.5) * angle.cos();
        let y = center_y + (radius - 1.5) * angle.sin();
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 2.0,
            color: arc_color,
        });
    }
    
    // Center display
    let speed_text = if speed_mbps > 0.0 {
        format!("{:.1}", speed_mbps)
    } else {
        "0.0".to_string()
    };
    
    // Note: print method doesn't exist in Context, we'll use rectangles for text representation
    let text_width = speed_text.len() as f64 * 3.0;
    ctx.draw(&Rectangle {
        x: center_x - text_width / 2.0,
        y: center_y - 2.0,
        width: text_width,
        height: 4.0,
        color: arc_color,
    });
}

/// Render enhanced network performance metrics with real-time visualization
fn render_network_metrics(ctx: &mut Context, download_progress: &DownloadProgress, area: Rect) {
    let metrics_x = area.x as f64 + 5.0;
    let metrics_y = area.y as f64 + area.height as f64 - 40.0;
    let metrics_width = area.width as f64 - 10.0;
    
    // Background for metrics panel
    ctx.draw(&Rectangle {
        x: metrics_x,
        y: metrics_y,
        width: metrics_width,
        height: 35.0,
        color: Color::Black,
    });
    
    // Border
    ctx.draw(&Rectangle {
        x: metrics_x,
        y: metrics_y,
        width: metrics_width,
        height: 35.0,
        color: Color::Gray,
    });
    
    // Calculate enhanced metrics
    let active_chunks: Vec<_> = download_progress.chunks.iter()
        .filter(|chunk| chunk.is_active)
        .collect();
    
    let total_progress = if download_progress.total > 0 {
        (download_progress.downloaded as f64 / download_progress.total as f64 * 100.0) as u16
    } else {
        0
    };
    
    let avg_speed = if !active_chunks.is_empty() {
        active_chunks.iter().map(|c| c.speed_mbps).sum::<f64>() / active_chunks.len() as f64
    } else {
        download_progress.speed_mbps
    };
    
    let max_speed = if !download_progress.chunks.is_empty() {
        download_progress.chunks.iter().map(|c| c.speed_mbps).fold(0.0, f64::max)
    } else {
        0.0
    };
    
    // Connection quality indicator (based on speed consistency)
    let quality = if active_chunks.len() > 1 {
        let speed_variance = active_chunks.iter()
            .map(|c| (c.speed_mbps - avg_speed).powi(2))
            .sum::<f64>() / active_chunks.len() as f64;
        let quality_score = (1.0 - (speed_variance / avg_speed.powi(2)).min(1.0)) * 5.0;
        quality_score.max(1.0).min(5.0)
    } else {
        3.0 // Neutral quality for single chunk
    };
    
    // Section 1: Speed gauge (left side)
    let speed_x = metrics_x + 10.0;
    let speed_y = metrics_y + 10.0;
    let speed_gauge_width = 60.0;
    
    // Speed background
    ctx.draw(&Rectangle {
        x: speed_x,
        y: speed_y,
        width: speed_gauge_width,
        height: 20.0,
        color: Color::DarkGray,
    });
    
    // Speed fill with gradient effect
    let speed_fill_width = ((avg_speed / 100.0).min(1.0) * speed_gauge_width * 0.9);
    let speed_color = if avg_speed > 50.0 {
        Color::Green
    } else if avg_speed > 20.0 {
        Color::Yellow
    } else {
        Color::Red
    };
    
    ctx.draw(&Rectangle {
        x: speed_x,
        y: speed_y + 2.0,
        width: speed_fill_width,
        height: 16.0,
        color: speed_color,
    });
    
    // Speed indicator lines
    for i in 0..5 {
        let line_x = speed_x + (i as f64 * speed_gauge_width / 4.0);
        ctx.draw(&CanvasLine {
            x1: line_x,
            y1: speed_y,
            x2: line_x,
            y2: speed_y + 20.0,
            color: Color::Gray,
        });
    }
    
    // Max speed indicator (small triangle)
    if max_speed > 0.0 {
        let max_speed_x = speed_x + ((max_speed / 100.0).min(1.0) * speed_gauge_width);
        ctx.draw(&CanvasLine {
            x1: max_speed_x - 3.0,
            y1: speed_y - 3.0,
            x2: max_speed_x + 3.0,
            y2: speed_y - 3.0,
            color: Color::Cyan,
        });
        ctx.draw(&CanvasLine {
            x1: max_speed_x,
            y1: speed_y - 5.0,
            x2: max_speed_x,
            y2: speed_y,
            color: Color::Cyan,
        });
    }
    
    // Section 2: Connection quality stars (middle-left)
    let quality_x = speed_x + speed_gauge_width + 15.0;
    let quality_y = speed_y + 5.0;
    
    for i in 0..5 {
        let star_x = quality_x + (i as f64 * 12.0);
        let star_color = if i < quality.ceil() as usize {
            if quality >= i as f64 + 0.5 {
                Color::Yellow
            } else {
                Color::LightYellow
            }
        } else {
            Color::DarkGray
        };
        
        // Draw star as diamond shape
        ctx.draw(&Circle {
            x: star_x,
            y: quality_y + 5.0,
            radius: 4.0,
            color: star_color,
        });
    }
    
    // Section 3: Chunk activity grid (middle-right)
    let chunks_x = quality_x + 70.0;
    let chunks_y = speed_y + 5.0;
    let grid_cols = 8;
    let grid_rows = 2;
    
    for (idx, chunk) in download_progress.chunks.iter().enumerate() {
        if idx >= grid_cols * grid_rows {
            break;
        }
        
        let col = idx % grid_cols;
        let row = idx / grid_cols;
        let cell_x = chunks_x + (col as f64 * 8.0);
        let cell_y = chunks_y + (row as f64 * 8.0);
        
        let cell_color = if chunk.is_active {
            let intensity = (chunk.speed_mbps / 50.0).min(1.0);
            if intensity > 0.7 {
                Color::Green
            } else if intensity > 0.3 {
                Color::Yellow
            } else {
                Color::Red
            }
        } else {
            Color::DarkGray
        };
        
        ctx.draw(&Rectangle {
            x: cell_x,
            y: cell_y,
            width: 6.0,
            height: 6.0,
            color: cell_color,
        });
    }
    
    // Section 4: Bandwidth utilization bar (rightmost)
    let bandwidth_x = metrics_x + metrics_width - 70.0;
    let bandwidth_y = speed_y;
    let bandwidth_height = 20.0;
    
    // Bandwidth background
    ctx.draw(&Rectangle {
        x: bandwidth_x,
        y: bandwidth_y,
        width: 15.0,
        height: bandwidth_height,
        color: Color::DarkGray,
    });
    
    // Bandwidth utilization fill
    let utilization = if download_progress.speed_mbps > 0.0 {
        (download_progress.speed_mbps / 100.0).min(1.0)
    } else {
        0.0
    };
    
    let fill_height = utilization * bandwidth_height * 0.9;
    let utilization_color = if utilization > 0.8 {
        Color::Red
    } else if utilization > 0.5 {
        Color::Yellow
    } else {
        Color::Green
    };
    
    ctx.draw(&Rectangle {
        x: bandwidth_x + 2.0,
        y: bandwidth_y + bandwidth_height - 2.0 - fill_height,
        width: 11.0,
        height: fill_height,
        color: utilization_color,
    });
    
    // Section 5: Latency indicator (small circles showing ping)
    let latency_x = bandwidth_x - 25.0;
    let latency_y = speed_y + 10.0;
    
    // Simulated latency visualization (3 dots with varying intensity)
    let latency_quality = quality; // Reuse quality as latency proxy
    
    for i in 0..3 {
        let dot_x = latency_x + (i as f64 * 6.0);
        let dot_intensity = if latency_quality > (3 - i) as f64 {
            1.0
        } else {
            0.3
        };
        
        let dot_color = if dot_intensity > 0.7 {
            Color::Green
        } else if dot_intensity > 0.3 {
            Color::Yellow
        } else {
            Color::Red
        };
        
        ctx.draw(&Circle {
            x: dot_x,
            y: latency_y,
            radius: 2.0 * dot_intensity,
            color: dot_color,
        });
    }
    
    // Progress bar at bottom of metrics panel
    let progress_x = metrics_x + 10.0;
    let progress_y = metrics_y + 25.0;
    let progress_width = metrics_width - 20.0;
    
    // Progress background
    ctx.draw(&Rectangle {
        x: progress_x,
        y: progress_y,
        width: progress_width,
        height: 5.0,
        color: Color::DarkGray,
    });
    
    // Progress fill
    let progress_fill_width = (total_progress as f64 / 100.0) * progress_width;
    let progress_color = if total_progress > 80 {
        Color::Green
    } else if total_progress > 50 {
        Color::Yellow
    } else {
        Color::Cyan
    };
    
    ctx.draw(&Rectangle {
        x: progress_x,
        y: progress_y,
        width: progress_fill_width,
        height: 5.0,
        color: progress_color,
    });
    
    // Progress percentage indicator
    let progress_text_x = progress_x + progress_width / 2.0;
    // Note: Text rendering would require font support, so we use a visual indicator instead
    ctx.draw(&Circle {
        x: progress_text_x,
        y: progress_y + 2.5,
        radius: 2.0,
        color: Color::White,
    });
}

/// Render verification progress chart canvas
pub fn render_verification_progress_chart(
    ctx: &mut Context,
    verification_progress: &[VerificationProgress],
    area: Rect,
) {
    if verification_progress.is_empty() {
        // Show "no verification in progress" message
        let center_x = area.width as f64 / 2.0;
        let center_y = area.height as f64 / 2.0;
        
        ctx.draw(&Rectangle {
            x: center_x - 80.0,
            y: center_y - 15.0,
            width: 160.0,
            height: 30.0,
            color: Color::DarkGray,
        });
        return;
    }
    
    let chart_width = area.width as f64 - 40.0;
    let chart_height = area.height as f64 - 40.0;
    let start_x = area.x as f64 + 20.0;
    let start_y = area.y as f64 + 20.0;
    
    // Background
    ctx.draw(&Rectangle {
        x: start_x,
        y: start_y,
        width: chart_width,
        height: chart_height,
        color: Color::Black,
    });
    
    // Grid lines for better readability
    for i in 0..5 {
        let y = start_y + (i as f64 * chart_height / 4.0);
        ctx.draw(&CanvasLine {
            x1: start_x,
            y1: y,
            x2: start_x + chart_width,
            y2: y,
            color: Color::DarkGray,
        });
    }
    
    // Render each verification as a progress bar with enhanced visuals
    let max_items = verification_progress.len().min(8); // Limit to 8 items for space
    for (i, ver) in verification_progress.iter().take(max_items).enumerate() {
        let y = start_y + (i as f64 * (chart_height / max_items as f64));
        let progress_percent = if ver.total_bytes > 0 {
            ver.verified_bytes as f64 / ver.total_bytes as f64
        } else {
            0.0
        };
        
        // Progress bar background
        ctx.draw(&Rectangle {
            x: start_x + 5.0,
            y: y + 2.0,
            width: chart_width - 10.0,
            height: chart_height / max_items as f64 - 4.0,
            color: Color::DarkGray,
        });
        
        // Progress bar fill with color based on speed
        let bar_color = if progress_percent > 0.9 {
            Color::Green
        } else if progress_percent > 0.5 {
            Color::Yellow
        } else {
            Color::Cyan
        };
        
        ctx.draw(&Rectangle {
            x: start_x + 5.0,
            y: y + 2.0,
            width: (chart_width - 10.0) * progress_percent,
            height: chart_height / max_items as f64 - 4.0,
            color: bar_color,
        });
        
        // Visual file size indicator (small box at the end)
        let size_indicator_width = (ver.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0)).min(1.0) * 20.0;
        ctx.draw(&Rectangle {
            x: start_x + chart_width - 25.0,
            y: y + 4.0,
            width: size_indicator_width,
            height: chart_height / max_items as f64 - 8.0,
            color: Color::Magenta,
        });
        
        // Animated verification indicator (pulsing circle)
        let pulse_phase = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as f64 + i as f64 * 0.5) % 2.0;
        let pulse_size = 1.0 + pulse_phase * 2.0;
        
        if progress_percent < 1.0 {
            ctx.draw(&Circle {
                x: start_x + 10.0,
                y: y + chart_height / max_items as f64 / 2.0,
                radius: pulse_size,
                color: Color::Red,
            });
        }
    }
    
    // Render overall verification statistics
    let total_files = verification_progress.len();
    let completed_files = verification_progress.iter()
        .filter(|v| v.verified_bytes >= v.total_bytes && v.total_bytes > 0)
        .count();
    
    let overall_progress = if total_files > 0 {
        completed_files as f64 / total_files as f64
    } else {
        0.0
    };
    
    // Overall progress bar at bottom
    let overall_y = start_y + chart_height - 10.0;
    ctx.draw(&Rectangle {
        x: start_x + 5.0,
        y: overall_y,
        width: chart_width - 10.0,
        height: 6.0,
        color: Color::DarkGray,
    });
    
    ctx.draw(&Rectangle {
        x: start_x + 5.0,
        y: overall_y,
        width: (chart_width - 10.0) * overall_progress,
        height: 6.0,
        color: Color::Green,
    });
    
    // Statistics boxes
    let stats_box_width = 30.0;
    let stats_box_height = 8.0;
    
    // Completed files box
    ctx.draw(&Rectangle {
        x: start_x + chart_width - 70.0,
        y: start_y + 5.0,
        width: stats_box_width,
        height: stats_box_height,
        color: Color::Black,
    });
    
    ctx.draw(&Rectangle {
        x: start_x + chart_width - 70.0,
        y: start_y + 5.0,
        width: stats_box_width * (completed_files as f64 / total_files as f64),
        height: stats_box_height,
        color: Color::Green,
    });
    
    // Active files box
    ctx.draw(&Rectangle {
        x: start_x + chart_width - 70.0,
        y: start_y + 18.0,
        width: stats_box_width,
        height: stats_box_height,
        color: Color::Black,
    });
    
    ctx.draw(&Rectangle {
        x: start_x + chart_width - 70.0,
        y: start_y + 18.0,
        width: stats_box_width * ((total_files - completed_files) as f64 / total_files as f64),
        height: stats_box_height,
        color: Color::Yellow,
    });
}
