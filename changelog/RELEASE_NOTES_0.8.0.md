# Release Notes - Version 0.8.0

**Release Date:** 2025-11-23  
**Type:** Feature Release - SHA256 Verification System

## Overview

Version 0.8.0 introduces a comprehensive SHA256 hash verification system for downloaded models, ensuring file integrity and detecting corrupted downloads. This release adds automatic post-download verification with visual progress tracking and manual verification capabilities.

---

## ✨ New Features

### 1. **Automatic SHA256 Verification**
- Downloads now automatically fetch SHA256 hashes from HuggingFace
- Post-download verification queued automatically when hash is available
- Multi-part GGUF files: all parts have their hashes fetched and verified
- Verification status tracked in registry (`Complete`, `Incomplete`, `HashMismatch`)

### 2. **Manual Verification (Press 'v')**
- Users can manually verify any downloaded file using the 'v' key
- Works on selected quantization in the UI
- Checks if file exists and has expected SHA256 hash available
- Queues verification with real-time progress tracking

### 3. **Parallel Verification Worker**
- Background verification worker processes queue continuously
- Up to 2 concurrent verifications (configurable via `MAX_CONCURRENT_VERIFICATIONS`)
- Uses 128KB buffer size optimized for SSDs
- Progress updates every ~12.8MB to minimize mutex contention

### 4. **Verification Progress Display**
- Real-time progress bars for active verifications
- Shows: filename, verified bytes, total size, speed (MB/s), percentage
- Supports multiple simultaneous verifications
- Queue counter shows pending verifications
- Separate from download progress for clarity

### 5. **Enhanced Status Tracking**
- Registry now includes `expected_sha256` field for each download
- Three status states:
  - `Complete`: Download finished, hash matched (or no hash available)
  - `Incomplete`: Download interrupted/partial
  - `HashMismatch`: Download completed but hash doesn't match
- Hash mismatch detection triggers warning in status bar

### 6. **Improved UI Layout**
- Status bar expanded to 4 lines (from 3)
- Two-line status display:
  - Line 1: Model selection info (name + URL)
  - Line 2: Action status (download/verification messages)
- Clearer separation of concerns in UI

---

## 🔧 Technical Improvements

### New Module: `src/verification.rs`
- **`verification_worker()`**: Main background worker for verification queue
- **`verify_file()`**: Individual file verification with progress tracking
- **`calculate_sha256_with_progress()`**: Streaming hash calculation with UI updates
- **`queue_verification()`**: Queue verification items for processing

### API Enhancements: `src/api.rs`
- **`fetch_multipart_sha256s()`**: Batch fetch SHA256 hashes for multi-part files
- Single API call fetches hashes for all parts at once
- Returns `HashMap<String, Option<String>>` mapping filename to hash

### Download Manager Updates: `src/download.rs`
- **`ENABLE_DOWNLOAD_VERIFICATION`**: Toggle for automatic verification (default: `true`)
- Downloads now return `VerificationQueueItem` for automatic queuing
- Verification queued after download completes (if hash available)

### Registry Schema Update: `src/registry.rs`
- Added `expected_sha256: Option<String>` field to `DownloadMetadata`
- Tracks expected hash for each file
- Enables post-download verification and resume validation

### Models Update: `src/models.rs`
- **`VerificationProgress`**: Tracks active verification state
- **`VerificationQueueItem`**: Represents queued verification job
- **`DownloadStatus::HashMismatch`**: New status for failed verifications
- **`QuantizationInfo`**: Added `sha256` field

### UI State: `src/ui/app.rs`
- Added `verification_queue`, `verification_progress`, `verification_queue_size`
- Added `selection_info` for persistent model info display
- Spawned verification worker on startup
- Keybinding 'v' triggers manual verification

### Rendering: `src/ui/render.rs`
- **`render_verification_progress()`**: Displays verification progress bars
- Two-gauge system: downloads and verifications rendered separately
- Filename-based progress tracking (race-condition safe)

---

## 📋 Dependencies Added

```toml
sha2 = "0.10"    # SHA256 hashing
hex = "0.4"      # Hex encoding for hash display
```

**Total new dependencies in Cargo.lock:**
- `sha2`, `hex`, `digest`, `crypto-common`, `block-buffer`, `cpufeatures`, `generic-array`, `typenum`

---

## 🔍 Verification Correctness

**Filename-Based Progress Tracking:**
- Each verification identifies itself by filename (unique identifier)
- No index-based tracking that can become invalid
- Concurrent verifications update their own progress safely
- See `VERIFICATION_CORRECTNESS.md` for detailed race-condition analysis

**Thread Safety:**
- `Arc<Mutex<Vec<VerificationProgress>>>` protects shared state
- UI renders from snapshots (no partial updates visible)
- Search-based updates (`find(|p| p.filename == filename)`)
- Safe removal using `retain(|p| p.filename != item.filename)`

---

## 🐛 Bug Fixes

### Multi-Part SHA256 Fetching
- Fixed: Multi-part files now fetch SHA256 for each part individually
- Single API call fetches all hashes efficiently
- Each part verified independently after download

