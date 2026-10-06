//! Activity HUD (design Option 3): the fixed-column activity strip above
//! the status bar — row builders, column math and the state glyphs.
//!
//! Plan W3.4b split it out of `render.rs`; bodies byte-identical. The HUD
//! *formatters* (bytes / speed / eta / name truncation) live in `crate::fmt`
//! (W1.4) and are consumed here, not duplicated.

use crate::models::{DownloadProgress, QueueItemSummary, VerificationProgress};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::sync::atomic::Ordering;

// ============================================================================
// Activity HUD (design Option 3: compact matrix — PLANS/design-options.md)
//
// One fixed-column line per pipeline item, rendered as a strip above the
// status bar. State glyphs (DL/VF/Q) sort rows into pipeline stages without
// boxes; numeric columns are right-aligned for vertical scan paths; row
// count and row height are fixed while active (zero reflow).
// ============================================================================

/// Data bundle for the activity HUD.
pub struct ActivityHudData<'a> {
    pub download_progress: &'a Option<DownloadProgress>,
    pub queue_size: usize,
    pub queue_bytes: u64,
    pub queue_items: &'a [QueueItemSummary],
    pub verification_progress: &'a [VerificationProgress],
    pub verification_queue_size: usize,
    pub verification_queue_bytes: u64,
    pub verified_ok: usize,
    pub verified_fail: usize,
}

/// Maximum item rows before truncation (footer always renders as row +1).
const HUD_MAX_ITEM_ROWS: usize = 8;
/// Queue rows shown individually before collapsing into "+N more".
const HUD_MAX_QUEUE_ROWS: usize = 3;

/// Height of the reserved HUD strip (item rows + 1 footer row + 2 border
/// rows), 0 when idle.
pub fn activity_hud_height(data: &ActivityHudData) -> u16 {
    let rows = hud_item_row_count(data);
    if rows == 0 {
        0
    } else {
        rows.min(HUD_MAX_ITEM_ROWS) as u16 + 1 + 2
    }
}

fn hud_item_row_count(data: &ActivityHudData) -> usize {
    let mut rows = 0usize;
    if data.download_progress.is_some() {
        rows += 1;
    }
    rows += data.verification_progress.len();
    let queue_len = data.queue_items.len();
    rows += queue_len.min(HUD_MAX_QUEUE_ROWS);
    if queue_len > HUD_MAX_QUEUE_ROWS {
        rows += 1; // "+N more" row
    }
    rows
}

/// Render the HUD into `area` (the strip reserved above the status bar).
pub fn render_activity_hud(frame: &mut Frame, area: Rect, data: &ActivityHudData) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Row builders lay out against the block's INNER width (borders excluded)
    // so every fixed-width column lands on the same x across all rows.
    let w = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    // --- Download row ---
    if let Some(p) = data.download_progress {
        lines.push(download_hud_line(p, w));
    }

    // --- Verification rows ---
    for ver in data.verification_progress {
        lines.push(verification_hud_line(ver, w));
    }

    // --- Queue rows ---
    let queue_len = data.queue_items.len();
    for item in data.queue_items.iter().take(HUD_MAX_QUEUE_ROWS) {
        lines.push(queue_hud_line(
            &item.filename,
            Some(item.total_size),
            w,
            false,
        ));
    }
    if queue_len > HUD_MAX_QUEUE_ROWS {
        let more = queue_len - HUD_MAX_QUEUE_ROWS;
        let more_bytes: u64 = data
            .queue_items
            .iter()
            .skip(HUD_MAX_QUEUE_ROWS)
            .map(|i| i.total_size)
            .sum();
        lines.push(queue_hud_line(
            &format!("+{more} more"),
            Some(more_bytes),
            w,
            true,
        ));
    }

    // Truncate to budget (footer aggregates what was cut)
    lines.truncate(HUD_MAX_ITEM_ROWS);

    // --- Footer aggregate ---
    lines.push(hud_footer_line(data, w));

    let widget =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Activity"));
    frame.render_widget(widget, area);
}

/// Column layout shared by all HUD rows, degraded on narrow terminals.
struct HudColumns {
    name_w: usize,
    pct_w: usize,
    speed_w: usize,
    eta_w: usize,
}

impl HudColumns {
    fn for_width(w: usize) -> Self {
        let (name_w, speed_w, eta_w) = if w >= 100 {
            (24, 9, 7)
        } else if w >= 84 {
            (16, 9, 0)
        } else if w >= 70 {
            (10, 9, 0)
        } else {
            (6, 0, 0)
        };
        HudColumns {
            name_w,
            pct_w: 4,
            speed_w,
            eta_w,
        }
    }

