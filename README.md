# Rust HF Downloader

A Terminal User Interface (TUI) application for searching, browsing, and downloading models from the HuggingFace model hub.

## Demo

### Search & Browse
![Search & Browse Demo](docs/images/searching.gif)

### Download Flow
![Download Flow Demo](docs/images/search_download.gif)

## Features

- 🔍 **Interactive Search**: Search through thousands of HuggingFace models with popup dialog
- 🎯 **Advanced Filtering**: Sort and filter models by downloads, likes, or last modified
- ⚡ **Filter Presets**: Quick access to no-filter, popular, highly-rated, or recent models
- 💾 **Filter Persistence**: Save your preferred filter settings
- 🔐 **Gated Model Support**: Download restricted models with HuggingFace token authentication
  - Token configuration in Options screen
  - Clear error messages with helpful guidance
  - Supports Llama-3.1, Llama-2, and other gated models
- ⚙️ **Persistent Configuration**: Customize and save settings (press 'o')
  - Download directory, concurrent threads, chunk sizes
  - Retry behavior, timeout settings
  - Rate limiting with configurable speed caps
  - Verification options
  - HuggingFace authentication token
  - Settings persist across restarts
- ⌨️ **Vim-like Controls**: Efficient keyboard navigation
- 📊 **Rich Display**: View model details including downloads, likes, and tags
- 📦 **Quantization Details**: See all available quantized versions (Q2, Q4, Q5, Q8, IQ4_XS, MXFP4, etc.) with file sizes
- 📥 **Smart Downloads**: Download models directly from the TUI with:
  - Adaptive chunk sizing for optimal performance across all file sizes
  - Configurable download speed limiting (token bucket rate limiter)
  - Real-time speed tracking with continuous updates
  - Progress tracking with per-chunk speed indicators showing actual/limit speeds
  - Remaining download size and ETA display (e.g., "Downloading (2 queued) 120GB remaining, ~45 minutes")
  - Intelligent ETA calculation based on current speed (shows minutes, rounds up conservatively)
  - Resume support for interrupted downloads
  - Multi-part GGUF file handling
  - Automatic subfolder organization by publisher/model
  - Fixed quantization folder duplication issue
  - Fixed GGUF file path duplication for subdirectory downloads
  - Download queue with status display
- ✅ **Download Tracking**: Visual indicators showing already downloaded files
- 🔒 **SHA256 Verification**: Automatic integrity checking with:
  - Post-download hash verification
  - Manual verification with 'v' key
  - Multi-part file support (all parts verified)
  - Real-time verification progress bars
  - Hash mismatch detection
- 🔄 **Resume on Startup**: Automatically detect and offer to resume incomplete downloads
- 💾 **Metadata Management**: TOML-based download registry for reliable tracking
- ⚡ **Async API**: Non-blocking UI with async API calls
- 🎨 **Colorful Interface**: Syntax-highlighted results for better readability

## Requirements

- **Rust**: 1.75.0 or newer (compatible with Ubuntu 22.04 LTS default compiler)
- **Cargo**: Latest stable version

## Installation

### From source

Clone this repository:
```bash
git clone https://github.com/JohannesBertens/rust-hf-downloader.git
```

Build:
```bash
cargo build --release
```

Run the application:
```bash
cargo run --release
```

### Using Crates.io

Install:
```bash
cargo install rust-hf-downloader
```

Run:
```bash
rust-hf-downloader
```

