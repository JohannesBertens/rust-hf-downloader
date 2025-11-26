# Filter System - TODO Summary

## ✅ Completed Features

### Phase 1: Popup Search (DONE)
- ✅ Search popup dialog with centered UI
- ✅ `/` key opens popup instead of inline editing
- ✅ Full keyboard support (Enter, ESC, cursor navigation)
- ✅ Popup event handling and rendering

### Phase 2: Basic Filters (DONE)
- ✅ Filter toolbar UI replacing inline search box
- ✅ Sort by Downloads, Likes, Modified, Name
- ✅ Sort direction toggle (Ascending/Descending) with visual indicators (▲/▼)
- ✅ Keyboard controls: `s` (cycle field), `Shift+S` (toggle direction)
- ✅ API integration with `fetch_models_filtered()`

### Phase 3: Advanced Filters (DONE)
- ✅ Min downloads filter with preset steps (0, 100, 1k, 10k, 100k, 1M)
- ✅ Min likes filter with preset steps (0, 10, 50, 100, 500, 1k, 5k)
- ✅ Filter field navigation: `f` (cycle focus), `+/-` (modify values)
- ✅ Filter reset: `r` (reset all to defaults)
- ✅ Client-side filtering (API doesn't support these filters)
- ✅ Yellow highlighting for focused filter field
- ✅ Status messages for all filter operations

### Bug Fixes (DONE)
- ✅ Fixed API error with ascending sort direction (client-side sorting)
- ✅ Fixed name-based sorting (API doesn't support it, implemented client-side)
- ✅ Zero build warnings, all tests passing

---

## 🚧 Phase 4: Polish & Finalization (TODO)

### 1. Filter Presets (TODO)
**Effort**: ~2 hours

Add one-key presets for common filter combinations:

- [ ] **Preset 1 (Key: `1`)**: Trending
  - Sort: Downloads (descending)
  - Filters: None
  - Default state

- [ ] **Preset 2 (Key: `2`)**: Popular
  - Sort: Downloads (descending)
  - Min Downloads: 10,000
  - Min Likes: 100

- [ ] **Preset 3 (Key: `3`)**: Highly Rated
  - Sort: Likes (descending)
  - Min Likes: 1,000

- [ ] **Preset 4 (Key: `4`)**: Recent
  - Sort: Modified (descending)
  - Filters: None

**Implementation**:
- [ ] Add `FilterPreset` enum to `src/models.rs`
- [ ] Add `apply_filter_preset()` method to `impl App`
- [ ] Add keyboard handlers `1-4` in `src/ui/app/events.rs`
- [ ] Update toolbar title to show preset keys

**Files to modify**:
- `src/models.rs` (+10 lines)
- `src/ui/app/events.rs` (+60 lines)

---

### 2. Preset Indicator (TODO)
**Effort**: ~1 hour

Show which preset is currently active in the toolbar:

- [ ] Detect if current filter state matches a preset
- [ ] Display preset name in toolbar (e.g., `[Popular]`)
- [ ] Green color for preset indicator
- [ ] Clear indicator when filters are manually modified

**Implementation**:
- [ ] Add preset detection logic in `render_filter_toolbar()`
- [ ] Append preset name to toolbar display line

**Files to modify**:
- `src/ui/render.rs` (+40 lines)

**Visual Example**:
```
┌─ Filters  [/: Search | 1-4: Presets | f: Focus | +/-: Modify | r: Reset] ─┐
│ Sort: Downloads ▼  |  Min Downloads: 10k  |  Min Likes: 100  |  [Popular]  │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

### 3. Config Persistence (TODO)
**Effort**: ~2-3 hours

Save and restore filter preferences across sessions:

- [ ] Add filter fields to `AppOptions` struct:
  - `default_sort_field: SortField`
  - `default_sort_direction: SortDirection`
  - `default_min_downloads: u64`
  - `default_min_likes: u64`

- [ ] Load defaults from config in `App::new()`
- [ ] Add `Ctrl+S` hotkey to save current filters as defaults
- [ ] Add `save_filter_settings()` method
- [ ] Update config serialization/deserialization

**Implementation**:
- [ ] Update `AppOptions` struct with `#[serde(default)]` attributes
- [ ] Modify `App::new()` to load filter state from config
- [ ] Add save method and keyboard handler

**Files to modify**:
- `src/models.rs` (+8 lines)
- `src/ui/app/state.rs` (+10 lines)
- `src/ui/app/events.rs` (+25 lines)

**Config Example**:
```toml
# ~/.config/jreb/config.toml
default_sort_field = "Likes"
default_sort_direction = "Descending"
default_min_downloads = 10000
default_min_likes = 100
```

---

### 4. Active Filters Indicator (TODO)
**Effort**: ~1 hour

Show visual feedback when filters are active (non-default):

- [ ] Detect if any filter is non-default
- [ ] Add `[Filters Active]` indicator to status bar
- [ ] Yellow color with bold styling
- [ ] Update only when filter state changes

**Implementation**:
- [ ] Add filter state check in status bar rendering
- [ ] Append indicator span to status line

**Files to modify**:
- `src/ui/render.rs` (+15 lines)

**Visual Example**:
```
┌─ Status ─────────────────────────────────────────────┐
│ Found 42 models (filtered from 100) [Filters Active] │
└───────────────────────────────────────────────────────┘
```

---

### 5. Documentation Updates (TODO)
**Effort**: ~1 hour

Document the new filtering system:

- [ ] **README.md**: Add filter controls section
  - Document keys: `s`, `S`, `f`, `+/-`, `r`, `1-4`, `Ctrl+S`
  - Add features: filtering, sorting, presets, persistence
  - Include visual examples

- [ ] **AGENTS.md**: Document filter architecture
  - Filter state management
  - Filter logic flow
  - Filter rendering system
  - Config persistence

**Files to modify**:
- `README.md` (+30 lines)
- `AGENTS.md` (+15 lines)

---

## 📊 Phase 4 Summary

### Total Estimated Effort
- **Presets**: ~2 hours
- **Preset Indicator**: ~1 hour
- **Config Persistence**: ~2-3 hours
- **Active Filters Indicator**: ~1 hour
- **Documentation**: ~1 hour

**Total**: ~7-8 hours

### Lines of Code
- **New**: ~173 lines
- **Modified**: ~15 lines
- **Documentation**: ~45 lines

### Files Affected
- `src/models.rs` - Add FilterPreset enum, update AppOptions
- `src/ui/app/state.rs` - Load config defaults
- `src/ui/app/events.rs` - Preset handlers, save handler
- `src/ui/render.rs` - Preset indicator, active filters indicator
- `README.md` - User documentation
- `AGENTS.md` - Developer documentation

### Testing Checklist
- [ ] All 4 presets apply correct filter combinations
- [ ] Preset indicator shows/hides correctly
- [ ] `Ctrl+S` saves current filters to config
- [ ] Config loads correctly on app restart
- [ ] Active filters indicator shows when applicable
- [ ] Documentation is accurate and complete
- [ ] No regressions in existing functionality

---

## 🎯 Priority Ranking

If implementing incrementally, suggested order:

1. **Filter Presets** (High impact, frequently used)
2. **Preset Indicator** (Improves UX for presets)
3. **Config Persistence** (High value for regular users)
4. **Active Filters Indicator** (Nice-to-have visual feedback)
5. **Documentation** (Should be done last, after features stabilize)

---

## 🔮 Future Enhancement Ideas (Not in Scope)

These were not in the original plan but could be considered later:

- **More Presets**: Minimal (e.g., <100 downloads for experimentation)
- **Tag Filtering**: Filter by model tags (e.g., "gguf", "chat", "instruct")
- **License Filtering**: Filter by license type
- **Author Filtering**: Search by specific author
- **Date Range**: Filter by last modified date range
- **Custom Preset Slots**: Save custom presets to keys 5-9
- **Filter History**: Remember last N filter states
- **Quick Toggle**: Toggle filters on/off without resetting values
- **Filter Profiles**: Named profiles saved in config
- **Export/Import**: Share filter configurations

---

## 📝 Notes

- Current implementation (Phases 1-3) is **fully functional and tested**
- Phase 4 features are **optional polish** - the core functionality is complete
- All Phase 4 features are **independent** - can be implemented in any order
- Config persistence is the **highest value** Phase 4 feature
- Presets are the **most user-friendly** Phase 4 feature

---

**Status**: Phases 1-3 Complete ✅ | Phase 4 Pending 🚧
