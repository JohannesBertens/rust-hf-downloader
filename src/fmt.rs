// W1.4a creates this module with oracle tests only — no production caller
// exists yet, so dead_code must be allowed until W1.4b (the immediately
// following commit) migrates the call sites and deletes this allow.
#![allow(dead_code)]

//! Formatting primitives shared by the CLI and TUI surfaces, with one
//! wrapper per surface pinning that surface's exact historical string.
//!
//! The full-vs-HUD presentation split is **intentional, not to be unified**
//! (W1.4, plans/readability-maintainability-refactor.md):
//!
//! - ETA: CLI `1h05m`-style [`eta_cli`] (f64 seconds, `"?"` guard) vs HUD
//!   `~1h05m` [`eta_hud`] (u64 seconds, `~` prefix, zero-padded hour
//!   minutes).
//! - Truncation: CLI paths keep the *tail* behind a leading `…`
//!   ([`truncate_path_cli`]); TUI filenames use a *middle* marker — `…`
//!   for the files panel ([`truncate_filename`]), `~` for the activity-HUD
//!   name column ([`truncate_name_middle_hud`]).
//! - Bytes: full `1.00 GB` (two decimals, space) vs compact HUD `1.0GB`
//!   (mixed decimals, no space).
//!
//! All byte formatters use binary (1024) thresholds; the count formatter
//! [`number`] is deliberately decimal (1.0K = one thousand) — that
//! asymmetry is the legacy behavior and is pinned by the oracle tests
//! below.
//!
//! Truncation is **char-count based, not unicode display width** — a CJK
//! character or emoji counts as one char. That is the legacy behavior of
//! every old helper and is deliberately preserved.
//!
//! The `#[cfg(test)]` `oracle` module holds verbatim copies of the old
//! helper bodies as frozen oracles (characterization-first, H1): the
//! differential table tests assert wrapper output == oracle output. While
//! the legacy helpers still exist in `utils`, `cli::report` and
//! `ui::render`, [`crate::fmt::tests::oracle_copies_match_live_legacy_helpers`]
//! additionally cross-checks oracle copy == live helper, so the copies
//! provably froze the real algorithms.

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// Unit names shared by every byte formatter (binary thresholds, 1024).
const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

/// Per-surface byte-formatting spec: decimal digits per unit and the
/// separator between value and unit.
struct ByteUnitSpec {
    /// Decimal digits for each unit index (0 = B, 1 = KB, 2 = MB, 3 = GB).
    decimals: [usize; 4],
    /// Separator between the number and the unit (`" "` or `""`).
    sep: &'static str,
}

/// Parameterized byte formatter: binary (1024) thresholds — the base every
/// legacy helper used — with per-unit decimals and separator chosen by the
/// surface spec. Byte counts below 1 KiB always print as bare integers.
fn bytes_with_units(bytes: u64, spec: &ByteUnitSpec) -> String {
    const K: u64 = 1024;
    const M: u64 = K * K;
    const G: u64 = M * K;
    let (unit, divisor) = if bytes >= G {
        (3, G)
    } else if bytes >= M {
        (2, M)
    } else if bytes >= K {
        (1, K)
    } else {
        (0, 1)
    };
    if unit == 0 {
        format!("{}{sep}{}", bytes, BYTE_UNITS[0], sep = spec.sep)
    } else {
        format!(
            "{:.*}{sep}{}",
            spec.decimals[unit],
            bytes as f64 / divisor as f64,
            BYTE_UNITS[unit],
            sep = spec.sep
        )
    }
}

