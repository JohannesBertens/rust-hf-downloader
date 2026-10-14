# Release Notes - Version 2.17.0

**Release Date**: 2026-10-14

Keeps every piece of download information visible on small terminals
while downloading. Found by a small-terminal review of the TUI layout;
the HUD strip used to vanish outright below 29 rows and clipped its
last column at common widths. Tests 489 → 490.

## User-visible changes

- **The Activity HUD now borrows rows instead of disappearing**: on
  short terminals the HUD strip may take up to 2 rows from the bottom
  panels (12 → 10, never from the toolbar, results list or status
  bar). The full 4-row HUD now fits at 31 terminal rows (was 33) and
  at least the current-file download row shows down to 30 rows; the
  full 11-row HUD fits at 36 rows (was 40).
- **New fallback notice**: when the terminal is too short for even
  the borrowed strip (≤ 29 rows) and download activity exists, the
  status bar's first line shows `[download active — terminal too
  small for activity HUD]`. It renders as line 1 so a wrapping
  selection line can never clip it off narrow (80-column) terminals.
- **Fixed a 1-character clip in the HUD download row**: the chunk-map
  label's two gaps were budgeted as one, so every download row
  overflowed by a single cell and ate the last character of the
  rightmost column — `32.8MB/s` and `~2m41s` used to render as
  `32.8MB/` and `~2m41` at common widths.

## Internal

- `render_ui` (src/ui/render/mod.rs) owns the borrow arithmetic: the
  strip is clamped to `height − 27` (base layout 29 minus the new
  `HUD_BORROW_ROWS = 2`) and hides below a 3-row strip (2 borders +
  1 line); the borrowed rows leave the bottom-panels `Constraint`
  explicitly so the layout split stays exact and the solver never
  redistributes a deficit.
- `download_hud_line` budgets `label_w = len + 2` for both literal
  gaps around the `ch n/m` chunk-map label (src/ui/render/hud.rs).
- Tests: the two HUD threshold tests re-pinned to the new 30/31/32
  boundary (including the independent Constraint re-split), one new
  assert-based test for the status notice at 100 and 80 columns;
  three snapshot goldens regenerated (complete `MB/s` / `~…s`
  strings, 2-row-shorter bottom panels on the clamped fixture).
