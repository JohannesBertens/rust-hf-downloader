# Release Notes - Version 0.9.5

**Release Date:** 2025-11-25  
**Type:** Feature Release - Authentication Support & Code Refactoring

## Overview

Version 0.9.5 introduces HuggingFace authentication support for downloading gated models and includes a major code refactoring that splits the monolithic `src/ui/app.rs` into focused, maintainable modules. This release enables access to restricted models like Llama-3.1-8B while improving code organization and maintainability.

---

## ✨ New Features

### 1. **HuggingFace Token Authentication**
- **Gated Model Support:** Download restricted models that require authentication
- **Token Integration:** HF token from config automatically used in API requests
- **Authenticated Downloads:** Token passed through entire download pipeline
- **Model Access:** Works with popular gated models like:
  - Llama-3.1-8B
  - Llama-2 variants
  - Other restricted HuggingFace models

**How It Works:**
1. User sets HF token in Options screen (Press 'o')
2. Token stored in `~/.config/jreb/config.toml`
3. All API requests automatically include `Authorization: Bearer {token}` header
4. Downloads of gated models proceed without manual intervention

### 2. **401 Unauthorized Error Popup**
- **Clear Error Messaging:** Friendly popup when authentication fails
- **Helpful Instructions:** Step-by-step guide to resolve access issues
- **Quick Access Links:**
  - Direct model URL display
  - Instructions to set HF token in Options
  - Link to HuggingFace token settings
- **Smart Detection:** Automatically triggered on 401 HTTP errors

**AuthError Popup Layout:**
```
┌─────────────────────────────────────────────────┐
│ ⚠ Authentication Required                      │
├─────────────────────────────────────────────────┤
│ This model requires authentication.            │
│                                                 │
│ Model: meta-llama/Llama-3.1-8B                 │
│                                                 │
│ Steps to fix:                                   │
│ 1. Accept model terms at:                      │
│    https://huggingface.co/meta-llama/...       │
│                                                 │
│ 2. Create HF token (if you don't have one):    │
│    https://huggingface.co/settings/tokens      │
│                                                 │
│ 3. Set token in Options (press 'o'):           │
│    - Navigate to 'HuggingFace Token'           │
│    - Press Enter to edit                        │
│    - Paste your token and press Enter          │
│                                                 │
│ Press Enter to close and open Options          │
│ Press Esc to close                             │
└─────────────────────────────────────────────────┘
```

### 3. **New HTTP Client Module**
- **Centralized Authentication:** Single module handles all authenticated requests
- **Consistent Headers:** Token automatically added to all HTTP calls
- **Error Handling:** Proper 401 detection and propagation
- **Clean API:** Simple interface for authenticated downloads

**Module: `src/http_client.rs`**
- **`make_authenticated_request(url: &str, token: Option<&str>)`**: Create authenticated reqwest client
- **Benefits:**
  - Eliminates code duplication
  - Consistent authentication across all API calls
  - Easy to test and maintain
  - Future-proof for additional auth methods

---

## 🔧 Code Refactoring

### Major App.rs Restructuring
The monolithic `src/ui/app.rs` (~1100+ lines) has been split into focused modules:

**New Module Structure:**
```
src/ui/app/
├── state.rs          # AppState struct and initialization (~158 lines)
├── events.rs         # Event handling and keyboard input (~709 lines)
├── models.rs         # Model browsing and search logic (~253 lines)
├── downloads.rs      # Download management (~460 lines)
└── verification.rs   # Verification UI and logic (~77 lines)
```

**Benefits:**
- **Improved Maintainability:** Smaller, focused files easier to navigate
- **Clear Separation of Concerns:** Each module has single responsibility
- **Better Code Organization:** Related functionality grouped together
- **Easier Testing:** Modular code simpler to test in isolation
- **Reduced Cognitive Load:** Developers work with ~250 lines vs 1100+ lines

### Module Breakdown

#### **`src/ui/app/state.rs`** (158 lines)
- **Purpose:** Application state management
- **Contents:**
  - `AppState` struct definition
  - Constructor: `new()`
  - Initialization logic
  - State synchronization methods
  - Configuration loading on startup