/// Compact duration: `<prefix>{h}h{m}m` / `{m}m{s}s` / `{s}s`. `secs < 60`
/// keeps the seconds-only form; hours drop seconds. `pad_hour_minutes`
/// zero-pads the hour-form minutes to two digits (HUD layout).
fn duration_compact(secs: u64, prefix: &str, pad_hour_minutes: bool) -> String {
    if secs >= 3600 {
        if pad_hour_minutes {
            format!("{prefix}{}h{:02}m", secs / 3600, (secs % 3600) / 60)
        } else {
            format!("{prefix}{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    } else if secs >= 60 {
        format!("{prefix}{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{prefix}{}s", secs)
    }
}

/// Bar body of `width` cells (no surrounding brackets): the filled-cell
/// count is the rounded fraction `done / total`, clamped to `width`;
/// `total == 0` renders a full bar (indeterminate).
fn progress_bar_body(done: u64, total: u64, width: usize, filled: char, empty: char) -> String {
    let filled_cells = if total == 0 {
        width
    } else {
        ((done as f64 / total as f64) * width as f64).round() as usize
    }
    .min(width);
    let mut bar = String::new();
    bar.extend(std::iter::repeat_n(filled, filled_cells));
    bar.extend(std::iter::repeat_n(empty, width - filled_cells));
    bar
}

/// Tail-keeping truncation: if `s` fits in `max` chars it is returned
/// unchanged; otherwise the output is `marker` followed by the final
/// `max - 1` chars (the differing tail). Char-count based.
fn truncate_keep_tail(s: &str, max: usize, marker: char) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(count + 1 - max).collect();
        format!("{marker}{tail}")
    }
}

/// Middle truncation: `head + marker + tail` fitting `max` chars, with the
/// last `tail_len` chars kept (`tail_len < max` must hold — the surface
/// wrappers own the degenerate-width policy: 0, 1, `< 3`). Strings that
/// fit are returned unchanged. Char-count based.
fn truncate_middle(s: &str, max: usize, marker: char, tail_len: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let head_len = max - 1 - tail_len;
        let head: String = s.chars().take(head_len).collect();
        let tail: String = s.chars().skip(count - tail_len).collect();
        format!("{head}{marker}{tail}")
    }
}

// ---------------------------------------------------------------------------
// Surface wrappers — each pins its surface's exact current string
// ---------------------------------------------------------------------------

/// Full-format byte size for CLI text and TUI detail panels: `1023 B`,
/// `1.00 KB`, `5.00 GB` (two decimals, space, binary thresholds).
pub fn size_full(bytes: u64) -> String {
    const SPEC: ByteUnitSpec = ByteUnitSpec {
        decimals: [0, 2, 2, 2],
        sep: " ",
    };
    bytes_with_units(bytes, &SPEC)
}

/// Compact byte size for the activity-HUD columns: `0B`, `2KB`, `512MB`,
/// `6.0GB` (no space; 0 decimals below GB, 1 at GB).
pub fn bytes_hud(bytes: u64) -> String {
    const SPEC: ByteUnitSpec = ByteUnitSpec {
        decimals: [0, 0, 0, 1],
        sep: "",
    };
    bytes_with_units(bytes, &SPEC)
}

/// Remaining-download bytes as whole GB for the HUD footer: empty for 0,
/// `<1GB` below 1 GiB, otherwise the byte count rounded **up** to whole GB.
pub fn remaining_gb(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    if bytes == 0 {
        String::new()
    } else if bytes < GB {
        "<1GB".to_string()
    } else {
        let gb = (bytes as f64 / GB as f64).ceil() as u64;
        format!("{gb}GB")
    }
}

/// Compact throughput for HUD columns from a MiB/s value: `32.8MB/s`, or
/// `2.0GB/s` at ≥ 1024 MiB/s.
pub fn speed_hud(mbps: f64) -> String {
    if mbps >= 1024.0 {
        format!("{:.1}GB/s", mbps / 1024.0)
    } else {
        format!("{:.1}MB/s", mbps)
    }
}

/// Decimal compact count for stats columns: `999`, `1.0K`, `1.2M`.
/// Deliberately base-1000 (unlike the byte formatters' base-1024).
pub fn number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// ETA for CLI progress lines, from (possibly unrounded or invalid) f64
/// seconds: `59s`, `1m35s`, `1h1m`; non-finite or negative input → `?`.
pub fn eta_cli(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "?".to_string();
    }
    duration_compact(secs.round() as u64, "", false)
}

/// ETA for the HUD eta column, from u64 seconds: `~42s`, `~12m21s`,
/// `~1h05m` (`~` prefix, hour minutes zero-padded to two digits).
pub fn eta_hud(secs: u64) -> String {
    duration_compact(secs, "~", true)
}

