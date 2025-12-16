# Release Notes - Version 1.1.1 (2025-12-16)

## 🐛 Bug Fixes

### Fixed GGUF File Path Duplication Issue

**Problem**: Downloads of GGUF files with subdirectory paths (e.g., `UD-Q6_K_XL/model.gguf`) resulted in double folder names like `~/models/author/model/UD-Q6_K_XL/UD-Q6_K_XL/model.gguf` instead of the expected `~/models/author/model/UD-Q6_K_XL/model.gguf`.

**Root Cause**: 
- HuggingFace API returns filenames that include their subdirectory paths (e.g., `"path":"Q4_1/model.gguf"`)
- The download logic was constructing `base_path` that included the file's subdirectory
- `start_download()` then joined this `base_path` with the full filename (which also contains the subdirectory)
- This resulted in the subdirectory appearing twice in the final file path

**Solution**: Modified both download entry points to calculate `base_path` as `base/author/model_name` only, without including the file's subdirectory. The subdirectory is preserved in the filename and appended once by `start_download()`.

#### Fixed Code Paths

1. **`src/ui/app/downloads.rs` - `confirm_download()` method (lines 137-155)**
   - **Before**: Used `validate_and_sanitize_path().parent()` which included file subdirectory
   - **After**: Directly calculates `model_path = base/author/model_name` by splitting model_id
   - **Result**: File's subdirectory is preserved in filename and appended once

2. **`src/ui/app/downloads.rs` - `resume_incomplete_downloads()` method (lines 278-302)**
   - **Before**: Derived base_path from `metadata.local_path.parent()` which included subdirectory
   - **After**: Calculates base_path from `model_id` as `default_dir/author/model_name`
   - **Added**: Fallback logic if model_id format is unexpected
   - **Result**: Consistent with `confirm_download()` approach

3. **`src/ui/app/downloads.rs` - `confirm_repository_download()`**
   - **Status**: Already using correct pattern (no changes needed)

#### Technical Details

- **Files affected**: `src/ui/app/downloads.rs`
- **Functions modified**: `confirm_download()`, `resume_incomplete_downloads()`
- **Pattern used**: `base/author/model_name` as base_path (without file subdirectory)
- **Verification**: Comprehensive audit of all `download_tx.send()` calls completed
- **Testing**: Compilation verified with `cargo check`

#### User Impact

**Before Fix:**
```
~/models/author/model/UD-Q6_K_XL/UD-Q6_K_XL/model.gguf  (Double path)
```

**After Fix:**
```
~/models/author/model/UD-Q6_K_XL/model.gguf  (Correct path)
```

## 🔄 Migration Instructions

For users experiencing this issue:

1. **Delete existing broken directory structure**:
   ```bash
   # Example: Remove duplicated subdirectories
   rm -rf ~/models/author/model/UD-Q6_K_XL/UD-Q6_K_XL/
   ```

2. **Rebuild the application**:
   ```bash
   cargo build --release
   ```

3. **Test fresh download** to verify fix works correctly

4. **Test resume functionality** to verify second fix works

## 🔍 Verification

The fix was verified through:
- **Code review**: All download entry points audited for consistent path handling
- **Compilation check**: `cargo check` passed successfully
- **API testing**: Confirmed HuggingFace API returns expected filename format with subdirectories
- **Pattern analysis**: Verified `start_download()` correctly appends filename to base_path

## 📁 Files Changed

- `src/ui/app/downloads.rs` (lines 137-155, 278-302)
- No breaking changes - purely internal bug fix

## 🙏 Acknowledgments

This issue was identified and resolved through systematic analysis of the download flow, path construction logic, and HuggingFace API response format.
