# Mutex Usage Performance Fixes

This document outlines the plan to optimize mutex usage across the codebase for improved performance and reduced contention.

## Overview

| Fix | Priority | Effort | Files Affected | Status |
|-----|----------|--------|----------------|--------|
| 1. Consolidate RateLimiter state | High | Medium | `src/rate_limiter.rs` | ✅ Complete (tested) |
| 2. Use AtomicUsize for verification_queue_size | High | Medium | Multiple files | ✅ Complete (tested) |
| 3. Use parking_lot crate | Medium | Low | `Cargo.toml`, multiple source files | ✅ Complete (tested) |
| 4. Consolidate verification queue operations | Low | Low | `src/verification.rs` | ✅ Complete (tested) |

**Last Updated:** 2026-02-01 - All fixes implemented and verified

---

## Fix 1: Consolidate RateLimiter State

**Problem:** The `RateLimiter` struct uses 4 separate `Arc<Mutex<T>>` fields (`tokens`, `max_tokens`, `rate`, `last_refill`), requiring multiple sequential lock acquisitions in `refill()` and `set_rate()`.

**Solution:** Consolidate into a single `Mutex<RateLimiterState>` struct.

### Checklist

- [x] **1.1** Create new `RateLimiterState` struct in `src/rate_limiter.rs`:
  ✅ Implemented - Created `RateLimiterState` with tokens, max_tokens, rate, last_refill fields
  ```rust
  struct RateLimiterState {
      tokens: f64,
      max_tokens: f64,
      rate: f64,
      last_refill: Instant,
  }
  ```

- [x] **1.2** Update `RateLimiter` struct:
  ✅ Implemented - Changed from 4 separate `Arc<Mutex<T>>` fields to single `Arc<Mutex<RateLimiterState>>`
  ```rust
  pub struct RateLimiter {
      state: Arc<Mutex<RateLimiterState>>,
      enabled: Arc<AtomicBool>,
      burst_seconds: f64,
  }
  ```

- [x] **1.3** Update `RateLimiter::new()`:
  ✅ Implemented - Initializes consolidated state with single mutex
  ```rust
  pub fn new(rate_bytes_per_sec: u64, burst_seconds: f64) -> Self {
      let rate = rate_bytes_per_sec as f64;
      let max_tokens = rate * burst_seconds;
      
      Self {
          state: Arc::new(Mutex::new(RateLimiterState {
              tokens: max_tokens,
              max_tokens,
              rate,
              last_refill: Instant::now(),
          })),
          enabled: Arc::new(AtomicBool::new(false)),
          burst_seconds,
      }
  }
  ```

- [x] **1.4** Refactor `acquire()` method to use single lock:
  ✅ Implemented - Refill and token acquisition in single lock acquisition
  ```rust
  pub async fn acquire(&self, bytes: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
      if !self.enabled.load(Ordering::Relaxed) {
          return Ok(());
      }
      
      let requested = bytes as f64;
      
      loop {
          let wait_duration = {
              let mut state = self.state.lock().await;
              
              // Refill tokens
              let now = Instant::now();
              let elapsed = now.duration_since(state.last_refill).as_secs_f64();
              if elapsed > 0.0 {
                  state.tokens = (state.tokens + state.rate * elapsed).min(state.max_tokens);
                  state.last_refill = now;
              }
              
              // Check if we have enough tokens
              if state.tokens >= requested {
                  state.tokens -= requested;
                  return Ok(());
              }
              
              // Calculate wait time
              let tokens_needed = requested - state.tokens;
              Duration::from_secs_f64(tokens_needed / state.rate)
          };
          
          tokio::time::sleep(wait_duration).await;
      }
  }
  ```

- [x] **1.5** Refactor `set_rate()` method:
  ✅ Implemented - Updates rate, max_tokens, and caps tokens in single lock acquisition
  ```rust
  pub async fn set_rate(&self, rate_bytes_per_sec: u64) {
      let new_rate = rate_bytes_per_sec as f64;
      let new_max = new_rate * self.burst_seconds;
      
      let mut state = self.state.lock().await;
      state.rate = new_rate;
      state.max_tokens = new_max;
      if state.tokens > new_max {
          state.tokens = new_max;
      }
  }
  ```

- [x] **1.6** Remove the separate `refill()` method (now inlined in `acquire()`)
  ✅ Implemented - Removed separate `refill()` method, logic inlined in `acquire()`

