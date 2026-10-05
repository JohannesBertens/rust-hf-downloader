// =====================================================================
// Style-signature + size-matrix snapshot tests (harness H5).
//
// `snap_ui` (see `snapshot_tests`) snapshots the TestBackend's *symbol*
// buffer only — characters and spacing; fg/bg/modifier styles are
// invisible to those files. The helpers below walk the `Buffer` cells
// directly and emit a compact run-length signature of (fg, bg,
// modifier) for a given region, skipping runs that are entirely
// default. That makes style decisions (focus/hover borders, selection
// highlight, popup Clear) fail a reviewable snapshot diff instead of
// silently changing under refactors (W3.4 border/panel-list dedup).
// =====================================================================

use super::snapshot_tests::{quantization_fixtures, three_model_fixtures};
use super::*;
use crate::models::{
    AppOptions, DownloadMetadata, DownloadProgress, DownloadStatus, FileTreeNode, ModelMetadata,
    QuantizationGroup, SortDirection, SortField,
};
use ratatui::{backend::TestBackend, Terminal};
use std::collections::HashMap;

// ----------------- style-signature helpers -----------------

/// Stable names for modifier bits instead of relying on bitflags'
/// Debug formatting.
fn modifier_names(m: Modifier) -> String {
    let mut names = Vec::new();
    if m.contains(Modifier::BOLD) {
        names.push("BOLD");
    }
    if m.contains(Modifier::DIM) {
        names.push("DIM");
    }
    if m.contains(Modifier::ITALIC) {
        names.push("ITALIC");
    }
    if m.contains(Modifier::UNDERLINED) {
        names.push("UNDERLINED");
    }
    if m.contains(Modifier::SLOW_BLINK) {
        names.push("SLOW_BLINK");
    }
    if m.contains(Modifier::RAPID_BLINK) {
        names.push("RAPID_BLINK");
    }
    if m.contains(Modifier::REVERSED) {
        names.push("REVERSED");
    }
    if m.contains(Modifier::CROSSED_OUT) {
        names.push("CROSSED_OUT");
    }
    names.join("+")
}

/// Run-length style signature of `area`: one line per non-empty row,
/// `<x>..<xEnd> fg=F,bg=B,mod=A+B` per styled run; runs where fg/bg/
/// modifier are all default are skipped so the signature stays
/// compact and the interesting style decisions stand out.
fn style_runs(terminal: &Terminal<TestBackend>, area: Rect) -> String {
    let buf = terminal.backend().buffer();
    let default = (Color::Reset, Color::Reset, Modifier::empty());
    let mut rows = Vec::new();
    for y in area.top()..area.bottom() {
        let mut runs = Vec::new();
        let mut x = area.left();
        while x < area.right() {
            let cell = &buf[(x, y)];
            let current = (cell.fg, cell.bg, cell.modifier);
            let mut end = x + 1;
            while end < area.right() {
                let cell = &buf[(end, y)];
                if (cell.fg, cell.bg, cell.modifier) != current {
                    break;
                }
                end += 1;
            }
            if current != default {
                let mut parts = Vec::new();
                if current.0 != Color::Reset {
                    parts.push(format!("fg={:?}", current.0));
                }
                if current.1 != Color::Reset {
                    parts.push(format!("bg={:?}", current.1));
                }
                let mods = modifier_names(current.2);
                if !mods.is_empty() {
                    parts.push(format!("mod={mods}"));
                }
                let range = if end - x > 1 {
                    format!("{x}..{}", end - 1)
                } else {
                    format!("{x}")
                };
                runs.push(format!("{range} {}", parts.join(",")));
            }
            x = end;
        }
        if !runs.is_empty() {
            rows.push(format!("y{y}: {}", runs.join(" | ")));
        }
    }
    rows.join("\n")
}

/// Snapshot a style signature (no version text can appear in it, so
/// unlike `snap_ui` no version filter is needed).
fn snap_style(name: &str, signature: &str) {
    insta::assert_snapshot!(name, signature);
}

// ----------------- fixture + draw helpers -----------------

/// Per-draw mutable state for the standard fixture pair from
/// `snapshot_tests` (three models / two quantization groups).
struct UiFixture {
    input: Input,
    models: Vec<ModelInfo>,
    list_state: ListState,
    quantizations: Vec<QuantizationGroup>,
    quant_list_state: ListState,
    quant_file_list_state: ListState,
}

