# Release Notes - Version 1.3.1

**Release Date**: 2026-01-21

## Enhancement: Support for Additional GGUF Quantization Types

### Problem
The quantization detection logic was missing support for two important GGUF quantization types:
1. **F16** (half-precision floating point) - commonly used for high-quality models
2. **TQ** (Tensor Quantization) - ternary packing for TriLMs/BitNet models

This caused these quantization types to be incorrectly labeled as "UNKNOWN" or completely missed during model browsing and download operations.

### Affected Models

**F16 Quantization Missing:**
- `unsloth/gpt-oss-120b-GGUF` - F16 file in root directory not detected
- Any models with F16 quantization in root (not in subdirectories)

**TQ Quantization Missing:**
- `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF` - TQ1_0 not detected
- Models using ternary packing quantization formats (TQ1_0, TQ2_0, etc.)

### Solution
Added pattern detection support for F16 and TQ quantization types to the extraction logic in `src/api.rs`:

**Functions Updated:**
1. `is_quantization_directory()` - Detects directories with TQ and F16 names
2. `extract_quantization_type_from_dirname()` - Extracts TQ and F16 from directory names
3. `extract_quantization_type()` - Extracts TQ and F16 from filenames

### Technical Details

#### F16 Pattern
- Added to special formats check: `BF16`, `F16`, `FP16`, `FP32`
- Pattern matches: `F16` (case-insensitive)
- Supports files like: `model-F16.gguf`

#### TQ Pattern
- Added ternary quantization support with pattern: `TQ` followed by digit
- Pattern matches: `TQ1_0`, `TQ2_0`, `TQ3_0`, etc.
- Supports files like: `model-TQ1_0.gguf`
- Handles both direct patterns and complex model names with hyphens

### Code Changes

**File Modified**: `src/api.rs`

**Key Changes:**
```rust
// is_quantization_directory() - Added TQ and F16 detection
if upper.starts_with('Q')
    || upper.starts_with("IQ")
    || upper.starts_with("TQ")        // NEW: TQ pattern
    || upper == "BF16"
    || upper == "F16"                 // NEW: F16 pattern
    || upper == "FP16"
{
    return true;
}

// extract_quantization_type() - Added TQ to is_quant_type helper
if upper.starts_with("TQ")
    && upper.len() > 2
    && upper.chars().nth(2).is_some_and(|c| c.is_ascii_digit())
{
    return true;
}
```

### Impact

**Before Fix:**
- F16 quantizations in root directory appeared as "UNKNOWN"
- TQ quantizations (TQ1_0, TQ2_0, etc.) appeared as "UNKNOWN"
- Users couldn't properly select or download these quantization types
- Download plan would miss these files entirely

**After Fix:**
- F16 quantizations correctly identified and listed
- TQ quantizations correctly identified and listed
- Users can select and download all available quantization types
- Download plans include all quantization files

### Testing

**Tested Models:**

1. `unsloth/gpt-oss-120b-GGUF`:
   - ✓ F16 (1 files, 60.88 GB) now correctly detected
   - ✓ All 17 quantizations listed

2. `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF`:
   - ✓ TQ1_0 (1 files, 7.46 GB) now correctly detected
   - ✓ All 28 quantizations listed

**Unit Tests:**
- ✓ All 7 existing unit tests pass
- ✓ No regressions introduced

### Compatibility

- **No Breaking Changes**: All existing functionality preserved
- **Backward Compatible**: Existing quantization types continue to work
- **Forward Compatible**: New patterns added without affecting existing logic
- **API Compatible**: No changes to public API

### Related Files

- `src/api.rs` - Quantization detection logic (33 lines added)
- `Cargo.toml` - Version bumped to 1.3.1
- `src/cli.rs` - Version bumped to 1.3.1

### Known Quantization Types

The application now supports the following quantization patterns:

**Q-Series:**
- Q2_K, Q2_K_L, Q2_K_XL, Q2_K_S
- Q3_K_M, Q3_K_S, Q3_K_XL
- Q4_0, Q4_1, Q4_K_M, Q4_K_S, Q4_K_XL
- Q5_K_M, Q5_K_S, Q5_K_XL
- Q6_K, Q6_K_XL
- Q8_0, Q8_K_XL

**IQ-Series:**
- IQ1_M, IQ1_S
- IQ2_M, IQ2_XXS, IQ2_XS
- IQ3_XXS
- IQ4_NL, IQ4_XS

**TQ-Series (NEW):**
- TQ1_0, TQ2_0, TQ3_0, etc.

**Floating Point:**
- BF16
- **F16 (NEW)**
- FP16
- FP32

**Other:**
- MXFP4, MXFP6, MXFP8, etc.

---

## Summary

This release enhances GGUF quantization support by adding detection for F16 and TQ quantization types. These patterns were previously unsupported, leading to missing or mislabeled quantizations. The update ensures comprehensive coverage of modern GGUF quantization formats, improving the user experience for downloading a wider variety of models.

**Upgrade Recommendation**: All users working with GGUF models should upgrade to ensure proper detection of all quantization types.

**Migration**: No migration required - simply rebuild with `cargo build --release`
