# Architecture Simplification Plan

## Executive Summary

The rust-hf-downloader codebase has evolved organically over multiple versions, resulting in:
- **~8,000 lines** of Rust code with significant duplication
- **Parallel code paths** between TUI and headless modes
- **Scattered utilities** that should be centralized
- **Inconsistent error handling** patterns

This plan identifies specific areas of duplication and proposes a consolidated architecture.

---

## Code Scan Results: Complete Duplication Inventory

### Identified Duplications (Full List)

| Area | Location 1 | Location 2 | Lines |
|------|------------|------------|-------|
| **File Size Formatting** | `headless.rs:82` `format_file_size()` | `utils.rs:11` `format_size()` | ~20 |
| **Duration Formatting** | `headless.rs:100` `format_duration()` | NOT in utils.rs | ~15 |
| **Model ID Validation** | `headless.rs:112` `validate_model_id()` | `download.rs:67` `validate_and_sanitize_path()` | ~15 |
| **Status Update Sending** | `download.rs:21+` ~15 instances | `headless.rs:193+` ~10 instances | ~250 |
| **Registry Save Calls** | `download.rs:257,332,398,419,585` | `ui/app/downloads.rs:241,400,495` | 8 sites |
| **Config Load** | `config.rs:21` `load_config()` | Used in 5+ locations | DRY ✓ |
| **Token Handling** | 15+ scattered `hf_token` references | Inconsistent patterns | N/A |
| **GGUF Detection** | `api.rs:135` `has_gguf_files()` | Used in 4 locations | Good reuse |
| **Quantization Parsing** | `api.rs:516` `extract_quantization_type()` | Used extensively | Good reuse |
| **File Tree Building** | `api.rs:143` `build_file_tree()` | Used in 3 locations | Good reuse |
| **Gated Model Check** | `headless.rs:394` `check_gated_model()` | Only in headless | Potential reuse |
| **Retry Logic** | `download.rs:298-406` | Only in download | Could generalize |
| **Rate Limiter** | `rate_limiter.rs` | Used in both modes | Good |

---

## Detailed Duplication Patterns

### Pattern 1: Format Functions

**File Size (DUPLICATE - CRITICAL)**
```rust
// headless.rs:82 (DUPLICATE - should be removed)
pub fn format_file_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    // ... identical logic
}

// utils.rs:11 (CORRECT VERSION)
pub fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    // ... identical logic
}
```
**Fix**: Delete `headless.rs::format_file_size()`, redirect callers to `utils::format_size()`

**Duration (MISSING)**
```rust
// headless.rs:100 (NOT in utils.rs)
pub fn format_duration(duration: std::time::Duration) -> String { ... }
```
**Fix**: Move to `utils.rs` as `format_duration()`

---

### Pattern 2: Status/Progress Message Sending

**Download Status (SCATTERED - 15+ instances)**

| File | Line | Pattern |
|------|------|---------|
| `download.rs` | 157, 167, 182, 190, 203, 218, 224, 234, 284, 286, 345, 350, 356, 359, 368, 385, 406 | `status_tx.send(format!(...))` |
| `headless.rs` | 242, 268, 516, 543 | `progress_tx.send(format!(...))` |
| `verification.rs` | 90, 109, 122, 124, 144 | `status_tx.send(format!(...))` |

**Pattern**: Every module has its own error/status message formatting
**Fix**: Create `core/reporter.rs` with `Reporter::info()`, `Reporter::error()`, `Reporter::success()`

---

### Pattern 3: Token Management (INCONSISTENT)

```rust
// In headless.rs - token extraction pattern
let token = hf_token.or(options.hf_token);

// In ui/app/models.rs - token from options
let token = self.options.hf_token.clone();

// In main.rs - token from CLI args
cli_args.token.as_ref()

// In http_client.rs - token validation
let has_token = token.is_some_and(|t| !t.is_empty());
```

**Inconsistency**: Token is passed around as `Option<&String>`, `Option<String>`, `String`, creating friction
**Fix**: Unified `Client` struct that manages token internally

---

### Pattern 4: Registry Save Pattern (DUPLICATED)

**8 identical call sites**:
```rust
// Pattern used everywhere:
registry::save_registry(&registry);
// After: download.rs:257, 332, 398, 419, 585
// After: ui/app/downloads.rs:241, 400, 495
// After: verification.rs:140
```

