# Release Notes - Version 0.7.1

**Release Date**: November 22, 2025

## 🐛 Bug Fix: Quantization Folder Duplication

Version 0.7.1 fixes a critical issue where quantization folders were being created twice in the download path structure.

### The Problem

When downloading models with quantization-specific directory structures (common with unsloth models), the application was creating duplicate folder paths:

**Before Fix:**
```
models/unsloth/MiniMax-M2-GGUF/
└── Q2_K_L/
    └── Q2_K_L/                    ← Duplicate!
        ├── MiniMax-M2-Q2_K_L-00001-of-00002.gguf
        └── MiniMax-M2-Q2_K_L-00002-of-00002.gguf
```

### The Solution

**After Fix:**
```
models/unsloth/MiniMax-M2-GGUF/
└── Q2_K_L/                        ← Single folder!
    ├── MiniMax-M2-Q2_K_L-00001-of-00002.gguf
    └── MiniMax-M2-Q2_K_L-00002-of-00002.gguf
```

### Technical Changes

1. **Fixed Download Path Logic**: Modified `start_download()` in `src/download.rs`
   - Extracts only the filename (last path component) for local storage
   - Preserves full path in URL for correct remote file access
   - Prevents duplicate quantization folders in local file system

2. **How it Works**:
   - Remote file on HuggingFace: `Q2_K_L/model.gguf`
   - URL uses full path: `https://.../{model}/resolve/main/Q2_K_L/model.gguf` ✓
   - Local base path already includes quantization folder: `/models/unsloth/Model/Q2_K_L/`
   - Local filename extracted: `model.gguf`
   - Final local path: `/models/unsloth/Model/Q2_K_L/model.gguf` ✓

3. **Key Insight**: The `base_path` parameter passed to download already includes the quantization directory structure from `validate_and_sanitize_path()`, so we only need the filename itself for local storage

### Files Changed

- `src/download.rs`: Modified `start_download()` to extract only filename for local paths
- `Cargo.toml`: Version bumped to 0.7.1

### Impact

- **Fixes**: Quantization folder duplication issue
- **Improves**: Cleaner download directory structure
- **Maintains**: Full backward compatibility
- **Zero Breaking Changes**: All existing functionality preserved

### Testing

The fix correctly handles various remote file structures:
- Remote: `"Q2_K_L/model.gguf"` → Local: `/base/author/model/Q2_K_L/model.gguf` ✓
- Remote: `"Q4_K_M/model.Q5_0.gguf"` → Local: `/base/author/model/Q4_K_M/model.Q5_0.gguf` ✓
- Remote: `"model.gguf"` → Local: `/base/author/model/model.gguf` ✓ (no subfolder)

### Related Issues

This fix resolves the quantization folder duplication that was affecting models from sources like unsloth where quantization directories are part of the repository structure.
