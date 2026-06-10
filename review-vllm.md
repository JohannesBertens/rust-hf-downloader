Now I have all the information I need. Let me write the comprehensive review.

## Completed

Reviewed PR #27 changes across all 15 modified files (c9f22c6). Found 4 critical bugs, 2 correctness issues, 1 style issue, and 1 dead code observation.

## Review Findings for PR #27

---

### 🔴 Critical bug: `status_tx.send()` and `progress_tx.send()` never execute — all messages silently lost

**Files:** `src/download.rs` (20 calls), `src/verification.rs` (5 calls), `src/headless.rs` (4 calls), `src/ui/app/downloads.rs` (1 call)

The PR changed all channels from `mpsc::UnboundedSender` to bounded `mpsc::Sender(1024)`. For bounded senders, `send()` returns an `impl Future` that must be `.await`ed. However, every single call site uses:

```rust
let _ = status_tx.send(format!("..."));
```

The `let _ =` drops the future without ever polling it. **The send never happens.** The message is silently discarded.

This affects:
- **`src/download.rs`** — All 20 `status_tx.send()` calls (download started, chunk errors, retries, verification queueing, auth errors, download completion). None have `.await`.
- **`src/verification.rs`** — All 5 `status_tx.send()` calls (verification progress, hash match/mismatch results). None have `.await`.
- **`src/headless.rs`** — 4 `progress_tx.send()` calls for queueing and resume status. None have `.await`.
- **`src/ui/app/downloads.rs:353`** — `self.download_tx.send(...)` in `resume_incomplete_downloads()` missing `.await`.

**Fix:** Add `.await` to every `tx.send(...)` call that targets a bounded channel. For fire-and-forget status messages, the `let _ = tx.send(...).await` pattern is appropriate. For the `HeadlessError`-returning paths (`headless.rs:216`, `headless.rs:243`, `headless.rs:527`), the existing `.await` is already present because those use `.map_err()` chaining — the `.await` is already correct there.

Missing `.await` call sites (non-exhaustive sample):
- `src/download.rs:157` — `let _ = status_tx.send(format!("Starting download: {}", filename));`
- `src/download.rs:167` — error reporting for invalid filename
- `src/download.rs:284` — `let _ = status_tx.send(format!("Queued {} for verification", filename));`
- `src/download.rs:385` — `let _ = status_tx.send(format!("AUTH_ERROR:{}", model_id));`
- `src/download.rs:406` — `let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));`
- `src/verification.rs:86` — status_tx.send
- `src/verification.rs:105` — `let _ = status_tx.send(format!("Verifying integrity of {}...", item.filename));`
- `src/headless.rs:227` — `let _ = progress_tx.send(format!("Queued: {}", quant_file.filename));`
- `src/ui/app/downloads.rs:353` — `let _ = self.download_tx.send((...))` in `resume_incomplete_downloads()`

The code compiles because dropping the future is valid Rust, but the messages are silently lost at runtime.

---

### 🔴 Critical bug: `futures::executor::block_on()` still used for parking_lot RwLock reads (WEB-51 incomplete)

**File:** `src/ui/app/models.rs`

The PR claims to fix WEB-51 ("Replace `futures::executor::block_on()` with direct parking_lot::RwLock reads"), but 4 calls remain:

**`clear_model_details()` (lines 363-369):**
```rust
pub fn clear_model_details(&mut self) {
    // Clear quantizations (GGUF mode)
    futures::executor::block_on(async {
        self.quantizations.write().clear();
    });

    // Clear metadata and file tree (Standard mode)
    futures::executor::block_on(async {
        *self.model_metadata.write() = None;
        *self.file_tree.write() = None;
    });
    ...
}
```

**`clear_search_results()` (lines 381-383):**
```rust
futures::executor::block_on(async {
    self.models.write().clear();
});
```

**`prefetch_adjacent_models()` (line 402):**
```rust
let mut last_time =
    futures::executor::block_on(async { self.last_prefetch_time.lock().await });
```

All four should be converted to direct parking_lot RwLock writes:
```rust
// Before
futures::executor::block_on(async { self.quantizations.write().clear(); });
// After
self.quantizations.write().clear();
```

---

### 🔴 Critical bug: `trigger_download()` still uses `block_on` (WEB-51 incomplete)

**File:** `src/ui/app/downloads.rs`

Lines 50 and 66:
```rust
let metadata = futures::executor::block_on(async {
    self.model_metadata.read().clone()
});

let quantizations = futures::executor::block_on(async {
    self.quantizations.read().clone()
});
```

