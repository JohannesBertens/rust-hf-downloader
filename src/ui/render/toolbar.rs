//! Filter & sort toolbar: the clickable hit areas and the version badge
//! (plan W3.4b split out of `render.rs`; body byte-identical).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Toolbar input group (W5.2; formerly flat `RenderParams` fields): the
/// live filter/sort values plus which field the keyboard focus ring
/// highlights. Owned by this module because only the toolbar reads it.
#[derive(Debug, Clone, Copy)]
pub struct FilterCtx {
    pub sort_field: crate::models::SortField,
    pub sort_direction: crate::models::SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
    pub focused_field: usize,
}

/// Render filter and sort toolbar; returns the three clickable field
/// rects in display order — sort (0), min downloads (1), min likes (2) —
/// for the caller's hit-rect registry (W5.2: returned instead of pushed
/// into a `&mut` out-param, so the render pass stays pure).
pub fn render_filter_toolbar(
    frame: &mut Frame,
    area: Rect,
    filters: FilterCtx,
) -> Vec<(usize, Rect)> {
    use crate::models::{SortDirection, SortField};

    let FilterCtx {
        sort_field,
        sort_direction,
        min_downloads,
        min_likes,
        focused_field,
    } = filters;

    // Hit-rects built in display order below (registration order is
    // behavior: first match wins hit-tests — see `MouseAreas`).
    let mut filter_areas = Vec::new();

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Filters  [Click to cycle | 1-4: Presets | r: Reset | Ctrl+S: Save]")
        .style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Sort arrow
    let sort_arrow = match sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };

    // Sort name
    let sort_name = match sort_field {
        SortField::Downloads => "Downloads",
        SortField::Likes => "Likes",
        SortField::Modified => "Modified",
        SortField::Name => "Name",
    };

    // Build display line with highlighting for focused field
    let sort_style = if focused_field == 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    let downloads_style = if focused_field == 1 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };

    let likes_style = if focused_field == 2 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };

    // Detect which preset is active (if any)
    let preset_name = if sort_field == SortField::Modified
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 0
    {
        Some("Recent")
    } else if sort_field == SortField::Likes
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 1_000
    {
        Some("Highly Rated")
    } else if sort_field == SortField::Downloads
        && sort_direction == SortDirection::Descending
        && min_downloads == 10_000
        && min_likes == 100
    {
        Some("Popular")
    } else if sort_field == SortField::Downloads
        && sort_direction == SortDirection::Descending
        && min_downloads == 0
        && min_likes == 0
    {
        Some("No Filters")
    } else {
        None
    };

    // Calculate text segments for click detection
    // Format: "Sort: {value}  |  Min Downloads: {value}  |  Min Likes: {value}"
    let sort_label = "Sort: ";
    let sort_value = format!("{} {}", sort_name, sort_arrow);
    let separator1 = "  |  ";
    let downloads_label = "Min Downloads: ";
    let downloads_value = crate::fmt::number(min_downloads);
    let separator2 = "  |  ";
    let likes_label = "Min Likes: ";
    let likes_value = crate::fmt::number(min_likes);

    // Calculate x positions for each clickable area
    let mut x = inner.x;

    // Sort area: includes label and value
    let sort_start = x;
    x += sort_label.len() as u16 + sort_value.len() as u16;
    let sort_area = Rect {
        x: sort_start,
        y: inner.y,
        width: x - sort_start,
        height: 1,
    };
    filter_areas.push((0, sort_area));

    x += separator1.len() as u16;

    // Downloads area: includes label and value
    let downloads_start = x;
    x += downloads_label.len() as u16 + downloads_value.len() as u16;
    let downloads_area = Rect {
        x: downloads_start,
        y: inner.y,
        width: x - downloads_start,
        height: 1,
    };
    filter_areas.push((1, downloads_area));

    x += separator2.len() as u16;

    // Likes area: includes label and value
    let likes_start = x;
    x += likes_label.len() as u16 + likes_value.len() as u16;
    let likes_area = Rect {
        x: likes_start,
        y: inner.y,
        width: x - likes_start,
        height: 1,
    };
    filter_areas.push((2, likes_area));

    let mut line_parts = vec![
        Span::styled(sort_label, Style::default().fg(Color::DarkGray)),
        Span::styled(sort_value, sort_style),
        Span::raw(separator1),
        Span::styled(downloads_label, Style::default().fg(Color::DarkGray)),
        Span::styled(downloads_value, downloads_style),
        Span::raw(separator2),
        Span::styled(likes_label, Style::default().fg(Color::DarkGray)),
        Span::styled(likes_value, likes_style),
    ];

    // Add preset indicator if a preset is active
    let mut preset_added = false;
    if let Some(preset) = preset_name {
        line_parts.push(Span::raw("  |  "));
        line_parts.push(Span::styled(
            format!("[{}]", preset),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
        preset_added = true;
    }

    // Version badge pinned flush-right in the top bar (kept out of the
    // clickable filter_areas; phase-2 update notifications reuse this slot —
    // see plans/self-update.md). The badge outranks the decorative preset
    // indicator: on narrow bars the preset (derivable from the filter values
    // themselves) is dropped so the version always fits; if even the bare
    // filter row leaves no room, the badge is skipped.
    let version_text = format!("v{}", env!("CARGO_PKG_VERSION"));
    let version_width = version_text.len() as u16;
    let left_width = |parts: &[Span]| parts.iter().map(|s| s.width() as u16).sum::<u16>();
    if preset_added && inner.width <= left_width(&line_parts) + version_width {
        // Drop the preset spans (separator + label — the last two).
        line_parts.truncate(line_parts.len() - 2);
    }
    let left = left_width(&line_parts);
    if inner.width > left + version_width {
        let pad = inner.width - left - version_width;
        line_parts.push(Span::raw(" ".repeat(pad as usize)));
        line_parts.push(Span::styled(
            version_text,
            Style::default().fg(Color::DarkGray),
        ));
    }

    let line = Line::from(line_parts);

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, inner);

    filter_areas
}
