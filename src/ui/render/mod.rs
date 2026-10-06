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
//! - `options_popup` — the 16-field options dialog renderer (a pure
//!   consumer since M5/U1: the dialog state + [`crate::ui::app::options::OPTIONS_FIELDS`]
//!   table live in `ui/app/options.rs`, imported from there)
//! - `toolbar` — the filter & sort toolbar with its click areas
//!   ([`FilterCtx`] is its input group)
//!
//! The remaining [`RenderParams`] groups live here: [`FocusCtx`],
//! [`ListCtx`], [`StatusCtx`] and the [`MouseAreas`] registry.
//!
//! Rendering never mutates `App`: everything arrives through
//! [`RenderParams`], built once per frame by `ui::app`, and the pass is
//! pure — mouse hit-rects and the reserved HUD strip come back through
//! [`RenderOutput`] for the caller to register. The file-tree
//! navigation model these panels draw is in `crate::ui::tree` (W3.4a), not
//! here.
//!
//! Module paths are load-bearing for the snapshots: `snap_ui` invokes the
//! insta macro in *this* module, so a snapshot is named
//! `rust_hf_downloader__ui__render__<name>` no matter which of the test
//! modules below takes it — and insta writes it next to the file that holds
//! the macro call, i.e. `src/ui/render/snapshots/`. That is why the test
//! modules are declared here as direct children instead of nesting deeper.

use crate::models::{FocusedPane, ModelDisplayMode, ModelInfo, PopupMode};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
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

// Panel-input groups consumed by [`RenderParams`] (W5.2): each bottom-
// panel renderer owns its own context struct, re-exported so `ui::app`
// can construct them next to the other groups.
use gguf::render_gguf_panels;
pub use gguf::GgufPanelContext;
use models_list::model_list_items;
use standard::render_standard_panels;
pub use standard::StandardPanelContext;

#[cfg(test)]
mod hud_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod style_size_tests;
#[cfg(test)]
mod tests;

/// Focus & hover state shared by every panel border (W5.2 group): the
/// triple [`border_style`] consults for its focus > hover > plain
/// precedence. One struct so the panel renderers no longer thread the
/// three values separately.
pub struct FocusCtx {
    pub popup_mode: PopupMode,
    pub focused_pane: FocusedPane,
    pub hovered_panel: Option<FocusedPane>,
}

/// Results-list inputs (the main content area). `input` is read only for
/// the empty-state title ("No models found" vs "Enter a search query").
pub struct ListCtx<'a> {
    pub input: &'a Input,
    pub models: &'a [ModelInfo],
    pub list_state: &'a mut ListState,
    pub loading: bool,
}

/// Status-bar inputs (the bottom block's two lines).
pub struct StatusCtx<'a> {
    pub error: &'a Option<String>,
    pub status: &'a str,
    pub selection_info: &'a str,
}

/// Mouse hit-rects produced by one render pass (W5.2): the render side is
/// pure — it RETURNS this registry instead of writing into caller-owned
/// `&mut Vec` out-params, and `App::draw` stores it once per frame.
/// Hit-testing (clicks, hover) is first-match over each list, so the
/// REGISTRATION ORDER below is behavior:
///
/// - `panels`: Results list first, then the active display mode's bottom
///   panels left-to-right — GGUF: QuantizationGroups, QuantizationFiles;
///   Standard: ModelMetadata, FileTree.
/// - `filters`: toolbar fields in display order (0 = sort, 1 = min
///   downloads, 2 = min likes).
#[derive(Debug, Default)]
pub struct MouseAreas {
    /// Panel rects in registration order (first match wins hit-tests).
    pub panels: Vec<(FocusedPane, Rect)>,
    /// Toolbar field rects in registration order (first match wins).
    pub filters: Vec<(usize, Rect)>,
}

/// Everything `App::draw` needs back from one render pass: the reserved
/// Activity HUD strip rect and the frame's mouse hit-rect registry.
pub struct RenderOutput {
    /// The reserved HUD strip (zero height when idle or the terminal is
    /// too short) — `App::draw` renders the activity HUD into it.
    pub hud_strip: Rect,
    /// This frame's hit-rects; the caller registers them on `App`.
    pub mouse: MouseAreas,
}

/// Parameters for rendering the UI (W5.2: the former 27-field bag grouped
/// into per-consumer contexts; the two `&mut Vec` out-params became the
/// returned [`RenderOutput::mouse`]). The group for the inactive display
/// mode is still supplied — `display_mode` picks which one is read.
pub struct RenderParams<'a> {
    pub display_mode: ModelDisplayMode,
    pub focus: FocusCtx,
    pub list: ListCtx<'a>,
    pub gguf: GgufPanelContext<'a>,
    pub standard: StandardPanelContext<'a>,
    pub filters: FilterCtx,
    pub status: StatusCtx<'a>,
    /// Activity HUD: DESIRED strip height above the status bar (the
    /// natural, uncapped activity_hud_height; render_ui clamps it against
    /// the base layout and returns the reserved strip rect — 0 = hidden)
    pub hud_height: u16,
}

