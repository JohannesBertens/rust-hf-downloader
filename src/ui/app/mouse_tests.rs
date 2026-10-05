//! Mouse behavior tests (G3, test-hardening pass), driving the private
//! `handle_mouse_*` handlers directly with rects from a REAL render pass
//! (`App::draw` over a TestBackend registers the frame's
//! `MouseAreas` — the same registry production uses).
//!
//! Pins:
//! - click inside a filter-field rect cycles that filter, and the
//!   filters-before-panels lookup order wins even when a filter rect is
//!   made to overlap the Models panel rect (first-match is behavior);
//! - scroll over the toolbar cycles the filter under the cursor instead
//!   of navigating the focused list, while scroll elsewhere navigates;
//! - clicks and scrolls are no-ops while any popup is open;
//! - a mouse move over a pane sets `hovered_panel` (and clears it with a
//!   popup open or when over no pane).

use super::state::App;
use crate::models::{FocusedPane, ModelInfo, PopupMode, SortField};
use crate::paths::{ENV_CONFIG_DIR, ENV_MUTEX};
use ratatui::backend::TestBackend;
/// Env guard restoring one variable on drop (same pattern as the other
/// env-mutating tests; App::new reads the ambient config path).
struct VarGuard {
    key: &'static str,
    saved: Option<std::ffi::OsString>,
}

impl VarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let saved = std::env::var_os(key);
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        Self { key, saved }
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Keeps `RUST_HF_DOWNLOADER_CONFIG_DIR` pointed at a fresh temp dir for
/// the guard's lifetime, so `App::new`'s config read is hermetic (filter
/// seeds are the compiled-in defaults regardless of the host's config).
struct TempConfigDir {
    tmp: std::path::PathBuf,
    _g: VarGuard,
}

impl TempConfigDir {
    fn install() -> Self {
        let tmp = std::env::temp_dir().join(format!("rhd-mouse-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create temp config dir");
        Self {
            _g: VarGuard::set(ENV_CONFIG_DIR, tmp.to_str()),
            tmp,
        }
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        author: None,
        downloads: 100,
        likes: 10,
        tags: Vec::new(),
        last_modified: None,
    }
}

/// App with three models, row 1 selected, and mouse hit-rects registered
/// by one REAL render pass at 100x30 (exactly what `App::run`'s draw
/// loop produces each frame). Call under `ENV_MUTEX`; keep the returned
/// `TempConfigDir` alive for the duration of the test body.
fn rendered_app() -> (App, TempConfigDir) {
    let cfg = TempConfigDir::install();
    let mut app = App::new();
    *app.models.write() = vec![model("a/one"), model("b/two"), model("c/three")];
    app.list_state.select(Some(1));

    let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| app.draw(frame))
        .expect("render pass for hit-rects");

    assert!(
        !app.mouse.areas.filters.is_empty() && !app.mouse.areas.panels.is_empty(),
        "render pass must register hit-rects"
    );
    (app, cfg)
}

/// The rect of filter field `idx` from the rendered registry.
fn filter_rect(app: &App, idx: usize) -> ratatui::layout::Rect {
    app.mouse
        .areas
        .filters
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("filter rect {idx} not registered"))
}

/// Center cell of a rect (this ratatui's `Rect` has no `center()`).
fn center(rect: ratatui::layout::Rect) -> ratatui::layout::Position {
    ratatui::layout::Position {
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    }
}

/// The Models panel rect from the rendered registry.
fn models_rect(app: &App) -> ratatui::layout::Rect {
    app.mouse
        .areas
        .panels
        .iter()
        .find(|(p, _)| *p == FocusedPane::Models)
        .map(|(_, r)| *r)
        .expect("Models rect registered")
}

#[test]
fn click_filter_rect_cycles_that_filter() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _cfg) = rendered_app();

    let rect = filter_rect(&app, 0); // sort field
    let pos = center(rect);
    app.handle_mouse_click(pos.x, pos.y);

    assert_eq!(
        app.filters.sort_field,
        SortField::Likes,
        "click cycles sort"
    );
    assert_eq!(
        app.focused_filter_field, 0,
        "click focuses the clicked field"
    );
    assert!(
        app.needs_search_models,
        "filter mutation schedules the refresh tail"
    );
    // The click targeted the toolbar, not the list: selection untouched.
    assert_eq!(app.list_state.selected(), Some(1));
}