/// 20-cell progress bar for CLI progress lines, `[████░░░░]`-style. A
/// total of 0 renders a full bar; over-100% progress is clamped.
pub fn bar_cli(done: u64, total: u64) -> String {
    const WIDTH: usize = 20;
    format!("[{}]", progress_bar_body(done, total, WIDTH, '█', '░'))
}

/// Tail-keeping truncation for CLI paths and repo ids (`…` head marker,
/// differing tail kept — the identifying part of shard names).
pub fn truncate_path_cli(s: &str, max: usize) -> String {
    truncate_keep_tail(s, max, '…')
}

/// Middle-`…` truncation for the TUI files panel: keeps head and tail so
/// shard index and extension stay visible. Widths 0/1 degenerate to `""`
/// / `…`; the tail share is one third of the budget.
pub fn truncate_filename(name: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    truncate_middle(name, max_chars, '…', (max_chars - 1) / 3)
}

/// Middle-`~` truncation for the activity-HUD name column (fixed-width
/// rows): widths below 3 hard-cut without a marker (no room for one); the
/// head/tail budget splits around the `~` with the longer head.
pub fn truncate_name_middle_hud(name: &str, max: usize) -> String {
    if name.chars().count() <= max || max < 3 {
        return name.chars().take(max).collect();
    }
    truncate_middle(name, max, '~', (max - 1) / 2)
}

