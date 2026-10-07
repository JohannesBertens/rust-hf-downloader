use super::test_utils::{draw_ui_with_overlay, test_terminal, UiFixture, TERMINAL_WIDTH};
use super::*;
use crate::models::{AppOptions, DownloadMetadata, DownloadStatus, SortDirection, SortField};
use crate::ui::app::options::OptionsDialogState;

#[test]
fn snapshot_render_ui_empty_state() {
    let mut fixture = UiFixture::empty();
    let mut terminal = test_terminal();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        ModelDisplayMode::Gguf,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "",
        |_, _| {},
    );
    snap_ui("snapshot_render_ui_empty_state", &terminal);
}

#[test]
fn snapshot_render_ui_model_list_selection() {
    let mut fixture = UiFixture::with_selection();
    let mut terminal = test_terminal();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        ModelDisplayMode::Gguf,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
        |_, _| {},
    );
    snap_ui("snapshot_render_ui_model_list_selection", &terminal);
}

#[test]
fn snapshot_render_ui_quantization_panels() {
    let mut fixture = UiFixture::quant_view();
    let mut terminal = test_terminal();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        ModelDisplayMode::Gguf,
        FocusedPane::QuantizationGroups,
        None,
        0,
        "2 quantization groups available",
        "",
        |_, _| {},
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
    let mut fixture = UiFixture::with_selection();
    let mut terminal = test_terminal();
    draw_ui_with_overlay(
        &mut terminal,
        &mut fixture,
        ModelDisplayMode::Gguf,
        FocusedPane::Models,
        None,
        0,
        "Press / to search",
        "Selection: 2 of 3",
        |frame, fixture| render_search_popup(frame, &fixture.input),
    );
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
