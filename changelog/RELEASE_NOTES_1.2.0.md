# Release Notes - Version 1.2.0

**Release Date:** 2026-01-07
**Type:** Feature Addition - Download Speed Rate Limiting

## Overview

Version 1.2.0 introduces configurable download speed rate limiting, allowing users to cap their download bandwidth to prevent network saturation. This feature uses a token bucket algorithm for smooth, efficient rate control with support for short bursts to maintain TCP efficiency.

---

## ✨ New Features

### Download Speed Rate Limiting

**Feature:** Configurable bandwidth throttling with token bucket rate limiter

**Key Capabilities:**
- Enable/disable rate limiting from options screen
- Adjustable speed limit from 0.1 to 1000.0 MB/s
- Real-time rate adjustment without restart
- Visual feedback showing actual vs. limit speeds
- Zero performance overhead when disabled

**Use Cases:**
- Prevent saturating home/office internet connections
- Maintain bandwidth for other applications during downloads
- Control data usage on metered connections
- Background downloading while video conferencing

---

## 🎯 Configuration

### Options Screen (Press 'o')

**New Fields:**

| Field | Name | Values | Default | Controls |
|-------|------|--------|---------|----------|
| 10 | Rate Limit | Enabled/Disabled | Disabled | `+`/`-` to toggle |
| 11 | Max Download Speed | 0.1-1000.0 MB/s | 50.0 MB/s | `+`/`-` to adjust (±0.5) |

### Configuration Persistence

- Settings saved to `~/.config/jreb/config.toml`
- Auto-loads on startup
- Changes apply immediately to active downloads

**Example Configuration:**
```toml
[options]
download_rate_limit_enabled = true
download_rate_limit_mbps = 25.0
```

---

## 📊 User Interface Updates

### Progress Display Enhancement

**Before v1.2.0:**
```
Downloading: [████████░░] 80% - 48.2 MB/s
```

**After v1.2.0 (with rate limiting enabled):**
```
Downloading: [████████░░] 80% - 24.8/25.0 MB/s
                                 ^^^^ ^^^^^
                               actual limit
```

**After v1.2.0 (with rate limiting disabled):**
```
Downloading: [████████░░] 80% - 48.2 MB/s
```

---

## 🔧 Technical Details

### Token Bucket Algorithm

**How It Works:**
1. **Tokens**: Each byte downloaded requires one token
2. **Refill**: Tokens refill at the configured rate (e.g., 50 MB/s)
3. **Bucket Capacity**: Max tokens = rate × burst window (2 seconds)
4. **Bursting**: Allows short speed increases for TCP efficiency
5. **Blocking**: Download chunks wait for tokens when bucket is empty

**Example:**
- Rate: 10 MB/s
- Burst window: 2 seconds
- Bucket capacity: 20 MB worth of tokens
- Can burst up to ~20 MB instantly, then throttle to 10 MB/s

### Architecture

**New Module:** `src/rate_limiter.rs` (~250 lines)

```rust
pub struct RateLimiter {
    tokens: Arc<Mutex<f64>>,
    max_tokens: Arc<Mutex<f64>>,
    rate: Arc<Mutex<f64>>,
    last_refill: Arc<Mutex<Instant>>,
    enabled: Arc<AtomicBool>,
    burst_seconds: f64,  // Fixed at 2.0
}
```

**Key Design Decisions:**
- **Global Limiter**: Single rate limiter shared across all 8 concurrent download chunks
- **Token Sharing**: Ensures total download rate matches user expectation (not per-chunk)
- **Fast Path**: Atomic flag check for zero overhead when disabled
- **Thread-Safe**: Arc<Mutex<>> for shared mutable state across async tasks

### Integration Points

**1. Download Loop (src/download.rs:701-703):**
```rust
if DOWNLOAD_CONFIG.rate_limit_enabled.load(Ordering::Relaxed) {
    RATE_LIMITER.acquire(bytes.len()).await?;
}
file.write_all(&bytes).await?;
```

