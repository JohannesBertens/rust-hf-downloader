use crate::models::{FocusedPane, InputMode, ModelInfo, QuantizationInfo, QuantizationGroup, DownloadProgress, VerificationProgress, ModelDisplayMode, ModelMetadata, FileTreeNode};
use crate::utils::{format_number, format_size};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap, Gauge, Clear},
};
use std::collections::HashMap;
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

    // Search input box
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title("Search HuggingFace Models")
        .border_style(if input_mode == InputMode::Editing {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });

    let width = input_block.inner(chunks[0]).width.max(3) - 1;
    let scroll = input.visual_scroll(width as usize);
    
    let input_widget = Paragraph::new(input.value())
        .style(Style::default())
        .block(input_block)
        .scroll((0, scroll as u16));
    
    frame.render_widget(input_widget, chunks[0]);

    if input_mode == InputMode::Editing {
        frame.set_cursor_position((
            chunks[0].x + ((input.visual_cursor()).max(scroll) - scroll) as u16 + 1,
            chunks[0].y + 1,
        ));
    }

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
    
    let line2 = if let Some(err) = error {
        format!("Error: {}", err)
    } else {
        status.to_string()
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

pub fn render_download_path_popup(
    frame: &mut Frame,
    download_path_input: &Input,
) {
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
) {
    let popup_width = 64.min(frame.area().width.saturating_sub(4));
    let popup_height = 26;
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
}