- [x] **1.7** Update all tests in `src/rate_limiter.rs` to work with new structure
  ✅ Implemented - Tests updated and verified passing

- [x] **1.8** Run tests: `cargo test rate_limiter`
  ✅ All 5 tests pass

- [x] **1.9** Verify no compile errors: `cargo check`
  ✅ Verified - code compiles successfully

---

## Fix 2: Use AtomicUsize for verification_queue_size

**Problem:** `verification_queue_size` is a simple counter wrapped in `Arc<Mutex<usize>>`, requiring async lock acquisition for increment/decrement operations.

**Solution:** Replace with `Arc<AtomicUsize>` for lock-free atomic operations.

### Checklist

- [x] **2.1** Update `src/ui/app/state.rs` - change field type:
  ```rust
  // Before
  pub verification_queue_size: Arc<Mutex<usize>>,
  
  // After
  pub verification_queue_size: Arc<AtomicUsize>,
  ```

- [x] **2.2** Update `src/ui/app/state.rs` - add import:
  ```rust
  use std::sync::atomic::AtomicUsize;
  ```

- [x] **2.3** Update `App::new()` initialization in `src/ui/app/state.rs`:
  ```rust
  // Before
  verification_queue_size: Arc::new(Mutex::new(0)),
  
  // After
  verification_queue_size: Arc::new(AtomicUsize::new(0)),
  ```

- [x] **2.4** Update `src/verification.rs` - change `queue_verification()` function:
  ```rust
  pub async fn queue_verification(
      verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
      verification_queue_size: Arc<AtomicUsize>,  // Changed type
      item: VerificationQueueItem,
  ) {
      let mut queue = verification_queue.lock().await;
      queue.push(item);
      verification_queue_size.fetch_add(1, Ordering::Relaxed);  // Atomic increment
  }
  ```

- [x] **2.5** Update `src/verification.rs` - change `verification_worker()` signature and decrement:
  ```rust
  pub async fn verification_worker(
      verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
      verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
      verification_queue_size: Arc<AtomicUsize>,  // Changed type
      status_tx: mpsc::UnboundedSender<String>,
      download_registry: Arc<Mutex<DownloadRegistry>>,
  ) {
      // ...
      if let Some(item) = item {
          // Decrement queue size atomically
          verification_queue_size.fetch_sub(1, Ordering::Relaxed);
          // ... rest of code
      }
  }
  ```

- [x] **2.6** Update `src/verification.rs` - add import:
  ```rust
  use std::sync::atomic::{AtomicUsize, Ordering};
  ```

- [x] **2.7** Update `src/ui/app.rs` - change `try_lock()` to atomic load in `draw()`:
  ```rust
  // Before
  let verification_queue_size = self
      .verification_queue_size
      .try_lock()
      .map(|guard| {
          self.cached_verification_queue_size = *guard;
          *guard
      })
      .unwrap_or(self.cached_verification_queue_size);
  
  // After
  let verification_queue_size = self.verification_queue_size.load(Ordering::Relaxed);
  ```

- [x] **2.8** Update `src/ui/app/state.rs` - remove `cached_verification_queue_size` field (no longer needed)

- [x] **2.9** Update `src/ui/app/state.rs` - add Ordering import for atomic operations:
  ```rust
  use std::sync::atomic::Ordering;
  ```

- [x] **2.10** Update `src/download.rs` - change `DownloadParams` field type:
  ```rust
  pub verification_queue_size: Arc<AtomicUsize>,
  ```

- [x] **2.11** Update `src/main.rs` - change initialization for headless mode:
  ```rust
  let verification_queue_size = std::sync::Arc::new(AtomicUsize::new(0));
  ```

- [x] **2.12** Update `src/headless.rs` - change function signatures and usages:
  - `run_download()` - change parameter type
  - `wait_for_verification()` - change parameter type and use `.load()` instead of `*lock()`
  - `run_resume()` - change parameter type

- [x] **2.13** Run full test suite: `cargo test` - all 7 tests pass

- [x] **2.14** Verify no compile errors: `cargo check` - compiles successfully

---

## Fix 3: Use parking_lot Crate

**Problem:** `std::sync::RwLock` and `std::sync::Mutex` have overhead from poisoning support and are slightly slower than alternatives.

**Solution:** Replace with `parking_lot` crate equivalents for better performance.

### Checklist

- [x] **3.1** Add dependency to `Cargo.toml`:
  ```toml
  [dependencies]
  parking_lot = "0.12"
  ```