// ---------------------------------------------------------------------------
// Oracle tests (W1.4a): frozen legacy algorithms + differential tables
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim copies of the legacy helper bodies — the frozen oracle.
    /// Every differential table below asserts `fmt::* == oracle::*`, and
    /// [`oracle_copies_match_live_legacy_helpers`] proves the copies match
    /// the still-live legacy helpers while those exist.
    mod oracle {
        pub fn format_size(bytes: u64) -> String {
            const GB: u64 = 1_073_741_824;
            const MB: u64 = 1_048_576;
            const KB: u64 = 1_024;

            if bytes >= GB {
                format!("{:.2} GB", bytes as f64 / GB as f64)
            } else if bytes >= MB {
                format!("{:.2} MB", bytes as f64 / MB as f64)
            } else if bytes >= KB {
                format!("{:.2} KB", bytes as f64 / KB as f64)
            } else {
                format!("{} B", bytes)
            }
        }

        pub fn format_number(n: u64) -> String {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.1}K", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        }

        pub fn render_bar(done: u64, total: u64) -> String {
            const PROGRESS_BAR_WIDTH: usize = 20;
            let filled = if total == 0 {
                PROGRESS_BAR_WIDTH
            } else {
                ((done as f64 / total as f64) * PROGRESS_BAR_WIDTH as f64).round() as usize
            }
            .min(PROGRESS_BAR_WIDTH);
            format!(
                "[{}{}]",
                "█".repeat(filled),
                "░".repeat(PROGRESS_BAR_WIDTH - filled)
            )
        }

        pub fn format_eta(secs: f64) -> String {
            if !secs.is_finite() || secs < 0.0 {
                return "?".to_string();
            }
            let secs = secs.round() as u64;
            if secs < 60 {
                format!("{}s", secs)
            } else if secs < 3600 {
                format!("{}m{}s", secs / 60, secs % 60)
            } else {
                format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
            }
        }

        pub fn truncate_path(s: &str, max: usize) -> String {
            if s.chars().count() <= max {
                s.to_string()
            } else {
                let tail: String = s.chars().skip(s.chars().count() + 1 - max).collect();
                format!("…{}", tail)
            }
        }

        pub fn format_bytes_hud(bytes: u64) -> String {
            const MB: f64 = 1_048_576.0;
            const GB: f64 = 1_073_741_824.0;
            let b = bytes as f64;
            if bytes >= 1 << 30 {
                format!("{:.1}GB", b / GB)
            } else if bytes >= 1 << 20 {
                format!("{:.0}MB", b / MB)
            } else if bytes >= 1024 {
                format!("{:.0}KB", b / 1024.0)
            } else {
                format!("{bytes}B")
            }
        }

        pub fn format_speed_hud(mbps: f64) -> String {
            if mbps >= 1024.0 {
                format!("{:.1}GB/s", mbps / 1024.0)
            } else {
                format!("{:.1}MB/s", mbps)
            }
        }

        pub fn format_eta_hud(secs: u64) -> String {
            if secs >= 3600 {
                format!("~{}h{:02}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("~{}m{}s", secs / 60, secs % 60)
            } else {
                format!("~{}s", secs)
            }
        }

        pub fn truncate_name_middle(name: &str, max: usize) -> String {
            let chars: Vec<char> = name.chars().collect();
            if chars.len() <= max || max < 3 {
                return name.chars().take(max).collect();
            }
            let tail_w = (max - 1) / 2;
            let head_w = max - 1 - tail_w;
            let head: String = chars[..head_w].iter().collect();
            let tail: String = chars[chars.len() - tail_w..].iter().collect();
            format!("{head}~{tail}")
        }

        pub fn truncate_filename(name: &str, max_chars: usize) -> String {
            let count = name.chars().count();
            if max_chars == 0 {
                return String::new();
            }
            if count <= max_chars {
                return name.to_string();
            }
            if max_chars == 1 {
                return "…".to_string();
            }
            let tail_len = (max_chars - 1) / 3;
            let head_len = max_chars - 1 - tail_len;
            let head: String = name.chars().take(head_len).collect();
            let tail: String = name.chars().skip(count - tail_len).collect();
            format!("{}…{}", head, tail)
        }

        pub fn format_remaining_gb(bytes: u64) -> String {
            const GB: u64 = 1_073_741_824;
            if bytes == 0 {
                String::new()
            } else if bytes < GB {
                "<1GB".to_string()
            } else {
                let gb = (bytes as f64 / GB as f64).ceil() as u64;
                format!("{}GB", gb)
            }
        }
    }

    /// Byte inputs hitting every unit boundary, both rounding precisions,
    /// exact halves (round-half-to-even in `{:.0}`), the TB range and the
    /// u64 ceiling.
    const SIZES: [u64; 24] = [
        0,
        1,
        512,
        999,
        1000,
        1023,
        1024,
        1025,
        1536, // 1.5 KB — half at 0 decimals
        2048,
        2560, // 2.5 KB — half at 0/2 decimals
        10_240,
        999_999,
        1_048_575,
        1_048_576,
        1_048_577,
        1_572_864, // 1.5 MB — half at 0 decimals
        1_073_741_823,
        1_073_741_824,
        2_684_354_560, // 2.5 GB — half at 1 decimal
        5_368_709_120,
        6_442_450_944,
        1_099_511_627_776, // 1 TiB
        u64::MAX,
    ];

    /// Counts spanning both decimal thresholds and the u64 ceiling.
    const COUNTS: [u64; 17] = [
        0,
        1,
        9,
        10,
        99,
        100,
        999,
        1000,
        1001,
        1500, // 1.5K — half at 1 decimal
        9999,
        999_999,
        1_000_000,
        1_234_567,
        1_500_000, // 1.5M — half at 1 decimal
        999_999_999,
        u64::MAX,
    ];

    /// f64 seconds for the CLI ETA: warm-up fractions, round-to-even
    /// boundaries, all three duration arms, the guard inputs, and huge
    /// saturating values.
    const ETAS_F64: [f64; 25] = [
        0.0,
        0.4,
        0.5,
        0.6,
        1.0,
        59.4,
        59.5,
        59.9,
        60.0,
        61.5,
        95.0,
        3599.9,
        3600.0,
        3601.0,
        3700.0,
        7260.0, // 2h1m / ~2h01m — the zero-padding divergence
        86_399.0,
        86_400.0,
        90_061.0,
        900_000.0,
        1e12,
        f64::MAX, // saturates to u64::MAX in the cast
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ];

    /// u64 seconds for the HUD ETA: all three arms plus padding edges.
    const ETAS_U64: [u64; 16] = [
        0,
        1,
        42,
        59,
        60,
        61,
        119,
        3599,
        3600,
        3601,
        3660,
        3720,
        7260,
        86_399,
        86_400,
        u64::MAX,
    ];

    /// Speeds (MiB/s) around the GB/s threshold and the `{:.1}` rounding
    /// boundary.
    const SPEEDS: [f64; 18] = [
        0.0, 0.05, 0.14, 0.15, 0.95, 1.0, 1.95, 9.99, 32.84, 32.85, 100.0, 1023.9, 1023.95, 1024.0,
        1536.0, 2048.0, 9999.5, 1e6,
    ];

    /// Bar (done, total) pairs: empty/indeterminate, exact cells, rounding
    /// boundaries, full, over-100% clamp, and the u64 extremes.
    const BARS: [(u64, u64); 15] = [
        (0, 0),
        (0, 10),
        (5, 10),
        (1, 3),
        (2, 3),
        (1, 6),
        (1, 8),
        (7, 10),
        (19, 20),
        (20, 20),
        (21, 20),
        (100, 100),
        (0, 100),
        (1, u64::MAX),
        (u64::MAX, u64::MAX),
    ];

    /// Truncation inputs: empty, 1-char, ASCII paths/names, full-width CJK
    /// (incl. U+3000 ideographic space), emoji (astral plane), combining
    /// accents, and mixed scripts. All are ≥1 char per grapheme so the
    /// char-count semantics are exercised, not display width.
    const TRUNC_INPUTS: [&str; 10] = [
        "",
        "a",
        "ab",
        "model-Q4_K_M.gguf",
        "author/model-name/subdir/file-Q4_K_M.gguf",
        "shard-00001-of-00002.safetensors",
        "模　型　名　称.gguf",
        "🦀🦀🦀🦀🦀🦀",
        "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
        "模型file.gguf",
    ];

    /// Widths from degenerate (0/1/2) through panel-realistic to no-op.
    const TRUNC_WIDTHS: [usize; 9] = [0, 1, 2, 3, 5, 7, 12, 20, 24];

    /// While the legacy helpers still exist, prove the oracle copies froze
    /// the real algorithms: oracle == live for every table input.
    #[test]
    fn oracle_copies_match_live_legacy_helpers() {
        use crate::cli::report;
        use crate::ui::render;
        use crate::utils;

        for &b in SIZES.iter().chain(COUNTS.iter()) {
            assert_eq!(oracle::format_size(b), utils::format_size(b), "size {b}");
        }
        for &n in COUNTS.iter() {
            assert_eq!(
                oracle::format_number(n),
                utils::format_number(n),
                "count {n}"
            );
        }
        for &(done, total) in BARS.iter() {
            assert_eq!(
                oracle::render_bar(done, total),
                report::render_bar(done, total),
                "bar {done}/{total}"
            );
        }
        for &secs in ETAS_F64.iter() {
            assert_eq!(
                oracle::format_eta(secs),
                report::format_eta(secs),
                "eta {secs}"
            );
        }
        for &secs in ETAS_U64.iter() {
            assert_eq!(
                oracle::format_eta_hud(secs),
                render::format_eta_hud(secs),
                "eta hud {secs}"
            );
        }
        for &s in TRUNC_INPUTS.iter() {
            assert_eq!(
                oracle::truncate_path(s, 42),
                report::truncate_path(s, 42),
                "path {s:?}"
            );
            assert_eq!(
                oracle::truncate_filename(s, 18),
                render::truncate_filename(s, 18),
                "filename {s:?}"
            );
            for &w in TRUNC_WIDTHS.iter() {
                assert_eq!(
                    oracle::truncate_path(s, w),
                    report::truncate_path(s, w),
                    "path {s:?} w{w}"
                );
                assert_eq!(
                    oracle::truncate_filename(s, w),
                    render::truncate_filename(s, w),
                    "filename {s:?} w{w}"
                );
                assert_eq!(
                    oracle::truncate_name_middle(s, w),
                    render::truncate_name_middle(s, w),
                    "name middle {s:?} w{w}"
                );
            }
        }
        for &b in SIZES.iter() {
            assert_eq!(
                oracle::format_bytes_hud(b),
                render::format_bytes_hud(b),
                "bytes hud {b}"
            );
            assert_eq!(
                oracle::format_remaining_gb(b),
                render::format_remaining_gb(b),
                "remaining gb {b}"
            );
        }
        for &v in SPEEDS.iter() {
            assert_eq!(
                oracle::format_speed_hud(v),
                render::format_speed_hud(v),
                "speed {v}"
            );
        }
    }

    #[test]
    fn size_full_matches_oracle() {
        for &b in SIZES.iter() {
            assert_eq!(size_full(b), oracle::format_size(b), "size_full({b})");
        }
    }

    #[test]
    fn bytes_hud_matches_oracle() {
        for &b in SIZES.iter() {
            assert_eq!(bytes_hud(b), oracle::format_bytes_hud(b), "bytes_hud({b})");
        }
    }

    #[test]
    fn remaining_gb_matches_oracle() {
        for &b in SIZES.iter() {
            assert_eq!(
                remaining_gb(b),
                oracle::format_remaining_gb(b),
                "remaining_gb({b})"
            );
        }
    }

    #[test]
    fn speed_hud_matches_oracle() {
        for &v in SPEEDS.iter() {
            assert_eq!(speed_hud(v), oracle::format_speed_hud(v), "speed_hud({v})");
        }
    }

    #[test]
    fn number_matches_oracle() {
        for &n in COUNTS.iter() {
            assert_eq!(number(n), oracle::format_number(n), "number({n})");
        }
    }

    #[test]
    fn eta_cli_matches_oracle() {
        for &secs in ETAS_F64.iter() {
            assert_eq!(eta_cli(secs), oracle::format_eta(secs), "eta_cli({secs})");
        }
    }

    #[test]
    fn eta_hud_matches_oracle() {
        for &secs in ETAS_U64.iter() {
            assert_eq!(
                eta_hud(secs),
                oracle::format_eta_hud(secs),
                "eta_hud({secs})"
            );
        }
    }

    #[test]
    fn bar_cli_matches_oracle() {
        for &(done, total) in BARS.iter() {
            assert_eq!(
                bar_cli(done, total),
                oracle::render_bar(done, total),
                "bar_cli({done}, {total})"
            );
        }
    }

    #[test]
    fn truncate_path_cli_matches_oracle() {
        for &s in TRUNC_INPUTS.iter() {
            for &w in TRUNC_WIDTHS.iter() {
                assert_eq!(
                    truncate_path_cli(s, w),
                    oracle::truncate_path(s, w),
                    "truncate_path_cli({s:?}, {w})"
                );
            }
        }
    }

    #[test]
    fn truncate_filename_matches_oracle() {
        for &s in TRUNC_INPUTS.iter() {
            for &w in TRUNC_WIDTHS.iter() {
                assert_eq!(
                    truncate_filename(s, w),
                    oracle::truncate_filename(s, w),
                    "truncate_filename({s:?}, {w})"
                );
            }
        }
    }

    #[test]
    fn truncate_name_middle_hud_matches_oracle() {
        for &s in TRUNC_INPUTS.iter() {
            for &w in TRUNC_WIDTHS.iter() {
                assert_eq!(
                    truncate_name_middle_hud(s, w),
                    oracle::truncate_name_middle(s, w),
                    "truncate_name_middle_hud({s:?}, {w})"
                );
            }
        }
    }

    /// Pin the deliberate per-surface divergences with literals, so a
    /// future "unification" cannot pass silently: these strings differ by
    /// design between the CLI and HUD surfaces.
    #[test]
    fn surface_divergences_are_pinned() {
        // ETA: no prefix vs `~`, unpadded vs zero-padded hour minutes.
        assert_eq!(eta_cli(7260.0), "2h1m");
        assert_eq!(eta_hud(7260), "~2h01m");
        assert_eq!(eta_cli(0.0), "0s");
        assert_eq!(eta_hud(0), "~0s");
        // Bytes: `1.00 GB` vs `1.0GB`.
        assert_eq!(size_full(1_073_741_824), "1.00 GB");
        assert_eq!(bytes_hud(1_073_741_824), "1.0GB");
        // Truncation markers: tail-`…` vs middle-`…` vs middle-`~`.
        let name = "shard-00001-of-00002.safetensors";
        assert_eq!(truncate_path_cli(name, 12), "…safetensors");
        assert_eq!(truncate_filename(name, 12), "shard-00…ors");
        assert_eq!(truncate_name_middle_hud(name, 12), "shard-~nsors");
    }
}
