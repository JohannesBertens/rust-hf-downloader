use super::*;
use crate::models::{
    AppOptions, DownloadMetadata, DownloadStatus, FileTreeNode, ModelMetadata, QuantizationGroup,
    QuantizationInfo, SortDirection, SortField,
};
use ratatui::{backend::TestBackend, Terminal};
use std::collections::HashMap;

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

/// Draw `render_ui` on the terminal with fixed defaults for every
/// RenderParams group not worth varying between tests (all filters at
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
    let hovered_panel: Option<FocusedPane> = None;

    terminal
        .draw(|frame| {
            render_ui(
                frame,
                RenderParams {
                    display_mode: ModelDisplayMode::Gguf,
                    focus: FocusCtx {
                        input_mode: InputMode::Normal,
                        focused_pane,
                        hovered_panel,
                    },
                    list: ListCtx {
                        input,
                        models,
                        list_state,
                        loading: false,
                    },
                    gguf: GgufPanelContext {
                        quantizations,
                        quant_list_state,
                        quant_file_list_state,
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
    snap_ui("snapshot_render_ui_empty_state", &terminal);
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
    snap_ui("snapshot_render_ui_model_list_selection", &terminal);
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
    snap_ui("snapshot_render_ui_quantization_panels", &terminal);
}

#[test]
fn snapshot_search_popup() {
    let input = Input::new("llama".to_string());
    let mut terminal = test_terminal();
    terminal
        .draw(|frame| render_search_popup(frame, &input))
        .expect("failed to draw search popup");
    snap_ui("snapshot_search_popup", &terminal);
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
    let hovered_panel: Option<FocusedPane> = None;

    let mut terminal = test_terminal();
    terminal
        .draw(|frame| {
            render_ui(
                frame,
                RenderParams {
                    display_mode: ModelDisplayMode::Gguf,
                    focus: FocusCtx {
                        input_mode: InputMode::Normal,
                        focused_pane: FocusedPane::Models,
                        hovered_panel,
                    },
                    list: ListCtx {
                        input: &input,
                        models: &models,
                        list_state: &mut list_state,
                        loading: false,
                    },
                    gguf: GgufPanelContext {
                        quantizations: &quantizations,
                        quant_list_state: &mut quant_list_state,
                        quant_file_list_state: &mut quant_file_list_state,
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
                        status: "Press / to search",
                        selection_info: "Selection: 2 of 3",
                    },
                    hud_height: 0,
                },
            );
            render_search_popup(frame, &input);
        })
        .expect("failed to draw UI + popup");
    snap_ui("snapshot_search_popup_over_populated_ui", &terminal);
}

#[test]
fn snapshot_download_path_popup() {
    let input = Input::new("/models/output".to_string());
    let mut terminal = test_terminal();
    terminal
        .draw(|frame| render_download_path_popup(frame, &input))
        .expect("failed to draw download path popup");
    snap_ui("snapshot_download_path_popup", &terminal);
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
    snap_ui("snapshot_auth_error_popup_no_token", &terminal);
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
    snap_ui("snapshot_auth_error_popup_with_token", &terminal);
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
            revision: None,
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
            revision: None,
        },
    ];
    let mut terminal = test_terminal();
    terminal
        .draw(|frame| render_resume_popup(frame, &incomplete))
        .expect("failed to draw resume popup");
    snap_ui("snapshot_resume_popup", &terminal);
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
            render_options_popup(
                frame,
                &options,
                &OptionsDialogState::default(),
                &directory_input,
                &token_input,
            );
        })
        .expect("failed to draw options popup");
    snap_ui("snapshot_options_popup", &terminal);
}

#[test]
fn snapshot_filter_toolbar_unfocused() {
    // focused_field 5 is out of range (valid: 0=sort, 1=downloads,
    // 2=likes), so no field is highlighted. Default values also
    // trigger the "[No Filters]" preset indicator.
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
                FilterCtx {
                    sort_field: SortField::Downloads,
                    sort_direction: SortDirection::Descending,
                    min_downloads: 0,
                    min_likes: 0,
                    focused_field: 5,
                },
            );
        })
        .expect("failed to draw filter toolbar");
    snap_ui("snapshot_filter_toolbar_unfocused", &terminal);
}

#[test]
fn snapshot_filter_toolbar_sort_focused() {
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
                FilterCtx {
                    sort_field: SortField::Likes,
                    sort_direction: SortDirection::Ascending,
                    min_downloads: 2_500,
                    min_likes: 300,
                    focused_field: 0,
                },
            );
        })
        .expect("failed to draw filter toolbar");
    snap_ui("snapshot_filter_toolbar_sort_focused", &terminal);
}

#[test]
fn snapshot_filter_toolbar_downloads_focused() {
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
                FilterCtx {
                    sort_field: SortField::Downloads,
                    sort_direction: SortDirection::Descending,
                    min_downloads: 10_000,
                    min_likes: 100,
                    focused_field: 1,
                },
            );
        })
        .expect("failed to draw filter toolbar");
    snap_ui("snapshot_filter_toolbar_downloads_focused", &terminal);
}
