use super::*;
use crate::models::{SortDirection, SortField};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn version_badge_is_flush_right_in_filter_toolbar() {
    let backend = TestBackend::new(80, 3);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut filter_areas = Vec::new();
    terminal
        .draw(|f| {
            render_filter_toolbar(
                f,
                f.area(),
                SortField::Downloads,
                SortDirection::Descending,
                0,
                0,
                0,
                &mut filter_areas,
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
    let mut filter_areas = Vec::new();
    terminal
        .draw(|f| {
            render_filter_toolbar(
                f,
                f.area(),
                SortField::Downloads,
                SortDirection::Descending,
                10_000,
                100,
                0,
                &mut filter_areas,
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

// ----------------- options field table (W4.7) -----------------

#[test]
fn options_fields_table_pins_dialog_shape() {
    use crate::models::AppOptions;
    use crate::utils::format_size;

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
    // Token masking: a bullet per char, capped at 20.
    assert_eq!(
        (OPTIONS_FIELDS[1].value)(&options, &empty, &empty),
        "•".repeat(20)
    );
    // The live edit buffer wins while editing the directory field.
    options.editing_directory = true;
    let typed = tui_input::Input::new("/tmp/typed".to_string());
    assert_eq!(
        (OPTIONS_FIELDS[0].value)(&options, &typed, &empty),
        "/tmp/typed"
    );
    // Toggle rendering (both directions) and byte-size formatting.
    options.editing_directory = false;
    options.download_rate_limit_enabled = true;
    assert_eq!(
        (OPTIONS_FIELDS[10].value)(&options, &empty, &empty),
        "Enabled"
    );
    options.verification_on_completion = false;
    assert_eq!(
        (OPTIONS_FIELDS[12].value)(&options, &empty, &empty),
        "Disabled"
    );
    assert_eq!(
        (OPTIONS_FIELDS[4].value)(&options, &empty, &empty),
        format_size(options.min_chunk_size)
    );
}