#### **`src/ui/app/events.rs`** (709 lines)
- **Purpose:** Event handling and user input
- **Contents:**
  - `handle_crossterm_events()`: Terminal event processing
  - `on_key_event()`: Keyboard input routing
  - Mode-specific handlers (Normal/Editing/Options)
  - Popup event handling
  - Keybinding logic for all features

#### **`src/ui/app/models.rs`** (253 lines)
- **Purpose:** Model browsing and search
- **Contents:**
  - `load_trending_models()`: Startup trending fetch
  - `search_models()`: Search query execution
  - `load_quantizations()`: Quantization fetch with cache
  - `start_background_prefetch()`: Async prefetch
  - Navigation helpers: `next_model()`, `previous_model()`
  - Detail display: `show_model_details()`

#### **`src/ui/app/downloads.rs`** (460 lines)
- **Purpose:** Download orchestration
- **Contents:**
  - `confirm_download()`: Download path popup and confirmation
  - `start_download()`: Queue downloads to worker
  - `resume_incomplete_downloads()`: Resume system
  - `delete_incomplete_downloads()`: Cleanup utilities
  - Path validation and sanitization
  - Multi-part file handling
  - Download status tracking

#### **`src/ui/app/verification.rs`** (77 lines)
- **Purpose:** SHA256 verification
- **Contents:**
  - `verify_file()`: Trigger verification
  - `verify_selected_file()`: UI integration
  - Progress monitoring
  - Verification status display

### Updated `src/ui/app.rs` (~48 lines)
- **Purpose:** Module exports and public API
- **Contents:**
  - Module declarations
  - Public re-exports
  - Minimal glue code
  - Clean interface for `src/ui/render.rs`

---

## 🔧 Technical Improvements

### Authentication Pipeline

**Token Flow:**
```
Config (load) → AppOptions → Download Channel → HTTP Client → API Request
```

1. **Config Storage:**
   - HF token stored in `~/.config/jreb/config.toml`
   - Field: `huggingface_token = "hf_..."`
   - Optional field (None if not set)

2. **Download Integration:**
   - `DownloadCommand` enum extended with `token: Option<String>`
   - Token passed through async channel to download worker
   - All HTTP requests use authenticated client

3. **API Request Updates:**
   - `fetch_model_files()`: Now accepts optional token
   - `fetch_multipart_sha256s()`: Authenticated when token provided
   - Download functions: Use `http_client::make_authenticated_request()`

### Error Handling

**401 Detection:**
```rust
if response.status() == StatusCode::UNAUTHORIZED {
    return Err(anyhow::anyhow!("401: Unauthorized - Authentication required"));
}
```

**Error Propagation:**
- Download errors bubble up through async channel
- UI detects "401: Unauthorized" in error messages
- `AuthErrorPopup` automatically triggered
- User guided to resolution steps

### HTTP Client Architecture

**Authenticated Request Creation:**
```rust
pub async fn make_authenticated_request(
    url: &str,
    token: Option<&str>,
) -> Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    
    if let Some(t) = token {
        let auth_value = format!("Bearer {}", t);
        headers.insert(
            reqwest::header::AUTHORIZATION,
            auth_value.parse()?,
        );
    }
    
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;
    
    Ok(client)
}
```

**Usage in Download Functions:**
```rust
let client = http_client::make_authenticated_request(&url, token.as_deref()).await?;
let response = client.get(&url).send().await?;
```

---

## 📋 Configuration Updates

### New Configuration Field

**`~/.config/jreb/config.toml`:**
```toml
# Existing fields...
default_directory = "/home/user/models"
concurrent_threads = 8

# New field (v0.9.5)
huggingface_token = "hf_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890"
```

**Field Properties:**
- **Optional:** Can be omitted if not using gated models
- **Secure:** Stored in user's home directory (Unix permissions)
- **Persistent:** Survives application restarts
- **Editable:** Can be set via Options screen or manual editing

### Options Screen Update