    /// Visible width of the right-aligned block (pct + speed + eta + gaps).
    fn right_w(&self) -> usize {
        let mut w = self.pct_w;
        if self.speed_w > 0 {
            w += 1 + self.speed_w;
        }
        if self.eta_w > 0 {
            w += 1 + self.eta_w;
        }
        w
    }

    /// Middle space available for bar (+ chunk map on the DL row).
    /// `total_w` is the block's inner width; the middle must be filled
    /// exactly (bar or padding) so the right block aligns on every row.
    fn middle_w(&self, total_w: usize) -> usize {
        total_w.saturating_sub(4 + self.name_w + 1 + 1 + self.right_w())
    }
}

/// Pad a (possibly truncated) name to exactly `width` chars so every row's
/// middle section starts on the same column (fixed-width column layout).
pub(super) fn pad_name(name: &str, width: usize) -> String {
    format!(
        "{:<width$}",
        crate::fmt::truncate_name_middle_hud(name, width),
        width = width
    )
}

fn download_hud_line(p: &DownloadProgress, w: usize) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    let pct = if p.total > 0 {
        (p.downloaded as f64 / p.total as f64 * 100.0) as u16
    } else {
        0
    };

    // Speed + ETA for the CURRENT FILE only; whole-queue totals live in the
    // footer line (see hud_footer_line)
    let current_remaining = p.total.saturating_sub(p.downloaded);
    let speed_str = if p.speed_mbps > 0.0 {
        crate::fmt::speed_hud(p.speed_mbps)
    } else {
        "--".to_string()
    };
    let eta_str = if p.speed_mbps > 0.0 {
        let secs = current_remaining as f64 / (p.speed_mbps * 1_048_576.0);
        crate::fmt::eta_hud(secs as u64)
    } else {
        "--".to_string()
    };

    let mut spans = vec!["DL ".into_cyan(), Span::raw("  ")];
    spans.push(Span::raw(pad_name(&p.filename, cols.name_w)));
    spans.push(Span::raw(" "));

    // Bar + (if room) "ch n/total" chunk map filling the remaining space
    let middle = cols.middle_w(w);
    let label = format!(
        "ch {}/{}",
        p.chunk_completed.iter().filter(|b| **b).count(),
        p.num_chunks
    );
    let active_ids: Vec<usize> = p
        .chunks
        .iter()
        .filter(|c| c.is_active)
        .map(|c| c.chunk_id)
        .collect();
    let label_w = label.chars().count() + 1; // + leading gap
    let mut bar_w = middle;
    let mut map_cells = 0;
    if p.num_chunks > 0 && middle >= 24 + label_w + 4 {
        bar_w = 24;
        map_cells = middle - bar_w - label_w;
        if map_cells > p.num_chunks {
            map_cells = p.num_chunks;
            bar_w = middle - map_cells - label_w;
        }
    }
    if bar_w > 0 {
        spans.push(bar_spans(pct, bar_w, Color::Cyan));
    }
    if map_cells > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
        spans.push(Span::raw(" "));
        spans.extend(chunk_map_spans(&p.chunk_completed, &active_ids, map_cells));
    }

    spans.push(right_block_spans(
        &cols,
        Some(pct),
        Some(&speed_str),
        Some(&eta_str),
    ));
    Line::from(spans)
}

fn verification_hud_line(ver: &VerificationProgress, w: usize) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    // Ordering::Relaxed is safe here: eventual consistency is fine for display
    let verified = ver.verified_bytes.load(Ordering::Relaxed);
    let pct = if ver.total_bytes > 0 {
        (verified as f64 / ver.total_bytes as f64 * 100.0) as u16
    } else {
        0
    };
    let speed_str = if ver.speed_mbps > 0.0 {
        crate::fmt::speed_hud(ver.speed_mbps)
    } else {
        "--".to_string()
    };
    let eta_str = if ver.speed_mbps > 0.0 && ver.total_bytes > verified {
        let remaining = (ver.total_bytes - verified) as f64 / (ver.speed_mbps * 1_048_576.0);
        crate::fmt::eta_hud(remaining as u64)
    } else {
        "--".to_string()
    };

    let mut spans = vec!["VF ".into_green(), Span::raw("  ")];
    spans.push(Span::raw(pad_name(&ver.filename, cols.name_w)));
    spans.push(Span::raw(" "));
    let middle = cols.middle_w(w);
    if middle >= 8 {
        spans.push(bar_spans(pct, middle, Color::Green));
    }
    spans.push(right_block_spans(
        &cols,
        Some(pct),
        Some(&speed_str),
        Some(&eta_str),
    ));
    Line::from(spans)
}

