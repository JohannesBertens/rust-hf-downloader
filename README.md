# Rust HF Downloader v2.3.0

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
- 🤖 **Scriptable CLI**: One-shot `download` subcommand with human or JSON Lines output (`--json`), stable exit codes, and `HF_ENDPOINT` mirror/test override — built for scripts and AI-agent skills (see [CLI Mode](#cli-mode-one-shot-download))
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

## Table of Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [TUI Mode (Interactive)](#tui-mode-interactive)
- [CLI Mode (One-shot Download)](#cli-mode-one-shot-download)
- [Technical Details](#technical-details)
- [Changelog](#changelog)
- [License](#license)

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

## TUI Mode (Interactive)

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

## CLI Mode (One-shot Download)

Running the binary **without arguments launches the TUI** as before. A single
non-interactive subcommand is available for scripts, cron jobs, and AI-agent
skills:

```bash
# Search HuggingFace (query-only; table or JSON array)
rust-hf-downloader search "qwen 2.5 gguf" --sort downloads --limit 20
rust-hf-downloader search "mistral gguf" --min-downloads 1000 --json | jq '.[0].id'

# Download a specific quantization
rust-hf-downloader download bartowski/Qwen2.5-7B-GGUF --quant Q4_K_M

# Exact file(s), whole repo, custom destination
rust-hf-downloader download org/model --file README.md --file config.json
rust-hf-downloader download org/model --all -o /data/models

# Machine-readable output for scripts and agents
rust-hf-downloader download org/model --quant Q4_K_M --json | jq -c 'select(.type=="progress")'
```

### Search

`search` is a query-only command — one bounded API call, no engine state:

```bash
rust-hf-downloader search "qwen 2.5 gguf" [--sort downloads|likes|modified|name]
                                        [--direction asc|desc]
                                        [--min-downloads N] [--min-likes N]
                                        [--limit N] [--json]
```

Unspecified flags fall back to the config defaults the TUI's filter toolbar
uses. Output rule: **queries emit one JSON document** (an array, `--json`),
**pipelines emit NDJSON events** (`download --json`) — on failure a single
`{"type":"error",…}` line is the only stdout output. A successful search
with zero results exits `0` with `[]` (scripts distinguish by array length).
The full-text `search` term itself is matched server-side by HuggingFace;
`--min-downloads`/`--min-likes` filter client-side, `--sort name` and
ascending sorts apply client-side too (the API only sorts descending).

### Selectors

| Selector | Meaning |
|---|---|
| *(none)* | Works only when the repo has exactly one downloadable file; otherwise exits 64 listing every file |
| `--quant Q4_K_M` | All files of that quantization (case-insensitive; includes every part of multi-part GGUFs) |
| `--file PATH` | Exact repo-relative path (repeatable) |
| `--all` | Everything in the repository |

Selectors are mutually exclusive. Ambiguity is never guessed: the failure
output includes the structured file list so an agent can pick a selector and
re-invoke in one round-trip.

### Other options

`-o/--output DIR` (default: config `default_directory`, usually `~/models`),
`--token TOKEN` (default `$HF_TOKEN`, then config), `--no-verify`,
`-q/--quiet`, and `HF_ENDPOINT` (base-URL override for mirrors such as
`https://hf-mirror.com` or local testing).

### Output and exit codes

Human mode prints progress to **stderr** (single-line rewrites when
interactive) and the summary to **stdout**. `--json` emits NDJSON events on
**stdout** — `resolved`, `download_start`, `progress` (500 ms throttle),
`file_complete`, `verification_start`, `verification_result`, `done` — and on
failure the `error` event is **always the last line**:

```json
{"type":"error","code":"ambiguous","message":"model has 2 downloadable file(s); …","available":[{"filename":"model-Q4_K_M.gguf","size_bytes":4947802324,"sha256":"…"}, …]}
```

| Exit code | Meaning |
|---|---|
| 0 | All files present (downloaded or already existed); verification OK or skipped |
| 1 | Download failed after retries, or a hash mismatch |
| 2 | Authentication required (`--token` / `$HF_TOKEN` / config token) |
| 64 | Usage error, unknown file, or ambiguous selection |
| 130 | Interrupted (Ctrl-C); unfinished files restart from scratch on the next run |

CLI downloads share the TUI's engine, config, and `~/models/hf-downloads.toml`
registry, so files downloaded headlessly show up in the TUI's resume/complete
views. Events and behavior are covered end-to-end by integration tests
against a mock HuggingFace server.

## Technical Details

### Architecture

- **Rust Edition**: 2021
- **Minimum Rust Version**: 1.75.0+ (Ubuntu 22.04 compatible)
- **TUI Framework**: [ratatui](https://github.com/ratatui/ratatui)
- **HTTP Client**: reqwest with async support and streaming downloads
- **TLS Backend**: rustls (pure Rust TLS implementation)
- **API**: HuggingFace REST API (`https://huggingface.co/api/models`); override the base URL with `HF_ENDPOINT` (mirrors, testing)
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
    ├── main.rs             # Entry point (TUI by default; `download` subcommand dispatch)
    ├── cli.rs              # One-shot download CLI (args, resolution, reporters)
    ├── engine.rs           # Shared download engine (manager + verification bootstrap)
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

| Version | Date | Summary |
|---------|------|---------|
| [2.3.0] | 2026-09-24 | CLI: one-shot `download` + query-only `search` subcommands (JSON output, exit codes, HF_ENDPOINT); shared download engine extracted |
| [2.2.0] | 2026-09-24 | Activity HUD: compact matrix queue view (chunks, parallel verification, queue) |
| [2.1.0] | 2026-09-24 | Verification: truthful progress/ETA, blocking-thread hashing, 4-way concurrency |
| [2.0.0] | 2026-09-23 | Breaking: removed headless/CLI mode; TUI-only binary; snapshot test suite |
| [1.4.0] | 2026-02-13 | Optimized verification progress with AtomicU64 and cache Entry API |
| [1.3.2] | 2026-01-21 | Exact model match for repository ID searches |
| [1.3.1] | 2026-01-21 | Added F16 and TQ quantization support |
| [1.2.2] | 2026-01-08 | Fixed color contrast on light terminals |
| [1.2.1] | 2026-01-07 | Download progress shows total remaining size |
| [1.2.0] | 2026-01-07 | Download speed rate limiting (token bucket) |
| [1.1.1] | 2025-12-16 | Fixed GGUF path duplication for subdirectories |
| [1.0.0] | 2025-11-27 | Removed trending models, search-only startup |
| [0.9.7] | 2025-11-25 | Fixed file path handling bugs |
| [0.9.5] | 2025-11-25 | HuggingFace token authentication for gated models |
| [0.9.0] | 2025-11-25 | Persistent configuration system with options screen |
| [0.8.0] | 2025-11-23 | SHA256 hash verification system |
| [0.7.5] | 2025-11-23 | Adaptive chunk sizing for downloads |
| [0.7.0] | 2025-11-21 | Complete modular architecture overhaul |
| [0.6.2] | 2025-11-21 | Switched to rustls for TLS |
| [0.6.0] | 2024-11-21 | Fixed path traversal vulnerability |

See [changelog/README.md](changelog/README.md) for detailed release notes.

## License

Copyright (c) Johannes Bertens

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
