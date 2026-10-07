// =====================================================================
// Shared render-test fixtures + the snapshot-name manifest (M5/U5).
//
// The four render test modules (`snapshot_tests`, `hud_tests`,
// `style_size_tests`, `tests`) used to carry three hand-copied variants
// of the same draw harness (the `RenderParams` literal with every
// group at its default: no popup, no filters, no error, empty
// complete-downloads, GGUF or Standard mode). They now share the
// [`UiFixture`] bundle + [`draw_ui_with_overlay`] here; cfg(test)-only,
// never production code.
//
// The manifest below is the exact set of snapshot names these suites
// produce (insta derives a name from the module where
// `assert_snapshot!` expands — `snap_ui` in `render/mod.rs` names
// `rust_hf_downloader__ui__render__<name>`, `snap_style` in
// `style_size_tests.rs` names
// `rust_hf_downloader__ui__render__style_size_tests__<name>`; both
// helpers REFUSE unregistered names). The manifest test at the bottom
// checks the manifest against the committed `.snap` files in BOTH
// directions, so a move that drops or orphans a golden fails loudly.
// =====================================================================

use super::{
    FilterCtx, FocusCtx, GgufPanelContext, ListCtx, ModelDisplayMode, PopupMode, RenderOutput,
    RenderParams, StandardPanelContext, StatusCtx,
};
use crate::models::{
    DownloadMetadata, FocusedPane, ModelInfo, ModelMetadata, QuantizationGroup, QuantizationInfo,
    SortDirection, SortField,
};
use ratatui::{backend::TestBackend, Frame, Terminal};
use std::collections::HashMap;
use tui_input::Input;

pub(super) const TERMINAL_WIDTH: u16 = 100;
pub(super) const TERMINAL_HEIGHT: u16 = 30;

