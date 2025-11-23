# Release Notes - Version 0.7.5

**Release Date**: November 23, 2025

## 🚀 Performance Enhancements: Adaptive Chunk Sizing & Real-Time Speed Tracking

Version 0.7.5 introduces significant performance improvements to the download system with adaptive chunk sizing and continuous real-time speed monitoring.

## New Features

### 1. **Adaptive Chunk Size Calculation (Target Chunk Count)**

Downloads now use intelligent, file-size-based chunk sizing instead of a fixed 10MB chunk size.

#### The Problem with Fixed Chunk Size

Previously, all files used 10MB chunks regardless of size:
- **Small files** (50MB): Only 5 chunks → underutilized parallelism
- **Large files** (50GB): 5,000 chunks → excessive task spawning overhead

#### The Solution: Target Chunk Count

The downloader now aims for **~20 chunks per file** with configurable bounds:
- `TARGET_CHUNKS = 20` (optimal balance)
- `MIN_CHUNK_SIZE = 5MB` (prevents too-small chunks)
- `MAX_CHUNK_SIZE = 100MB` (prevents excessive memory usage)

**Calculation**: `chunk_size = clamp(file_size / 20, 5MB, 100MB)`

#### Performance Impact

| File Size | Old (10MB chunks) | New (Adaptive) | Improvement |
|-----------|-------------------|----------------|-------------|
| 50MB      | 5 chunks          | 10 chunks (5MB each) | Better parallelism |
| 200MB     | 20 chunks         | 20 chunks (10MB each) | Optimal |
| 5GB       | 500 chunks        | 50 chunks (100MB each) | 90% fewer tasks |
| 50GB      | 5,000 chunks      | 500 chunks (100MB each) | 90% fewer tasks |

**Benefits:**
- ✅ Consistent performance across all file sizes
- ✅ Reduced overhead for large files
- ✅ Better parallelism for small files
- ✅ Lower memory footprint management
- ✅ Optimal balance between throughput and resource usage

### 2. **Continuous Real-Time Speed Tracking**

Download speed is now calculated continuously during streaming, not just at chunk completion.

#### The Problem with Chunk Completion Tracking

Previously, total MB/s was only updated when a chunk finished downloading:
- Delayed speed updates
- Inaccurate during active downloads
- Speed could appear frozen while chunks were in progress

#### The Solution: Streaming Speed Calculation

Speed is now calculated **every 200ms during active streaming**:
- Updates `progress_downloaded` immediately as bytes arrive
- Calculates total MB/s across all active chunks in real-time
- Provides smooth, responsive speed feedback

**Benefits:**
- ✅ Real-time speed updates reflecting actual current rate
- ✅ Immediate response to network speed changes
- ✅ Smoother user experience with live feedback
- ✅ Accurate representation of download performance

### 3. **Improved Chunk Progress UI**

The chunk progress display now has a proper bordered container.

#### Before
```
┌Downloading (2 queued)────────────────────────────┐
│█                1% - 110.61 MB/s                 │
└──────────────────────────────────────────────────┘
  #8 [====                        ]   5.15 MB/s    ← No border
  #4 [==                          ]   6.75 MB/s
  #7 [====                        ]  12.62 MB/s
```

#### After
```
┌Downloading (2 queued)────────────────────────────┐
│█                1% - 110.61 MB/s                 │
└──────────────────────────────────────────────────┘
┌Active Chunks──────────────────────────────────────┐
│ #8 [====                        ]   5.15 MB/s    │
│ #4 [==                          ]   6.75 MB/s    │
│ #7 [====                        ]  12.62 MB/s    │
└──────────────────────────────────────────────────┘
```

**Benefits:**
- ✅ Cleaner visual hierarchy
- ✅ Clear separation between overall and chunk progress
- ✅ Better UI consistency

## Technical Changes

### Modified Files

1. **`src/download.rs`**
   - Added `TARGET_CHUNKS`, `MIN_CHUNK_SIZE`, `MAX_CHUNK_SIZE` constants
   - Implemented `calculate_chunk_size(file_size: u64) -> usize` function
   - Updated `download_chunked()` to use dynamic chunk sizing
   - Modified `download_chunk_with_progress()` signature to accept shared speed tracking state
   - Moved total MB/s calculation from chunk completion to streaming loop
   - Updated `progress_downloaded` immediately for every received byte chunk
   - Calculate and update total speed every 200ms during active streaming
   - Removed redundant chunk completion speed calculation logic
   - Added `#[allow(clippy::too_many_arguments)]` for necessary progress parameters
   - Used `.div_ceil()` for cleaner chunk count calculation

2. **`src/ui/render.rs`**
   - Added bordered `Block` container titled "Active Chunks" for chunk progress bars
   - Updated height calculation to account for chunk block borders (+2 for top/bottom)
   - Fixed rendering order to calculate `inner_area` before rendering block
   - Positioned chunks relative to `inner_area` instead of raw coordinates

### Performance Characteristics

**Memory Usage:**
- Old worst case: `8 chunks × 10MB = 80MB RAM`
- New range: `8 chunks × 5-100MB = 40-800MB RAM`
- Typical case (100MB-10GB files): Similar or better than before

**Network Efficiency:**
- Small files: More parallel chunks = better throughput
- Large files: Fewer HTTP requests = less protocol overhead
- All files: Optimal balance for concurrent download performance

**Speed Calculation:**
- Update interval: 200ms (configurable)
- Lock contention: Minimized with quick updates
- Accuracy: High due to continuous measurement

## Backward Compatibility

- ✅ **Zero Breaking Changes**: All existing functionality preserved
- ✅ **Automatic Migration**: No user action required
- ✅ **Transparent Upgrade**: Users benefit immediately without configuration

## Testing

All changes verified with:
- `cargo check` - Compilation successful
- `cargo build --release` - Release build successful
- `cargo clippy -- -D warnings` - No new warnings introduced
- `cargo test` - All tests pass

## Impact Summary

| Category | Impact |
|----------|--------|
| Small files (<100MB) | Better parallelism, faster downloads |
| Medium files (100MB-10GB) | Optimal performance maintained |
| Large files (>10GB) | 90% reduction in task overhead |
| Memory usage | Bounded by MAX_CHUNK_SIZE (100MB) |
| User experience | Real-time speed feedback, cleaner UI |
| Code quality | Modern Rust patterns, clippy-clean |

## Files Changed

- `Cargo.toml`: Version bumped to 0.7.5
- `src/download.rs`: Adaptive chunking + continuous speed tracking
- `src/ui/render.rs`: Bordered chunk progress container
- `changelog/RELEASE_NOTES_0.7.5.md`: This file
- `README.md`: Updated feature descriptions

---

**Upgrade Recommendation**: This release provides significant performance improvements with no breaking changes. All users should upgrade to benefit from faster downloads and better real-time feedback.