- [x] **3.2** Update `src/ui/app/state.rs` imports:
  ```rust
  // Before
  use std::sync::RwLock;
  
  // After
  use parking_lot::RwLock;
  ```

- [x] **3.3** Remove `.unwrap()` calls after `read()` and `write()` (parking_lot doesn't have poisoning):
  
  Search and replace pattern in affected files:
  ```rust
  // Before
  .read().unwrap()
  .write().unwrap()
  
  // After
  .read()
  .write()
  ```

- [x] **3.4** Update files using RwLock:
  - [x] `src/ui/app/state.rs`
  - [x] `src/ui/app/events.rs`
  - [x] `src/ui/app/models.rs`
  - [x] `src/ui/app/downloads.rs`
  - [x] `src/ui/app.rs`
  - [x] `src/ui/app/verification.rs`
  - [x] `src/ui/render.rs` (not needed - doesn't use RwLock directly)

- [x] **3.5** Run full test suite: `cargo test` - all 7 tests pass

- [x] **3.6** Verify no compile errors: `cargo check` - compiles successfully

- [ ] **3.7** (Optional) Benchmark before/after with `cargo bench` or manual testing

---

## Fix 4: Consolidate Verification Queue Operations

**Problem:** In `verification_worker()`, queue item removal and size decrement are done with separate lock acquisitions.

**Solution:** Perform both operations while holding the queue lock (after Fix 2, size is atomic so this is even simpler).

### Checklist

- [x] **4.1** Update `verification_worker()` in `src/verification.rs`:
  ✅ Implemented - Queue removal and size decrement now happen atomically while holding the lock
  ```rust
  loop {
      let item = {
          let mut queue = verification_queue.lock().await;
          if queue.is_empty() {
              None
          } else {
              let item = queue.remove(0);
              // Decrement size atomically while holding the lock
              verification_queue_size.fetch_sub(1, Ordering::Relaxed);
              Some(item)
          }
      };
      
      // ... rest unchanged
  }
  ```

- [x] **4.2** Run tests: `cargo test verification` - 0 verification tests found (all 7 tests pass)

- [x] **4.3** Verify no compile errors: `cargo check` - compiles successfully

---

## Testing Plan

After implementing all fixes:

- [x] Run full test suite: `cargo test` - all 7 tests pass
- [x] Run clippy: `cargo clippy` - no warnings
- [x] Verify no compile errors: `cargo check` - compiles successfully
- [x] Code review confirms all fixes implemented correctly

### Implementation Summary

All 4 mutex optimization fixes have been successfully implemented:

1. **RateLimiter state consolidated** - Single mutex for tokens, max_tokens, rate, last_refill
2. **AtomicUsize for verification_queue_size** - Lock-free atomic operations for queue size counter
3. **parking_lot crate adopted** - Using `parking_lot::RwLock` for better performance (no poisoning overhead)
4. **Verification queue operations consolidated** - Item removal and size decrement happen atomically while holding lock

### Files Modified

- `Cargo.toml` - Added `parking_lot = "0.12"` dependency
- `src/rate_limiter.rs` - Consolidated state into single mutex
- `src/verification.rs` - AtomicUsize for queue size, consolidated operations
- `src/ui/app/state.rs` - parking_lot RwLock, AtomicUsize for queue size
- `src/ui/app/models.rs` - parking_lot RwLock (no unwrap needed)
- `src/ui/app/events.rs` - parking_lot RwLock (no unwrap needed)
- `src/ui/app/downloads.rs` - parking_lot RwLock (no unwrap needed)
- `src/ui/app.rs` - AtomicUsize load for verification queue size
- `src/download.rs` - AtomicUsize in DownloadParams
- `src/main.rs` - AtomicUsize initialization
- `src/headless.rs` - AtomicUsize usage with load()

---

## Rollback Plan

If issues arise, fixes can be reverted independently:

1. **Fix 1:** Revert `src/rate_limiter.rs` to use separate mutex fields
2. **Fix 2:** Revert to `Arc<Mutex<usize>>` for verification_queue_size
3. **Fix 3:** Remove parking_lot dependency and restore std::sync imports
4. **Fix 4:** Separate queue operations again (minimal impact)

---

## Notes

- Fixes are ordered by priority and can be implemented incrementally
- Fix 2 should be completed before Fix 4 (Fix 4 assumes atomic size counter)
- Fix 3 (parking_lot) is optional but recommended for additional performance
- All fixes maintain the existing lock hierarchy documented in AGENTS.md