#[test]
fn click_filter_wins_first_match_even_overlapping_models_rect() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _cfg) = rendered_app();

    // Forge the overlap the lookup order exists for: the sort field's
    // rect now ALSO covers the Models panel. First-match must resolve to
    // the FILTER entry, never the panel beneath it.
    let models = models_rect(&app);
    app.mouse.areas.filters[0].1 = models;
    let pos = center(models);
    app.handle_mouse_click(pos.x, pos.y);

    assert_eq!(
        app.filters.sort_field,
        SortField::Likes,
        "overlapping click must resolve to the filter (first match)"
    );
    assert_eq!(app.list_state.selected(), Some(1), "list not navigated");
}

#[test]
fn scroll_over_toolbar_cycles_filter_not_list() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _cfg) = rendered_app();

    // Scroll NOT over any filter rect falls through to the focused list
    // (do this FIRST: a toolbar scroll's refresh tail clears the model
    // list, which would no-op the navigation below).
    let models = models_rect(&app);
    let pos = center(models);
    app.handle_mouse_scroll(false, pos.x, pos.y);
    assert_eq!(
        app.list_state.selected(),
        Some(2),
        "scroll outside the toolbar navigates the focused list"
    );

    // Scroll-down over the min-downloads field (0 → next step 100);
    // the Models list holds focus and a selection that must NOT move.
    let rect = filter_rect(&app, 1);
    let pos = center(rect);
    app.handle_mouse_scroll(false, pos.x, pos.y);
    assert_eq!(app.filters.min_downloads, 100, "scroll-down steps forward");
    assert_eq!(app.focused_filter_field, 1);
    assert_eq!(app.list_state.selected(), Some(2), "list not scrolled");

    // Scroll-up over the sort field cycles it BACKWARD.
    let rect = filter_rect(&app, 0);
    let pos = center(rect);
    app.handle_mouse_scroll(true, pos.x, pos.y);
    assert_eq!(
        app.filters.sort_field,
        SortField::Name,
        "scroll-up cycles the sort field backward (Downloads → Name)"
    );
    assert_eq!(
        app.list_state.selected(),
        Some(2),
        "list still not scrolled"
    );
}

#[test]
fn click_and_scroll_with_popup_open_are_noops() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _cfg) = rendered_app();
    app.popup_mode = PopupMode::Options;

    let rect = filter_rect(&app, 0);
    let pos = center(rect);
    app.handle_mouse_click(pos.x, pos.y);
    app.handle_mouse_scroll(false, pos.x, pos.y);

    assert_eq!(app.filters.sort_field, SortField::Downloads, "click no-op");
    assert_eq!(app.filters.min_downloads, 0, "scroll no-op");
    assert_eq!(app.focused_filter_field, 0, "focus field untouched");
    assert_eq!(app.list_state.selected(), Some(1), "list untouched");
    assert_eq!(app.popup_mode, PopupMode::Options, "popup stays open");

    // Hover is suppressed too: a move over a live panel rect reports no
    // hover while a popup owns the screen.
    let models = models_rect(&app);
    let pos = center(models);
    app.update_hover_state(pos.x, pos.y);
    assert_eq!(app.mouse.hovered_panel, None, "no hover under a popup");
}

#[test]
fn mouse_moved_over_a_pane_sets_hovered_panel() {
    let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _cfg) = rendered_app();

    let models = models_rect(&app);
    let pos = center(models);
    app.update_hover_state(pos.x, pos.y);
    assert_eq!(app.mouse.hovered_panel, Some(FocusedPane::Models));

    // A bottom pane (GGUF mode registers QuantizationGroups left,
    // QuantizationFiles right): hover the bottom-left panel rect.
    let (_, bottom_left) = app.mouse.areas.panels[1];
    let pos = center(bottom_left);
    app.update_hover_state(pos.x, pos.y);
    assert_eq!(
        app.mouse.hovered_panel,
        Some(FocusedPane::QuantizationGroups)
    );

    // Toolbar row: not a panel rect — hover clears.
    app.update_hover_state(50, 0);
    assert_eq!(app.mouse.hovered_panel, None, "toolbar row is no pane");
}
