# Alternative 3: Hierarchical Tag Browser - Visual Mockup

## Overall Layout (4 Panes)

```
┌═══════════════════════════════════════════════════════════════════════════════════════════┐
║ Filters  [Sort: Downloads ▼] | [Min Downloads: 10k] | [Min Likes: 100] | [Tags: 3]      ║
├═══════════════════════════════════════════════════════════════════════════════════════════┐
│┌────────┐┌════════════════════════════════════┐┌══════════════════════════════════════┐║
││        ││  Model ID                    DL Likes │┌─Tag Browser───────────────────────┐║
││Models  ││  llama-2-7b              15.2M  2.1k ││Categories           Tags Active │║
││        ││  mixtral-8x7b            8.4M   1.8k ││                                        │║
││        ││  code-llama-13b          12.1M  3.2k ││[▼] Language Models        (8)     │║
││        ││  wizard-3               5.7M    890  ││  ├─text-generation    ✓       │║
││  ↓     ││  llama-13b              25.3M  5.1k ││  ├─chat              ○       │║
││        ││  wizard-2               3.2M    445  ││  └─summarization     ○       │║
││        ││                           ...      ││                                        │║
│└────────┘└════════════════════════════════════┘┌─[▼] Computer Vision      (6)     │║
                                                   ││  ├─image-generation ○       │║
                                                   ││  ├─image-classif    ○       │║
                                                   ││  └─object-detection ○       │╬
                                                   ││                                        │║
                                                   │┌─[▼] Audio Processing   (4)     │║
                                                   ││  ├─text-to_speech   ○       │║
                                                   ││  └─speech_recognition ○       │╬
                                                   ││                                        │║
                                                   │┌─[▶] Libraries               │║
                                                   ││  ├─pytorch        ✓       │║
                                                   ││  └─tensorflow     ○       │╬
                                                   ││                                        │║
                                                   │└Active Tags (3)────────────────┘║
                                                   │  • text-generation              │
                                                   │  • pytorch                      │
                                                   │  • multimodal                  │
                                                   └═══════════════════════════════║
                                                                                                │
┌═══════════════════════════════════════════════════════════════════════════════════════════┐
║ Status: Tag Browser - Use Tab to navigate, Enter to select | Press 'g' to close browser                ║
└═══════════════════════════════════════════════════════════════════════════════════════════┘
```

## Key Visual Elements

### 1. Filter Toolbar Enhancement
```
[Sort: Downloads ▼] | [Min Downloads: 10k] | [Min Likes: 100] | [Tags: 3]
                                                                 ↑^^^^
                                                                 Shows count of active tags
```

### 2. Tag Browser Pane (Focused State)
```
┌─Tag Browser───────────────────────────┐
│Categories           Tags Active │
│                                        │
│[▼] Language Models        (8)     │ <- Expanded category
│  ├─text-generation    ✓       │ <- Selected tag (checkmark)
│  ├─chat              ○       │ <- Unselected tag
│  └─summarization     ○       │
│                                        │
│[▶] Computer Vision      (6)     │ <- Collapsed category
│                                        │
│Active Tags (3)────────────────┘ <- Selection summary
│  • text-generation              │
│  • pytorch                      │
│  • multimodal                 │
└═══════════════════════════════╘
```

### 3. Focus Navigation States

**When Models Pane is Focused:**
```
┌═══════════════════════════════════════════════════════════════════════════════════════════┐
│┌═════════════════════════════════════════════════════════════════════════════════════════┐║
││  Model ID                    DL Likes │ <- Focus indicator on models
││  llama-2-7b              15.2M  2.1k │ <- Selected model (highlighted)
││  mixtral-8x7b            8.4M   1.8k │
││  code-llama-13b          12.1M  3.2k │
││  wizard-3               5.7M    890  │ <- Current selection
││  llama-13b              25.3M  5.1k │
││                           ...      │
│└═════════════════════════════════════════════════════════════════════════════════════════┘║
│┌─Tag Browser───────────────────────────┐ <- No focus indicator
││Categories           Tags Active │
││[▼] Language Models        (8)     │
│  ├─text-generation    ✓       │
│  ├─chat              ○       │
│  └─summarization     ○       │
││                                        │
││[▶] Computer Vision      (6)     │
││                                        │
│└Active Tags (3)───────────────────────┘
```

**When Tag Browser is Focused:**
```
┌═══════════════════════════════════════════════════════════════════════════════════════════┐
│┌═════════════════════════════════════════════════════════════════════════════════════════┐║
││  Model ID                    DL Likes │
││  llama-2-7b              15.2M  2.1k │
││  mixtral-8x7b            8.4M   1.8k │ <- No focus indicator
││  code-llama-13b          12.1M  3.2k │
││  wizard-3               5.7M    890  │
││  llama-13b              25.3M  5.1k │
││                           ...      │
│└═════════════════════════════════════════════════════════════════════════════════════════┘║
│┌═════════════════════════════════════════════════════════════════════════════════════════┐║ <- Focus border
││Categories           Tags Active │ <- Focus indicator
││                                        │
││[▼] Language Models        (8)     │
││  ├─text-generation    ✓       │ <- Current selection in browser
││  ├─chat              ○       │
││  └─summarization     ○       │
││                                        │
││[▶] Computer Vision      (6)     │
││                                        │
│└Active Tags (3)───────────────────────┘
```

### 4. Category Expansion States

**Collapsed Category:**
```
┌─[▶] Computer Vision      (6)     │ <- Right-pointing triangle
│                                        │
│(Content hidden until expanded)         │
```

