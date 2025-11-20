# hf-search

A Terminal User Interface (TUI) application for searching and browsing models from the HuggingFace model hub.

## Features

- 🔍 **Interactive Search**: Search through thousands of HuggingFace models
- ⌨️ **Vim-like Controls**: Efficient keyboard navigation
- 📊 **Rich Display**: View model details including downloads, likes, and tags
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
| `Enter` | Execute search (in search mode) / Show model details (in browse mode) |
| `Esc` | Return to browse mode from search mode |
| `j` or `↓` | Move selection down |
| `k` or `↑` | Move selection up |
| `q` or `Ctrl+C` | Quit application |

### How to Use

1. **Start the application** - You'll see an empty search interface
2. **Press `/`** to enter search mode (the search box will be highlighted in yellow)
3. **Type your query** (e.g., "gpt", "llama", "mistral")
4. **Press Enter** to search
5. **Navigate results** with `j`/`k` or arrow keys
6. **Press Enter** on a model to see full details in the status bar
7. **Press `/`** again to start a new search

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