fn queue_hud_line(name: &str, size: Option<u64>, w: usize, dim: bool) -> Line<'static> {
    let cols = HudColumns::for_width(w);
    let size_str = size
        .map(crate::fmt::bytes_hud)
        .unwrap_or_else(|| "--".to_string());
    let name_span = if dim {
        Span::styled(
            pad_name(name, cols.name_w),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::raw(pad_name(name, cols.name_w))
    };

    let mut spans = vec!["Q  ".into_gray(), Span::raw("  ")];
    spans.push(name_span);
    spans.push(Span::raw(" "));
    // Whitespace fills the middle (no decorative filler): fixed-column
    // layout — stable columns + whitespace separation beat dotted rails,
    // which read as noise and mask misalignment.
    let middle = cols.middle_w(w);
    spans.push(Span::raw(" ".repeat(middle)));
    spans.push(right_block_spans(
        &cols,
        None,
        Some(&size_str),
        Some("wait"),
    ));
    Line::from(spans)
}

fn hud_footer_line(data: &ActivityHudData<'_>, w: usize) -> Line<'static> {
    let mut parts: Vec<String> = Vec::new();
    if data.verified_ok > 0 || data.verified_fail > 0 {
        parts.push(format!(
            "hash ✓{} ✗{}",
            data.verified_ok, data.verified_fail
        ));
    }
    if data.verification_queue_size > 0 {
        parts.push(format!(
            "verify q {} ({})",
            data.verification_queue_size,
            crate::fmt::bytes_hud(data.verification_queue_bytes)
        ));
    }
    if data.queue_size > 0 {
        parts.push(format!(
            "dl q {} ({})",
            data.queue_size,
            crate::fmt::bytes_hud(data.queue_bytes)
        ));
    }
    if let Some(p) = data.download_progress {
        if p.speed_mbps > 0.0 {
            let total_remaining = p.total.saturating_sub(p.downloaded) + data.queue_bytes;
            let secs = total_remaining as f64 / (p.speed_mbps * 1_048_576.0);
            parts.push(format!(
                "remaining {} {}",
                crate::fmt::remaining_gb(total_remaining),
                crate::fmt::eta_hud(secs as u64)
            ));
        }
    }

    let inner_w = w.saturating_sub(2); // borders
    let text = if parts.is_empty() {
        "idle".to_string()
    } else {
        parts.join(" · ")
    };
    let used = text.chars().count() + 4; // "── " prefix + " " suffix
    let fill = inner_w.saturating_sub(used);
    let mut s = format!("── {text} ");
    s.push_str(&"─".repeat(fill));
    Line::from(Span::styled(s, Style::default().fg(Color::DarkGray)))
}

/// Filled/empty bar span in the given color, exactly `width` cells wide.
fn bar_spans(pct: u16, width: usize, color: Color) -> Span<'static> {
    let filled = ((width as f64 * pct as f64 / 100.0).round() as usize).min(width);
    let empty = width - filled;
    Span::styled(
        format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty)),
        Style::default().fg(color),
    )
}

/// Map each of `cells` positions to its chunk bucket and pick a state char.
pub(super) fn chunk_map_spans(
    completed: &[bool],
    active_ids: &[usize],
    cells: usize,
) -> Vec<Span<'static>> {
    let total = completed.len();
    if total == 0 || cells == 0 {
        return Vec::new();
    }
    let mut spans = Vec::with_capacity(cells);
    for i in 0..cells {
        let bucket = i * total / cells;
        let (ch, color) = if completed[bucket] {
            ('█', Color::Cyan) // done
        } else if active_ids.contains(&bucket) {
            ('▓', Color::Yellow) // active
        } else {
            ('░', Color::DarkGray) // pending
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }
    spans
}

/// Right-aligned pct / speed / eta block.
fn right_block_spans(
    cols: &HudColumns,
    pct: Option<u16>,
    speed: Option<&str>,
    eta: Option<&str>,
) -> Span<'static> {
    let mut s = String::new();
    match pct {
        Some(p) => s.push_str(&format!("{:>3}%", p)),
        None => s.push_str("  --"),
    }
    if cols.speed_w > 0 {
        let sp = speed.unwrap_or("");
        s.push(' ');
        s.push_str(&format!("{:>width$}", sp, width = cols.speed_w));
    }
    if cols.eta_w > 0 {
        let e = eta.unwrap_or("");
        s.push(' ');
        s.push_str(&format!("{:>width$}", e, width = cols.eta_w));
    }
    Span::raw(s)
}

trait StateGlyph {
    fn into_cyan(self) -> Span<'static>;
    fn into_green(self) -> Span<'static>;
    fn into_gray(self) -> Span<'static>;
}

impl StateGlyph for &str {
    fn into_cyan(self) -> Span<'static> {
        Span::styled(
            format!("{:<2}", self),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }
    fn into_green(self) -> Span<'static> {
        Span::styled(
            format!("{:<2}", self),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    }
    fn into_gray(self) -> Span<'static> {
        Span::styled(format!("{:<2}", self), Style::default().fg(Color::DarkGray))
    }
}
