# Rust HF Downloader

A Terminal User Interface (TUI) application for searching, browsing, and downloading models from the HuggingFace model hub.

> **Security Note**: Version 0.6.0 includes important security fixes for path traversal vulnerabilities. See [SECURITY.md](SECURITY.md) for details and remaining security considerations.

## Features

- 🔍 **Interactive Search**: Search through thousands of HuggingFace models
- ⌨️ **Vim-like Controls**: Efficient keyboard navigation
- 📊 **Rich Display**: View model details including downloads, likes, and tags
- 📦 **Quantization Details**: See all available quantized versions (Q2, Q4, Q5, Q8, IQ4_XS, MXFP4, etc.) with file sizes
- 📥 **Smart Downloads**: Download models directly from the TUI with:
  - Progress tracking with speed indicators
  - Resume support for interrupted downloads
  - Multi-part GGUF file handling
  - Automatic subfolder organization by publisher/model
  - Download queue with status display
- ✅ **Download Tracking**: Visual indicators showing already downloaded files
- 🔄 **Resume on Startup**: Automatically detect and offer to resume incomplete downloads
- 💾 **Metadata Management**: TOML-based download registry for reliable tracking
- ⚡ **Async API**: Non-blocking UI with async API calls
- 🎨 **Colorful Interface**: Syntax-highlighted results for better readability

## Requirements

- **Rust**: 1.75.0 or newer (compatible with Ubuntu 22.04 LTS default compiler)
- **Cargo**: Latest stable version

## Installation

```bash
cargo build --release
```

## Usage

Run the application:

```bash
cargo run --release
```

### Controls

| Key | Action |
|-----|--------|
| `/` | Enter search mode |
| `Tab` | Switch focus between Models and Quantizations lists |
| `d` | Download selected quantization (when Quantizations list is focused) |
| `Enter` | Execute search (in search mode) / Show details (in browse mode) |
| `Esc` | Return to browse mode from search mode / Cancel popup |
| `j` or `↓` | Move selection down in focused list |
| `k` or `↑` | Move selection up in focused list |
| `q` or `Ctrl+C` | Quit application |

#### Resume Download Popup (on startup)
| Key | Action |
|-----|--------|
| `Y` | Resume all incomplete downloads |
| `N` | Skip incomplete downloads |
| `D` | Delete incomplete files and skip |

### How to Use

1. **Start the application** - If incomplete downloads exist, you'll see a resume popup first
   - Press `Y` to resume incomplete downloads
   - Press `N` to skip and continue
   - Press `D` to delete incomplete files
   
2. **Press `/`** to enter search mode (the search box will be highlighted in yellow)

3. **Type your query** (e.g., "gpt", "llama", "mistral")

4. **Press Enter** to search

5. **Navigate model results** with `j`/`k` or arrow keys (Models list is focused by default, yellow border)

6. **View quantization details** automatically as you select different models
   - Green `[downloaded]` indicator shows files you already have

7. **Press Tab** to switch focus to the Quantizations list (yellow border moves)

8. **Navigate quantizations** with `j`/`k` or arrow keys

9. **Press `d`** to download the selected quantization:
   - A popup will appear with the default path `~/models`
   - Edit the path if needed
   - Press Enter to confirm and start download
   - Files are saved to: `{path}/{author}/{model-name}/{filename}`
   - For multi-part GGUFs, all parts are queued automatically
   - Press Esc to cancel
   - Download progress appears in the top right corner with:
     - Progress percentage
     - Download speed (MB/s)
     - Queue count if multiple downloads pending

10. **Press Enter** to see full details of the selected item in the status bar

11. **Press Tab** again to return focus to the Models list

12. **Press `/`** to start a new search

The **Quantization Details** section shows all available GGUF quantized versions with:
- **Left**: Combined file size (formatted as GB/MB/KB) - sum of all parts for multi-part files
- **Middle**: Quantization type (Q2_K, Q4_K_M, Q5_0, Q8_0, IQ4_XS, MXFP4, etc.)
- **Right**: Filename with green `[downloaded]` indicator if already on disk

### Example Searches

- Search for GPT models: `/` → type `gpt` → `Enter`
- Search for image models: `/` → type `stable-diffusion` → `Enter`
- Search for translation models: `/` → type `translation` → `Enter`