**Problem**: Every download/verification step saves registry
**Fix**: Registry auto-saves on state change (Observer pattern) or batch save

---

### Pattern 5: Model ID Validation (DUPLICATE)

```rust
// headless.rs:112
pub fn validate_model_id(model_id: &str) -> Result<(), HeadlessError> {
    if !model_id.contains('/') { return Err(...) }
    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() { return Err(...) }
    Ok(())
}

// download.rs:67 (similar validation logic)
pub fn validate_and_sanitize_path(...) {
    let model_parts: Vec<&str> = model_id.split('/').collect();
    if model_parts.len() != 2 { return Err(...) }
    // ...
}
```

**Fix**: Consolidate into `utils::validate_model_id()` with clear error messages

---

### Pattern 6: GGUF Detection (GOOD - REUSE)

```rust
// api.rs:135 - Well extracted
pub fn has_gguf_files(metadata: &ModelMetadata) -> bool { ... }

// Used in:
headless.rs:205, 379, 452, 750
ui/app/models.rs:262, 451
```

**Status**: ✅ Good - no duplication needed

---

### Pattern 7: Quantization Parsing (GOOD - REUSE)

```rust
// api.rs:516 - Well extracted
pub fn extract_quantization_type(filename: &str) -> Option<String> { ... }

// Helpers also extracted:
pub fn parse_multipart_filename(filename: &str) -> Option<(u32, u32)> { ... }
pub fn is_quantization_directory(dirname: &str) -> bool { ... }
pub fn get_multipart_base_name(filename: &str) -> String { ... }
```

**Status**: ✅ Good - no duplication needed

---

### Pattern 8: File Tree Building (GOOD - REUSE)

```rust
// api.rs:143 - Well extracted
pub fn build_file_tree(files: Vec<RepoFile>) -> FileTreeNode { ... }

// Used in:
headless.rs:1080, 1221
ui/app/models.rs:322, 473
```

**Status**: ✅ Good - no duplication needed

---

### Pattern 9: Gated Model Check (DUPLICATE CANDIDATE)

```rust
// headless.rs:394 - Only used in headless mode
fn check_gated_model(
    metadata: &ModelMetadata,
    hf_token: &Option<String>,
) -> Result<(), HeadlessError> {
    let is_gated = match &metadata.gated {
        serde_json::Value::Bool(true) => true,
        serde_json::Value::String(s) if s == "auto" || s == "manual" => true,
        _ => false,
    };
    if is_gated && hf_token.is_none() || hf_token.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
        return Err(HeadlessError::AuthRequired(...));
    }
    Ok(())
}
```

**Problem**: TUI also needs this check but doesn't use it
**Fix**: Move to `api.rs` as `check_gated_model()` returning `Result<(), AuthError>`

---

### Pattern 10: Config Loading (GOOD - REUSE)

```rust
// config.rs:21 - Single source
pub fn load_config() -> AppOptions { ... }

// Used in main.rs:186, ui/app/state.rs:105, headless.rs:196
```

**Status**: ✅ Good - single source of truth

---

### Pattern 11: Retry Logic (SINGLE - COULD GENERALIZE)

```rust
// download.rs:298-406 - Only in download module
let mut retries = DOWNLOAD_CONFIG.max_retries.load(Ordering::Relaxed);
loop {
    match download().await {
        Ok(_) => break,
        Err(e) if retries > 0 && is_transient_error(&e) => {
            retries -= 1;
            tokio::time::sleep(Duration::from_secs(retry_delay)).await;
        }
        Err(e) => return Err(e),
    }
}
```

**Potential**: Could be generalized to `retry_async()` utility

---

### Pattern 12: Verification Worker (DUPLICATE POTENTIAL)

```rust
// verification.rs:48-69 - Verification worker
// Used in main.rs:67 and ui/app.rs:40 (TUI)
```

**Status**: ✅ Good - already reused via function call

---

## Summary Statistics

| Category | Count | Lines Affected |
|----------|-------|----------------|
| **Duplicates (FIX)** | 4 functions | ~80 lines |
| **Scattered patterns (REFACTOR)** | 5 patterns | ~400 lines |
| **Good reuse (KEEP)** | 6 patterns | N/A |
| **Missing utilities (ADD)** | 1 function | ~15 lines |

### Module Complexity

