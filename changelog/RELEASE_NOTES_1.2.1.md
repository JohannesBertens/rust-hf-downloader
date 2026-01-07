# Release Notes - Version 1.2.1

**Release Date:** 2026-01-07
**Type:** UI Enhancement - Download Progress Display

## Overview

Version 1.2.1 enhances the download progress display by showing the total remaining download size for both active downloads and queued files. This provides users with better visibility into how much data remains to be downloaded.

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

---

## 📊 User Interface Updates

### Progress Display Enhancement

**Before v1.2.1:**
```
Downloading (2 queued)
```

**After v1.2.1:**
```
Downloading (2 queued) 120GB remaining
```

**Display Rules:**
1. **Queue + Remaining**: `"Downloading (2 queued) 120GB remaining"`
2. **Queue only** (size unknown): `"Downloading (2 queued)"`
3. **Remaining only** (no queue): `"Downloading 20GB remaining"`
4. **Neither**: `"Downloading"` (fallback)

**Size Formatting:**
- Rounds up to nearest GB (e.g., 1.2GB → 2GB)
- Shows "<1GB" for downloads under 1GB
- Omits size text if total is 0 or unknown

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

4. **Formatting:**
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
   - Updated `render_progress_bars()` signature (added `download_queue_bytes` parameter)
   - Updated `render_download_progress()` signature (added `queue_bytes` parameter)
   - Implemented new title generation logic with 4 display cases

### Edge Cases Handled

| Scenario | Handling |
|----------|----------|
| File size unknown (0 or None) | Uses `.unwrap_or(0)`, omits size from display |
| Total remaining = 0 | Omits "remaining" text entirely |
| Total < 1GB | Shows "<1GB" |
| Download send failures | Decrements both counters by failed file sizes |
| Resume with partial progress | Uses `total_size`, worker handles resume point |
| Queue empty but download active | Shows "Downloading XGB remaining" (current only) |

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

**Scenario 1: Single File Download**
- Queue 1 file (5GB)
- Expected: "Downloading 5GB remaining"
- As download progresses to 50%: "Downloading 3GB remaining"

**Scenario 2: Multiple Files in Queue**
- Queue 3 files (2GB, 3GB, 5GB)
- Expected: "Downloading (2 queued) 10GB remaining"
- After 1GB downloaded: "Downloading (2 queued) 9GB remaining"
- First completes, second starts: "Downloading (1 queued) 8GB remaining"

**Scenario 3: Small Downloads (<1GB)**
- Queue 5 files totaling 800MB
- Expected: "Downloading (4 queued) <1GB remaining"

**Scenario 4: Resume Incomplete Downloads**
- 2 incomplete downloads (10GB each, one at 50% = 5GB downloaded, one at 0%)
- Resume both
- Expected: "Downloading (1 queued) 15GB remaining"

**Scenario 5: Repository Download**
- Queue 100 files totaling 8GB
- Expected: "Downloading (99 queued) 8GB remaining"

---

## 📝 Code Changes Summary

### Files Modified (4 files)

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

4. **src/ui/render.rs** - Display logic
   - Added formatting helper function
   - Updated render signatures
   - New title generation logic
   - ~25 lines added/modified

**Total Changes:** ~85 lines added/modified across 4 files

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
✅ **Time Estimation** - Combined with speed, estimate completion time
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
- Updated "Smart Downloads" section to mention remaining size display
- Updated download queue description
- Added version 1.2.1 to Changelog section

### New Documentation
- `changelog/RELEASE_NOTES_1.2.1.md` (this file)

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

### Alternative Approaches Considered

| Approach | Pros | Cons | Verdict |
|----------|------|------|---------|
| Extend DownloadMessage tuple | Clean, single source of truth | Update all send/receive points | ✅ **Selected** |
| Parallel HashMap<url, size> | No tuple changes needed | Complex, prone to desync | ❌ Rejected |
| Query registry for bytes | No new fields needed | Includes old incomplete downloads | ❌ Rejected |
| Calculate from incomplete_downloads | Simple | Only works for resume, not active queue | ❌ Rejected |

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