## Technical Details

### Architecture

- **Rust Edition**: 2021
- **Minimum Rust Version**: 1.75.0+ (Ubuntu 22.04 compatible)
- **TUI Framework**: [ratatui](https://github.com/ratatui/ratatui)
- **HTTP Client**: reqwest with async support and streaming downloads
- **TLS Backend**: rustls (pure Rust TLS implementation)
- **API**: HuggingFace REST API (`https://huggingface.co/api/models`)
- **Text Input**: tui-input for search box handling
- **Download Management**: 
  - TOML-based metadata registry (`~/models/hf-downloads.toml`)
  - Automatic resume from byte position
  - Retry logic with exponential backoff
  - Multi-part file detection and grouping
  - In-memory tracking of completed downloads

### API Integration

The application queries the HuggingFace API with the following parameters:
- Search query from user input
- Results limited to 50 models
- Sorted by downloads in descending order

### Project Structure

```
rust-hf-downloader/
├── Cargo.toml              # Dependencies and project metadata
├── README.md               # This file
├── changelog/              # Release notes for all versions
└── src/
    ├── main.rs             # Entry point
    ├── models.rs           # Data structures & types
    ├── utils.rs            # Formatting utilities
    ├── api.rs              # HuggingFace API client
    ├── registry.rs         # Download registry persistence
    ├── download.rs         # Download manager & security
    └── ui/
        ├── mod.rs          # UI module declaration
        ├── app.rs          # App state & event handling
        └── render.rs       # TUI rendering logic
```

**Version 0.7.0** introduces a modular architecture with clear separation of concerns:
- **6 top-level modules** for business logic
- **2 UI submodules** for presentation layer
- **~240 lines average** per file (previously 2,074 in one file)
- **Improved maintainability, testability, and readability**

## Dependencies

- `ratatui`: TUI framework
- `crossterm`: Terminal manipulation
- `tokio`: Async runtime
- `reqwest`: HTTP client with streaming support
- `serde`: JSON serialization
- `tui-input`: Text input widget
- `color-eyre`: Error handling
- `toml`: TOML serialization for download metadata
- `regex`: Multi-part filename pattern matching
- `urlencoding`: URL-safe query encoding
- `futures`: Async stream utilities

## Security

For security considerations and vulnerability reporting, see [SECURITY.md](SECURITY.md).

Key security features in v0.6.0:
- ✅ Path traversal protection with comprehensive validation
- ✅ Sanitization of all user inputs and API responses
- ✅ Canonicalization checks for download paths

## Changelog

### Version 0.7.0 (2025-11-21)
- **Major Refactoring**: Complete modular architecture overhaul
- Split monolithic 2,074-line `main.rs` into 9 focused modules
- Created 6 top-level modules: `models`, `utils`, `api`, `registry`, `download`, `ui`
- Created 2 UI submodules: `app` (state/logic) and `render` (presentation)
- Improved maintainability, testability, and readability
- Average file size reduced to ~240 lines per module
- No breaking changes - purely internal refactoring
- See [changelog/RELEASE_NOTES_0.7.0.md](changelog/RELEASE_NOTES_0.7.0.md) for full details

### Version 0.6.5 (2025-11-21)
- Pinned indexmap dependency to v2.2.6

### Version 0.6.4 (2025-11-21)
- Pinned backtrace dependency to v0.3.71 for Rust 1.75.0 compatibility

### Version 0.6.3 (2025-11-21)
- Pinned additional dependencies (url, idna) for Rust 1.75.0 compatibility

### Version 0.6.2 (2025-11-21)
- Switched from native-tls to rustls for TLS implementation
- Added support for Rust 1.75.0+ (Ubuntu 22.04 compatibility)

### Version 0.6.1 (2025-11-21)
- Changed Rust edition from 2024 to 2021 in Cargo.toml
- Ensures broader compatibility with stable Rust toolchains

### Version 0.6.0 (2024-11-21)
- **Security**: Fixed HIGH severity path traversal vulnerability
- Added comprehensive path validation and sanitization
- See [changelog/RELEASE_NOTES_0.6.0.md](changelog/RELEASE_NOTES_0.6.0.md) for details

### Older Versions

For detailed release notes of older versions, see the [changelog directory](changelog/).

## License

Copyright (c) Johannes Bertens

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
