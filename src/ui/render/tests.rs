use super::*;
use crate::models::{SortDirection, SortField};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn version_badge_is_flush_right_in_filter_toolbar() {
    let backend = TestBackend::new(80, 3);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_filter_toolbar(
                f,
                f.area(),
                FilterCtx {
                    sort_field: SortField::Downloads,
                    sort_direction: SortDirection::Descending,
                    min_downloads: 0,
                    min_likes: 0,
                    focused_field: 0,
                },
            );
        })
        .unwrap();

    let buf = terminal.backend().buffer();
    // Inner row of the bordered toolbar: columns 1..79, middle line y=1.
    let mut row = String::new();
    for x in 1..79 {
        row.push_str(buf[(x, 1)].symbol());
    }
    let expected = format!("v{}", env!("CARGO_PKG_VERSION"));
    let trimmed = row.trim_end();
    assert!(
        trimmed.ends_with(&expected),
        "version not at the right edge; row = {row:?}"
    );
    // The three filter fields must still be present (left content intact).
    assert!(trimmed.contains("Sort:"), "row = {row:?}");
    assert!(trimmed.contains("Min Downloads:"), "row = {row:?}");
}

#[test]
fn version_badge_skipped_when_toolbar_too_narrow() {
    let backend = TestBackend::new(30, 3);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_filter_toolbar(
                f,
                f.area(),
                FilterCtx {
                    sort_field: SortField::Downloads,
                    sort_direction: SortDirection::Descending,
                    min_downloads: 10_000,
                    min_likes: 100,
                    focused_field: 0,
                },
            );
        })
        .unwrap();

    let buf = terminal.backend().buffer();
    let mut row = String::new();
    for x in 1..29 {
        row.push_str(buf[(x, 1)].symbol());
    }
    let expected = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(
        !row.contains(&expected),
        "version should be skipped on narrow bars; row = {row:?}"
    );
}

// ----------------- mouse hit-rect registration order (W5.2) -----------------

#[test]
fn mouse_areas_register_in_lookup_order() {
    // The first-match hit-tests in `App` (click, scroll, hover) depend on
    // registration order: filter fields 0,1,2 in display order, then the
    // Results list before the bottom panels, bottom panels left-to-right.
    // This pins the order render_ui returns in `MouseAreas`.
    use super::snapshot_tests::{quantization_fixtures, three_model_fixtures};
    use crate::models::{DownloadMetadata, FileTreeNode, ModelMetadata};
    use ratatui::widgets::ListState;
    use std::collections::HashMap;

    let draw = |display_mode, metadata: Option<ModelMetadata>| {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let input = Input::default();
        let models = three_model_fixtures();
        let mut list_state = ListState::default();
        let quantizations = quantization_fixtures();
        let mut quant_list_state = ListState::default();
        let mut quant_file_list_state = ListState::default();
        let file_tree: Option<FileTreeNode> = None;
        let mut file_tree_state = ListState::default();
        let complete_downloads: HashMap<String, DownloadMetadata> = HashMap::new();
        let error: Option<String> = None;

        let mut out = None;
        terminal
            .draw(|frame| {
                out = Some(render_ui(
                    frame,
                    RenderParams {
                        display_mode,
                        focus: FocusCtx {
                            input_mode: InputMode::Normal,
                            focused_pane: FocusedPane::Models,
                            hovered_panel: None,
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
                            model_metadata: &metadata,
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
                            status: "",
                            selection_info: "",
                        },
                        hud_height: 0,
                    },
                ));
            })
            .unwrap();
        out.unwrap().mouse
    };

    // GGUF mode: Models, QuantizationGroups, QuantizationFiles.
    let mouse = draw(ModelDisplayMode::Gguf, None);
    let panes: Vec<FocusedPane> = mouse.panels.iter().map(|(pane, _)| *pane).collect();
    assert_eq!(
        panes,
        vec![
            FocusedPane::Models,
            FocusedPane::QuantizationGroups,
            FocusedPane::QuantizationFiles,
        ]
    );
    let fields: Vec<usize> = mouse.filters.iter().map(|(idx, _)| *idx).collect();
    assert_eq!(fields, vec![0, 1, 2]);

    // Standard mode: Models, ModelMetadata, FileTree.
    let mouse = draw(
        ModelDisplayMode::Standard,
        Some(ModelMetadata {
            model_id: "meta-llama/Llama-3.1-8B".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: Vec::new(),
            tags: Vec::new(),
            sha: None,
        }),
    );
    let panes: Vec<FocusedPane> = mouse.panels.iter().map(|(pane, _)| *pane).collect();
    assert_eq!(
        panes,
        vec![
            FocusedPane::Models,
            FocusedPane::ModelMetadata,
            FocusedPane::FileTree,
        ]
    );

    // Rect sanity: every hit-rect lives inside the terminal and the
    // bottom-panel pairs are side-by-side (left.x < right.x).
    for (_, area) in &mouse.panels {
        assert!(
            area.right() <= 100 && area.bottom() <= 30,
            "area {area:?} outside terminal"
        );
    }
    let left = mouse.panels[1].1;
    let right = mouse.panels[2].1;
    assert!(
        left.x < right.x,
        "bottom panels must register left-to-right"
    );
}

