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