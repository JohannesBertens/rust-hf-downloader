# PR #27 Review: Fix all 10 rust-hf-downloader Linear issues

**Branch:** `fix/rust-hf-downloader-issues`
**Commit:** `39fa945`

---

## Summary

This PR tackles 10 issues: replacing `futures::executor::block_on` with `parking_lot::RwLock` reads, removing the `--token` CLI arg, adding connection pooling, bounding channels, fixing rate-limiter divide-by-zero, adding section comments, removing dead code, improving config error messages, adding `rust-toolchain.toml`, and using `env!("CARGO_PKG_VERSION")`.

The PR includes a large Cargo.lock update (dependency roll-forward) alongside the code changes.

---

## Findings

### 🔴 Critical Bug — `mpsc::Sender::send()` calls not `.await`ed (messages silently dropped)

The migration from `mpsc::UnboundedSender` to `mpsc::Sender` (bounded) is **incomplete**. `UnboundedSender::send()` is synchronous, but `Sender::send()` is `async`. Several call sites use `let _ = sender.send(val)` without `.await`, which drops the returned future without polling it — meaning the message is **never sent**.

**Affected locations:**

1. **`src/download.rs`** — all 20+ `status_tx.send()` calls are fire-and-forget without `.await`. No download status messages (starting, progress, completion, errors, auth errors) will be emitted.
   ```
   let _ = status_tx.send(format!("Starting download: {}", filename));  // line 157
   let _ = status_tx.send(format!("Download complete: {}", filename));  // line 356
   let _ = status_tx.send(format!("AUTH_ERROR:{}", model_id));         // line 385
   ... and ~17 more
   ```

2. **`src/verification.rs`** — all 5 `status_tx.send()` calls are also not `.await`ed (lines 86, 105, 118, 120, 140). No verification status messages will be emitted.

3. **`src/headless.rs`** — `progress_tx.send()` calls at lines 227, 254, 510, 538 are not `.await`ed. Queued/resumed progress messages silently dropped.

4. **`src/ui/app/downloads.rs` line 353** — `self.download_tx.send(...)` in `resume_incomplete_downloads()` is NOT `.await`ed (the other two `send` calls at lines 277 and 531 were properly updated). This means **incomplete downloads resumed from registry are silently not re-queued**.

**Fix:** Either:
- Add `.await` to every `send()` call site (preferred — gives backpressure awareness), or
- Use `try_send()` for best-effort fire-and-forget sends where dropping is acceptable, or
- Revert the channel type to `UnboundedSender`/`UnboundedReceiver` where backpressure is not needed.

---

### 🟡 Correctness Issue — `futures::executor::block_on` removal is incomplete (WEB-51)

The PR description says "Replace `futures::executor::block_on()` with direct `parking_lot::RwLock` reads", but only `src/ui/app/events.rs` was updated. Six call sites remain:

- `src/ui/app/models.rs` lines 363, 368, 381, 402
- `src/ui/app/downloads.rs` lines 50, 66

These still call `futures::executor::block_on(async { self.something.read()/.write() })` on `parking_lot::RwLock`, which doesn't need async at all. The same pattern was fixed in `events.rs` — the remaining instances should be fixed consistently.

---

### 🟡 Correctness Issue — `check_config_permissions()` is defined but never called

The new function `check_config_permissions()` in `src/config.rs` (added as part of WEB-52) is a dead function — it is never called anywhere. The compiler confirms: `warning: function check_config_permissions is never used`. It should be called at startup (e.g., in `main.rs` or during `load_config()`) to actually warn users about insecure config permissions.

---

### 🟡 Correctness Issue — Config permissions use `std::fs` instead of `tokio::fs`

`save_config()` in `src/config.rs` uses `std::fs::write()`, `std::fs::metadata()`, and `std::fs::set_permissions()` — synchronous blocking I/O. Since the config is saved from within the TUI async runtime (the options popup), this could cause brief UI stalls. This was pre-existing, but the new `chmod 600` code extends the blocking section. Consider using `tokio::fs` or `tokio::task::spawn_blocking`.

---

### 🟡 Correctness Issue — `DEFAULT_CLIENT` timeout is hardcoded and cannot be configured

The new `DEFAULT_CLIENT` in `http_client.rs` has a hardcoded `timeout(Duration::from_secs(300))`. The application has a configurable `download_timeout_secs` option (via `DOWNLOAD_CONFIG`), but `DEFAULT_CLIENT` ignores it entirely. Meanwhile, `get_with_optional_token()` uses `DEFAULT_CLIENT` for API calls, so API call timeouts are now fixed at 300s regardless of user config.

Additionally, `build_client_with_token()` (used in `download.rs`) still creates a **new `Client` per download**, completely bypassing the connection pool for the actual download operations — only the lightweight API calls in `get_with_optional_token()` benefit from the pool. The WEB-54 issue about "connection pooling" is only partially addressed.

---

