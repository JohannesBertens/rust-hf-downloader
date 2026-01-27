# Release Notes - Version 1.3.2

**Release Date**: 2026-01-21

## Feature: Exact Model Match for Repository ID Searches

### Problem
When users searched using an exact repository ID (e.g., `meta-llama/Llama-3.1-8B`), they received all 50 default API results instead of showing only the requested model. This made it difficult to quickly navigate to specific models known by their full repository ID.

### Solution
Added logic to detect when a search query appears to be a repository ID (contains `/` character) and contains an exact match in the results. When an exact match is found, only that single model is displayed.

### Changes

**File Modified**: `src/ui/app/models.rs`

**Key Changes:**
```rust
// Check if query looks like a repository ID (contains /)
// If so, and there's an exact match, show only that repository
let exact_match_idx = if query.contains('/') {
    results.iter().position(|m| m.id.to_lowercase() == query.to_lowercase())
} else {
    None
};

let filtered_results = if let Some(idx) = exact_match_idx {
    vec![results[idx].clone()]
} else {
    results
};
```

**Status Messages Updated:**
- When exact match is used: "(exact match)" displayed in status bar
- When cached results are filtered: "(cached, exact match)" displayed

### Examples

**Before:**
```
/ → type "meta-llama/Llama-3.1-8B" → Enter
→ Shows 50 trending/popular results
```

**After:**
```
/ → type "meta-llama/Llama-3.1-8B" → Enter
→ Shows only "meta-llama/Llama-3.1-8B" (exact match)
```

### Technical Details

- Case-insensitive matching: `meta-llama/LLAMA-3.1-8B` matches `meta-llama/Llama-3.1-8B`
- Only triggers when query contains `/` (repository ID pattern)
- Falls back to showing all results if no exact match found
- Works with both cached and fresh API results
- Status bar indicates when exact match mode is active

### Compatibility

- **No Breaking Changes**: All existing functionality preserved
- **Backward Compatible**: Existing search behavior unchanged for non-ID searches
- **API Compatible**: No changes to public API

### Related Files

- `src/ui/app/models.rs` - Search logic updated (39 insertions, 5 modifications)
- `Cargo.toml` - Version bumped to 1.3.2
- `src/cli.rs` - Version bumped to 1.3.2

---

## Summary

This release improves the search experience when users know the exact repository ID of a model. Instead of showing 50 results, the application now detects repository ID searches and displays only the matching model for a cleaner, more focused interface.

**Upgrade Recommendation**: Optional - no security or stability fixes
**Migration**: No migration required - simply rebuild with `cargo build --release`