Both `self.model_metadata` and `self.quantizations` are `parking_lot::RwLock` (not tokio Mutex), so direct reads work without `block_on`:
```rust
let metadata = self.model_metadata.read().clone();
let quantizations = self.quantizations.read().clone();
```

---

### 🟡 Correctness issue: `check_config_permissions()` is never called

**File:** `src/config.rs:76-113`

The function `pub fn check_config_permissions()` is defined and intended to warn users if the config file has group/other-readable permissions. But it is never invoked anywhere in the codebase. The compiler warning confirms this:

```
warning: function `check_config_permissions` is never used
  --> src/config.rs:76:8
```

**Fix:** Call it during startup, e.g., in `main()` after `config::load_config()` or inside `config::load_config()` itself when the config file exists. The TUI and headless modes would both benefit from this startup check.

---

### 🟡 Correctness issue: `#[arg(skip)]` on `token` field means it's always `None`

**File:** `src/cli.rs:17-18`

```rust
/// HuggingFace authentication token (populated from HF_TOKEN env var in config)
#[arg(skip)]
pub token: Option<String>,
```

The doc comment says "populated from HF_TOKEN env var", but `#[arg(skip)]` means clap never populates this — it's always `None` after parsing. The actual `HF_TOKEN` env var reading happens inside `AppOptions::default()` (in `src/models.rs:355`). This means:

1. The field still exists on the struct but is always `None` — somewhat misleading.
2. Headless mode works because `headless.rs` calls `config::load_config()` which reads the env var, so the token is available via `options.hf_token`. But it's indirect.

**Fix:** Either remove the field entirely (since it's never populated by clap) and always go through `config::load_config()` + `AppOptions::default()`, or keep the field but remove `#[arg(skip)]` and use clap's env var support (`#[arg(long, env = "HF_TOKEN")]`). The latter would make the `--token` arg work again while also reading from the environment variable.

---

### 🟡 Correctness issue: `save_config()` chmod 600 runs after `fs::write()` — race window on Unix

**File:** `src/config.rs:67-73`

The permissions are set after the file is written:
```rust
fs::write(&config_path, toml_string)?;

#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(&config_path)?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&config_path, perms)?;
}
```

There is a brief window between `fs::write()` (which creates the file with default umask permissions) and `fs::set_permissions()` where the file may be world-readable. Consider using `PermissionsExt::set_mode()` before writing, or using a temporary file with restricted permissions that is then atomically renamed. In practice this is a small race window, but for security-sensitive token data, it's worth tightening.

**Fix:** Write to a temp file with 0o600 permissions, then `fs::rename()` atomically. Or set umask before the write.

---

### ⚪ Style: 22+ hidden lifetime warnings from `[lints]`

**File:** `Cargo.toml:36` (new `rust.rust-2018-idioms = "warn"`)

Adding `rust.rust-2018-idioms = "warn"` produces 22+ warnings about elided lifetimes, primarily in `src/ui/render.rs` and `src/ui/app.rs`:

```
warning: hidden lifetime parameters in types are deprecated
  --> src/ui/app.rs:116:36
   |
116 |     fn draw(&mut self, frame: &mut Frame) {
    |                                    ^^^^^ expected lifetime parameter
```

These affect `Frame`, `ListItem`, `Line`, `StandardPanelContext`, `GgufPanelContext`, `RenderParams`, etc. The help text suggests adding `'_`. These should be fixed or allowed with `#[allow(elided_lifetimes_in_paths)]` to keep the build clean.

---

### ⚪ Dead code: `InputMode::Editing` variant is now unreachable

**File:** `src/models.rs:181`

The `Editing` variant of `InputMode` is no longer used since the PR didn't change the search popup model (search is always a popup now, never inline editing). The compiler warns:
```
warning: variant `Editing` is never constructed
```

Consider removing the variant or marking it with `#[allow(dead_code)]` explicitly if it's kept for future use.

---

## Summary

| Severity | Count | Key Files |
|----------|-------|-----------|
| 🔴 Critical | 4 | `download.rs`, `verification.rs`, `headless.rs`, `models.rs`, `downloads.rs` |
| 🟡 Correctness | 3 | `config.rs`, `cli.rs` |
| ⚪ Style/Dead code | 2 | `models.rs`, `Cargo.toml`, `render.rs` |

The most impactful issue is the silent dropping of all `status_tx.send()` and `progress_tx.send()` futures — this is not just a bug but a complete silent failure of the status/progress/verification reporting system. All 30 call sites need `.await` added. The remaining `futures::executor::block_on()` calls (6 sites) should also be converted to complete WEB-51.