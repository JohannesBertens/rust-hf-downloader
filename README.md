# hf-search

A Terminal User Interface (TUI) application for searching and browsing models from the HuggingFace model hub.

## Features

- 🔍 **Interactive Search**: Search through thousands of HuggingFace models
- ⌨️ **Vim-like Controls**: Efficient keyboard navigation
- 📊 **Rich Display**: View model details including downloads, likes, and tags
- 📦 **Quantization Details**: See all available quantized versions (Q2, Q4, Q5, Q8, etc.) with file sizes
- ⚡ **Async API**: Non-blocking UI with async API calls
- 🎨 **Colorful Interface**: Syntax-highlighted results for better readability

## Installation

```bash
cargo build --release
```

## Usage

Run the application:

```bash
cargo run --release
```

### Controls

| Key | Action |
|-----|--------|
| `/` | Enter search mode |
| `Tab` | Switch focus between Models and Quantizations lists |
| `Enter` | Execute search (in search mode) / Show details (in browse mode) |
| `Esc` | Return to browse mode from search mode |
| `j` or `↓` | Move selection down in focused list |
| `k` or `↑` | Move selection up in focused list |
| `q` or `Ctrl+C` | Quit application |

### How to Use

1. **Start the application** - You'll see an empty search interface
2. **Press `/`** to enter search mode (the search box will be highlighted in yellow)
3. **Type your query** (e.g., "gpt", "llama", "mistral")
4. **Press Enter** to search
5. **Navigate model results** with `j`/`k` or arrow keys (Models list is focused by default, yellow border)
6. **View quantization details** automatically as you select different models
7. **Press Tab** to switch focus to the Quantizations list (yellow border moves)
8. **Navigate quantizations** with `j`/`k` or arrow keys
9. **Press Enter** to see full details of the selected item in the status bar
10. **Press Tab** again to return focus to the Models list
11. **Press `/`** to start a new search

The **Quantization Details** section shows all available GGUF quantized versions of the selected model with three columns:
- **Left**: File size (formatted as GB/MB/KB)
- **Middle**: Quantization type only (Q2_K, Q4_K_M, Q5_0, Q8_0, etc.)
- **Right**: Full filename for reference

### Example Searches

- Search for GPT models: `/` → type `gpt` → `Enter`
- Search for image models: `/` → type `stable-diffusion` → `Enter`
- Search for translation models: `/` → type `translation` → `Enter`

## Technical Details

### Architecture

- **TUI Framework**: [ratatui](https://github.com/ratatui/ratatui)
- **HTTP Client**: reqwest with async support
- **API**: HuggingFace REST API (`https://huggingface.co/api/models`)
- **Text Input**: tui-input for search box handling

### API Integration

The application queries the HuggingFace API with the following parameters:
- Search query from user input
- Results limited to 50 models
- Sorted by downloads in descending order

### Project Structure

```
hf-search/
├── Cargo.toml          # Dependencies and project metadata
├── README.md           # This file
└── src/
    └── main.rs         # Main application code
```

## Dependencies

- `ratatui`: TUI framework
- `crossterm`: Terminal manipulation
- `tokio`: Async runtime
- `reqwest`: HTTP client
- `serde`: JSON serialization
- `tui-input`: Text input widget
- `color-eyre`: Error handling

## License

Copyright (c) Johannes Bertens <>

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
