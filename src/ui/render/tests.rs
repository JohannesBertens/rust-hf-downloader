use super::models_list::model_has_flare_tag;
use super::*;
use crate::models::{SortDirection, SortField};
use crate::ui::app::options::{
    OptionsDialogState, OptionsFieldId, OptionsFieldKind, OPTIONS_FIELDS,
};
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
    use super::test_utils::{
        draw_ui_with_overlay, quantization_fixtures, three_model_fixtures, UiFixture,
    };
    use crate::models::ModelMetadata;

    let draw = |display_mode, metadata: Option<ModelMetadata>| {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut fixture = UiFixture::empty();
        fixture.models = three_model_fixtures();
        fixture.quantizations = quantization_fixtures();
        fixture.model_metadata = metadata;

        draw_ui_with_overlay(
            &mut terminal,
            &mut fixture,
            display_mode,
            FocusedPane::Models,
            None,
            0,
            "",
            "",
            |_, _| {},
        )
        .mouse
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
    // T5 (test-hardening): byte-size value accessor pinned against the
    // LITERAL string — the former `size_full(options.min_chunk_size)`
    // compared the renderer to its own input function (self-referential:
    // any change to size_full passed vacuously). The default
    // min_chunk_size is 5 × 1 MiB → binary-threshold "5.00 MB".
    assert_eq!(
        (OPTIONS_FIELDS[4].value)(&options, &dialog, &empty, &empty),
        "5.00 MB"
    );
}

// ---------------------------------------------------------------------
// plans/flare-models.md §3.4a: the search-list FLARE badge rule.
// ---------------------------------------------------------------------

#[test]
fn flare_badge_rule_matches_the_three_real_tag_vectors() {
    let tags = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };

    // The two flagship repos that carry the exact `flare` tag
    // (local-inference-lab, lflare).
    assert!(model_has_flare_tag(&tags(&["text-generation", "flare"])));
    // JohannesBertens' repo: `flare-runtime-package`, NO `flare` tag —
    // the prefix arm is what badges it.
    assert!(model_has_flare_tag(&tags(&[
        "flare-runtime-package",
        "text-generation"
    ])));
    // An ordinary repo: no flare-family tag at all.
    assert!(!model_has_flare_tag(&tags(&["text-generation", "llama"])));
    assert!(!model_has_flare_tag(&[]));
    // Near misses must NOT badge: `flare` as a substring is not a prefix
    // match, and `flare-` must lead.
    assert!(!model_has_flare_tag(&tags(&["flared"])));
    assert!(!model_has_flare_tag(&tags(&["x-flare"])));
    // The exact tag still badges when other flare- tags coexist.
    assert!(model_has_flare_tag(&tags(&[
        "flare",
        "flare-runtime-package"
    ])));
}

// ---------------------------------------------------------------------
// plans/flare-models.md §3.5 — revision-picker window follows the cursor
// ---------------------------------------------------------------------

/// Nine revisions (`rev-0`..=`rev-8`), cursor at the LAST row: the list
/// window follows the cursor — the first row scrolls OUT, the tail rows
/// stay on screen (impl-gate P2: the old `.take(8)` froze the window at
/// rows 0..=7 while the cursor kept advancing, so Enter could pick an
/// invisible row).
///
/// DEVIATION PIN (impl-gate round 2): the specified assertion — “row
/// index 8 IS visible/highlighted” — FAILS against the current renderer.
/// `render_flare_revision_popup` positions each row at
/// `popup.y + 3 + ABSOLUTE idx` (src/ui/render/popups.rs), so a scrolled
/// window (`visible_from == 1`) shifts every row DOWN by one: the first
/// list slot renders EMPTY and the last visible row — the CURSOR row,
/// idx 8 — lands on the same line as the `... and 1 more` overflow note,
/// which renders after the rows and clobbers the `>> rev-8` prefix
/// (leaving `... and 1 more] @00000008  1000 B`). The assertions below
/// pin what renders TODAY — the window DID scroll past `rev-0` (the core
/// windowing behavior) but the cursor row is left unhighlighted and
/// partially overwritten. When the renderer positions rows
/// window-relative (`popup.y + 3 + (idx - visible_from)`), flip this
/// test to the specified form: `>> rev-8` visible, `rev-1`..=`rev-8`
/// on screen, `rev-0` absent, the overflow note on its own line.
#[test]
fn flare_popup_window_follows_cursor_at_the_tail() {
    use crate::models::{FlareRevisionChoice, RefKind};

    // Distinctive revision names AND distinct 8-hex commit prefixes —
    // row identity survives partial overwrites (the clobber below eats
    // the row's head, not its `@<commit>` tail).
    let choices: Vec<FlareRevisionChoice> = (0..9)
        .map(|i| FlareRevisionChoice {
            revision: format!("rev-{i}"),
            aliases: Vec::new(),
            kind: RefKind::Branch,
            target_commit: format!("{i:08x}{:032x}", 0),
            package_name: format!("pkg-{i}.safetensors"),
            size: 1_000,
            files: 1,
            arch: None,
        })
        .collect();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| render_flare_revision_popup(frame, "org/model", &choices, 8, None))
        .expect("failed to draw flare revision popup");

    let buf = terminal.backend().buffer();
    let rows: Vec<String> = (0..30)
        .map(|y| {
            (0..100)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();
    let screen = rows.join("\n");

    // The window scrolled: the FIRST choice is NOT on screen...
    assert!(
        !screen.contains("rev-0"),
        "row 0 must scroll out when the cursor is at index 8; screen:\n{screen}"
    );
    // ...while the window body rows 1..=7 render fully.
    for i in 1..8 {
        assert!(
            screen.contains(&format!("rev-{i} [branch]")),
            "row {i} must be visible; screen:\n{screen}"
        );
    }

    // The cursor row (idx 8) IS in the draw list but is positioned on
    // the overflow line: its `>> rev-8` head is clobbered by
    // `... and 1 more`, leaving only the row's `@<commit> <size>` tail
    // merged into that line (the deviation pin — see the doc comment).
    let overflow = rows
        .iter()
        .find(|row| row.contains("and 1 more"))
        .expect("overflow note must render when 9 rows exceed the 8-row window");
    assert!(
        overflow.contains("@00000008"),
        "cursor row 8's remnant renders on the overflow line (clobber pin); \
         line: {overflow:?}"
    );
    // The `>> ` highlight marker is destroyed by that clobber — today
    // NO row shows the cursor. (With the window-relative fix this flips
    // to: the `and 1 more` line carries no row tail, and a line starting
    // `>> rev-8` exists.)
    assert!(
        !screen.contains(">> "),
        "the cursor marker is clobbered by the overflow note; screen:\n{screen}"
    );
}