**Expanded Category:**
```
┌─[▼] Language Models        (8)     │ <- Down-pointing triangle
│  ├─text-generation    ✓       │ <- Individual tags listed
│  ├─chat              ○       │ <- Selection states
│  ├─summarization     ○       │
│  ├─translation       ○       │
│  └─question-answer   ○       │
```

### 5. Tag Selection States

**Selected Tag:**
```
  text-generation    ✓       │ <- Checkmark (✓) and different color
```

**Unselected Tag:**
```
  chat              ○       │ <- Empty circle (○) for unselected
```

**Current Navigation Item:**
```
  text-generation    ✓       │ <- Highlighted background when selected with navigation
```

### 6. Category States

**Category with Selected Tags:**
```
┌─[▼] Language Models   ✓  (8)     │ <- Category has selections
│  ├─text-generation    ✓       │
│  ├─chat              ○       │
│  └─summarization     ○       │
```

**Category with All Tags Selected:**
```
┌─[▼] Language Models   ●  (8)     │ <- Filled circle (●) for all selected
│  ├─text-generation    ✓       │
│  ├─chat              ✓       │
│  └─summarization     ✓       │
```

### 7. Status Bar Updates

**When Tag Browser Opens:**
```
Status: Tag Browser - Use Tab to navigate, Enter to select | Press 'g' to close browser
```

**When Tag is Selected:**
```
Status: Tag 'text-generation' selected - 3 tags active
```

**When Category is Expanded:**
```
Status: Expanded 'Language Models' category - 8 tags available
```

**When Results Filter:**
```
Status: Filtering by tags: text-generation, pytorch - Showing 15 of 47 models
```

### 8. Keyboard Navigation Visualization

**Tab Navigation Cycle:**
```
Model List ──Tab──> Tag Browser ──Tab──> Model Details ──Tab──> Model List
    ↑                                                                    │
    └──────────────────────────────────────────────────────────────┘
```

**In Tag Browser:**
```
j/k (up/down) ── navigates through categories and tags
Enter ── expands/collides categories OR selects/deselects tags
Space ── quick tag toggle (alternative to Enter)
c ── clears all selected tags
```

### 9. Empty/Loading States

**No Tags Available:**
```
┌─Tag Browser───────────────────────────┐
│Categories           Tags Active │
│                                        │
│  No tags available for current search │
│                                        │
│  Try changing your model search query │
│                                        │
└Active Tags (0)───────────────────────┘
```

**Loading Tags:**
```
┌─Tag Browser───────────────────────────┐
│Categories           Tags Active │
│                                        │
│  [▱] Loading tag hierarchy...         │ <- Loading spinner
│                                        │
│Active Tags (0)───────────────────────┘
```

### 10. Active Tags Summary

**When No Tags Selected:**
```
Active Tags (0)
(No active tag filters)
```

**When Tags Selected:**
```
Active Tags (3)
  • text-generation
  • pytorch
  • multimodal
```

**When Many Tags Selected:**
```
Active Tags (12) ┌
  • text-generation         ▼
  • pytorch
  • multimodal
  • image-generation
  • chat
  • question-answer
  • summarization
  • translation
  • object-detection
  • image-classification
  • text-to-speech
  • speech-recognition
```

## Color Scheme

### Tag Browser Pane
- **Border/Focus:** Green accent color
- **Category Headers:** Bold text
- **Selected Tags:** Green checkmarks (✓)
- **Unselected Tags:** Gray empty circles (○)
- **Active Count:** Yellow numbers
- **Background:** Slightly darker than other panes when focused

### Integration Colors
- **Filter Toolbar:** Consistent with existing design
- **Model List:** No visual changes
- **Status Bar:** Shows active tag count in toolbar

## Responsive Layout

**Minimum Width Scenario:**
```
┌═══════════════════════════════════════════════════════════════════════════════════════════┐
│[Sort: Downloads] | [Min DL: 10k] | [Min Likes: 100] | [Tags: 3] │ <- Toolbar truncates
├═══════════════════════════════════════════════════════════════════════════════════════════┐
│┌══════┐┌══════════════════════════════════════┐┌════════════════════════════════════┐║
││Model ││  llama-2-7b              15.2M  2.1k │┌─Tags────────────────────────┐║
││      ││  mixtral-8x7b            8.4M   1.8k ││Language (3)                 │║
││  ↓   ││  code-llama-13b          12.1M  3.2k ││  • text-gen    ✓           │║
││      ││  wizard-3               5.7M    890  ││  • chat        ○           │║
│└══════┘└══════════════════════════════════════┘└══════════════════════════════════┘║
└═══════════════════════════════════════════════════════════════════════════════════════════┘
```

## Animation States

**When Opening Tag Browser:**
```
[Initial]                    [After 'g' Press]         [After Tab Navigation]
┌────────┐                  ┌────────┐┌══════════════┐   ┌────────┐┌═══════════════════┐
│ Models │                  │ Models ││  Models     │   │ Models ││  Models  Tag Br   │
│        │  ──open───>      │        ││              │   │        ││  │      │          │
│        │                  │        ││              │   │        ││  │      │          │
│        │                  │        ││              │   │        ││  │      │          │
└────────┘                  └────────┘└══════════════┘   └────────┘└═══════════════════┘
```

This mockup shows how the Hierarchical Tag Browser would integrate seamlessly with the existing interface while providing a powerful, intuitive way to filter models by their associated tags.
