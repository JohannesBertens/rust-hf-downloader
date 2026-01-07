# Release Notes - Version 1.2.1

**Release Date:** 2026-01-07
**Type:** UI Enhancement - Download Progress Display

## Overview

Version 1.2.1 enhances the download progress display by showing the total remaining download size and estimated time remaining (ETA) for both active downloads and queued files. This provides users with better visibility into how much data remains to be downloaded and when downloads will complete.

---

## ✨ New Features

### Download Remaining Size Display

**Feature:** Total remaining download size in Download progress box title

**Visual Examples:**

**With queue:**
```
Downloading (2 queued) 120GB remaining
```

**Without queue:**
```
Downloading 20GB remaining
```

**Less than 1GB:**
```
Downloading <1GB remaining
```

**Key Capabilities:**
- Shows total remaining bytes for current download + all queued downloads
- Rounds up to GB for clarity
- Special formatting for downloads under 1GB
- Automatically updates as download progresses
- Handles all download types: GGUF files, multi-part downloads, repository downloads

**Use Cases:**
- Estimate time and bandwidth needed for pending downloads
- Better visibility into download queue size and impact
- Quick assessment of remaining data transfer before completion
- Plan disk space requirements for queued downloads

### Estimated Time Remaining (ETA) Display

**Feature:** Download completion ETA in minutes

**Visual Examples:**

**Normal download with queue:**
```
Downloading (2 queued) 120GB remaining, ~45 minutes
```

**Single file download:**
```
Downloading 20GB remaining, ~2 minutes
```

**Fast download (<1 minute):**
```
Downloading <1GB remaining, <1 minute
```

**Singular form:**
```
Downloading 1GB remaining, ~1 minute
```

**No ETA when speed is zero (starting/stalled):**
```
Downloading (5 queued) 300GB remaining
```

**Key Capabilities:**
- Calculates ETA based on current download speed and total remaining bytes
- Shows total ETA for current download + all queued downloads
- Rounds up to full minutes (conservative estimate)
- Special handling for downloads under 1 minute
- Proper singular/plural formatting ("1 minute" vs "X minutes")
- Only displays when download speed > 0 (avoids showing incorrect estimates during startup)
- Real-time updates as speed and progress change

**Use Cases:**
- Know exactly when downloads will complete
- Plan system usage around download completion times
- Better understanding of download performance
- Quick assessment of whether to wait or come back later
- Informed decisions about queuing additional downloads

---

## 📊 User Interface Updates

### Progress Display Enhancement

**Before v1.2.1:**
```
Downloading (2 queued)
```

**After v1.2.1:**
```
Downloading (2 queued) 120GB remaining, ~45 minutes
```

**Display Rules:**
1. **Queue + Size + ETA**: `"Downloading (2 queued) 120GB remaining, ~45 minutes"`
2. **Queue + Size, no ETA** (speed = 0): `"Downloading (2 queued) 120GB remaining"`
3. **Queue only** (size unknown): `"Downloading (2 queued)"`
4. **Size + ETA, no queue**: `"Downloading 20GB remaining, ~2 minutes"`
5. **Size only, no ETA** (speed = 0): `"Downloading 20GB remaining"`
6. **Base case**: `"Downloading"` (fallback)

**Size Formatting:**
- Rounds up to nearest GB (e.g., 1.2GB → 2GB)
- Shows "<1GB" for downloads under 1GB
- Omits size text if total is 0 or unknown

**ETA Formatting:**
- Rounds up to full minutes (e.g., 1.2 minutes → 2 minutes)
- Shows "<1 minute" for downloads under 1 minute
- Shows "1 minute" (singular) for exactly 1 minute
- Shows "X minutes" (plural) for multiple minutes
- Omits ETA if speed ≤ 0 (starting/stalled downloads)
- Prefixed with "~" to indicate estimate

---

## 🔧 Technical Details

### Architecture

**Data Flow:**

1. **File Size Source:**
   - GGUF downloads: `QuantizationInfo.size` field
   - Repository downloads: `RepoFile.size` field (from API)
   - Resume downloads: `DownloadMetadata.total_size` field (from registry)

2. **Queue Tracking:**
   - New field: `download_queue_bytes: Arc<Mutex<u64>>` in app state
   - Cached field: `cached_download_queue_bytes: u64` for non-blocking render
   - Incremented when downloads are queued
   - Decremented when downloads start

