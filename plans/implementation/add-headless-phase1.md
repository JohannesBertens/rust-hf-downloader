# Phase 1: CLI Parsing Implementation

**Status**: ✅ Completed
**Completion Date**: 2026-01-08
**Estimated Time**: 2 hours
**Actual Time**: 2 hours
**Dependencies**: None
**Blocked By**: Nothing

## Overview
Add `clap` dependency and define CLI argument structure for headless mode. This phase focuses on setting up the command-line interface foundation.

## Objectives
- Add `clap` library for CLI argument parsing
- Define all CLI commands and arguments
- Set up proper help text and documentation
- Test argument parsing in isolation

## Tasks Checklist

### 1.1 Add clap Dependency
- [x] Edit `Cargo.toml`
- [x] Add `clap = { version = "4.5", features = ["derive"] }` to dependencies
- [x] Run `cargo build` to verify dependency resolution
- [x] Verify no conflicts with existing dependencies

**Expected Code Change:**
```toml
# In Cargo.toml [dependencies]
clap = { version = "4.5", features = ["derive"] }
```

### 1.2 Create CLI Module Structure
- [x] Create `src/cli.rs` file
- [x] Add `mod cli;` to `src/main.rs`
- [x] Set up module structure for commands

**File Structure:**
```
src/
├── cli.rs          # NEW: CLI definitions
├── main.rs         # MODIFIED: Add cli module
├── headless.rs     # TODO: Phase 2
└── ...
```

### 1.3 Define Command Enum
- [x] Define `Command` enum with variants:
  - `Search { query: String, sort_field: Option<String>, min_downloads: Option<u64>, min_likes: Option<u64>, json: bool }`
  - `Download { model_id: String, quantization: Option<String>, all: bool, output: Option<String>, token: Option<String> }`
  - `List { model_id: String, json: bool }`
  - `Resume { }`
- [x] Implement `Default` for `Command` (TUI mode)
- [x] Add derive macros (`Parser`, `Subcommand`)

**Expected Code:**
```rust
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "rust-hf-downloader")]
#[command(about = "TUI for searching and downloading HuggingFace models", long_about = None)]
struct Cli {
    /// Run in headless mode (no TUI)
    #[arg(long, global = true)]
    headless: bool,

    /// Output in JSON format
    #[arg(long, global = true)]
    json: bool,

    /// HuggingFace authentication token
    #[arg(long, global = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Search for models
    Search {
        /// Search query
        query: String,

        /// Sort field (downloads, likes, modified, name)
        #[arg(long)]
        sort: Option<String>,

        /// Minimum downloads filter
        #[arg(long)]
        min_downloads: Option<u64>,

        /// Minimum likes filter
        #[arg(long)]
        min_likes: Option<u64>,
    },

    /// Download a model
    Download {
        /// Model ID (e.g., "meta-llama/Llama-3.1-8B")
        model_id: String,

        /// Filter by quantization type (e.g., "Q4_K_M", "Q8_0")
        #[arg(long)]
        quantization: Option<String>,

        /// Download all files from the model
        #[arg(long)]
        all: bool,

        /// Output directory
        #[arg(short, long)]
        output: Option<String>,
    },

    /// List available files for a model
    List {
        /// Model ID (e.g., "meta-llama/Llama-3.1-8B")
        model_id: String,
    },

    /// Resume incomplete downloads
    Resume,
}
```

### 1.4 Add Argument Parsing Tests
- [x] Create `tests/cli_tests.rs` (optional, can be manual testing)
- [x] Test basic argument parsing: `--headless search "llama"`
- [x] Test download command: `--headless download "model_id" --quantization "Q4_K_M"`
- [x] Test list command: `--headless list "model_id"`
- [x] Test resume command: `--headless resume`
- [x] Test --help output
- [x] Test --version output (if added)

**Test Commands:**
```bash
cargo run -- --help
cargo run -- --headless --help
cargo run -- --headless search --help
cargo run -- --headless download --help
```

### 1.5 Integrate with main.rs
- [x] Modify `main.rs` to parse CLI arguments
- [x] Add early return for TUI mode (no `--headless` flag)
- [x] Ensure TUI mode works exactly as before
- [x] Verify no TUI initialization occurs when `--headless` is present

**Expected Code Change in main.rs:**
```rust
// At the top of main()
mod cli;  // Add this

async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Parse CLI arguments
    let cli = cli::Cli::parse();

    // If --headless flag is present, run in headless mode
    if cli.headless {
        // TODO: Phase 2 - Implement headless mode
        eprintln!("Headless mode not yet implemented!");
        return Ok(());
    }

    // Original TUI flow (unchanged)
    use crossterm::event::EnableMouseCapture;
    use crossterm::execute;
    use std::io::stdout;
    execute!(stdout(), EnableMouseCapture)?;

    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();

    execute!(stdout(), DisableMouseCapture)?;

    result
}
```

## Verification Steps

### Manual Testing
- [ ] Run `cargo run -- --help` and verify help text displays
- [ ] Run `cargo run` (no args) and verify TUI starts normally
- [ ] Run `cargo run -- --headless search "test"` and verify "not yet implemented" message
- [ ] Run `cargo run -- --headless` (no subcommand) and verify error or help

### Build Verification
- [ ] `cargo build --release` succeeds
- [ ] No new compiler warnings
- [ ] Binary size increase is minimal (~100-200KB for clap)

## Success Criteria

### Must Have
- ✅ `clap` dependency added successfully
- ✅ CLI arguments parse without errors
- ✅ TUI mode unchanged (no regressions)
- ✅ Help text displays correctly
- ✅ Early detection of `--headless` flag in main.rs

### Nice to Have
- Custom error messages for invalid arguments
- Shell completion scripts (bash/zsh)
- Man page generation

## Next Phase Link
Once this phase is complete, proceed to **Phase 2: Extract Core Logic from UI** (`add-headless-phase2.md`).

## Notes
- Keep CLI structure simple and extensible
- Follow clap best practices for argument naming
- Ensure global flags (`--json`, `--token`) propagate to all subcommands
- Consider adding `--verbose` flag for debugging