/// Rows the base layout needs besides the HUD strip: 3 toolbar + 10
/// main content (Min) + 12 bottom panels + 4 status bar. The desired HUD
/// height is clamped against this budget so it never steals base-layout
/// rows — W4.10 single-homed here (next to the Constraint list it
/// guards); it previously lived in `App::draw` as `base_layout_rows = 29`
/// with a second, manual strip-rect computation.
const BASE_LAYOUT_ROWS: u16 = 3 + 10 + 12 + 4;

/// Render the main UI and return the reserved Activity HUD strip rect
/// plus this frame's mouse hit-rects (see [`RenderOutput`]; zero strip
/// height when idle or when the terminal is too short for the base
/// layout). `App::draw` renders the activity HUD into the returned rect
/// and stores the hit-rects — one owner for the vertical layout since
/// W4.10, pure render pass since W5.2.
pub fn render_ui(frame: &mut Frame, params: RenderParams) -> RenderOutput {
    let RenderParams {
        display_mode,
        focus,
        list,
        gguf,
        standard,
        filters,
        status,
        hud_height,
    } = params;

    let ListCtx {
        input,
        models,
        list_state,
        loading,
    } = list;
    let StatusCtx {
        error,
        status,
        selection_info,
    } = status;

    // Hit-rects registered this frame, in lookup order (see MouseAreas).
    let mut mouse = MouseAreas {
        panels: Vec::new(),
        filters: Vec::new(),
    };

    // Clamp the desired HUD strip so it never steals rows the base
    // layout needs (see BASE_LAYOUT_ROWS); the strip rect is chunks[3].
    let hud_height = hud_height.min(frame.area().height.saturating_sub(BASE_LAYOUT_ROWS));

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

    // Render filter toolbar (registers the three field hit-rects)
    mouse.filters = render_filter_toolbar(frame, chunks[0], filters);

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

    let list = panel_list(items, list_title, border_style(FocusedPane::Models, &focus));

    // Register panel area for click/hover detection (registration order
    // is behavior: first match wins — Models before the bottom panels)
    mouse.panels.push((FocusedPane::Models, chunks[1]));
    frame.render_stateful_widget(list, chunks[1], list_state);

    // Split bottom panel into left and right sections
    let bottom_panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    // Render based on display mode (each renderer registers its panels'
    // areas left-to-right, after the Results list above)
    match display_mode {
        ModelDisplayMode::Gguf => {
            render_gguf_panels(frame, bottom_panel_chunks, gguf, &focus, &mut mouse.panels);
        }
        ModelDisplayMode::Standard => {
            render_standard_panels(
                frame,
                bottom_panel_chunks,
                standard,
                &focus,
                &mut mouse.panels,
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
    let has_filters = filters.min_downloads > 0
        || filters.min_likes > 0
        || filters.sort_field != crate::models::SortField::Downloads
        || filters.sort_direction != crate::models::SortDirection::Descending;

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

    // The reserved HUD strip: the rect App::draw renders the activity HUD
    // into. Whenever it is non-empty the constraints fit the terminal
    // exactly, so it is always the rows directly above the status bar.
    RenderOutput {
        hud_strip: chunks[3],
        mouse,
    }
}

/// Border style of a panel: yellow while the pane holds keyboard focus
/// (no popup open), cyan while the mouse hovers it, default otherwise.
/// Single home for the guard the four panel renderers repeated verbatim
/// (W3.4c); the H5 style-signature snapshots pin the precedence
/// focus > hover > plain.
pub(super) fn border_style(pane: FocusedPane, focus: &FocusCtx) -> Style {
    if focus.popup_mode == PopupMode::None && focus.focused_pane == pane {
        Style::default().fg(Color::Yellow)
    } else if focus.hovered_panel == Some(pane) {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

/// The shared list-panel shape: bordered block, pane title, pane border
/// style and one selection highlight everywhere (W3.4c — the Results list,
/// the file tree and both GGUF lists built this by hand). Callers still
/// register their area in the panel hit-rects for click/hover detection.
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

/// Centered popup Rect over `area` (W4.8 — the five popup prologues'
/// shared geometry). Width is clamped to `area.width - 4` exactly as every
/// historical site did; height is taken as given (only the options dialog
/// ever clamped its height against `area.height - 4` — it does so before
/// calling). Both axes center with plain integer division: every site used
/// `/ 2`, so odd remainders truncate — no rounding variants existed.
pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    Rect {
        x: area.width.saturating_sub(width) / 2,
        y: area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Shared popup prologue (W4.8): clear `area`, render the bordered titled
/// block, and return the block's inner area. `style` styles the WHOLE
/// block (borders + title) — the convention of the four `popups.rs`
/// overlays. The options dialog deliberately does NOT use this helper's
/// styling: it applies `border_style` (borders only, title unstyled) — a
/// visible difference pinned by the options-popup snapshots — and keeps
/// its own Clear + Block construction.
pub(super) fn popup_shell(frame: &mut Frame, area: Rect, title: &str, style: Style) -> Rect {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(style);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
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