**New Option Field:**
```
┌─────────────────────────────────────────────────┐
│ Options (Press 'o' to toggle, Esc to close)   │
├─────────────────────────────────────────────────┤
│ General                                         │
│ > Default Directory: /home/user/models          │
│   HuggingFace Token: hf_***************************│
│                                                 │
│ [Rest of options...]                            │
└─────────────────────────────────────────────────┘
```

**Interaction:**
- Press Enter to edit token field
- Input widget accepts token (masked display)
- Auto-saved on Esc
- Applied to all future API requests

---

## 🎯 User Experience Improvements

### Seamless Gated Model Access
- **Before:** Downloads of gated models silently failed or showed cryptic 401 errors
- **After:** Clear guidance with specific steps to resolve authentication

### Helpful Error Messages
- **Before:** Generic "Download failed" status
- **After:** Contextual popup with:
  - Model-specific URL
  - Direct link to token settings
  - Instructions to set token in app
  - Option to open Options directly

### One-Time Setup
- **Before:** Must authenticate for every gated model
- **After:** Set token once, works for all gated models across sessions

### Visual Feedback
- Token field shows masked value in Options (`hf_***...`)
- Clear indication when authentication is configured
- Status bar shows authentication progress

---

## 🐛 Bug Fixes

### Download Pipeline
- Fixed: Gated models now download successfully with authentication
- Fixed: 401 errors properly detected and reported to user
- Fixed: Token persistence across application restarts

### Code Quality
- Fixed: Clippy warnings resolved in refactored modules
- Fixed: Import organization cleaned up
- Fixed: Consistent error handling across modules

---

## 📊 File Statistics

### Code Organization Impact

**Before (v0.9.0):**
- `src/ui/app.rs`: ~1107 lines (monolithic)

**After (v0.9.5):**
- `src/ui/app.rs`: ~48 lines (module exports)
- `src/ui/app/state.rs`: 158 lines
- `src/ui/app/events.rs`: 709 lines
- `src/ui/app/models.rs`: 253 lines
- `src/ui/app/downloads.rs`: 460 lines
- `src/ui/app/verification.rs`: 77 lines
- **Total:** ~1657 lines (+14% for added features, but better organized)

**New Files:**
- `src/http_client.rs`: 47 lines (authentication)

**Modified Files (Non-UI):**
- `src/api.rs`: +283 lines (authentication integration)
- `src/download.rs`: +85 lines (token passing)
- `src/models.rs`: +82 lines (AuthErrorPopup, token field)
- `src/ui/render.rs`: +585 lines (AuthError popup rendering)

**Total Changes:**
- +1757 insertions
- -292 deletions
- Net: +1465 lines

---

## 🎮 Keybindings (Unchanged)

All existing keybindings remain functional:
- **'/'**: Enter search mode
- **'o'**: Toggle options screen
- **Tab**: Switch focus between lists
- **'j'/'k'**: Navigate lists
- **Enter**: Show details / confirm action
- **'d'**: Download selected model
- **'v'**: Verify selected file
- **'q'**: Quit application
- **Esc**: Close popups / return to normal mode

---

## 🧪 Testing Checklist

- [x] HF token saved in config
- [x] Token loaded on startup
- [x] Token passed to API calls
- [x] Authenticated downloads succeed for gated models
- [x] 401 errors trigger AuthError popup
- [x] AuthError popup displays correct model URL
- [x] "Open Options" button works from AuthError popup
- [x] Token field masked in Options screen
- [x] Token editable in Options
- [x] All refactored modules compile
- [x] Clippy warnings resolved
- [x] Existing functionality unchanged (no regressions)
- [x] Background prefetch still works
- [x] Verification still works
- [x] Resume downloads still works

---

## 📝 Known Limitations

1. **Token Security**
   - Token stored in plain text in config file
   - Protected by Unix file permissions (~/.config/jreb/)
   - Future enhancement: OS keychain integration

2. **Token Masking**
   - Token partially masked in Options (`hf_***...`)
   - Full token visible during editing
   - No clipboard protection

