//! TUI drawing (plan W3.4b): the `render_ui` shell, the parameter bundle it
//! consumes, and the pure renderers it delegates to.
//!
//! The former single `render.rs` is a facade over seven submodules; every
//! `crate::ui::render::X` import keeps compiling unchanged:
//!
//! - `models_list` — the Results list item spans
//! - `standard` — `StandardPanelContext`, `render_standard_panels` and the
//!   file-tree panel (Standard display mode)
//! - `gguf` — `GgufPanelContext` and `render_gguf_panels` (GGUF display mode)
//! - `hud` — [`ActivityHudData`], [`activity_hud_height`],
//!   `render_activity_hud` and the HUD row/column builders
//! - `popups` — resume / search / download-path / auth-error overlays
//! - `options_popup` — the 16-field options dialog
//! - `toolbar` — the filter & sort toolbar with its click areas
//!
//! Rendering never mutates `App`: everything arrives through
//! [`RenderParams`], built once per frame by `ui::app`. The file-tree
//! navigation model these panels draw is in `crate::ui::tree` (W3.4a), not
//! here.
//!
//! Module paths are load-bearing for the snapshots: `snap_ui` invokes the
//! insta macro in *this* module, so a snapshot is named
//! `rust_hf_downloader__ui__render__<name>` no matter which of the test
//! modules below takes it — and insta writes it next to the file that holds
//! the macro call, i.e. `src/ui/render/snapshots/`. That is why the test
//! modules are declared here as direct children instead of nesting deeper.

use crate::models::{
    FileTreeNode, FocusedPane, InputMode, ModelDisplayMode, ModelInfo, ModelMetadata,
    QuantizationGroup,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::collections::HashMap;
use tui_input::Input;

mod gguf;
mod hud;
mod models_list;
mod options_popup;
mod popups;
mod standard;
mod toolbar;

pub use hud::*;
pub use options_popup::*;
pub use popups::*;
pub use toolbar::*;

use gguf::{render_gguf_panels, GgufPanelContext};
use models_list::model_list_items;
use standard::{render_standard_panels, StandardPanelContext};

#[cfg(test)]
mod hud_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod style_size_tests;
#[cfg(test)]
mod tests;

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

    // Results list (chunks[1])
    let items = model_list_items(models);

    let list_title = if loading {
        "Results [Loading...]"
    } else if models.is_empty() && !input.value().is_empty() {
        "Results [No models found]"
    } else if models.is_empty() {
        "Results [Enter a search query]"
    } else {
        "Results"
    };

    let list = panel_list(
        items,
        list_title,
        border_style(FocusedPane::Models, input_mode, focused_pane, hovered_panel),
    );

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

/// Border style of a panel: yellow while the pane holds keyboard focus
/// (Normal mode only), cyan while the mouse hovers it, default otherwise.
/// Single home for the guard the four panel renderers repeated verbatim
/// (W3.4c); the H5 style-signature snapshots pin the precedence
/// focus > hover > plain.
pub(super) fn border_style(
    pane: FocusedPane,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    hovered_panel: &Option<FocusedPane>,
) -> Style {
    if input_mode == InputMode::Normal && focused_pane == pane {
        Style::default().fg(Color::Yellow)
    } else if hovered_panel.as_ref() == Some(&pane) {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

/// The shared list-panel shape: bordered block, pane title, pane border
/// style and one selection highlight everywhere (W3.4c — the Results list,
/// the file tree and both GGUF lists built this by hand). Callers still
/// register their area in `panel_areas` for click/hover detection.
pub(super) fn panel_list<'a>(items: Vec<ListItem<'a>>, title: &'a str, style: Style) -> List<'a> {
    List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(style),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ")
}

// =====================================================================
// Snapshot tests (insta)
//
// The four test modules are sibling files (`snapshot_tests`, `hud_tests`,
// `tests`, `style_size_tests`); `snap_ui` stays in THIS module on purpose:
// insta names a snapshot `{module_path}__{name}` and stores it in the
// `snapshots/` folder next to the file that expands the macro, so expanding
// it here keeps every name `rust_hf_downloader__ui__render__*` (only the
// directory moved with this file). All snapshots use a fixed 100x30
// TestBackend terminal and literal fixture constants so the output is fully
// deterministic (no time, randomness, or environment reads).
// =====================================================================

/// Assert a named terminal snapshot with the crate version normalized to
/// `v<VERSION>`, so UI footer snapshots stay byte-stable across version
/// bumps — the version lives only in Cargo.toml. `env!` is compile-time,
/// so the filter always matches the *current* build's rendered footer.
/// (Regex filter — dots in the version are escaped.)
#[cfg(test)]
fn snap_ui(name: &str, terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) {
    let pattern = format!("v{}", env!("CARGO_PKG_VERSION")).replace('.', r"\.");
    let mut settings = insta::Settings::new();
    settings.add_filter(&pattern, "v<VERSION>");
    settings.bind(|| {
        insta::assert_snapshot!(name, terminal.backend());
    });
}