### 🟡 Correctness Issue — `#[arg(skip)]` on `token` removes `--token` from help but doesn't populate from `HF_TOKEN` env

The comment says "populated from HF_TOKEN env var in config", but `#[arg(skip)]` simply removes the field from CLI parsing. The token is NOT populated from `HF_TOKEN` at CLI parse time. Instead, `HF_TOKEN` is read later in `models.rs:355` via `std::env::var("HF_TOKEN")` as part of `AppOptions`. The comment is misleading — it should either populate the field or the comment should be corrected.

In headless mode, `cli_args.token` is passed directly to functions like `run_download()` — since it's always `None`, only the config-based token will be used. This is intentional per the issue, but the implementation is confusing.

---

### 🟡 Correctness Issue — `rust-toolchain.toml` pins to `nightly` with no specific version

```toml
[toolchain]
channel = "nightly"
```

This forces **any** contributor to use the latest nightly, with no reproducibility pin. Nightly Rust can break at any time. Consider either:
- Pinning a specific nightly version: `channel = "nightly-2026-06-05"`
- Using stable if no nightly features are required (the project compiles fine on nightly but may not need it)
- Adding a `components` or `targets` field for clarity

---

### 🟡 Correctness Issue — Model ID not URL-encoded in download URL construction

In `src/download.rs` line 173:
```rust
let url = format!(
    "https://huggingface.co/{}/resolve/main/{}",
    model_id, sanitized_filename
);
```

While `sanitized_filename` is sanitized for path traversal, `model_id` is used raw in the URL. If a model ID contains special characters (e.g., spaces, `+`, `%`), the URL will be malformed. The `sanitize_path_component` function only rejects `..`/`/`/`\`/`\0`, but doesn't URL-encode. HuggingFace model IDs typically use safe characters, but this is still a correctness gap.

---

### 🟢 Suggestion — Dead code: `ConfigError` variant and `Editing` variant still exist

- `HeadlessError::ConfigError` is marked `#[allow(dead_code)]` but never constructed. Consider removing it or adding a `TODO` comment.
- `models::InputMode::Editing` is never constructed. The PR removed the `#[allow(dead_code)]` annotation from both the variant and the enum, but the compiler now emits `warning: variant Editing is never constructed`. Add `#[allow(dead_code)]` back to the variant specifically.

---

### 🟢 Suggestion — `once_cell::sync::Lazy` could be `std::sync::LazyLock`

Since the project now requires nightly (via `rust-toolchain.toml`), `std::sync::LazyLock` is stable in nightly and would remove the `once_cell` dependency entirely. The project already uses `once_cell::sync::Lazy` in `download.rs` as well.

---

### 🟢 Suggestion — `build_client_with_token()` is now only used in `download.rs`

After the refactoring, `build_client_with_token()` is only called from `download.rs`. Consider consolidating the client construction logic — either:
- Make `download.rs` use the shared `DEFAULT_CLIENT` with per-request auth headers (like `get_with_optional_token` does), or
- Move `build_client_with_token` to `download.rs` as a private helper.

---

### ⚪ Style Nit — Section comments placement

The `// ─── Section ───` comments added to `headless.rs` and `render.rs` are a nice touch for navigation. Minor: in `render.rs`, one comment is placed after `#[allow(clippy::too_many_arguments)]` but before the function signature, which looks slightly off:

```rust
#[allow(clippy::too_many_arguments)]
// ─── File Tree Panel ───────────────────────────────────────────────────────────
fn render_file_tree_panel(
```

Prefer placing section comments above the attribute.

---

### ⚪ Style Nit — Inline tuple type in `headless.rs`

The `DownloadMessage` type alias was removed from `headless.rs` (good deduplication), but the replacement uses an inline 6-tuple:

```rust
mpsc::Sender<(String, String, PathBuf, Option<String>, Option<String>, u64)>
```

This appears 4 times in function signatures. Importing the `DownloadMessage` alias from `state.rs` (or moving it to `models.rs`) would improve readability. Currently it's duplicated in `state.rs` and used as an inline tuple in `headless.rs`.

---

### ⚪ Style Nit — Cargo.lock version downgrade

`Cargo.lock` header changed from `version = 4` to `version = 3`. This happens when a newer Cargo.lock is regenerated by an older Cargo version. Not harmful but worth noting — it suggests the lock file was regenerated with an older Cargo.

---

## Verdict

**🔴 Must fix before merge:** The `Sender::send()` without `.await` bug will cause all download status messages, verification messages, and resume-queue messages to be silently dropped. Downloads may still work (the actual file download logic doesn't depend on status messages), but users will see zero feedback. More critically, the `resume_incomplete_downloads()` in `downloads.rs` silently fails to re-queue files because its `send` is not awaited.

**🟡 Should fix before merge:** The remaining `futures::executor::block_on` calls, unused `check_config_permissions()`, and `nightly` toolchain pin without version.

**🟢/⚪ Can follow up:** Style nits and suggestions are non-blocking.
