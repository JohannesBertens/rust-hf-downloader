---
title: Agents Guide — src/ui
---

# Agents Guide (src/ui)

UI is split into:
- render/: all drawing; pure functions consuming App state — a facade (mod.rs)
  over one file per panel: models_list, standard, gguf, hud, popups,
  options_popup, toolbar
- app/ (mod.rs, W3.9 — no app.rs indirection): runtime loop; spawns background
  workers and manages frame redraw cadence; owns the crossterm event loop and
  the mouse click/scroll/hover handlers
- app/ submodule: state container, event dispatch, search/download flows; ui/app/filters.rs single-homes the filter/sort values and their cycle/step mutation rules; ui/app/events/ is the keyboard layer (mod.rs dispatch + shared navigation, keys.rs normal-mode/popup key handlers); ui/app/options.rs owns the options dialog (M5/U1: OptionsDialogState — cursor row, live-edit flags, the two text-edit buffers — plus the OPTIONS_FIELDS table; the renderer is a pure consumer); App groups mouse state (state.rs::MouseState: areas/hovered_panel/last_move) and options-dialog transient state (options_dialog: OptionsDialogState, docs/DEFERRED.md#options-dialog-transient-state — AppOptions is pure config schema)
- tree.rs: file-tree navigation model (not drawing) — see Tree operations

Terminal stack: ratatui for rendering, crossterm for input, tui-input for text fields.

Panes and focus
- FocusedPane controls which list responds to j/k, arrows, Enter
- Modes:
  • ModelDisplayMode::Gguf → left models, bottom left quantization groups, bottom right files
  • ModelDisplayMode::Standard → left models, right split: Model metadata and File tree
- PopupMode overlays: Search, Options, ResumeDownload, DownloadPath, AuthError

app/mod.rs (W3.9: was app.rs)
- App::run: sets running, syncs options to atomics, scans for incomplete downloads, spawns the shared engine tasks (engine::spawn_verification_worker + engine::spawn_manager on self.engine — the same sequence the CLI runs via engine::bootstrap):
  • engine::spawn_verification_worker (background)
  • engine::spawn_manager (download manager consuming download_rx and calling download::start_download)
- Main loop draws, then conditionally calls async loaders flagged by state:
  • needs_search_models → App::search_models()
  • needs_load_quantizations → App::spawn_load_quantizations() and prefetch_adjacent_models() (async since W4.11 — awaited in the loop; it used to block_on the debounce mutex)
- handle_crossterm_events polls key events and status messages, updates popup mode and status; both event branches (select! + drain loop) dispatch through one process_terminal_event helper (W4.8): Press-only keys, immediate click/scroll, mouse moves coalesced into the latest position for the throttled hover update

render/ (mod.rs is the facade)
- render_ui(Frame, RenderParams) lives in mod.rs: renders toolbar → results → bottom panels → status + both progress overlays; it owns the vertical layout (W4.10) — clamps the desired HUD height against BASE_LAYOUT_ROWS (3 toolbar + 10 main + 12 bottom + 4 status) and RETURNS RenderOutput { hud_strip, mouse } (W5.2); App::draw renders the activity HUD into hud_strip and stores mouse.panels/mouse.filters on App as the frame's hit-rect registry (the strip geometry has one owner — no second manual rect math in app/mod.rs)
- RenderParams is grouped per consumer (W5.2): FocusCtx (input_mode/focused_pane/hovered_panel), ListCtx (input/models/list_state/loading), GgufPanelContext + StandardPanelContext (bottom-panel data; the inactive mode's group is supplied but unread), FilterCtx (toolbar), StatusCtx (status bar) + display_mode + hud_height
- MouseAreas registration order is behavior (first-match hit-tests): filter fields 0,1,2 in display order; Models, then bottom panels left-to-right (GGUF: QuantizationGroups, QuantizationFiles; Standard: ModelMetadata, FileTree) — pinned by mouse_areas_register_in_lookup_order in render/tests.rs
- mod.rs also keeps the cfg(test) snap_ui helper: insta derives the snapshot name from the module path *and* stores the file next to the macro call site, so the test modules are direct children (render/snapshot_tests.rs, hud_tests.rs, style_size_tests.rs, tests.rs) and the .snap files live in render/snapshots/
- Toolbar shows and highlights current sort and filters; indicates active preset
- GGUF path: render_gguf_panels → left groups (size, type, [downloaded]), right files with downloaded mark
- Standard path: render_standard_panels → left metadata summary, right file tree (flattened with expansion)
- Progress: render_activity_hud overlays queue/download/verification activity in a reserved HUD strip (natural height from activity_hud_height, passed DESIRED/uncapped via RenderParams.hud_height and clamped inside render_ui; hidden when idle); hud_strip_rect_threshold_agreement in style_size_tests.rs pins the returned rect against the historical App::draw formula at the threshold-1/threshold/threshold+1 heights
- Popups: search input, download path chooser, resume list, auth error steps, options dialog with 16 fields; all five share the centered_rect geometry (width clamped to terminal - 4, /2 integer centering) and the four non-options overlays share popup_shell (Clear + whole-block-styled Block returning the inner area). The options dialog intentionally diverges: it clamps its height against terminal - 4 and styles borders only (border_style) — pinned by snapshots (W4.8)

Design notes
- Rendering functions never mutate App; they read params built in the app/mod.rs run loop and RETURN hit-rects (W5.2: no &mut out-params — the render pass is pure; App::draw assigns the returned MouseAreas)
- Large lists: keep allocations local; format helpers in crate::fmt (fmt.rs)
- Tree operations: ui/tree.rs is the single home for the navigation model
  • flatten_tree_for_navigation, toggle_node_expansion, count_tree_files
  • render draws the flattened list; app/events + app/downloads consume the same helpers

Where to add UI features
- New pane/section → add a pure renderer in the owning render/<panel>.rs (new panel = new submodule) and pass data via the matching RenderParams group (extend that panel's context struct; register its hit-rect left-to-right after the Results list)
- New status or badges → augment spans in list or right panels
- New popup → add render_* in render/popups.rs (options dialog: render/options_popup.rs) and the key handler in events/keys.rs, popup state in models/ui.rs (enums) + ui/app/options.rs (OptionsDialogState + OPTIONS_FIELDS, M5/U1), and mouse dispatch (if any) in app/mod.rs
- New options field → append an OptionsFieldSpec to OPTIONS_FIELDS (ui/app/options.rs, M5/U1) plus a modify_option arm keyed by its OptionsFieldId; the cursor bound and rendering follow the table automatically

Quality
- Keep draws quick; long ops go to spawned tasks with progress tracked in shared state
- Cross-check ListState selections when vectors may be empty; defensive bounds checks are used throughout
