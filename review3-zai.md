# PR #27 Review — Iteration 3 (Final)

**Branch:** `fix/rust-hf-downloader-issues`
**Reviewer:** Worker Agent (Zai)
**Date:** 2026-06-10

---

## Summary

This PR resolves all 10 tracked Linear issues across the codebase. All previously identified bugs from iterations 1 and 2 have been addressed. The codebase compiles cleanly and all 7 tests pass.

---

## ✅ Verified Fixes (All Correct)

### Iteration 1 Fixes — Confirmed Working

1. **`mpsc::send()` without `.await`** — ✅ All `UnboundedSender` replaced with bounded `Sender`. Every `.send()` call on a bounded channel now uses `.await`. Non-async contexts correctly use `.try_send()`. No remaining unbounded channels anywhere in the codebase.

2. **`block_on` calls removed** — ✅ All `futures::executor::block_on` calls eliminated. The `parking_lot::RwLock` fields (`.models`, `.quantizations`, `.file_tree`, `.model_metadata`) are now accessed directly with `.read()` / `.write()`. The tokio `Mutex` (`last_prefetch_time`) uses `try_lock()` with a reasonable fallback.

3. **`check_config_permissions()`** — ✅ Added before parsing config. Warns if file is world/group-readable. `save_config()` now sets `0o600` permissions. All error messages improved with file path context.

### Iteration 2 Fixes — Confirmed Working

4. **Divide-by-zero in rate limiter** — ✅ `rate > 0.0` guard added at `src/rate_limiter.rs:104`. Prevents `NaN` / `Infinity` sleep. *(See correctness note below about a remaining edge case.)*

5. **URL-encoded `model_id` in all URLs** — ✅ `urlencoding::encode(model_id)` applied consistently in:
   - `src/api.rs:241` — `fetch_model_files`
   - `src/api.rs:284` — subdir fetching
   - `src/api.rs:369` — `fetch_multipart_sha256s`
   - `src/download.rs:177` — download URL
   - `src/ui/app/downloads.rs:207, 472` — UI download URLs

6. **Permissions check before parse** — ✅ `check_config_permissions()` called before `fs::read_to_string()` in `load_config()`.

### Additional Improvements

7. **Shared HTTP client with connection pooling** — ✅ `DEFAULT_CLIENT` static using `once_cell::Lazy`. API calls reuse the pooled client. Chunked downloads still use per-request `build_client_with_token` (needs custom timeout). Good separation of concerns.

8. **Version from `CARGO_PKG_VERSION`** — ✅ CLI now uses `env!("CARGO_PKG_VERSION")` instead of hardcoded string.

9. **Removed `sort` CLI arg** — ✅ The broken/unused `--sort` arg is removed. `None` is passed to `run_search`, which defaults to `SortField::Downloads`.

10. **Removed duplicate `format_file_size`** — ✅ All references now use `crate::utils::format_size`. No dangling references.

---

## 🟡 Correctness Issue — Rate Limiter Infinite Loop When `rate = 0` and Enabled

**File:** `src/rate_limiter.rs:78–112` (the `acquire()` method)

**Problem:** The divide-by-zero guard at line 104 correctly prevents a `NaN` sleep duration, but when `rate = 0.0` and the limiter is **enabled**, the function enters an infinite loop:

```
1. tokens = 0, max_tokens = 0 (set by RateLimiter::new(0, ...) or set_rate(0))
2. tokens (0) >= requested (N)? → No
3. tokens_needed = N - 0 = N
4. rate = 0.0 → wait_secs = 0.0
5. sleep(0) → instant return
6. Refill: tokens = (0 + 0 * elapsed).min(0) = 0
7. → Go to step 2 → infinite loop
```

**Reachability:** The UI clamps `download_rate_limit_mbps` to `[0.1, 1000.0]`, so it's not reachable via the TUI options popup. However, a user can manually set `download_rate_limit_mbps = 0.0` in the config file, or `set_rate(0)` could be called programmatically.

**Severity:** 🟡 Low practical impact (requires manual config edit to trigger), but it's a correctness defect that would hang downloads with no error message.

**Suggested fix:**
```rust
// In acquire(), after the rate == 0 check:
let wait_secs = if state.rate > 0.0 {
    tokens_needed / state.rate
} else {
    // Rate is 0 with limiter enabled = effectively unlimited, grant immediately
    state.tokens = 0.0; // Reset tokens
    return Ok(());
};
```

Or alternatively, in `set_rate()` / `RateLimiter::new()`, treat rate=0 as "disable the limiter" by calling `self.enabled.store(false)`.

---

## 🟢 Suggestions (Non-blocking)

### 1. Channel capacity 1024 may be oversized
`mpsc::channel(1024)` is used for both download messages and status messages. For status messages, even 256 would be generous. This is fine as-is, just noting it.

### 2. `cli_args.token` is always `None` (dead code path)
The `token` field in `Cli` has `#[arg(skip)]`, so it's never populated from CLI args. The actual token comes from `HF_TOKEN` env var via `AppOptions::default()`. The comment says "populated from HF_TOKEN env var in config" which is misleading — the env var is read in `models.rs:355` during `Default::default()`, not via this field. Consider removing the field or actually populating it from `HF_TOKEN`.

### 3. Inline tuple type in headless.rs is less readable
The removed `DownloadMessage` type alias is now inlined as `(String, String, PathBuf, Option<String>, Option<String>, u64)` in 5 function signatures in `headless.rs`. Consider defining a local type alias or using a struct for clarity.

---

## ⚪ Nits (Pre-existing, not from this PR)

- `InputMode::Editing` variant is unused — the `#[allow(dead_code)]` was moved from the enum to individual fields, but `Editing` still triggers a warning.
- The `rust-toolchain.toml` pins to `nightly`, which may surprise contributors expecting stable Rust.

---

## Compilation & Tests

```
$ cargo check   ✅ (23 pre-existing warnings, no errors)
$ cargo test    ✅ (7/7 tests passed in 2.00s)
```

---

## Verdict

All previously identified issues from iterations 1 and 2 are **properly fixed**. The PR is in good shape. The only actionable finding is the 🟡 **rate-limiter infinite loop edge case** when rate=0 is enabled, which is low practical risk but worth fixing for correctness. Everything else is minor suggestions or pre-existing nits.

**Recommendation:** Approve with optional fix for the rate-limiter edge case.