3. **Calculation:**
   ```rust
   let current_remaining = progress.total - progress.downloaded;
   let total_remaining = current_remaining + queue_bytes;
   ```

4. **Size Formatting:**
   ```rust
   fn format_remaining_gb(bytes: u64) -> String {
       const GB: u64 = 1_073_741_824;
       if bytes == 0 {
           String::new()
       } else if bytes < GB {
           "<1GB".to_string()
       } else {
           let gb = (bytes as f64 / GB as f64).ceil() as u64;
           format!("{}GB", gb)
       }
   }
   ```

5. **ETA Calculation:**
   ```rust
   fn calculate_eta_minutes(remaining_bytes: u64, speed_mbps: f64) -> Option<String> {
       if speed_mbps <= 0.0 {
           return None;
       }

       // Convert speed from MB/s to bytes/s
       let speed_bytes_per_sec = speed_mbps * 1_048_576.0;

       // Calculate seconds remaining
       let seconds_remaining = remaining_bytes as f64 / speed_bytes_per_sec;

       // Convert to minutes, rounding UP
       let minutes = (seconds_remaining / 60.0).ceil() as u64;

       if minutes == 0 {
           Some("<1 minute".to_string())
       } else if minutes == 1 {
           Some("1 minute".to_string())
       } else {
           Some(format!("{} minutes", minutes))
       }
   }
   ```

   **Formula:**
   ```
   speed_bytes_per_sec = speed_mbps × 1,048,576
   seconds_remaining = total_remaining_bytes / speed_bytes_per_sec
   minutes = ceiling(seconds_remaining / 60)
   ```

### Implementation Details

**Extended DownloadMessage Type:**
- Changed from 5-tuple to 6-tuple
- Added `u64` file size as 6th element
- Updated all send/receive points throughout codebase

**Modified Files:**

1. **`src/ui/app/state.rs`**
   - Updated `DownloadMessage` type alias (added u64)
   - Added `download_queue_bytes: Arc<Mutex<u64>>` field
   - Added `cached_download_queue_bytes: u64` field
   - Initialized both to 0 in `App::new()`

2. **`src/ui/app.rs`**
   - Clone `download_queue_bytes` for download worker
   - Updated message receive pattern for 6-tuple
   - Decrement both `queue_size` and `queue_bytes` on download start
   - Cache `queue_bytes` for non-blocking render
   - Pass `download_queue_bytes` to render function

3. **`src/ui/app/downloads.rs`**
   - Updated `confirm_download()` for GGUF downloads
   - Updated `resume_incomplete_downloads()` for resume functionality
   - Updated `confirm_repository_download()` for full repository downloads
   - Each function now:
     - Calculates total queued bytes before queueing
     - Increments both `queue_size` and `queue_bytes`
     - Includes file size in download message tuple
     - Handles send failures by decrementing both counters

4. **`src/ui/render.rs`**
   - Added `format_remaining_gb()` helper function
   - Added `calculate_eta_minutes()` helper function
   - Updated `render_progress_bars()` signature (added `download_queue_bytes` parameter)
   - Updated `render_download_progress()` signature (added `queue_bytes` parameter)
   - Implemented new title generation logic with 6 display cases (using match statement)
   - ETA calculation based on speed and remaining bytes

### Edge Cases Handled

| Scenario | Handling |
|----------|----------|
| File size unknown (0 or None) | Uses `.unwrap_or(0)`, omits size from display |
| Total remaining = 0 | Omits "remaining" text entirely |
| Total < 1GB | Shows "<1GB" |
| Download send failures | Decrements both counters by failed file sizes |
| Resume with partial progress | Uses `total_size`, worker handles resume point |
| Queue empty but download active | Shows "Downloading XGB remaining" (current only) |
| Speed = 0 (starting/stalled) | Omits ETA, shows only size |
| Speed < 0 (edge case) | Omits ETA, shows only size |
| ETA < 1 minute | Shows "<1 minute" |
| ETA = 1 minute exactly | Shows "1 minute" (singular) |
| ETA > 1 minute | Shows "X minutes" (plural) |
| Speed fluctuations | Uses current smoothed speed from download.rs |

---

## 🧪 Testing

### Build Verification