impl UiFixture {
    /// Models list with row 1 selected (mirrors
    /// `snapshot_render_ui_model_list_selection`).
    fn with_selection() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(1));
        Self {
            input: Input::new("llama".to_string()),
            models: three_model_fixtures(),
            list_state,
            quantizations: Vec::new(),
            quant_list_state: ListState::default(),
            quant_file_list_state: ListState::default(),
        }
    }

    /// Quantization panels with group 0 / file 0 selected (mirrors
    /// `snapshot_render_ui_quantization_panels`).
    fn quant_view() -> Self {
        let mut quant_list_state = ListState::default();
        quant_list_state.select(Some(0));
        let mut quant_file_list_state = ListState::default();
        quant_file_list_state.select(Some(0));
        Self {
            input: Input::default(),
            models: three_model_fixtures(),
            list_state: ListState::default(),
            quantizations: quantization_fixtures(),
            quant_list_state,
            quant_file_list_state,
        }
    }
}

/// Draw `render_ui` with the shared defaults of `snapshot_tests::
/// draw_render_ui` (no error/metadata/file-tree, GGUF mode, no
/// filters) but parameterized on focus, hover and HUD height, then
/// run `overlay` in the SAME draw closure — mirrors the app loop,
/// where popups and the HUD render on top of the live UI. Returns
/// the HUD strip rect render_ui reserved (W4.10).
#[allow(clippy::too_many_arguments)]
fn draw_ui_with_overlay(
    terminal: &mut Terminal<TestBackend>,
    fixture: &mut UiFixture,
    focused: FocusedPane,
    hovered: Option<FocusedPane>,
    hud_height: u16,
    status: &str,
    selection_info: &str,
    overlay: impl FnOnce(&mut Frame),
) -> Rect {
    let error: Option<String> = None;
    let model_metadata: Option<ModelMetadata> = None;
    let file_tree: Option<FileTreeNode> = None;
    let mut file_tree_state = ListState::default();
    let complete_downloads: HashMap<String, DownloadMetadata> = HashMap::new();

    let mut hud_rect = Rect::default();
    terminal
        .draw(|frame| {
            hud_rect = render_ui(
                frame,
                RenderParams {
                    display_mode: ModelDisplayMode::Gguf,
                    focus: FocusCtx {
                        input_mode: InputMode::Normal,
                        focused_pane: focused,
                        hovered_panel: hovered,
                    },
                    list: ListCtx {
                        input: &fixture.input,
                        models: &fixture.models,
                        list_state: &mut fixture.list_state,
                        loading: false,
                    },
                    gguf: GgufPanelContext {
                        quantizations: &fixture.quantizations,
                        quant_list_state: &mut fixture.quant_list_state,
                        quant_file_list_state: &mut fixture.quant_file_list_state,
                        loading_quants: false,
                        complete_downloads: &complete_downloads,
                    },
                    standard: StandardPanelContext {
                        model_metadata: &model_metadata,
                        file_tree: &file_tree,
                        file_tree_state: &mut file_tree_state,
                        loading: false,
                    },
                    filters: FilterCtx {
                        sort_field: SortField::Downloads,
                        sort_direction: SortDirection::Descending,
                        min_downloads: 0,
                        min_likes: 0,
                        focused_field: 5,
                    },
                    status: StatusCtx {
                        error: &error,
                        status,
                        selection_info,
                    },
                    hud_height,
                },
            )
            .hud_strip;
            overlay(frame);
        })
        .expect("failed to draw UI");
    hud_rect
}

fn draw_ui(
    terminal: &mut Terminal<TestBackend>,
    fixture: &mut UiFixture,
    focused: FocusedPane,
    hovered: Option<FocusedPane>,
    hud_height: u16,
    status: &str,
    selection_info: &str,
) {
    draw_ui_with_overlay(
        terminal,
        fixture,
        focused,
        hovered,
        hud_height,
        status,
        selection_info,
        |_| {},
    );
}

// ----------------- style snapshots -----------------

