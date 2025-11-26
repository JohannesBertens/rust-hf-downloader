# Search Enhancement Plan: Popup Search with Filters and Sorting

## Overview

This plan outlines the enhancement of the search functionality by:
1. Moving the inline search bar to a popup dialog
2. Replacing the top bar with filters and sorting controls
3. Adding comprehensive model filtering and sorting capabilities

## Current State Analysis

### Existing Search Implementation
- **Location**: Inline search bar at top of screen (3 lines fixed height)
- **Mode**: InputMode::Editing enters search mode, ESC returns to Normal
- **Behavior**: Search executes on Enter, fetches models via `fetch_models()` API
- **Results**: Display 50 models sorted by downloads (descending)
- **Search Key**: `/` to activate

### Current API Capabilities
- **Search endpoint**: `https://huggingface.co/api/models?search={query}&limit=50&sort=downloads&direction=-1`
- **Trending endpoint**: `https://huggingface.co/models-json?p={page}&sort=trending&withCount=true`
- **Model fields available**: id, author, downloads, likes, tags, last_modified

## Implementation Phases

### [Phase 1: Popup Search (MVP)](./01-phase1-popup-search.md)
Move search from inline input to a popup dialog. This is the minimum viable change that provides the foundation for the rest of the enhancements.

**Deliverables**:
- Search popup dialog with text input
- Popup event handling
- Top bar freed up for filters

### [Phase 2: Basic Filters](./02-phase2-basic-filters.md)
Add a filter toolbar to the top bar with basic sorting capabilities.

**Deliverables**:
- Filter toolbar UI
- Sort by field (Downloads, Likes, Modified, Name)
- Sort direction toggle (Ascending/Descending)
- Keyboard controls (`s`, `S`)

### [Phase 3: Advanced Filters](./03-phase3-advanced-filters.md)
Implement numeric filters and field navigation for power users.

**Deliverables**:
- Min downloads filter
- Min likes filter
- Filter field navigation (`f`, `+`, `-`)
- Client-side filtering logic

### [Phase 4: Polish](./04-phase4-polish.md)
Add quality-of-life improvements and finalize the feature.

**Deliverables**:
- Filter presets
- Config persistence
- Filter reset hotkey
- Status bar integration
- Documentation updates

## Benefits

1. **Better UX**: Popup search is non-intrusive, can be dismissed easily
2. **More Results Space**: Reclaiming 3 lines from inline search for model list
3. **Power User Features**: Sort and filter for precise model discovery
4. **Flexibility**: Easy to add more filters later (license, language, etc.)
5. **Consistency**: Popup pattern matches existing download/options dialogs

## Backward Compatibility

- `/` key still triggers search (now opens popup instead of inline mode)
- Search behavior unchanged (query → fetch → display)
- No breaking changes to config or data files
- All existing keyboard shortcuts remain functional

## Files to Modify

1. **`src/models.rs`**: Add SortField, SortDirection enums
2. **`src/ui/app/state.rs`**: Add filter/sort state fields, update App::new()
3. **`src/ui/app/events.rs`**: Add popup handling, filter hotkeys
4. **`src/ui/app/models.rs`**: Update search to use filtered API
5. **`src/api.rs`**: Add fetch_models_filtered()
6. **`src/ui/render.rs`**: Add search popup + filter toolbar rendering
7. **`src/config.rs`**: Add filter state to AppOptions (optional, Phase 4)

## Testing Strategy

Each phase should be tested independently before moving to the next:

1. **Phase 1**: Test search popup (open, type, search, cancel)
2. **Phase 2**: Test all sort combinations with real API calls
3. **Phase 3**: Test filter combinations, verify client-side filtering
4. **Phase 4**: Test config persistence, presets, reset functionality

## Timeline Estimate

- **Phase 1**: 2-3 hours (core functionality)
- **Phase 2**: 2-3 hours (toolbar + sorting)
- **Phase 3**: 3-4 hours (filters + navigation)
- **Phase 4**: 2-3 hours (polish + docs)

**Total**: 9-13 hours of development work

## Success Criteria

✅ Search popup opens with `/` key and allows query entry
✅ Results display with applied sort order
✅ Filters reduce result set correctly
✅ All keyboard shortcuts work as documented
✅ No regression in existing functionality
✅ Config persists across app restarts (Phase 4)