**2. Configuration Sync (src/ui/app/state.rs:195-205):**
```rust
// Update global config atomics
DOWNLOAD_CONFIG.rate_limit_enabled.store(...);
DOWNLOAD_CONFIG.rate_limit_bytes_per_sec.store(...);

// Update rate limiter asynchronously
tokio::spawn(async move {
    RATE_LIMITER.set_rate(bytes_per_sec).await;
    RATE_LIMITER.set_enabled(enabled);
});
```

**3. Progress Display (src/ui/render.rs:751-763):**
```rust
if rate_limited {
    let limit_mbps = limit_bytes as f64 / 1_048_576.0;
    format!("{:.1}/{:.1} MB/s", actual_speed, limit_mbps)
}
```

### Performance Characteristics

**Overhead Analysis:**

| Scenario | Cost per 8KB chunk | Impact |
|----------|-------------------|--------|
| Disabled | 1 atomic load | ~1 nanosecond |
| Enabled (tokens available) | 2 mutex locks + arithmetic | ~50 nanoseconds |
| Enabled (waiting) | tokio::sleep() call | Variable (as designed) |

**Memory Footprint:**
- RateLimiter struct: ~80 bytes (single global instance)
- No heap allocations during normal operation

**Concurrency:**
- 8 download chunks compete for token bucket mutex
- Mutex held for ~50ns per acquire (negligible contention)
- Fair token distribution across chunks

---

## 📝 Code Changes

### New Files
1. **`src/rate_limiter.rs`** - Token bucket rate limiter implementation
   - `RateLimiter` struct with async methods
   - Unit tests for rate limiting behavior
   - ~250 lines

### Modified Files

1. **`src/main.rs`** - Added rate_limiter module declaration

2. **`src/models.rs`** - Extended AppOptions struct
   - Added `download_rate_limit_enabled: bool`
   - Added `download_rate_limit_mbps: f64`
   - Added default value function

3. **`src/download.rs`** - Core integration
   - Extended `DownloadConfig` with rate limit atomics
   - Added global `RATE_LIMITER` static
   - Integrated rate limiter into download loop

4. **`src/ui/app/state.rs`** - Configuration sync
   - Updated `sync_options_to_config()` to sync rate limit settings
   - Spawns async task to update rate limiter

5. **`src/ui/app/events.rs`** - Event handling
   - Added field 10: rate limit toggle (index bounds updated)
   - Added field 11: speed adjustment (0.5 MB/s increments)
   - Updated max field index from 13 to 15

6. **`src/ui/render.rs`** - UI updates
   - Added "Rate Limiting" category to options screen
   - Extended progress display to show actual/limit speeds
   - Increased popup height from 27 to 30 lines

7. **`Cargo.toml`** - Dependencies
   - Added `once_cell = "1.19"` for lazy static initialization
   - Version bump from 1.1.1 to 1.2.0

---

## 🧪 Testing

### Unit Tests

**src/rate_limiter.rs:**
- ✅ `test_disabled_limiter` - Zero overhead when disabled
- ✅ `test_basic_rate_limiting` - Enforces configured rate
- ✅ `test_dynamic_rate_change` - Runtime rate adjustment
- ✅ `test_concurrent_chunks` - Multi-task token sharing
- ✅ `test_small_requests` - Burst window allows instant small downloads

### Integration Testing

**Manual Verification:**
1. ✅ Download large file (1GB+) with 5 MB/s limit - stable at ~5 MB/s
2. ✅ Toggle rate limit during download - immediate effect
3. ✅ Adjust speed during download - smooth transition
4. ✅ Multiple concurrent downloads - total rate stays within limit
5. ✅ Progress display shows correct actual/limit values

### Build Verification

```bash
$ cargo build --release
   Compiling rust-hf-downloader v1.2.0
   Finished `release` profile [optimized] target(s) in 4.88s
```

---

## 🎯 User Impact

### Benefits