### Status Bar Clarity
- Fixed: Model selection info no longer overwritten by status messages
- Persistent line 1: Selected model info
- Dynamic line 2: Action status and errors

---

## 🎯 User Experience Improvements

### Visual Indicators
- **✓** Green checkmark for successful verification
- **✗** Red X for hash mismatch
- Clear error messages with abbreviated hashes (first 16 chars)

### Status Messages
- "Verifying integrity of {filename}..."
- "✓ Hash verified for {filename}"
- "✗ Hash mismatch for {filename}: expected abc123..., got def456..."
- "Warning: Failed to verify {filename}: {error}"

### Queue Management
- Verification queue size displayed in UI
- Shows number of pending verifications
- Clear indication of active vs queued verifications

---

## 📊 Performance Characteristics

### Verification Speed
- **Buffer Size:** 128KB (optimized for SSDs)
- **Update Frequency:** Every ~12.8MB or 0.2 seconds
- **Parallel Limit:** 2 concurrent verifications
- **Typical Speed:** 400-800 MB/s on modern SSDs

### Memory Usage
- Minimal overhead: 128KB buffer per active verification
- Max 2 concurrent = 256KB peak buffer memory

---

## 🔐 Security Enhancements

### File Integrity Validation
- SHA256 hash verification ensures file authenticity
- Detects corrupted downloads, network errors, storage issues
- Prevents using compromised or incomplete model files

### Hash Source
- Hashes fetched from HuggingFace API (`lfs.oid` field)
- Verified against official repository metadata
- Not user-editable or modifiable

---

## 📚 Documentation Updates

### New Files
- **`VERIFICATION_CORRECTNESS.md`**: Technical analysis of thread-safe verification
- **`changelog/RELEASE_NOTES_0.8.0.md`**: This document

### Updated Files
- **`README.md`**: Added verification feature description
- **`AGENTS.md`**: Updated architecture and version history
- **`Cargo.toml`**: Version bump to 0.8.0

---

## 🚀 Usage Examples

### Automatic Verification
```
1. Search for a model: press '/', type query, Enter
2. Select quantization: navigate with j/k, Tab to switch lists
3. Download: press 'd', enter path, confirm
4. Download completes → verification queued automatically
5. Progress bar shows verification status
6. Status: "✓ Hash verified for model.gguf"
```

### Manual Verification
```
1. Select downloaded model in quantization list
2. Press 'v' to verify
3. If no hash: "No SHA256 hash available, cannot verify"
4. If file missing: "File not found: /path/to/file"
5. If valid: Queue verification, show progress
```

### Multi-Part Verification
```
- Download multi-part model (e.g., 5 parts)
- Each part verified individually after download
- Status shows: "✓ Hash verified for model-Q4_K-00001-of-00005.gguf"
- All parts must pass verification
```

---

## ⚙️ Configuration

### Disable Automatic Verification
Edit `src/download.rs`:
```rust
pub const ENABLE_DOWNLOAD_VERIFICATION: bool = false;
```

**Note:** Manual verification (press 'v') always works regardless of this setting.

### Adjust Concurrent Verifications
Edit `src/verification.rs`:
```rust
const MAX_CONCURRENT_VERIFICATIONS: usize = 4;  // Default: 2
```

---

## 🔄 Migration Notes

### Registry Compatibility
- Existing `hf-downloads.toml` registries are compatible
- New `expected_sha256` field added (optional, backward compatible)
- Old entries without hashes will show as `Complete` (no verification)

### Download Behavior
- Downloads with available hashes: automatic verification
- Downloads without hashes: marked `Complete` immediately (legacy behavior)
- No breaking changes to download flow

---

## 🧪 Testing Checklist

- [x] Single file download with verification
- [x] Multi-part file download with verification
- [x] Parallel verifications (2+ files simultaneously)
- [x] Manual verification with 'v' key
- [x] Hash mismatch detection
- [x] Missing file detection
- [x] No hash available handling
- [x] Registry persistence across restarts
- [x] UI progress bar updates
- [x] Status message display

---

## 📝 Known Limitations

1. **No Retry on Hash Mismatch**
   - Files with mismatched hashes must be manually deleted and re-downloaded
   - Future enhancement: automatic re-download option

2. **Single Hash per Entry**
   - Multi-part files track hash for first part only in display
   - All parts are verified individually (implementation is correct)

3. **No Hash Validation on Resume**
   - Incomplete downloads resume without verifying already-downloaded bytes
   - Only full file is verified after completion

---

## 🎉 Contributors

**Johannes Bertens** - Initial implementation and release

---

## 📎 Related Issues

- Feature request: SHA256 verification for downloads
- Bug fix: Multi-part file hash fetching
- Enhancement: Two-line status display

---

## 🔗 Links

- **Repository:** https://github.com/JohannesBertens/rust-hf-downloader
- **Documentation:** `AGENTS.md`, `VERIFICATION_CORRECTNESS.md`
- **Previous Release:** [v0.7.5](RELEASE_NOTES_0.7.5.md)

---

**Version:** 0.8.0  
**Previous Version:** 0.7.5  
**Next Version:** TBD
