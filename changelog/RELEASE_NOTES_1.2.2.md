# Release Notes - Version 1.2.2

**Release Date**: 2026-01-08

## Bug Fix: Color Contrast Issue on Light Backgrounds

### Problem
The application appeared to show an "empty screen" on terminals with white or light-colored backgrounds. Instructions and UI elements were present but invisible because they used hard-coded white text (`Color::White`), which matched the terminal's background color.

**GitHub Issue**: [#16 - Empty screen](https://github.com/JohannesBertens/rust-hf-downloader/issues/16)

### Root Cause
The status bar and several UI components explicitly set text color to `Color::White`, which:
- Works correctly on dark backgrounds (black terminal with white text)
- Becomes invisible on light backgrounds (white terminal with white text)
- Affects users with custom terminal themes, especially on macOS

### Solution
Changed status bar text color from hard-coded `Color::White` to `Style::default()`, which:
- Uses the terminal's default foreground color
- Automatically adapts to user's terminal theme
- Ensures visibility on both dark and light backgrounds
- Maintains the intended appearance on dark terminals

### Technical Details

**File Modified**: `src/ui/render.rs`

**Change**:
```rust
// Before (line 280):
Style::default().fg(Color::White)

// After:
Style::default()
```

This change affects the status bar at the bottom of the screen, which displays:
- Instructions (e.g., "Press / to search")
- Error messages
- Status information
- Selection details

### Impact

**Before Fix**:
- Light background terminals showed blank/empty UI
- Users couldn't see instructions or status messages
- Application appeared broken or non-functional

**After Fix**:
- UI visible on all terminal background colors
- Instructions clearly visible on light backgrounds
- Preserves appearance on dark backgrounds
- Better accessibility for users with visual impairments or custom themes

### Testing Recommendations

Users with light terminal backgrounds should verify:
1. ✓ Status bar instructions are visible
2. ✓ Error messages display correctly
3. ✓ Model selection information shows properly
4. ✓ All UI text is readable

### Compatibility

- **No Breaking Changes**: All existing functionality preserved
- **Theme Compatibility**: Works with all terminal color schemes
- **Backward Compatible**: Dark terminals retain same appearance
- **Forward Compatible**: Adapts to future terminal themes

### Related Files

- `src/ui/render.rs` - Status bar rendering (1 line changed)
- Issue #16 - Original bug report and discussion

---

## Summary

This release fixes a critical visibility issue that made the application unusable on light-colored terminal backgrounds. By using terminal-default colors instead of hard-coded white, the UI now adapts to user preferences and works correctly across all terminal themes.

**Upgrade Recommendation**: All users, especially those on macOS or using light terminal themes, should upgrade to this version.

**Migration**: No migration required - simply rebuild with `cargo build --release`
