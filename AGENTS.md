# AGENT.md - AI Agent Guide for Rust HF Downloader

This document provides AI agents with a comprehensive understanding of the Rust HF Downloader codebase, its architecture, and how to work with it.

## Project Overview

**Rust HF Downloader** is a Terminal User Interface (TUI) application written in Rust that allows users to search, browse, and download models from the HuggingFace model hub. It provides an interactive, keyboard-driven interface with vim-like controls and comprehensive download management.

### Modular Design 
The application follows a modular architecture with clear separation of concerns:

```
src/
├── main.rs           # Entry point (~20 lines)
├── models.rs         # Data structures and types
├── config.rs         # Configuration persistence (v0.9.0)
├── api.rs            # HuggingFace API client with authentication (v0.9.5)
├── http_client.rs    # Authenticated HTTP requests (v0.9.5)
├── registry.rs       # Download metadata management
├── download.rs       # Download orchestration with auth (v0.9.5)
├── rate_limiter.rs   # Token bucket rate limiter (v1.2.0)
├── verification.rs   # SHA256 verification (v0.8.0)
├── utils.rs          # Helper functions
└── ui/
    ├── mod.rs        # UI module exports
    ├── app.rs        # Module re-exports (~48 lines, v0.9.5)
    ├── app/          # App submodules (v0.9.5)
    │   ├── state.rs      # AppState and initialization (~158 lines)
    │   ├── events.rs     # Event handling (~709 lines)
    │   ├── models.rs     # Model browsing logic (~253 lines)
    │   ├── downloads.rs  # Download management (~460 lines)
    │   └── verification.rs # Verification UI (~77 lines)
    └── render.rs     # UI rendering functions
```
Check `README.md` for more information.

### Filter & Sort System (v1.0.0)
- **Filter State**: `src/ui/app/state.rs` - sort_field, sort_direction, filter_min_*
- **Filter Logic**: `src/ui/app/events.rs` - keyboard controls and presets
- **Filter UI**: `src/ui/render.rs` - toolbar rendering with focus highlighting
- **Filter API**: `src/api.rs` - fetch_models_filtered() with client-side filtering
- **Filter Config**: `src/config.rs` - default_sort_*, default_min_* persistence

### Mouse Integration System
The TUI supports full mouse interaction with panels and filter toolbar:

**State tracking** (`src/ui/app/state.rs`):
- `panel_areas: Vec<(FocusedPane, Rect)>` - clickable regions for each panel
- `filter_areas: Vec<(usize, Rect)>` - clickable regions for filter fields (0=sort, 1=downloads, 2=likes)
- `hovered_panel: Option<FocusedPane>` - currently hovered panel for border highlighting
- `mouse_position: Option<(u16, u16)>` - last known mouse position

**Event handling** (`src/ui/app.rs`):
- `handle_mouse_click(column, row)` - focus panel or cycle filter on click
- `handle_mouse_scroll(scroll_up, column, row)` - navigate panel or cycle filter on scroll
- `handle_filter_click(field_idx)` - cycle filter value forward on click
- `handle_filter_scroll(field_idx, scroll_up)` - cycle filter value bidirectionally
- `update_hover_state(column, row)` - update hovered panel for border effects
- Event coalescing: drains pending events, coalesces mouse moves into single hover update

**Rendering** (`src/ui/render.rs`):
- `RenderParams` includes `panel_areas`, `filter_areas`, `hovered_panel`
- Each panel stores its area in `panel_areas` during render
- `render_filter_toolbar()` calculates and stores `filter_areas`
- Border styles: yellow for focused, cyan for hovered, default otherwise

**Non-blocking design**:
- Uses `try_lock()` for tokio Mutexes during render to prevent deadlocks
- Cached render fields provide fallback when locks unavailable
- Mouse handler is synchronous to avoid blocking issues

### Mutex Lock Ordering (Critical for Deadlock Prevention)

To prevent deadlocks, all async code must acquire locks in the following order. NEVER hold a higher-numbered lock while acquiring a lower-numbered lock.

```
Lock Hierarchy (acquire in this order):

1. download_rx (Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>)
2. download_queue_size (Arc<Mutex<usize>>)
3. download_queue_bytes (Arc<Mutex<u64>>)
4. download_progress (Arc<Mutex<Option<DownloadProgress>>>)
5. complete_downloads (Arc<Mutex<CompleteDownloads>>)
6. verification_queue (Arc<Mutex<Vec<VerificationQueueItem>>>)
7. verification_queue_size (Arc<Mutex<usize>>)
8. verification_progress (Arc<Mutex<Vec<VerificationProgress>>>)
9. download_registry (Arc<Mutex<DownloadRegistry>>)
10. RateLimiter internal locks (tokens, rate, last_refill, max_tokens)
11. status_rx (Arc<Mutex<mpsc::UnboundedReceiver<String>>>)

Key Rules:
- ALWAYS acquire locks in the order above
- Release locks before acquiring locks from the same level if needed
- Use try_lock() for non-blocking access in UI rendering
- NEVER hold a lock across an await point unless absolutely necessary
- When receiving from a channel wrapped in Mutex, lock only for the recv() call
```

**Example - CORRECT pattern (download manager):**
```rust
// ✅ Lock only when receiving, release immediately
loop {
    let message = {
        let mut rx = download_rx.lock().await;  // Lock level 1
        match rx.recv().await {
            Some(msg) => msg,
            None => break,
        }
    }; // Lock level 1 released

    // Now safe to acquire level 2 and 3
    let mut queue_size = download_queue_size.lock().await;  // Lock level 2
    let mut queue_bytes = download_queue_bytes.lock().await;  // Lock level 3
    // ... process
}
```

**Example - INCORRECT pattern (causes deadlock):**
```rust
// ❌ WRONG: Holding level 1 while acquiring level 2
let mut rx = download_rx.lock().await;  // Lock level 1
while let Some(msg) = rx.recv().await {
    let mut queue_size = download_queue_size.lock().await;  // Lock level 2
    // DEADLOCK: If another task holds level 2 and needs level 1
}
```