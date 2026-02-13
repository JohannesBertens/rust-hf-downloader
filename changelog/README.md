# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## [1.4.0] - 2026-02-13

### Version 1.4.0 (2026-02-13)
- **Performance**: Optimized verification progress tracking with AtomicU64 and Entry API
- **Performance**: Simplified mutex usage with AtomicUsize
- **Refactoring**: Reduced lock contention in verification progress updates
- **Files Modified**: `src/verification.rs` (AtomicU64 for progress tracking)
- **No Breaking Changes**: All existing functionality preserved

## [1.3.2] - 2026-01-21

### Version 1.3.2 (2026-01-21)
- **Version bump**: Updated to version 1.3.2
- **No Breaking Changes**: All existing functionality preserved

### Version 1.3.1 (2026-01-21)
- **Enhancement**: Added support for F16 (half-precision floating point) quantization type
- **Enhancement**: Added support for TQ (Tensor Quantization) for TriLMs/BitNet ternary packing
- **Fixed**: Missing F16 quantization detection for models with F16 in root directory
- **Fixed**: Missing TQ quantization detection (TQ1_0, TQ2_0, etc.)
- **Impact**: All quantization types now correctly identified and listed for download
- **Files Modified**: `src/api.rs` (33 lines added)
- **No Breaking Changes**: All existing functionality preserved
- See [RELEASE_NOTES_1.3.1.md](RELEASE_NOTES_1.3.1.md) for full details

### Version 1.2.2 (2026-01-08)
- **Bug Fix**: Fixed color contrast issue on light terminal backgrounds
- **Visibility**: Status bar and UI text now adapt to terminal theme
- **Issue Resolution**: [#16 - Empty screen](https://github.com/JohannesBertens/rust-hf-downloader/issues/16)
- **Problem**: Hard-coded white text was invisible on white/light terminal backgrounds
- **Solution**: Changed to terminal-default colors for automatic theme adaptation
- **Impact**: UI now visible on all terminal color schemes (dark, light, custom)
- **Files Modified**: `src/ui/render.rs` (1 line)
- **No Breaking Changes**: All existing functionality preserved
- See [RELEASE_NOTES_1.2.2.md](RELEASE_NOTES_1.2.2.md) for full details

### Version 1.2.1 (2026-01-07)
- **Enhancement**: Download progress now displays total remaining size
- **UI Improvement**: Progress box title shows combined size of current + queued downloads
- **Display Format**: "Downloading (2 queued) 120GB remaining" or "Downloading <1GB remaining"
- **Implementation**: Extended download message tuple to include file size tracking
- **Files Modified**: 4 files (state.rs, app.rs, downloads.rs, render.rs)
- **No Breaking Changes**: All existing functionality preserved
- See [RELEASE_NOTES_1.2.1.md](RELEASE_NOTES_1.2.1.md) for full details

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
- See [RELEASE_NOTES_1.2.0.md](RELEASE_NOTES_1.2.0.md) for full details

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
- See [RELEASE_NOTES_1.0.0.md](RELEASE_NOTES_1.0.0.md) for full details

### Version 0.9.7 (2025-11-25)
- **Critical Fix**: Fixed file path handling bugs causing incorrect file locations
- **Bug Fix #1**: Download worker now preserves subdirectory structure in filenames
- **Bug Fix #2**: Repository downloads calculate correct base path for each file
- **Compatibility**: Added clippy allow attribute for Rust 1.75.0 (Ubuntu 22.04)
- Files now save to correct locations: root files in model root, subdirectory files in subdirectories
- Example: `tokenizer/config.json` → `maya1/tokenizer/config.json` (not `maya1/config.json`)
- See [RELEASE_NOTES_0.9.7.md](RELEASE_NOTES_0.9.7.md) for full details

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
- See [RELEASE_NOTES_0.9.5.md](RELEASE_NOTES_0.9.5.md) for full details

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
- See [RELEASE_NOTES_0.9.0.md](RELEASE_NOTES_0.9.0.md) for full details

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
- See [RELEASE_NOTES_0.8.0.md](RELEASE_NOTES_0.8.0.md) for full details

### Version 0.7.5 (2025-11-23)
- **Performance**: Adaptive chunk sizing for optimal download performance across all file sizes
- **Enhancement**: Real-time continuous speed tracking (updated every 200ms during streaming)
- **UI Improvement**: Added bordered container for chunk progress display
- Target ~20 chunks per file with 5MB-100MB size bounds
- 90% reduction in task overhead for large files (>10GB)
- Better parallelism for small files (<100MB)
- See [RELEASE_NOTES_0.7.5.md](RELEASE_NOTES_0.7.5.md) for full details

### Version 0.7.2 (2025-11-22)
- Fixed quantization folder duplication issue in download paths
- Improved local file path handling for quantization subdirectories
- See [RELEASE_NOTES_0.7.2.md](RELEASE_NOTES_0.7.2.md) for details

### Version 0.7.1 (2025-11-22)
- Fixed quantization folder duplication issue
- See [RELEASE_NOTES_0.7.1.md](RELEASE_NOTES_0.7.1.md) for details

### Version 0.7.0 (2025-11-21)
- **Major Refactoring**: Complete modular architecture overhaul
- Split monolithic 2,074-line `main.rs` into 9 focused modules
- Created 6 top-level modules: `models`, `utils`, `api`, `registry`, `download`, `ui`
- Created 2 UI submodules: `app` (state/logic) and `render` (presentation)
- Improved maintainability, testability, and readability
- Average file size reduced to ~240 lines per module
- No breaking changes - purely internal refactoring
- See [RELEASE_NOTES_0.7.0.md](RELEASE_NOTES_0.7.0.md) for full details

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
- See [RELEASE_NOTES_0.6.0.md](RELEASE_NOTES_0.6.0.md) for details

### Older Versions

For detailed release notes of older versions, see the [changelog directory](.).

[Unreleased]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.4.0...main
[1.4.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.3.2...v1.4.0
[1.3.2]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.3.1...v1.3.2
[1.3.1]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.3.0...v1.3.1
[1.2.2]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.2.1...v1.2.2
[1.2.1]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v1.0.0...v1.1.1
[1.0.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.9.7...v1.0.0
[0.9.7]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.9.5...v0.9.7
[0.9.5]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.9.0...v0.9.5
[0.9.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.7.5...v0.8.0
[0.7.5]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.7.0...v0.7.5
[0.7.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/JohannesBertens/rust-hf-downloader/compare/v0.5.0...v0.6.0