/// Fixed 100x30 TestBackend terminal — every snapshot uses it so the
/// output is fully deterministic.
pub(super) fn test_terminal() -> Terminal<TestBackend> {
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
pub(super) fn three_model_fixtures() -> Vec<ModelInfo> {
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
pub(super) fn quantization_fixtures() -> Vec<QuantizationGroup> {
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

/// Per-draw mutable state for the render harness: every list input and
/// cursor `render_ui` touches, in one bundle. The constructors below
/// reproduce the historical fixtures:
#[derive(Debug)]
pub(super) struct UiFixture {
    pub input: Input,
    pub models: Vec<ModelInfo>,
    pub list_state: ratatui::widgets::ListState,
    pub quantizations: Vec<QuantizationGroup>,
    pub quant_list_state: ratatui::widgets::ListState,
    pub quant_file_list_state: ratatui::widgets::ListState,
    /// Standard-mode metadata panel content (`None` in GGUF-mode tests).
    pub model_metadata: Option<ModelMetadata>,
    /// Standard-mode file tree (`None` in GGUF-mode tests).
    pub file_tree: Option<crate::models::FileTreeNode>,
    pub file_tree_state: ratatui::widgets::ListState,
}

impl UiFixture {
    /// Empty lists (the `snapshot_render_ui_empty_state` fixture).
    pub(super) fn empty() -> Self {
        Self {
            input: Input::default(),
            models: Vec::new(),
            list_state: ratatui::widgets::ListState::default(),
            quantizations: Vec::new(),
            quant_list_state: ratatui::widgets::ListState::default(),
            quant_file_list_state: ratatui::widgets::ListState::default(),
            model_metadata: None,
            file_tree: None,
            file_tree_state: ratatui::widgets::ListState::default(),
        }
    }

    /// Models list with row 1 selected (mirrors
    /// `snapshot_render_ui_model_list_selection`).
    pub(super) fn with_selection() -> Self {
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(1));
        Self {
            input: Input::new("llama".to_string()),
            models: three_model_fixtures(),
            list_state,
            quantizations: Vec::new(),
            quant_list_state: ratatui::widgets::ListState::default(),
            quant_file_list_state: ratatui::widgets::ListState::default(),
            model_metadata: None,
            file_tree: None,
            file_tree_state: ratatui::widgets::ListState::default(),
        }
    }

    /// Quantization panels with group 0 / file 0 selected (mirrors
    /// `snapshot_render_ui_quantization_panels`).
    pub(super) fn quant_view() -> Self {
        let mut quant_list_state = ratatui::widgets::ListState::default();
        quant_list_state.select(Some(0));
        let mut quant_file_list_state = ratatui::widgets::ListState::default();
        quant_file_list_state.select(Some(0));
        Self {
            input: Input::default(),
            models: three_model_fixtures(),
            list_state: ratatui::widgets::ListState::default(),
            quantizations: quantization_fixtures(),
            quant_list_state,
            quant_file_list_state,
            model_metadata: None,
            file_tree: None,
            file_tree_state: ratatui::widgets::ListState::default(),
        }
    }
}

/// Draw `render_ui` with the shared defaults every harness copy used
/// (no popup, not loading, no filters, no error, no complete-downloads,
/// `focused_field` 5 = none highlighted) and the per-call parameters
/// (display mode, focus/hover, HUD height, status lines), then run
/// `overlay` — which receives the fixture read-only — in the SAME draw
/// closure, mirroring the app loop where popups and the HUD render on
/// top of the live UI. Returns render_ui's full [`RenderOutput`] (the
/// reserved HUD strip rect + the mouse hit-rects).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_ui_with_overlay(
    terminal: &mut Terminal<TestBackend>,
    fixture: &mut UiFixture,
    display_mode: ModelDisplayMode,
    focused: FocusedPane,
    hovered: Option<FocusedPane>,
    hud_height: u16,
    status: &str,
    selection_info: &str,
    overlay: impl FnOnce(&mut Frame, &UiFixture),
) -> RenderOutput {
    let error: Option<String> = None;
    let complete_downloads: HashMap<String, DownloadMetadata> = HashMap::new();
    let mut out = RenderOutput {
        hud_strip: ratatui::layout::Rect::default(),
        mouse: super::MouseAreas::default(),
    };
    terminal
        .draw(|frame| {
            out = super::render_ui(
                frame,
                RenderParams {
                    display_mode,
                    focus: FocusCtx {
                        popup_mode: PopupMode::None,
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
                        model_metadata: &fixture.model_metadata,
                        file_tree: &fixture.file_tree,
                        file_tree_state: &mut fixture.file_tree_state,
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
            );
            overlay(frame, fixture);
        })
        .expect("failed to draw UI");
    out
}

// ---------------------------------------------------------------------
// Snapshot-name manifest (M5/U5)
// ---------------------------------------------------------------------

/// The exact set of snapshot names the render suites produce — the
/// committed goldens in `src/ui/render/snapshots/`, file stem by file
/// stem. `snap_ui` (render/mod.rs) and `snap_style`
/// (style_size_tests.rs) refuse names not listed here, so a new
/// snapshot cannot land without registering it; the manifest test below
/// fails when a registered name has no committed golden, or when a
/// committed golden belongs to no registered name (an orphan left by a
/// deleted test).
pub(super) const SNAPSHOT_MANIFEST: &[&str] = &[
    // snapshot_tests.rs — render_ui + the five popups + toolbar (via
    // snap_ui in render/mod.rs)
    "rust_hf_downloader__ui__render__snapshot_render_ui_empty_state",
    "rust_hf_downloader__ui__render__snapshot_render_ui_model_list_selection",
    "rust_hf_downloader__ui__render__snapshot_render_ui_quantization_panels",
    "rust_hf_downloader__ui__render__snapshot_search_popup",
    "rust_hf_downloader__ui__render__snapshot_search_popup_over_populated_ui",
    "rust_hf_downloader__ui__render__snapshot_download_path_popup",
    "rust_hf_downloader__ui__render__snapshot_auth_error_popup_no_token",
    "rust_hf_downloader__ui__render__snapshot_auth_error_popup_with_token",
    "rust_hf_downloader__ui__render__snapshot_resume_popup",
    "rust_hf_downloader__ui__render__snapshot_options_popup",
    "rust_hf_downloader__ui__render__snapshot_filter_toolbar_unfocused",
    "rust_hf_downloader__ui__render__snapshot_filter_toolbar_sort_focused",
    "rust_hf_downloader__ui__render__snapshot_filter_toolbar_downloads_focused",
    // hud_tests.rs
    "rust_hf_downloader__ui__render__snapshot_activity_hud_renders_matrix",
    // style_size_tests.rs — size matrix + HUD threshold (snap_ui names)
    "rust_hf_downloader__ui__render__size_matrix_models_focus_80x24",
    "rust_hf_downloader__ui__render__size_matrix_models_focus_120x40",
    "rust_hf_downloader__ui__render__size_matrix_models_focus_60x20",
    "rust_hf_downloader__ui__render__size_matrix_models_focus_61x23",
    "rust_hf_downloader__ui__render__size_matrix_quant_view_80x24",
    "rust_hf_downloader__ui__render__size_matrix_quant_view_120x40",
    "rust_hf_downloader__ui__render__size_matrix_quant_view_60x20",
    "rust_hf_downloader__ui__render__size_matrix_quant_view_61x23",
    "rust_hf_downloader__ui__render__size_matrix_standard_80x24",
    "rust_hf_downloader__ui__render__size_matrix_standard_120x40",
    "rust_hf_downloader__ui__render__size_matrix_standard_60x20",
    "rust_hf_downloader__ui__render__size_matrix_standard_61x23",
    "rust_hf_downloader__ui__render__hud_threshold_full",
    "rust_hf_downloader__ui__render__hud_threshold_clamped",
    // style_size_tests.rs — style signatures (snap_style names embed
    // the style_size_tests module)
    "rust_hf_downloader__ui__render__style_size_tests__style_focus_border_yellow_on_models_pane",
    "rust_hf_downloader__ui__render__style_size_tests__style_unfocused_models_pane_border_is_plain",
    "rust_hf_downloader__ui__render__style_size_tests__style_hovered_pane_border_is_cyan",
    "rust_hf_downloader__ui__render__style_size_tests__style_focus_border_yellow_on_file_tree_pane",
    "rust_hf_downloader__ui__render__style_size_tests__style_focus_border_yellow_on_model_metadata_pane",
    "rust_hf_downloader__ui__render__style_size_tests__style_standard_unfocused_bottom_panes_border_is_plain",
    "rust_hf_downloader__ui__render__style_size_tests__style_standard_hovered_file_tree_pane_border_is_cyan",
    "rust_hf_downloader__ui__render__style_size_tests__style_selected_list_item_highlight",
    "rust_hf_downloader__ui__render__style_size_tests__style_options_popup_clear_and_border",
    "rust_hf_downloader__ui__render__style_size_tests__style_resume_popup_clear_and_background",
];

/// Gate used by `snap_ui`/`snap_style`: a snapshot name the suites
/// produce must be registered in [`SNAPSHOT_MANIFEST`], or the test run
/// fails HERE with the fix spelled out (register the name — and commit
/// the golden — in the same PR).
pub(super) fn require_snapshot_name(full_name: &str) {
    assert!(
        SNAPSHOT_MANIFEST.contains(&full_name),
        "snapshot name `{full_name}` is not registered in \
         ui/render/test_utils.rs::SNAPSHOT_MANIFEST — add it there (and \
         commit its .snap file) in the same PR, or the manifest test \
         will fail"
    );
}

/// Manifest ↔ goldens agreement (M5/U5): every registered snapshot name
/// has a committed `.snap` file in `src/ui/render/snapshots/`, and every
/// committed `.snap` file belongs to the manifest. A move that drops or
/// orphans a golden now fails loudly, even without cargo-insta's
/// `--unreferenced=reject`.
#[test]
fn snapshot_name_manifest_matches_committed_goldens() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/render/snapshots");
    let mut committed: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("read_dir entry: {e}"))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".snap"))
        .map(|name| name.trim_end_matches(".snap").to_string())
        .collect();
    committed.sort();

    let mut manifest = SNAPSHOT_MANIFEST.to_vec();
    manifest.sort_unstable();

    let missing: Vec<&str> = manifest
        .iter()
        .filter(|m| !committed.iter().any(|c| c == *m))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "manifest names without a committed .snap in src/ui/render/snapshots/ \
         (accept the golden or drop the name): {missing:?}"
    );

    let orphans: Vec<&String> = committed
        .iter()
        .filter(|c| !manifest.contains(&c.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "committed .snap files in src/ui/render/snapshots/ that no suite \
         produces (delete the orphan, or register the name + restore its \
         test): {orphans:?}"
    );

    assert_eq!(
        committed.len(),
        manifest.len(),
        "manifest ({}) and committed goldens ({}) disagree",
        manifest.len(),
        committed.len()
    );
}
