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
