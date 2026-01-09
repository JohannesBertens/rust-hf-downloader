# Headless Mode Implementation Plan

## Overview
Implement command-line arguments to run `rust-hf-downloader` in a headless mode without TUI, suitable for automated/CI environments.

## Current Architecture Analysis
- **Entry Point**: `main.rs` - Initializes TUI with `ratatui::init()` and runs `ui::App::new().run()`
- **UI Framework**: ratatui (TUI) with crossterm for terminal handling
- **Download Logic**: Fully async in `download.rs` with progress tracking via channels
- **API Client**: Reusable functions in `api.rs` (search, fetch models, download files)
- **Configuration**: TOML-based config system in `config.rs`

## Design Approach: CLI Argument Flag
Add a `--headless` flag that bypasses TUI initialization and provides a simple command-line interface.

### Proposed CLI Arguments
```bash
# Search and list models (no download)
rust-hf-downloader --headless search "llama"

# Download specific model files
rust-hf-downloader --headless download <model_id> --quantization <type> --output <dir>

# Download all files from a model
rust-hf-downloader --headless download <model_id> --all --output <dir>

# Resume incomplete downloads
rust-hf-downloader --headless resume

# List available quantizations for a model
rust-hf-downloader --headless list <model_id>
```

## Implementation Phases

### Phase 1: Add CLI Parsing Library
**Status**: ✅ Completed
**File**: `plans/implementation/add-headless-phase1.md`
**Estimated Time**: 2 hours
**Actual Time**: 2 hours

- [x] Add `clap` dependency to `Cargo.toml`
- [x] Create `src/cli.rs` with CLI argument definitions
- [x] Define `Command` enum (Search, Download, List, Resume)
- [x] Add argument parsing with derive macros
- [x] Test CLI argument parsing

### Phase 2: Extract Core Logic from UI
**Status**: ✅ Complete
**File**: `plans/implementation/add-headless-phase2.md`
**Estimated Time**: 2 hours
**Actual Time**: 2 hours
**Estimated Time**: 2 hours
**Actual Time**: 2 hours

- [x] Create `src/headless.rs` module
- [x] Implement `search_models()` function
- [x] Implement `list_quantizations()` function
- [x] Implement `download_model()` function
- [x] Implement `resume_downloads()` function
- [x] Add headless progress reporting functions
- [x] Refactor `main.rs` for early headless detection

### Phase 3: Implement Headless Commands
**Status**: ✅ Complete
**File**: `plans/implementation/add-headless-phase3.md`
**Estimated Time**: 4 hours
**Actual Time**: 2.5 hours
**Estimated Time**: 4 hours

- [x] Implement search command (JSON + table output)
- [x] Implement download command with quantization filter
- [x] Implement download command with --all flag
- [x] Implement list command (show available files)
- [x] Implement resume command (batch resume)
- [x] Test each command individually

### Phase 4: Progress & Error Handling
**Status**: 📋 Not Started
**File**: `plans/implementation/add-headless-phase4.md`
**Estimated Time**: 2 hours

- [ ] Replace UI progress bars with console output
- [ ] Reuse `status_tx` channel for headless messages
- [ ] Implement proper exit codes (0=success, 1=error, 2=auth)
- [ ] Add graceful error handling for missing files
- [ ] Preserve existing retry logic in headless mode
- [ ] Test error scenarios

### Phase 5: Testing & Documentation
**Status**: 📋 Not Started
**File**: `plans/implementation/add-headless-phase5.md`
**Estimated Time**: 1 hour

- [ ] Add `--dry-run` flag for testing
- [ ] Test all commands with real models
- [ ] Verify config loading in headless mode
- [ ] Update README.md with headless examples
- [ ] Document all CLI flags and exit codes
- [ ] Add CI/CD usage examples

## Key Technical Decisions

### Why `clap`?
- Mature, well-documented CLI library
- Derive macros for clean argument definition
- Built-in help text generation
- Widely used in Rust ecosystem

### Why New `headless.rs` Module?
- **Separation of Concerns**: Keep UI code isolated
- **Reusability**: Core download logic remains unchanged
- **Maintainability**: Headless-specific code (console output, CLI parsing) separate from TUI

### Why JSON Output Option?
- Enables scripting/automation
- Easy parsing by other tools
- Optional (default: human-readable)

## File Changes Summary

### New Files
- `src/cli.rs` - CLI argument definitions with clap
- `src/headless.rs` - Headless mode logic

### Modified Files
- `Cargo.toml` - Add `clap` dependency
- `src/main.rs` - Add CLI parsing, early headless detection
- `src/api.rs` - No changes (already reusable)
- `src/download.rs` - No changes (already async)
- `src/config.rs` - No changes (already works without UI)

## Benefits

### Zero Breaking Changes
- TUI mode remains unchanged
- All existing functionality preserved
- Opt-in via `--headless` flag

### Code Reuse
- 95% of logic shared between modes
- No duplication of download/API logic
- Single codebase to maintain

### CI/CD Ready
- Easy integration into automated pipelines
- Scriptable via JSON output
- Proper exit codes for automation

## Example Usage After Implementation

```bash
# Automated download script
#!/bin/bash
rust-hf-downloader --headless download "meta-llama/Llama-3.1-8B" \
  --quantization "Q4_K_M" \
  --output "/models" \
  --token "$HF_TOKEN"

# List and grep for specific quantization
rust-hf-downloader --headless list "TheBloke/llama-2-7b-GGUF" | grep Q4

# Search popular models
rust-hf-downloader --headless search "llama" \
  --min-downloads 10000 \
  --min-likes 100 \
  --json | jq '.[].id'

# Resume all incomplete downloads
rust-hf-downloader --headless resume
```

## Progress Tracking

### Overall Status
- **Phase 1**: ✅ Completed (5/5 tasks)
- **Phase 2**: 📋 Not Started (0/6 tasks)
- **Phase 3**: 📋 Not Started (0/6 tasks)
- **Phase 4**: 📋 Not Started (0/6 tasks)
- **Phase 5**: 📋 Not Started (0/6 tasks)

### Total Progress
- **Tasks Completed**: 5/29 (17%)
- **Estimated Time Remaining**: ~9 hours
- **Current Phase**: Phase 2 - Extract Core Logic from UI

## Dependencies
- Rust 1.75.0+ (already required)
- `clap` v4.x (new dependency)
- All existing dependencies remain unchanged

## Testing Strategy
1. Start with small models for faster testing
2. Test each command in isolation
3. Verify TUI mode still works
4. Test error scenarios (auth failures, network errors)
5. Validate JSON output format

## Rollback Plan
If critical issues are found:
- Revert `main.rs` to remove headless detection
- Keep `src/cli.rs` and `src/headless.rs` (can be disabled)
- No changes to core download/API logic
- TUI mode unaffected