See: [rust-hf-downloader on crates.io](https://crates.io/crates/rust-hf-downloader)

### Controls

#### Keyboard Controls

| Key | Action |
|-----|--------|
| `/` | Open search popup |
| `o` | Toggle options screen (configure settings) |
| `Tab` | Switch focus between Models and Quantizations lists |
| `d` | Download selected quantization (when Quantizations list is focused) |
| `v` | Verify SHA256 hash of downloaded file (when Quantizations list is focused) |
| `Enter` | Execute search (in search popup) / Show details (in browse mode) / Edit directory (in options) |
| `Esc` | Close search popup / Cancel popup / Close options |
| `j` or `↓` | Move selection down in focused list / Navigate options down |
| `k` or `↑` | Move selection up in focused list / Navigate options up |
| `+` | Increment numeric option value (in options screen) / Increment focused filter |
| `-` | Decrement numeric option value (in options screen) / Decrement focused filter |
| `Space` | Toggle boolean option (in options screen) |
| `q` or `Ctrl+C` | Quit application |

#### Mouse Controls

| Action | Effect |
|--------|--------|
| **Click on panel** | Focus that panel and select first item |
| **Scroll in panel** | Navigate up/down in the focused panel |
| **Hover over panel** | Highlight panel border (cyan) |
| **Click on filter field** | Focus field and cycle to next value |
| **Scroll on filter field** | Cycle filter value up/down |

Mouse-supported panels:
- **Models list**: Click to focus, scroll to navigate models (loads details automatically)
- **Quantization Groups**: Click to focus, scroll to navigate groups
- **Quantization Files**: Click to focus, scroll to navigate files
- **File Tree**: Click to focus, scroll to navigate tree
- **Filter Toolbar**: Click/scroll on Sort, Min Downloads, or Min Likes to cycle values

#### Filter & Sort Controls
| Key | Action |
|-----|--------|
| `s` | Cycle sort field (Downloads → Likes → Modified → Name) |
| `S` (Shift+s) | Toggle sort direction (Ascending ↔ Descending) |
| `f` | Cycle focus between filter fields |
| `+` or `→` | Increment focused filter value |
| `-`, `_` or `←` | Decrement focused filter value |
| `r` | Reset all filters to defaults |
| `1` | Preset: No Filters (default) |
| `2` | Preset: Popular (10k+ downloads, 100+ likes) |
| `3` | Preset: Highly Rated (1k+ likes) |
| `4` | Preset: Recent (sorted by last modified) |
| `Ctrl+S` | Save current filter settings as defaults |

#### Resume Download Popup (on startup)
| Key | Action |
|-----|--------|
| `Y` | Resume all incomplete downloads |
| `N` | Skip incomplete downloads |
| `D` | Delete incomplete files and skip |

### How to Use

1. **Start the application**
   - App starts with empty screen - press '/' to search for models
   - If incomplete downloads exist, you'll see a resume popup first
     - Press `Y` to resume incomplete downloads
     - Press `N` to skip and continue
     - Press `D` to delete incomplete files
   
2. **Search for models** - Press '/' to search

3. **Configure settings (optional)** - Press `o` to open options screen
   - Navigate with `j`/`k`
   - Edit directory: Press Enter, type path, Enter again
   - Edit HuggingFace Token: Press Enter, paste token, Enter again (required for gated models)
   - Adjust numbers: Press `+`/`-` (including download speed limit in MB/s)
   - Toggle options: Press `+`/`-` or Space (including rate limiting enable/disable)
   - Press Esc to close and save

4. **For gated models (Llama-3.1, Llama-2, etc.)**:
   - Get a HuggingFace token from: https://huggingface.co/settings/tokens
   - Accept model terms on the model's page (e.g., https://huggingface.co/meta-llama/Llama-3.1-8B)
   - Press `o` to open options, navigate to "HuggingFace Token", press Enter, paste token, press Enter again
   - Token is saved and will be used for all future downloads

5. **Type your query** (e.g., "gpt", "llama", "mistral")

6. **Press Enter** to search

7. **Navigate model results** with `j`/`k` or arrow keys (Models list is focused by default, yellow border)

8. **View quantization details** automatically as you select different models
   - Green `[downloaded]` indicator shows files you already have

9. **Press Tab** to switch focus to the Quantizations list (yellow border moves)

10. **Navigate quantizations** with `j`/`k` or arrow keys

11. **Press `d`** to download the selected quantization:
   - A popup will appear with the default path `~/models`
   - Edit the path if needed
   - Press Enter to confirm and start download
   - Files are saved to: `{path}/{author}/{model-name}/{filename}`
   - For multi-part GGUFs, all parts are queued automatically
   - Press Esc to cancel
   - Download progress appears in the top right corner with:
     - Progress percentage
     - Download speed (shows as "actual/limit MB/s" when rate limiting is enabled)
     - Queue count and total remaining size (e.g., "(2 queued) 120GB remaining")
     - Shows "<1GB remaining" for downloads under 1GB

12. **Press `v`** to verify a downloaded file (if SHA256 hash is available):
   - Verification runs in background with progress bar
   - Shows verification speed and percentage
   - Status shows success (✓) or hash mismatch (✗)

13. **Press Enter** to see full details of the selected item in the status bar

14. **Press Tab** again to return focus to the Models list

15. **Press `/`** to start a new search

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
  - Adaptive chunk sizing (targets ~20 chunks per file, 5MB-100MB range)
  - Parallel downloads with up to 8 concurrent chunks
  - Token bucket rate limiting with 2-second burst window
  - Real-time speed tracking (updated every 200ms during streaming)
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
    ├── config.rs           # Configuration persistence (v0.9.0)
    ├── utils.rs            # Formatting utilities
    ├── api.rs              # HuggingFace API client with auth (v0.9.5)
    ├── http_client.rs      # Authenticated HTTP requests (v0.9.5)
    ├── registry.rs         # Download registry persistence
    ├── download.rs         # Download manager & security
    ├── rate_limiter.rs     # Token bucket rate limiter (v1.2.0)
    ├── verification.rs     # SHA256 verification worker
    └── ui/
        ├── mod.rs          # UI module declaration
        ├── app.rs          # Module re-exports (v0.9.5)
        ├── app/            # App submodules (v0.9.5)
        │   ├── state.rs        # AppState initialization
        │   ├── events.rs       # Event handling
        │   ├── models.rs       # Model browsing logic
        │   ├── downloads.rs    # Download management
        │   └── verification.rs # Verification UI
        └── render.rs       # TUI rendering logic
```

**Version 0.7.0** introduces a modular architecture with clear separation of concerns:
- **6 top-level modules** for business logic
- **2 UI submodules** for presentation layer
- **~240 lines average** per file (previously 2,074 in one file)
- **Improved maintainability, testability, and readability**

**Version 0.9.5** further refines the architecture:
- **Split app.rs** into 5 focused submodules (~250 lines each)
- **New http_client module** for authentication
- **Better code organization** with clear responsibility separation

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
- `sha2`: SHA256 hash calculation
- `hex`: Hex encoding for hash display
- `once_cell`: Lazy static initialization for rate limiter

## Security

Key security features in v0.6.0:
- ✅ Path traversal protection with comprehensive validation
- ✅ Sanitization of all user inputs and API responses
- ✅ Canonicalization checks for download paths

## Changelog

### Version 1.2.1 (2026-01-07)
- **Enhancement**: Download progress now displays total remaining size
- **UI Improvement**: Progress box title shows combined size of current + queued downloads
- **Display Format**: "Downloading (2 queued) 120GB remaining" or "Downloading <1GB remaining"
- **Implementation**: Extended download message tuple to include file size tracking
- **Files Modified**: 4 files (state.rs, app.rs, downloads.rs, render.rs)
- **No Breaking Changes**: All existing functionality preserved
- See [changelog/RELEASE_NOTES_1.2.1.md](changelog/RELEASE_NOTES_1.2.1.md) for full details

### Version 1.2.0 (2026-01-07)
- **Feature**: Download speed rate limiting with token bucket algorithm
- **New Module**: `src/rate_limiter.rs` for bandwidth control
- **Configuration Options**: Two new settings in options screen
  - Rate Limit toggle (field 10): Enable/disable rate limiting
  - Max Download Speed (field 11): Adjust speed from 0.1 to 1000.0 MB/s (±0.5 MB/s increments)
- **UI Enhancement**: Progress display shows "actual/limit MB/s" when rate limiting is enabled
- **Technical Details**:
  - Token bucket implementation with fixed 2-second burst window
  - Global rate limiter shared across all concurrent download chunks
  - Zero overhead when disabled (atomic flag fast-path)
  - Dynamic rate adjustment without restart
- **Default Settings**: Disabled by default, 50.0 MB/s default limit when enabled
- **Dependency**: Added `once_cell` v1.19 for lazy static initialization
- See [changelog/RELEASE_NOTES_1.2.0.md](changelog/RELEASE_NOTES_1.2.0.md) for full details

### Version 1.1.1 (2025-12-16)
- **Bug Fix**: Fixed GGUF file path duplication issue for subdirectory downloads
- **Files Affected**: `src/ui/app/downloads.rs` - path construction logic in `confirm_download()` and `resume_incomplete_downloads()`
- **Issue**: Downloads with subdirectory paths (e.g., `UD-Q6_K_XL/model.gguf`) created double folder names
- **Solution**: Modified base_path calculation to exclude file subdirectory, preserving it only in filename
- **Verification**: Comprehensive audit of all download entry points completed
- **Migration**: Delete existing broken directories and rebuild with `cargo build --release`

### Version 1.0.0 (2025-11-27)
- **Major Change**: Removed trending models automatic loading on startup
- **Empty Screen**: App now starts with empty screen instead of 60 trending models
- **Search-Only**: Only normal API used for retrieving results (no special trending endpoint)
- **Faster Startup**: No network calls during application initialization
- **Updated UX**: Welcome message prompts user to search for models
- **Code Cleanup**: Removed all trending-related dead code (~58 lines)
- **Documentation**: Updated README and user guide
- **No Breaking Changes**: All existing features and configurations preserved
- See [changelog/RELEASE_NOTES_1.0.0.md](changelog/RELEASE_NOTES_1.0.0.md) for full details

### Version 0.9.7 (2025-11-25)
- **Critical Fix**: Fixed file path handling bugs causing incorrect file locations
- **Bug Fix #1**: Download worker now preserves subdirectory structure in filenames
- **Bug Fix #2**: Repository downloads calculate correct base path for each file
- **Compatibility**: Added clippy allow attribute for Rust 1.75.0 (Ubuntu 22.04)
- Files now save to correct locations: root files in model root, subdirectory files in subdirectories
- Example: `tokenizer/config.json` → `maya1/tokenizer/config.json` (not `maya1/config.json`)
- See [changelog/RELEASE_NOTES_0.9.7.md](changelog/RELEASE_NOTES_0.9.7.md) for full details

### Version 0.9.5 (2025-11-25)
- **Feature**: HuggingFace token authentication for gated models
- **401 Error Popup**: Clear guidance when authentication is required with model URL and setup instructions
- **New Module**: `src/http_client.rs` for authenticated HTTP requests
- **Code Refactoring**: Split monolithic `src/ui/app.rs` (~1107 lines) into 5 focused submodules:
  - `state.rs`: AppState initialization (~158 lines)
  - `events.rs`: Event handling (~709 lines)
  - `models.rs`: Model browsing logic (~253 lines)
  - `downloads.rs`: Download management (~460 lines)
  - `verification.rs`: Verification UI (~77 lines)
- **Token Configuration**: HF token stored in `~/.config/jreb/config.toml`
- **Options Screen**: New "HuggingFace Token" field for token management
- **Authenticated Downloads**: Token passed through download pipeline to all API calls
- **Code Quality**: Clippy warnings resolved, better maintainability
- See [changelog/RELEASE_NOTES_0.9.5.md](changelog/RELEASE_NOTES_0.9.5.md) for full details

### Version 0.9.0 (2025-11-25)
- **Feature**: Persistent configuration system with interactive options screen
- **Trending Models**: Automatically load 60 trending models on startup (2 pages in parallel)
- **Options Screen**: Press 'o' to customize all settings interactively
- **Configuration File**: Settings saved to `~/.config/jreb/config.toml`
- **Configurable Settings**: Download threads, chunk sizes, retry behavior, verification options
- **Auto-save/Auto-load**: Options persist across restarts with fallback to defaults
- **New Module**: `src/config.rs` for configuration management
- **API Enhancement**: Parallel fetching of trending models from HuggingFace
- **UI Improvements**: Interactive navigation with +/- keys, Enter to edit, Space to toggle
- See [changelog/RELEASE_NOTES_0.9.0.md](changelog/RELEASE_NOTES_0.9.0.md) for full details

### Version 0.8.0 (2025-11-23)
- **Feature**: SHA256 hash verification system
- **Automatic Verification**: Downloads automatically verify integrity after completion
- **Manual Verification**: Press 'v' to verify any downloaded file
- **Multi-part Support**: All parts of split GGUF files are verified individually
- **Progress Tracking**: Real-time verification progress bars with speed indicators
- **Status Display**: Visual feedback with ✓ (success) or ✗ (hash mismatch)
- **Registry Enhancement**: Added `expected_sha256` field to download metadata
- **New States**: Three download states - Complete, Incomplete, HashMismatch
- **New Module**: `src/verification.rs` with background verification worker
- **Dependencies**: Added `sha2` and `hex` for hash calculation
- See [changelog/RELEASE_NOTES_0.8.0.md](changelog/RELEASE_NOTES_0.8.0.md) for full details

### Version 0.7.5 (2025-11-23)
- **Performance**: Adaptive chunk sizing for optimal download performance across all file sizes
- **Enhancement**: Real-time continuous speed tracking (updated every 200ms during streaming)
- **UI Improvement**: Added bordered container for chunk progress display
- Target ~20 chunks per file with 5MB-100MB size bounds
- 90% reduction in task overhead for large files (>10GB)
- Better parallelism for small files (<100MB)
- See [changelog/RELEASE_NOTES_0.7.5.md](changelog/RELEASE_NOTES_0.7.5.md) for full details

### Version 0.7.2 (2025-11-22)
- Fixed quantization folder duplication issue in download paths
- Improved local file path handling for quantization subdirectories
- See [changelog/RELEASE_NOTES_0.7.2.md](changelog/RELEASE_NOTES_0.7.2.md) for details

### Version 0.7.1 (2025-11-22)
- Fixed quantization folder duplication issue
- See [changelog/RELEASE_NOTES_0.7.1.md](changelog/RELEASE_NOTES_0.7.1.md) for details

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
