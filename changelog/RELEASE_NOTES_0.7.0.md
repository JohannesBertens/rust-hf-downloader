# Release Notes - Version 0.7.0

**Release Date**: November 21, 2025

## 🏗️ Major Refactoring: Modular Architecture

Version 0.7.0 represents a significant architectural improvement with a complete refactoring of the codebase from a monolithic structure to a well-organized modular design.

### Before vs After

**Before:**
- Single `main.rs` file with 2,074 lines
- Mixed concerns (UI, API, download logic, data models, utilities)
- Difficult to maintain, test, and extend

**After:**
- 9 files across 6 modules with clear separation of concerns
- Average file size: ~240 lines
- Improved maintainability, testability, and readability

### New Module Structure

```
src/
├── main.rs (15 lines)        # Minimal entry point
├── models.rs (87 lines)      # Data structures & types
├── utils.rs (25 lines)       # Formatting utilities
├── api.rs (336 lines)        # HuggingFace API client
├── registry.rs (50 lines)    # Download registry persistence
├── download.rs (407 lines)   # Download manager & path validation
└── ui/
    ├── mod.rs (4 lines)      # UI module declaration
    ├── app.rs (790 lines)    # Application state & event handling
    └── render.rs (458 lines) # TUI rendering logic
```

### Key Improvements

1. **Maintainability**
   - Clear separation of concerns
   - Each module has a single, well-defined responsibility
   - Easier to locate and fix bugs

2. **Testability**
   - Modules can be tested independently
   - Business logic separated from UI code
   - API and download logic can be unit tested

3. **Readability**
   - Smaller, focused files (~200-400 lines vs 2,074)
   - Clear module boundaries
   - Self-documenting structure

4. **Reusability**
   - API client (`api.rs`) can be imported by other projects
   - Download manager (`download.rs`) is standalone
   - Path validation utilities can be reused

5. **Collaboration**
   - Multiple developers can work on different modules
   - Reduced merge conflicts
   - Clear ownership boundaries

### Module Descriptions

#### `src/main.rs` (15 lines)
Entry point that initializes the application and delegates to the UI module.

#### `src/models.rs` (87 lines)
All data structures and type definitions:
- `ModelInfo` - HuggingFace model metadata
- `QuantizationInfo` - Quantization file information
- `DownloadMetadata` - Download tracking data
- `DownloadRegistry` - Persistent download history
- Enums: `DownloadStatus`, `PopupMode`, `InputMode`, `FocusedPane`

#### `src/utils.rs` (25 lines)
Utility functions for formatting:
- `format_number()` - Human-readable numbers (1.2M, 500K)
- `format_size()` - Human-readable file sizes (2.5 GB, 100 MB)

#### `src/api.rs` (336 lines)
HuggingFace API client:
- `fetch_models()` - Search for models
- `fetch_model_files()` - Get model file tree
- `extract_quantization_type()` - Parse quantization info
- `parse_multipart_filename()` - Multi-part GGUF detection
- Helper functions for quantization directories and base names

#### `src/registry.rs` (50 lines)
Download registry management:
- `load_registry()` - Read TOML registry from disk
- `save_registry()` - Persist registry to disk
- `get_incomplete_downloads()` - Find resumable downloads
- `get_complete_downloads()` - Track downloaded files

#### `src/download.rs` (407 lines)
Download manager with security features:
- `start_download()` - Main download coordinator
- `download_with_resume()` - HTTP download with resume support
- `validate_and_sanitize_path()` - Path traversal protection
- `sanitize_path_component()` - Component-level validation
- Retry logic with transient error detection

#### `src/ui/app.rs` (790 lines)
Application state and event handling:
- `App` struct with all application state
- Event loop and keyboard input handling
- Model search and selection logic
- Download queue management
- Resume/delete incomplete downloads

#### `src/ui/render.rs` (458 lines)
TUI rendering logic:
- `render_ui()` - Main UI layout
- `render_progress_bar()` - Download progress widget
- `render_resume_popup()` - Incomplete downloads dialog
- `render_download_path_popup()` - Path input dialog

### Breaking Changes

None. This is purely an internal refactoring with no changes to:
- User interface or controls
- API behavior
- Download functionality
- Configuration files

### Migration Notes

No migration required. Users can upgrade seamlessly from 0.6.x to 0.7.0.

### Testing

- ✅ All modules compile successfully
- ✅ No warnings in `cargo check`
- ✅ Builds successfully with `cargo build`
- ✅ All existing functionality preserved

### Future Benefits

This refactoring enables:
- **Easier testing**: Unit tests can be added per module
- **Library extraction**: API and download modules can become a separate crate
- **Plugin architecture**: New features can be added as modules
- **Performance optimization**: Individual modules can be profiled and optimized
- **Documentation**: Each module can have focused documentation

## Technical Details

- **Rust Edition**: 2021
- **Minimum Rust Version**: 1.75.0+
- **Lines of Code**: 2,172 total (previously 2,074 in single file)
- **Modules**: 6 top-level modules + 2 UI submodules

## Contributors

- Johannes Bertens ([@JohannesBertens](https://github.com/JohannesBertens))

---

For the complete version history, see the [changelog directory](../changelog/).