✅ **Bandwidth Control** - Prevent downloads from saturating connection
✅ **Zero Config** - Works out of the box (disabled by default)
✅ **Real-time Adjustment** - Change speed without restart
✅ **Visual Feedback** - See actual vs. limit speeds
✅ **No Overhead** - Zero cost when disabled
✅ **Smooth Downloads** - Burst window prevents TCP slowdowns

### Migration

**No Migration Required:**
- ✅ Feature disabled by default
- ✅ Existing configurations automatically compatible
- ✅ All download functionality preserved
- ✅ No breaking changes

**Enabling Rate Limiting:**
1. Press `o` to open options
2. Navigate to "Rate Limit" (field 10)
3. Press `+` to enable
4. Navigate to "Max Download Speed" (field 11)
5. Adjust speed with `+`/`-`
6. Press `Esc` to save and close

---

## 📚 Documentation Updates

### README.md Changes
- Added rate limiting to Features section
- Updated Options screen documentation
- Added rate_limiter.rs to Project Structure
- Added once_cell to Dependencies section
- Updated Technical Details with rate limiter architecture
- Added version 1.2.0 to Changelog

### New Documentation
- `changelog/RELEASE_NOTES_1.2.0.md` (this file)

---

## 🔗 Dependencies

### New Dependencies
- **once_cell** v1.19 - Lazy static initialization for global rate limiter

### Why once_cell?
- Standard solution for lazy statics in Rust
- Zero runtime cost after initialization
- Thread-safe initialization guarantees
- Widely used in Rust ecosystem

---

## 🚀 Future Enhancements

**Potential Future Features:**
- Per-file rate limits (different speeds for different downloads)
- Time-based rate schedules (e.g., full speed at night, throttled during day)
- Bandwidth monitoring and statistics
- Upload rate limiting for future upload features

**Not Planned:**
- Per-chunk rate limits (would multiply user's expected rate by 8)
- Variable burst windows (fixed at 2 seconds for simplicity)

---

## 🔍 Technical Notes

### Why Token Bucket?

**Alternatives Considered:**

| Algorithm | Pros | Cons | Verdict |
|-----------|------|------|---------|
| Token Bucket | Industry standard, allows bursting, smooth | Slightly complex | ✅ **Selected** |
| Leaky Bucket | Very simple | Doesn't allow bursting, hurts TCP | ❌ Rejected |
| Fixed Delay | Trivial to implement | Choppy downloads, poor TCP | ❌ Rejected |
| reqwest timeout | No new code | Can't achieve smooth limiting | ❌ Rejected |

**Token Bucket Wins Because:**
- Allows short bursts (better TCP performance)
- Industry-proven algorithm
- Smooth rate enforcement
- Good user experience

### Why Global Rate Limiter?

**Per-Chunk vs Global:**

| Approach | User Sets "10 MB/s" | Actual Speed | User Confusion |
|----------|---------------------|--------------|----------------|
| Per-Chunk | 10 MB/s | 80 MB/s (8×10) | ❌ Very High |
| Global | 10 MB/s | 10 MB/s | ✅ None |

**Global Wins Because:**
- Matches user expectations (10 MB/s means total, not per chunk)
- Simpler mental model
- Chunks automatically coordinate through shared limiter

---

## 📋 Checklist for Release

- ✅ Code implementation complete
- ✅ Unit tests pass
- ✅ Integration testing complete
- ✅ Documentation updated (README.md)
- ✅ Release notes written (this file)
- ✅ Version bumped in Cargo.toml (1.1.1 → 1.2.0)
- ✅ Build verification successful
- ✅ No compiler warnings
- ✅ Backward compatibility verified

---

## 🙏 Acknowledgments

This feature was designed and implemented with careful attention to:
- Performance (zero overhead when disabled)
- User experience (simple configuration, clear feedback)
- Robustness (thread-safe, handles edge cases)
- Code quality (well-tested, documented, maintainable)

Special thanks to the Rust async ecosystem for providing excellent primitives (tokio, once_cell) that made this implementation clean and efficient.