3. **Single Token**
   - Only one HF token supported
   - No per-model or per-organization tokens
   - Sufficient for most use cases

4. **No Token Validation**
   - App doesn't validate token format on input
   - Invalid tokens detected only on first API call
   - Future enhancement: Token validation on save

5. **Manual Model Terms Acceptance**
   - User must accept model terms on HuggingFace website
   - App cannot automate this step (requires web browser)
   - One-time action per gated model

---

## 🔄 Migration Notes

### Upgrading from 0.9.0
1. **No breaking changes** to existing functionality
2. First run will add `huggingface_token` field to config (optional)
3. Existing downloads and registry unchanged
4. No data migration required

### Setting Up Authentication
```bash
# First time setup for gated models:
1. Run application: cargo run
2. Press 'o' to open Options
3. Navigate to "HuggingFace Token"
4. Press Enter to edit
5. Paste your token from: https://huggingface.co/settings/tokens
6. Press Enter to confirm
7. Press Esc to save
8. Download gated models as normal
```

---

## 🎉 Contributors

**Johannes Bertens** - Initial implementation and release  
**factory-droid[bot]** - Co-author on authentication implementation

---

## 📎 Commit History

```
44d4a45 - Add authentication support for gated models and 401 error popup
cd8a309 - Clippy fixes
572fd45 - Slight refactor
```

**Key Changes:**
- **44d4a45**: Main feature commit
  - New `http_client.rs` module
  - Authentication pipeline integration
  - 401 error popup implementation
  - Code refactoring into app/ submodules
  - Token field in config and Options
- **cd8a309**: Code quality improvements
- **572fd45**: Minor refactoring

---

## 🚀 Usage Examples

### Downloading a Gated Model (First Time)

```
1. Start application: cargo run
2. Search for gated model: /llama-3.1-8b
3. Navigate to desired model: j/k
4. Press 'd' to download
5. Press Enter to confirm path
6. [401 Error popup appears]
7. Press Enter to open Options
8. Navigate to "HuggingFace Token"
9. Press Enter to edit
10. Paste token: hf_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890
11. Press Enter to confirm
12. Press Esc to close Options
13. Press 'd' again to retry download
14. [Download proceeds successfully]
```

### Downloading Gated Models (After Setup)

```
1. Start application: cargo run
2. Token automatically loaded from config
3. Search for any gated model
4. Press 'd' to download
5. [Download starts immediately - no authentication popup]
```

### Manual Token Configuration

**Edit `~/.config/jreb/config.toml`:**
```toml
huggingface_token = "hf_YOUR_TOKEN_HERE"
```

**Restart application:**
```bash
cargo run
```

### Testing Authentication

**Try downloading popular gated models:**
- `meta-llama/Llama-3.1-8B-Instruct`
- `meta-llama/Llama-2-7b-hf`
- `mistralai/Mistral-7B-Instruct-v0.2`

---

## 🔗 Links

- **Repository:** https://github.com/JohannesBertens/rust-hf-downloader
- **Documentation:** `AGENTS.md`
- **Previous Release:** [v0.9.0](RELEASE_NOTES_0.9.0.md)
- **HuggingFace Token Settings:** https://huggingface.co/settings/tokens
- **Config Location:** `~/.config/jreb/config.toml`

---

## 🔐 Security Considerations

### Token Storage
- Config file stored in `~/.config/jreb/` (user directory)
- Default Unix permissions: 644 (readable by user)
- **Recommendation:** Set restrictive permissions:
  ```bash
  chmod 600 ~/.config/jreb/config.toml
  ```

### Token Transmission
- Token sent via HTTPS only (TLS encryption)
- Uses `rustls-tls` for security
- No token logging or printing to terminal

### Best Practices
1. Use read-only tokens (not write tokens)
2. Create dedicated token for CLI tools
3. Revoke tokens when no longer needed
4. Don't commit config files to version control
5. Set restrictive file permissions on config

---

**Version:** 0.9.5  
**Previous Version:** 0.9.0  
**Branch:** v0.9.5-split-and-add  
**Next Version:** TBD
