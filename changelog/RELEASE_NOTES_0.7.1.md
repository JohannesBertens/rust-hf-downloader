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

1. **New Helper Function**: Added `extract_filename_without_quant_dir()` in `src/api.rs`
   - Strips quantization directory prefixes from filenames
   - Handles complex nested patterns correctly
   - Maintains backward compatibility

2. **Updated Download Logic**: Modified `confirm_download()` in `src/ui/app.rs`
   - Extracts clean filename before path validation
   - Uses clean filenames for all download operations
   - Prevents double folder creation

3. **Path Validation**: Improved path handling to avoid nested quantization directories

### Files Changed

- `src/api.rs`: Added `extract_filename_without_quant_dir()` function
- `src/ui/app.rs`: Updated download logic to use clean filenames
- `Cargo.toml`: Version bumped to 0.7.1

### Impact

- **Fixes**: Quantization folder duplication issue
- **Improves**: Cleaner download directory structure
- **Maintains**: Full backward compatibility
- **Zero Breaking Changes**: All existing functionality preserved

### Testing

The fix handles various filename patterns correctly:
- `"Q2_K_L/model.gguf"` → `"model.gguf"`
- `"Q4_K_M/model.Q5_0.gguf"` → `"model.Q5_0.gguf"`
- `"IQ4_XS/model-BF16.gguf"` → `"model-BF16.gguf"`
- `"model.gguf"` → `"model.gguf"` (no change)

### Related Issues

This fix resolves the quantization folder duplication that was affecting models from sources like unsloth where quantization directories are part of the repository structure.