#[test]
fn style_focus_border_yellow_on_models_pane() {
    // Keyboard focus on Models: the Results border is fg=Yellow while
    // the unfocused bottom panes keep the default border style.
    let mut fixture = UiFixture::with_selection();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
    );
    snap_style(
        "style_focus_border_yellow_on_models_pane",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

#[test]
fn style_unfocused_models_pane_border_is_plain() {
    // Same fixture, focus moved to the quantization-files pane: the
    // Results border must LOSE fg=Yellow (default style) while the
    // Files pane border gains it — the contrast pair for (a).
    let mut fixture = UiFixture::quant_view();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::QuantizationFiles,
        None,
        0,
        "2 quantization groups available",
        "",
    );
    snap_style(
        "style_unfocused_models_pane_border_is_plain",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

#[test]
fn style_hovered_pane_border_is_cyan() {
    // Mouse hover on the quantization-groups pane while keyboard
    // focus stays on Models: hovered border renders fg=Cyan (see
    // border_style in render/mod.rs — hover beats plain, focus beats
    // hover).
    let mut fixture = UiFixture::quant_view();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        Some(FocusedPane::QuantizationGroups),
        0,
        "2 quantization groups available",
        "",
    );
    snap_style(
        "style_hovered_pane_border_is_cyan",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

#[test]
fn style_selected_list_item_highlight() {
    // Row 1 selected in the Results list: the highlight style paints
    // the full row bg=DarkGray + BOLD, including the ">> " marker
    // gutter.
    let mut fixture = UiFixture::with_selection();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
    );
    snap_style(
        "style_selected_list_item_highlight",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

#[test]
fn style_options_popup_clear_and_border() {
    // Options popup drawn over the populated UI (same draw closure,
    // like the app loop): Clear must wipe the underlying styles
    // inside the popup area (interior stays default) while the popup
    // border is fg=Yellow.
    let options = AppOptions {
        default_directory: "/home/testuser/models".to_string(),
        hf_token: None,
        ..AppOptions::default()
    };
    let directory_input = Input::default();
    let token_input = Input::default();
    let mut fixture = UiFixture::with_selection();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
        |frame| render_options_popup(frame, &options, &directory_input, &token_input),
    );
    snap_style(
        "style_options_popup_clear_and_border",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

#[test]
fn style_resume_popup_clear_and_background() {
    // Resume popup: Clear + a Block styled fg=Yellow/bg=Black for the
    // WHOLE popup rect (Block::style, not border_style), so the
    // signature must show one yellow-on-black run across the popup
    // and default styles outside it.
    let incomplete = vec![DownloadMetadata {
        model_id: "meta-llama/Llama-3.1-8B".to_string(),
        filename: "Llama-3.1-8B-Q4_K_M.gguf".to_string(),
        url: "https://huggingface.co/meta-llama/Llama-3.1-8B/resolve/main/Llama-3.1-8B-Q4_K_M.gguf"
            .to_string(),
        local_path: "/home/user/models/meta-llama/Llama-3.1-8B/Llama-3.1-8B-Q4_K_M.gguf"
            .to_string(),
        total_size: 4_921_860_096,
        downloaded_size: 1_230_465_024,
        status: DownloadStatus::Incomplete,
        expected_sha256: None,
        revision: None,
    }];
    let mut fixture = UiFixture::with_selection();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
        |frame| render_resume_popup(frame, &incomplete),
    );
    snap_style(
        "style_resume_popup_clear_and_background",
        &style_runs(&terminal, Rect::new(0, 0, 100, 30)),
    );
}

// ----------------- size matrix -----------------

/// Render both main layouts (models-list focus and the
/// downloads/quant view) at one size and snapshot each buffer.
fn size_matrix_at(width: u16, height: u16, label: &str) {
    let mut fixture = UiFixture::with_selection();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
    );
    snap_ui(&format!("size_matrix_models_focus_{label}"), &terminal);

    let mut fixture = UiFixture::quant_view();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    draw_ui(
        &mut terminal,
        &mut fixture,
        FocusedPane::QuantizationGroups,
        None,
        0,
        "2 quantization groups available",
        "",
    );
    snap_ui(&format!("size_matrix_quant_view_{label}"), &terminal);
}

#[test]
fn size_matrix_80x24_models_and_quant() {
    size_matrix_at(80, 24, "80x24");
}

#[test]
fn size_matrix_120x40_models_and_quant() {
    size_matrix_at(120, 40, "120x40");
}

#[test]
fn size_matrix_60x20_models_and_quant() {
    size_matrix_at(60, 20, "60x20");
}

#[test]
fn size_matrix_61x23_models_and_quant() {
    size_matrix_at(61, 23, "61x23");
}

// ----------------- HUD threshold boundary -----------------

#[test]
fn hud_threshold_boundary_full_and_clamped() {
    // Replicates the App::draw arithmetic (src/ui/app.rs): the HUD
    // strip is min(natural height, area.height - 29 reserved
    // base-layout rows), rendered above the 4-row status bar. This
    // fixture (one download row, nothing else) has natural height 4,
    // so 33 rows is the exact boundary where the full 4-row HUD fits
    // and 32 rows is one below it -> clamped to 3 rows.
    let dl = DownloadProgress {
        model_id: "meta-llama/Llama-3.1-8B".to_string(),
        filename: "Llama-3.1-8B-Q4_K_M.gguf".to_string(),
        downloaded: 1_230_465_024,
        total: 4_921_860_096,
        speed_mbps: 32.8,
        chunks: Vec::new(),
        verifying: false,
        num_chunks: 8,
        chunk_completed: vec![true, true, true, false, false, false, false, false],
    };
    let progress = Some(dl);
    let data = ActivityHudData {
        download_progress: &progress,
        queue_size: 0,
        queue_bytes: 0,
        queue_items: &[],
        verification_progress: &[],
        verification_queue_size: 0,
        verification_queue_bytes: 0,
        verified_ok: 0,
        verified_fail: 0,
    };
    assert_eq!(activity_hud_height(&data), 4);

    for (label, height) in [("full", 33u16), ("clamped", 32u16)] {
        let hud_height = activity_hud_height(&data).min(height.saturating_sub(29));
        assert_eq!(hud_height, height.saturating_sub(29));
        let mut fixture = UiFixture::with_selection();
        let mut terminal = Terminal::new(TestBackend::new(100, height)).unwrap();
        draw_ui_with_overlay(
            &mut terminal,
            &mut fixture,
            FocusedPane::Models,
            None,
            hud_height,
            "Downloading Llama-3.1-8B-Q4_K_M.gguf",
            "",
            |frame| {
                let area = Rect::new(0, height - 4 - hud_height, 100, hud_height);
                render_activity_hud(frame, area, &data);
            },
        );
        snap_ui(&format!("hud_threshold_{label}"), &terminal);
    }
}

#[test]
fn hud_strip_rect_threshold_agreement() {
    // W4.10: the HUD strip geometry used to be computed twice — App::draw's
    // manual math (base_layout_rows 29; y = height - 4 - hud_height) and
    // render_ui's Constraint list implied the same rect. render_ui now owns
    // the layout and returns the strip rect; this test pins that rect
    // against the historical formula at the visibility threshold (natural
    // height 4 fixture -> boundary at 29 + 4 = 33 rows), proving agreement
    // by construction.
    let dl = DownloadProgress {
        model_id: "meta-llama/Llama-3.1-8B".to_string(),
        filename: "Llama-3.1-8B-Q4_K_M.gguf".to_string(),
        downloaded: 1_230_465_024,
        total: 4_921_860_096,
        speed_mbps: 32.8,
        chunks: Vec::new(),
        verifying: false,
        num_chunks: 8,
        chunk_completed: vec![true, true, true, false, false, false, false, false],
    };
    let progress = Some(dl);
    let data = ActivityHudData {
        download_progress: &progress,
        queue_size: 0,
        queue_bytes: 0,
        queue_items: &[],
        verification_progress: &[],
        verification_queue_size: 0,
        verification_queue_bytes: 0,
        verified_ok: 0,
        verified_fail: 0,
    };
    assert_eq!(activity_hud_height(&data), 4);

    for (label, height, expected_rect) in [
        ("below threshold", 32u16, Rect::new(0, 25, 100, 3)),
        ("at threshold", 33, Rect::new(0, 25, 100, 4)),
        ("above threshold", 34, Rect::new(0, 26, 100, 4)),
    ] {
        // Pinned formula (the historical App::draw math, kept here as the
        // single copy): clamp the natural height against the reserved base
        // rows, then place the strip manually above the 4-row status bar.
        let hud_height = activity_hud_height(&data).min(height.saturating_sub(29));
        let pinned = Rect::new(0, height - 4 - hud_height, 100, hud_height);
        assert_eq!(
            pinned, expected_rect,
            "pinned formula at {label} (h={height})"
        );

        // render_ui owns the layout since W4.10: it takes the DESIRED
        // (uncapped) height, clamps it against the base layout itself and
        // returns the reserved strip rect — which must match the pin.
        let mut fixture = UiFixture::with_selection();
        let mut terminal = Terminal::new(TestBackend::new(100, height)).unwrap();
        let returned = draw_ui_with_overlay(
            &mut terminal,
            &mut fixture,
            FocusedPane::Models,
            None,
            activity_hud_height(&data), // desired, uncapped — render_ui clamps
            "Downloading Llama-3.1-8B-Q4_K_M.gguf",
            "",
            |_| {},
        );
        assert_eq!(returned, pinned, "render_ui rect at {label} (h={height})");

        // And an independent re-split of the historical Constraint list
        // still lands on the same rect (ratatui geometry re-pin).
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(12),
                Constraint::Length(returned.height),
                Constraint::Length(4),
            ])
            .split(Rect::new(0, 0, 100, height));
        assert_eq!(
            chunks[3], returned,
            "constraint split at {label} (h={height})"
        );
    }
}