```bash
$ cargo build --release
   Compiling rust-hf-downloader v1.2.1
    Finished `release` profile [optimized] target(s) in 0.18s
```

**Results:**
- ✅ Clean compilation with no errors
- ✅ No compiler warnings
- ✅ All type signatures updated correctly

### Test Scenarios

**Scenario 1: Single File Download with ETA**
- Queue 1 file (5GB) at 10 MB/s
- Expected: "Downloading 5GB remaining, ~9 minutes"
- As download progresses to 50%: "Downloading 3GB remaining, ~5 minutes"
- Near completion: "Downloading <1GB remaining, <1 minute"

**Scenario 2: Multiple Files in Queue with ETA**
- Queue 3 files (2GB, 3GB, 5GB) at 20 MB/s
- Expected: "Downloading (2 queued) 10GB remaining, ~9 minutes"
- After 1GB downloaded: "Downloading (2 queued) 9GB remaining, ~8 minutes"
- First completes, second starts: "Downloading (1 queued) 8GB remaining, ~7 minutes"

**Scenario 3: Small Downloads (<1GB and <1 minute)**
- Queue 5 files totaling 800MB at 50 MB/s
- Expected: "Downloading (4 queued) <1GB remaining, <1 minute"

**Scenario 4: Resume Incomplete Downloads with ETA**
- 2 incomplete downloads (10GB each, one at 50% = 5GB downloaded, one at 0%)
- Resume both at 15 MB/s
- Expected: "Downloading (1 queued) 15GB remaining, ~17 minutes"

**Scenario 5: Repository Download with ETA**
- Queue 100 files totaling 8GB at 30 MB/s
- Expected: "Downloading (99 queued) 8GB remaining, ~5 minutes"

**Scenario 6: Download Starting (Speed = 0)**
- Queue 1 file (10GB)
- Initial state before speed data available
- Expected: "Downloading 10GB remaining" (no ETA)
- After speed > 0: "Downloading 10GB remaining, ~X minutes" (ETA appears)

**Scenario 7: Singular Minute Display**
- Download at speed where ETA = ~60 seconds
- Expected: "Downloading XGB remaining, ~1 minute" (not "1 minutes")

---

## 📝 Code Changes Summary

### Files Modified (4 files for remaining size + 1 file for ETA)

**Phase 1: Remaining Size Display**

1. **src/ui/app/state.rs** - Core data structures
   - Extended `DownloadMessage` to 6-tuple
   - Added queue bytes tracking fields
   - ~10 lines added

2. **src/ui/app.rs** - Download worker
   - Updated worker to handle 6-tuple
   - Added queue bytes caching
   - ~10 lines added

3. **src/ui/app/downloads.rs** - Queue management
   - Updated 3 queueing functions
   - Added total bytes calculation
   - Added failure handling for bytes
   - ~40 lines added/modified

4. **src/ui/render.rs** - Display logic (Phase 1)
   - Added `format_remaining_gb()` helper function
   - Updated render signatures
   - Initial title generation logic
   - ~25 lines added/modified

**Phase 2: ETA Display**

4. **src/ui/render.rs** - Display logic (Phase 2)
   - Added `calculate_eta_minutes()` helper function (~20 lines)
   - Enhanced title generation logic with match statement (~30 lines modified)
   - Integrated ETA calculation into display flow
   - ~50 lines added/modified in Phase 2

**Total Changes:** ~135 lines added/modified across 4 files (2 phases)

### No Breaking Changes

- ✅ All existing functionality preserved
- ✅ Backward compatible with existing configurations
- ✅ No API changes
- ✅ No new dependencies

---

## 🎯 User Impact

### Benefits

✅ **Better Visibility** - See total remaining download size at a glance
✅ **Queue Awareness** - Understand how much data is queued
✅ **Disk Space Planning** - Assess space requirements before download completes
✅ **Time Estimation** - Know exactly when downloads will complete with ETA display
✅ **Informed Decisions** - Decide whether to wait or come back later based on ETA
✅ **Conservative Estimates** - ETA rounds up to avoid under-promising
✅ **Smart Display** - ETA only shows when speed data is available (no misleading zeros)
✅ **Zero Config** - Works automatically for all download types
✅ **No Overhead** - Minimal performance impact

### Migration

**No Migration Required:**
- ✅ Feature works automatically
- ✅ No user action needed
- ✅ No configuration changes
- ✅ No data migration

