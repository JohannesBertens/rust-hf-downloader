---
title: Agents Guide — src/ui
---

# Agents Guide (src/ui)

UI is split into:
- render.rs: all drawing; pure functions consuming App state
- app.rs: runtime loop; spawns background workers and manages frame redraw cadence
- app/ submodule: state container, event handling, search/download flows

Terminal stack: ratatui for rendering, crossterm for input, tui-input for text fields.

Panes and focus
- FocusedPane controls which list responds to j/k, arrows, Enter
- Modes:
  • ModelDisplayMode::Gguf → left models, bottom left quantization groups, bottom right files
  • ModelDisplayMode::Standard → left models, right split: Model metadata and File tree
- PopupMode overlays: Search, Options, ResumeDownload, DownloadPath, AuthError

app.rs
- App::run: sets running, syncs options to atomics, scans for incomplete downloads, spawns:
  • verification::verification_worker (background)
  • download manager task consuming download_rx and calling download::start_download
- Main loop draws, then conditionally calls async loaders flagged by state:
  • needs_search_models → App::search_models()
  • needs_load_quantizations → App::spawn_load_quantizations() and prefetch_adjacent_models()
- handle_crossterm_events polls key events and status messages, updates popup mode and status

render.rs
- render_ui(Frame, RenderParams): renders toolbar → results → bottom panels → status + both progress overlays
- Toolbar shows and highlights current sort and filters; indicates active preset
- GGUF path: render_gguf_panels → left groups (size, type, [downloaded]), right files with downloaded mark
- Standard path: render_standard_panels → left metadata summary, right file tree (flattened with expansion)
- Progress: render_progress_bars overlays download and verification gauges in right side
- Popups: search input, download path chooser, resume list, auth error steps, options dialog with 14 fields

Design notes
- Rendering functions never mutate App; they read params built in app.rs run loop
- Large lists: keep allocations local; format helpers in utils.rs
- Tree operations: render flattens FileTreeNode; navigation uses matching helper in app/events

Where to add UI features
- New pane/section → add pure renderer in render.rs and pass data via RenderParams
- New status or badges → augment spans in list or right panels
- New popup → add render_* in render.rs and event handler in events.rs and popup state in models.rs

Quality
- Keep draws quick; long ops go to spawned tasks with progress tracked in shared state
- Cross-check ListState selections when vectors may be empty; defensive bounds checks are used throughout