// ----------------- options field table (W4.7) -----------------

#[test]
fn options_fields_table_pins_dialog_shape() {
    use crate::fmt::size_full;
    use crate::models::AppOptions;

    // The table is the options dialog's single source (W4.7): 16 rows in
    // the historical display order, ids at the historical indices, so the
    // cursor bound (len - 1 = 15) and modify_option arms keep today's
    // semantics.
    assert_eq!(OPTIONS_FIELDS.len(), 16);
    let ids: Vec<_> = OPTIONS_FIELDS.iter().map(|f| f.id).collect();
    assert_eq!(
        ids,
        vec![
            OptionsFieldId::DefaultDirectory,
            OptionsFieldId::HfToken,
            OptionsFieldId::ConcurrentThreads,
            OptionsFieldId::NumChunks,
            OptionsFieldId::MinChunkSize,
            OptionsFieldId::MaxChunkSize,
            OptionsFieldId::MaxRetries,
            OptionsFieldId::DownloadTimeoutSecs,
            OptionsFieldId::RetryDelaySecs,
            OptionsFieldId::ProgressUpdateIntervalMs,
            OptionsFieldId::RateLimitEnabled,
            OptionsFieldId::RateLimitMbps,
            OptionsFieldId::VerificationEnabled,
            OptionsFieldId::ConcurrentVerifications,
            OptionsFieldId::VerificationBufferSize,
            OptionsFieldId::VerificationUpdateInterval,
        ]
    );

    // Labels render verbatim (category-boundary anchors — the renderer's
    // category_offsets table starts groups at fields 0, 2, 10, 12).
    assert_eq!(OPTIONS_FIELDS[0].label, "Default Directory:");
    assert_eq!(OPTIONS_FIELDS[2].label, "Concurrent Threads:");
    assert_eq!(OPTIONS_FIELDS[10].label, "Rate Limit:");
    assert_eq!(OPTIONS_FIELDS[12].label, "Enable Verification:");
    assert_eq!(OPTIONS_FIELDS[15].label, "Verification Update Interval:");

    // Interaction classes: 2 text, 2 toggles, 12 numbers.
    let count = |k: OptionsFieldKind| OPTIONS_FIELDS.iter().filter(|f| f.kind == k).count();
    assert_eq!(count(OptionsFieldKind::Text), 2);
    assert_eq!(count(OptionsFieldKind::Toggle), 2);
    assert_eq!(count(OptionsFieldKind::Number), 12);

    // Value accessors reproduce the historical strings.
    let mut options = AppOptions {
        hf_token: Some("secret-token-1234567890".to_string()),
        ..AppOptions::default()
    };
    let empty = tui_input::Input::default();
    let mut dialog = OptionsDialogState::default();
    // Token masking: a bullet per char, capped at 20.
    assert_eq!(
        (OPTIONS_FIELDS[1].value)(&options, &dialog, &empty, &empty),
        "•".repeat(20)
    );
    // The live edit buffer wins while editing the directory field.
    dialog.editing_directory = true;
    let typed = tui_input::Input::new("/tmp/typed".to_string());
    assert_eq!(
        (OPTIONS_FIELDS[0].value)(&options, &dialog, &typed, &empty),
        "/tmp/typed"
    );
    // Toggle rendering (both directions) and byte-size formatting.
    dialog.editing_directory = false;
    options.download_rate_limit_enabled = true;
    assert_eq!(
        (OPTIONS_FIELDS[10].value)(&options, &dialog, &empty, &empty),
        "Enabled"
    );
    options.verification_on_completion = false;
    assert_eq!(
        (OPTIONS_FIELDS[12].value)(&options, &dialog, &empty, &empty),
        "Disabled"
    );
    assert_eq!(
        (OPTIONS_FIELDS[4].value)(&options, &dialog, &empty, &empty),
        size_full(options.min_chunk_size)
    );
}