| Module | Lines | Concerns |
|--------|-------|----------|
| `ui/app/events.rs` | 1100+ | Complex event handling, mouse + keyboard coalescing |
| `ui/render.rs` | 1500+ | All rendering logic, UI state |
| `headless.rs` | 1300+ | Complete CLI implementation |
| `download.rs` | 850+ | Download orchestration + validation |

---

## Proposed Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Unified Core Layer (NEW)                             │
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │                    api/client.rs (MERGE api.rs + http_client.rs)        ││
│  │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐ ││
│  │  │ Client::new()   │  │ fetch_models()  │  │ fetch_quantizations()   │ ││
│  │  │ with_token(opt) │  │ fetch_metadata()│  │ with caching & retries  │ ││
│  │  └─────────────────┘  └─────────────────┘  └─────────────────────────┘ ││
│  └─────────────────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │                    core/download.rs (MERGE with download.rs)            ││
│  │  ┌──────────────┐  ┌───────────────┐  ┌─────────────────────────────┐  ││
│  │  │ Downloader   │  │ PathValidator │  │ VerificationWorker         │  ││
│  │  │::new(token)  │  │::sanitize()   │  │ (reusable for TUI + CLI)   │  ││
│  │  └──────────────┘  └───────────────┘  └─────────────────────────────┘  ││
│  └─────────────────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │                    core/registry.rs (IMPROVE)                           ││
│  │  ┌─────────────────┐  ┌────────────────┐  ┌──────────────────────────┐ ││
│  │  │ Registry::load()│  │ Registry::save()│  │ Registry::get_progress()│ ││
│  │  └─────────────────┘  └────────────────┘  └──────────────────────────┘ ││
│  └─────────────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                    ┌───────────────┼───────────────┐
                    │               │               │
         ┌──────────▼──────┐ ┌──────▼──────┐ ┌──────▼──────┐
         │    cli/mod.rs   │ │  tui/mod.rs │ │ core/errors │
         │ (NEW - commands)│ │ (REFACTOR)  │ │ (NEW - unify│
         └─────────────────┘ └─────────────┘ └─────────────┘
                    │               │
         ┌──────────▼──────┐ ┌──────▼──────┐
         │ run_search()    │ │ App::run()  │
         │ run_download()  │ │ App::exec() │
         │ run_list()      │ │             │
         │ run_resume()    │ │             │
         └─────────────────┘ └─────────────┘
```

---

## Phase 1: Consolidate HTTP/API Layer

### Problem
`api.rs` and `http_client.rs` contain overlapping functionality. Both TUI and headless modes call `api::fetch_*` functions.

### Solution: Create `api::Client`

```rust
// NEW: api/client.rs
pub struct Client {
    client: Option<reqwest::Client>,
    base_url: String,
    token: Option<String>,
}

impl Client {
    pub fn new(token: Option<&str>) -> Self {
        Self {
            client: token.map(|_| reqwest::Client::new()),
            base_url: "https://huggingface.co/api".to_string(),
            token: token.map(|s| s.to_string()),
        }

    pub async fn fetch_models(
        &self,
        query: &str,
        sort: SortField,
        min_dl: u64,
        min_likes: u64,
    ) -> Result<Vec<ModelInfo>, ApiError> { /* ... */ }

    pub async fn fetch_model_metadata(&self, id: &str) -> Result<ModelMetadata, ApiError> { /* ... */ }

    pub async fn fetch_quantizations(&self, id: &str) -> Result<Vec<QuantizationGroup>, ApiError> { /* ... */ }
}
```

### Benefits
- **Single source** for all API calls
- **Token managed** at client level, not per-call
- **Easier testing** with mock client
- **Consistent error handling**

### Migration Steps
1. Create `api::Client` struct with methods
2. Update `ui/app/models.rs` to use `Client::new()`
3. Update `headless.rs` to use same `Client`
4. Remove duplicate token handling code

---

## Phase 2: Unified Download Orchestration

### Problem
Download logic is duplicated:
- `download.rs` has `start_download()` and `validate_and_sanitize_path()`
- `headless.rs` has `download_model()` with similar logic
- Verification worker exists separately in `verification.rs`

### Solution: Single `Downloader` struct

```rust
// NEW: core/downloader.rs
pub struct Downloader {
    client: Client,
    registry: Registry,
    rate_limiter: RateLimiter,
    options: AppOptions,
}

impl Downloader {
    pub fn new(token: Option<&str>, options: AppOptions) -> Self { /* ... */ }

    pub async fn download_model(
        &self,
        model_id: &str,
        quant_filter: Option<&str>,
        output_dir: &str,
    ) -> Result<DownloadResult, DownloadError> { /* ... */ }

    pub async fn verify(&self, filename: &str) -> Result<VerificationResult, DownloadError> { /* ... */ }

    pub async fn resume(&self) -> Result<Vec<DownloadResult>, DownloadError> { /* ... */ }
}
```

### Benefits
- **DRY principle**: Download logic defined once
- **Consistent state**: Rate limiter, registry, options all managed together
- **Shared verification**: Same verification code for TUI and CLI

### Migration Steps
1. Extract `start_download()` from `download.rs` to `Downloader::download()`
2. Move `validate_and_sanitize_path()` to `Downloader`
3. Merge `verification.rs` into `Downloader::verify()`
4. Update `headless.rs` to use `Downloader`
5. Update TUI download path to use `Downloader`

---

## Phase 3: Extract Shared Utilities

### Problem
- `format_file_size()` exists in both `headless.rs` and `utils.rs`
- `format_duration()` in `headless.rs` should be in `utils.rs`
- `validate_model_id()` duplicated

### Solution: Centralize in `utils.rs`

```rust
// utils.rs (ENHANCED)
pub fn format_file_size(bytes: u64) -> String { /* ... */ }

pub fn format_duration(duration: Duration) -> String { /* ... */ }

pub fn validate_model_id(model_id: &str) -> Result<(), ValidationError> { /* ... } }

pub fn extract_quantization_type(filename: &str) -> Option<String> { /* from api.rs */ }

pub fn parse_error(error: &str) -> ErrorType { /* categorize errors */ }
```

### Benefits
- **Single implementation** of all utilities
- **Easier maintenance**: Change in one place
- **Better testability**: Test utilities independently

---

## Phase 4: Unified Error Handling

### Problem
- `color-eyre` for TUI (with nice diagnostics)
- `HeadlessError` enum for CLI mode (with exit codes)
- Inconsistent error types across modules

### Solution: Error taxonomy

```rust
// core/error.rs (NEW)
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),

    #[error("Download error: {0}")]
    Download(#[from] DownloadError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Authentication required for {0}")]
    AuthRequired { model_url: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::AuthRequired(_) => EXIT_AUTH_ERROR,
            _ => EXIT_ERROR,
        }
    }
}
```

### Benefits
- **Single error type** for entire application
- **Consistent** exit codes and messages
- **Error context** preserved throughout stack

---

## Phase 5: Reduce UI Complexity

### Problem
- `events.rs` has 1100+ lines handling multiple concerns
- Mouse event coalescing adds complexity
- State management interleaved with event handling

### Solution: Split event handling

```
ui/events/
├── keyboard.rs    # Key press handling only
├── mouse.rs       # Mouse handling only
├── bindings.rs    # Key->Action mapping (configurable)
└── mod.rs         # Event loop orchestration
```

### Benefits
- **Easier testing**: Test keyboard separately from mouse
- **Configurable bindings**: Move key mapping to config file
- **Maintainable**: Each module has single responsibility

### Current Complexity in `events.rs`:
- Line 100-200: Key handling (Tab, Enter, Esc)
- Line 200-400: Navigation (j/k, arrow keys)
- Line 400-600: Filter controls (f, s, +, -)
- Line 600-800: Mouse handling with coalescing
- Line 800-1000: State transitions

---

## Phase 6: Configuration Consolidation

### Problem
- `AppOptions` contains UI state (`selected_field`, `editing_directory`)
- Config loaded separately in TUI vs headless
- Filter defaults stored in options, not config file

### Solution: Config hierarchy

```
config/
├── defaults.rs    # Static defaults
├── file.rs        # TOML file reading/writing
├── options.rs     # Runtime options (may override file)
└── filters.rs     # Filter presets (separate struct)
```

### Benefits
- **Clear separation**: Static vs dynamic vs persisted
- **Easier presets**: Filter presets in separate struct
- **Testable**: Can test config loading independently

---

## Implementation Roadmap (Prioritized by Impact)

### Quick Wins (Week 1)
- [ ] **Delete duplicate**: Remove `headless.rs::format_file_size()`, use `utils::format_size()`
- [ ] **Move missing**: Add `utils::format_duration()` (move from `headless.rs`)
- [ ] **Add utility**: Create `utils::validate_model_id()` (merge `headless.rs` + `download.rs`)

### Core Consolidation (Weeks 2-3)
- [ ] **Create `api::Client`** with all fetch methods
  - Token managed internally
  - Unified error handling
  - Update all 6 call sites
- [ ] **Create `core/reporter.rs`** for status messages
  - `Reporter::info()`, `error()`, `success()`
  - Consolidate 25+ `status_tx.send()` calls
- [ ] **Create `core/error.rs`** with unified `AppError`
  - Replace `HeadlessError` enum
  - Consistent exit codes

### Download Consolidation (Weeks 4-5)
- [ ] Extract `Downloader` struct from `download.rs`
- [ ] Move `validate_and_sanitize_path()` to `Downloader`
- [ ] Move `check_gated_model()` to `api.rs`
- [ ] Merge `verification.rs` into `Downloader::verify()`
- [ ] Update both TUI and headless paths

### UI Refactoring (Week 6)
- [ ] Split `events.rs` into `keyboard.rs`/`mouse.rs`
- [ ] Extract key bindings to `events/bindings.rs`
- [ ] Move UI-only state from `AppOptions`

### Testing (Week 7)
- [ ] Add unit tests for utilities
- [ ] Add integration tests for headless mode
- [ ] Verify TUI mode still works
- [ ] Update documentation

---

## Estimated Impact

| Metric | Current | After | Change |
|--------|---------|-------|--------|
| Total lines | ~8000 | ~6500 | -19% |
| Duplicated code | ~500 | ~50 | -90% |
| Test coverage | Unknown | +40% | Improved |
| Modules | 12 | 10 | Simplified |

---

## Backwards Compatibility

All changes are internal refactoring:
- CLI interface unchanged (`--headless search/download/list/resume`)
- TUI interface unchanged (same keys, same layout)
- Config file format unchanged
- No breaking changes to user experience

---

## Risk Mitigation

1. **Incremental migration**: Each phase can be tested independently
2. **Feature flags**: Keep old code until new code is verified
3. **Parallel runs**: Test both implementations during transition
4. **Rollback plan**: Keep git tags for each phase

---

## References

- Current duplication points: `grep -r "TODO\|FIXME"` in `src/`
- API patterns: `src/api.rs:fetch_*` functions
- Download patterns: `src/download.rs:start_download()`
- Event patterns: `src/ui/app/events.rs` (complex sections)

## Appendix: All Duplication Locations

| Pattern | File | Line(s) | Description |
|---------|------|---------|-------------|
| format_file_size | headless.rs | 82 | DUPLICATE - use utils::format_size() |
| format_duration | headless.rs | 100 | MISSING from utils.rs |
| validate_model_id | headless.rs | 112 | DUPLICATE - merge with download.rs |
| status_tx.send | download.rs | 157,167,182,190,203,218,224,234,284,286,345,350,356,359,368,385,406 | 17 sites |
| status_tx.send | headless.rs | 242,268,516,543 | 4 sites |
| status_tx.send | verification.rs | 90,109,122,124,144 | 5 sites |
| save_registry | download.rs | 257,332,398,419,585 | 5 sites |
| save_registry | ui/app/downloads.rs | 241,400,495 | 3 sites |
| save_registry | verification.rs | 140 | 1 site |
| check_gated_model | headless.rs | 394-424 | Only in headless mode |
| hf_token ref | 15+ files | Various | Inconsistent patterns |
| config::load_config | 3 files | Various | Good - single source |

### Additional Findings (from 2026-01-27 scan)

| Pattern | File | Line(s) | Description | Priority |
|---------|------|---------|-------------|----------|
| model ID validation | headless.rs:112 | 5+ locations | Model ID parsing in download.rs:65-92 + headless.rs:112 | HIGH |
| path sanitization | download.rs:42-62 | 21 lines | sanitize_path_component() - could be extracted | MEDIUM |
| ProgressReporter | headless.rs:795+ | 10+ methods | JSON/text output methods - could extract | MEDIUM |
| config save pattern | events.rs:336,358,846,947 | 4 locations | Config save after changes - consistent but repeated | LOW |
| model ID contains('/') check | download.rs:90 | Only download.rs | Not in headless.rs validation | LOW |
| HTTP client build | http_client.rs:5 | 2 locations | build_client_with_token + get_with_optional_token | GOOD - reusable |