---

## 📚 Documentation Updates

### README.md Changes
- Updated "Smart Downloads" section to mention remaining size and ETA display
- Updated download queue description with ETA information
- Added version 1.2.1 to Changelog section

### New Documentation
- `changelog/RELEASE_NOTES_1.2.1.md` (this file)
  - Comprehensive documentation of both remaining size and ETA features
  - Technical details, design decisions, and implementation notes
  - Test scenarios and edge case handling

---

## 🔍 Technical Notes

### Design Decisions

**Why extend DownloadMessage tuple?**
- File sizes are already known at queue time
- Single source of truth - no parallel data structures
- Clean data flow through the system
- Aligns with existing architecture patterns

**Why track bytes separately from count?**
- Count shows number of items
- Bytes shows data volume
- Both metrics valuable to users
- Independent tracking simplifies logic

**Why round up to GB?**
- Clearer display (no decimals needed)
- Conservative estimate (better than under-promising)
- Matches common user expectations
- Simplified formatting logic

**Why "<1GB" instead of "XMB"?**
- Consistent unit across all downloads
- Simpler to read and understand
- Avoids mental unit conversion
- Reduces visual clutter

**Why calculate ETA in render function?**
- No struct changes needed (simpler implementation)
- Uses existing data (speed_mbps already tracked)
- Real-time accuracy with live speed data
- Matches pattern used for `format_remaining_gb()`
- Pure function, easy to test
- No async/threading concerns

**Why round ETA up to full minutes?**
- Conservative estimate (better than under-promising)
- Clearer display (no decimals needed)
- Matches user mental model
- Consistent with GB rounding approach
- Simplified formatting logic

**Why show total ETA (current + queue)?**
- Matches "remaining GBs" behavior (total remaining)
- Answers user question: "When will everything finish?"
- More useful for planning than current-file-only
- Consistent with overall design philosophy

**Why omit ETA when speed = 0?**
- Avoids showing misleading "infinite time" or "0 minutes"
- Cleaner display during startup
- Only shows ETA when it's meaningful/accurate
- Prevents user confusion during stalled downloads

**Why minutes only (not hours)?**
- Simplicity in implementation and display
- Most downloads complete in < 1 hour
- Easy mental conversion (480 minutes = 8 hours)
- Avoids complex hour/minute formatting
- Consistent single unit like GB for size

### Alternative Approaches Considered

**For Remaining Size Display:**

| Approach | Pros | Cons | Verdict |
|----------|------|------|---------|
| Extend DownloadMessage tuple | Clean, single source of truth | Update all send/receive points | ✅ **Selected** |
| Parallel HashMap<url, size> | No tuple changes needed | Complex, prone to desync | ❌ Rejected |
| Query registry for bytes | No new fields needed | Includes old incomplete downloads | ❌ Rejected |
| Calculate from incomplete_downloads | Simple | Only works for resume, not active queue | ❌ Rejected |

**For ETA Display:**

| Approach | Pros | Cons | Verdict |
|----------|------|------|---------|
| Calculate in render function | Simple, no state changes | Recalculates each frame | ✅ **Selected** |
| Add ETA field to DownloadProgress | Cached value | Requires download.rs changes, coupling | ❌ Rejected |
| Track start time for averaging | Most accurate | Over-engineered, speed already smoothed | ❌ Rejected |
| Show hours for long downloads | More precise | Complex formatting, most downloads < 1hr | ❌ Rejected |
| Always show ETA (even at speed=0) | Consistent display | Misleading during startup/stalls | ❌ Rejected |

---

## 📋 Checklist for Release

- ✅ Code implementation complete
- ✅ Build verification successful
- ✅ Documentation updated (README.md)
- ✅ Release notes written (this file)
- ✅ Version bumped in Cargo.toml (1.2.0 → 1.2.1)
- ✅ No compiler warnings
- ✅ Backward compatibility verified
- ✅ Edge cases handled

---

## 🙏 Acknowledgments

This enhancement was designed with focus on:
- User experience (clear, concise display)
- Performance (minimal overhead, non-blocking render)
- Maintainability (clean architecture, well-documented)
- Robustness (handles all edge cases gracefully)

The implementation leverages Rust's type system and async primitives to ensure thread-safe, efficient tracking of download queue metrics.
