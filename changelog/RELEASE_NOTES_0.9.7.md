# Release Notes - Version 0.9.7

**Release Date:** 2025-11-25  
**Type:** Bug Fix Release - Critical Path Handling

## Overview

Version 0.9.7 fixes two critical bugs in file path handling that caused files to be saved in incorrect locations when downloading HuggingFace repositories. These fixes ensure proper directory structure preservation for all downloaded files.

---

## 🐛 Bug Fixes

### 1. **Download Worker Path Stripping Bug**
**Location:** `src/download.rs` lines 178-180  
**Severity:** HIGH - Files lost subdirectory structure

**Problem:**
The download worker was stripping all directory structure from filenames, causing files to be saved flat instead of preserving their repository structure.

**Example:**
- File: `tokenizer/config.json`
- Expected: `~/models/maya-research/maya1/tokenizer/config.json`
- Actual (BUGGY): `~/models/maya-research/maya1/config.json`

**Root Cause:**
```rust
// BUGGY CODE:
let local_filename = sanitized_filename.rsplit('/').next().unwrap_or(&sanitized_filename);
let final_path = canonical_base.join(local_filename);
```
This extracted only the last path component, discarding subdirectories.

**Fix:**
```rust
// FIXED CODE:
let final_path = canonical_base.join(&sanitized_filename);
```

**Impact:**
- ✅ Files with subdirectories now save correctly
- ✅ Repository structure fully preserved
- ✅ Works with all HuggingFace models (GGUF and non-GGUF)

---

### 2. **Repository Download Base Path Bug**
**Location:** `src/ui/app/downloads.rs` lines 422-440  
**Severity:** HIGH - All files saved to wrong directory

**Problem:**
When downloading full repositories, the base path was calculated from the **first file** in the download list. If that file was in a subdirectory (e.g., `tokenizer/chat_template.jinja`), **all files** would be saved to that subdirectory.

**Example:**
Repository: `maya-research/maya1`
- First file in list: `tokenizer/chat_template.jinja`
- Calculated base: `~/models/maya-research/maya1/tokenizer/`
- Result: **ALL files** saved to `tokenizer/` subfolder (WRONG!)

**Root Cause:**
```rust
// BUGGY CODE:
let model_path = validate_and_sanitize_path(&base_path, &model.id, &files_to_download[0].rfilename)?
    .parent().to_path_buf();
// Used same model_path for all files
```

**Fix:**
```rust
// FIXED CODE:
let model_parts: Vec<&str> = model.id.split('/').collect();
let model_root = if model_parts.len() == 2 {
    PathBuf::from(&base_path).join(model_parts[0]).join(model_parts[1])
} else {
    PathBuf::from(&base_path)
};
// Each file uses model_root as base, preserving its own subdirectory structure
```

**Impact:**
- ✅ Root files saved to model root directory
- ✅ Subdirectory files saved with correct structure
- ✅ No more files in wrong locations
- ✅ Works with any file ordering

---

## 📊 Before/After Comparison

### Bug #1 Example: `maya-research/maya1`
**Before (v0.9.6):**
```
~/models/maya-research/maya1/
├── tokenizer/
│   ├── README.md              ❌ WRONG LOCATION
│   ├── config.json            ❌ WRONG LOCATION
│   ├── tokenizer.json         ❌ WRONG LOCATION
│   ├── special_tokens_map.json❌ WRONG LOCATION
│   ├── chat_template.jinja    ✓ Correct
│   └── tokenizer_config.json  ✓ Correct
└── (other files also in wrong location)
```

**After (v0.9.7):**
```
~/models/maya-research/maya1/
├── README.md                  ✅ FIXED
├── config.json                ✅ FIXED
├── model-00001-of-00002.safetensors ✅ FIXED
├── model-00002-of-00002.safetensors ✅ FIXED
└── tokenizer/
    ├── chat_template.jinja    ✅ Correct
    ├── tokenizer.json         ✅ Correct
    ├── tokenizer_config.json  ✅ Correct
    └── special_tokens_map.json✅ Correct
```

---

## 🔧 Additional Fixes

### 3. **Clippy Compatibility Fix**
**Location:** `src/verification.rs` line 167  
**Change:** Added `#[allow(clippy::manual_is_multiple_of)]` attribute

**Reason:**
The `is_multiple_of()` method is not available in Rust 1.75.0 (Ubuntu 22.04 LTS default compiler). The attribute allows the modulo operator pattern while maintaining compatibility.

---

## ✅ Testing

All tests pass:
- ✅ Release build successful
- ✅ All clippy checks pass
- ✅ All unit tests pass (2/2)

---

## 🎯 Affected Users

**Who Should Update:**
- ✅ Anyone who downloaded repositories with subdirectories (like `maya-research/maya1`)
- ✅ Users experiencing files in wrong locations
- ✅ Users with incomplete downloads showing in startup popup

**Migration Steps:**
1. Update to v0.9.7
2. Delete incorrectly placed files
3. Clear affected registry entries from `~/models/hf-downloads.toml`
4. Re-download affected repositories

---

## 📝 Technical Details

### Modified Files
1. `src/download.rs` - Fixed path stripping logic
2. `src/ui/app/downloads.rs` - Fixed base path calculation
3. `src/verification.rs` - Added clippy compatibility attribute

### No Breaking Changes
- ✅ Existing correct downloads unaffected
- ✅ Registry format unchanged
- ✅ Configuration format unchanged
- ✅ API compatibility maintained

---

## 🙏 Acknowledgments

Thanks to the community for reporting these issues and providing detailed reproduction steps.

---

## 📚 Related Documentation
- [AGENTS.md](../AGENTS.md) - Architecture documentation
- [README.md](../README.md) - User guide
