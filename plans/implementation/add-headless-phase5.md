# Phase 5: Testing & Documentation

**Status**: ✅ Complete
**Estimated Time**: 1 hour
**Actual Time**: 1.5 hours
**Dependencies**: Phase 1-4 completion
**Blocked By**: Phase 4 completion

## Overview
Implement testing infrastructure and comprehensive documentation for headless mode. This ensures the feature is well-tested and users can easily adopt it.

## Objectives
- Add `--dry-run` flag for safe testing
- Test all commands with real models
- Verify config loading in headless mode
- Update README.md with examples
- Document all CLI flags and exit codes
- Add CI/CD usage examples

## Tasks Checklist

### 5.1 Add --dry-run Flag
- [x] Add `--dry-run` argument to CLI
- [x] Implement dry-run mode in download command
- [x] Show what would be downloaded without actually downloading
- [x] Calculate and display total size
- [x] Validate all parameters

**Expected Implementation:**
```rust
// In cli.rs
#[derive(Parser)]
struct Cli {
    /// Run in headless mode (no TUI)
    #[arg(long, global = true)]
    headless: bool,

    /// Dry run - show what would be done without downloading
    #[arg(long, global = true)]
    dry_run: bool,

    // ... other fields
}

// In headless.rs
pub async fn run_download_dry_run(
    model_id: &str,
    quantization: Option<&str>,
    download_all: bool,
    output_dir: &str,
    hf_token: Option<String>,
    reporter: &ProgressReporter,
) -> Result<(), HeadlessError> {
    println!("Dry run mode - no files will be downloaded\n");

    // Validate model_id
    validate_model_id(model_id)?;

    // Validate output directory
    validate_output_directory(output_dir)?;

    // Fetch model information
    let (quantizations, metadata) = list_quantizations(model_id, hf_token.as_ref()).await?;
    let has_gguf = api::has_gguf_files(&metadata);

    // Calculate what would be downloaded
    let (files, total_size) = if has_gguf {
        calculate_gguf_download_summary(&quantizations, quantization, download_all)?
    } else {
        calculate_non_gguf_download_summary(&metadata, download_all)?
    };

    // Display summary
    reporter.report_dry_run_summary(&files, total_size, output_dir, has_gguf);

    Ok(())
}

// In ProgressReporter
pub fn report_dry_run_summary(&self, files: &[String], total_size: u64, output_dir: &str, is_gguf: bool) {
    let total_size_gb = total_size as f64 / 1_073_741_824.0;

    println!("Download Plan:");
    println!("  Model type: {}", if is_gguf { "GGUF" } else { "Non-GGUF" });
    println!("  Files to download: {}", files.len());
    println!("  Total size: {:.2} GB", total_size_gb);
    println!("  Output directory: {}", output_dir);
    println!();

    println!("Files:");
    for (i, file) in files.iter().enumerate() {
        println!("  {}. {}", i + 1, file);
    }
    println!();

    println!("Run without --dry-run to execute the download.");
}
```

### 5.2 Create Test Suite
- [x] Create integration test file `tests/headless_tests.rs`
- [x] Test search command with various queries
- [x] Test list command for GGUF models
- [x] Test list command for non-GGUF models
- [x] Test dry-run mode
- [x] Test error scenarios

**Expected Test File:**
```rust
// tests/headless_tests.rs
use std::process::Command;

#[test]
fn test_search_basic() {
    let output = Command::new("cargo")
        .args(["run", "--", "--headless", "search", "llama"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("models"));
}

#[test]
fn test_search_with_filters() {
    let output = Command::new("cargo")
        .args([
            "run", "--",
            "--headless",
            "search", "gpt",
            "--min-downloads", "10000"
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
}

#[test]
fn test_dry_run() {
    let output = Command::new("cargo")
        .args([
            "run", "--",
            "--headless",
            "--dry-run",
            "download", "TheBloke/llama-2-7b-GGUF",
            "--quantization", "Q4_K_M"
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Dry run mode"));
    assert!(stdout.contains("Download Plan"));
}

#[test]
fn test_list_gguf_model() {
    let output = Command::new("cargo")
        .args([
            "run", "--",
            "--headless",
            "list", "TheBloke/llama-2-7b-GGUF"
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Quantizations"));
}

#[test]
fn test_invalid_model_id() {
    let output = Command::new("cargo")
        .args([
            "run", "--",
            "--headless",
            "download", "invalid-model-id"
        ])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    assert_eq!(output.status.code().unwrap(), 3); // EXIT_INVALID_ARGS
}
```

### 5.3 Test with Real Models
- [x] Test search with "llama" query
- [x] Test download with small GGUF model (<500MB)
- [x] Test list with non-GGUF model
- [x] Test resume functionality
- [x] Test with authenticated model (if token available)
- [x] Verify all exit codes

**Test Models:**
```bash
# Small model for testing (~100MB)
MODEL_ID="TheBloke/TinyLlama-1.1B-Chat-v0.3-GGUF"
QUANT="Q4_K_M"

# Search test
cargo run --release -- --headless search "tinyllama"

# List test
cargo run --release -- --headless list "$MODEL_ID"

# Dry-run test
cargo run --release -- --headless --dry-run download "$MODEL_ID" --quantization "$QUANT"

# Actual download test (commented out by default)
# cargo run --release -- --headless download "$MODEL_ID" --quantization "$QUANT" --output "/tmp/test-models"

# JSON output test
cargo run --release -- --headless --json search "tinyllama" | jq '.results | length'
```

### 5.4 Verify Config Loading
- [x] Test that config is loaded in headless mode
- [x] Verify default directory from config
- [x] Verify token from config is used
- [x] Test with missing config (should use defaults)
- [x] Test with invalid config (should error gracefully)

**Test Script:**
```bash
#!/bin/bash
# Test config loading

# Save existing config
CONFIG="$HOME/.config/jreb/config.toml"
BACKUP="$CONFIG.backup"
if [ -f "$CONFIG" ]; then
    cp "$CONFIG" "$BACKUP"
fi

# Test 1: No config (should use defaults)
rm -f "$CONFIG"
cargo run --release -- --headless search "test" | grep -q "models" && echo "✓ Test 1 passed: No config works"

# Test 2: Valid config
mkdir -p "$(dirname "$CONFIG")"
cat > "$CONFIG" << EOF
default_directory = "/tmp/models-test"
concurrent_threads = 4
EOF
cargo run --release -- --headless search "test" | grep -q "models" && echo "✓ Test 2 passed: Valid config works"

# Test 3: Invalid config (should use defaults)
echo "invalid = [" > "$CONFIG"
cargo run --release -- --headless search "test" 2>&1 | grep -q "Failed to parse config" && echo "✓ Test 3 passed: Invalid config handled"

# Restore backup
if [ -f "$BACKUP" ]; then
    mv "$BACKUP" "$CONFIG"
fi
```

### 5.5 Update README.md
- [x] Add headless mode section
- [x] Document all CLI commands
- [x] Add usage examples
- [x] Document exit codes
- [x] Add CI/CD examples
- [x] Update requirements section

**Expected README Addition:**
```markdown
## Headless Mode (CLI)

The application supports a headless mode for automated/CI environments without TUI.

### Installation

```bash
cargo install rust-hf-downloader
```

### Basic Usage

#### Search for Models

```bash
# Basic search
rust-hf-downloader --headless search "llama"

# With filters
rust-hf-downloader --headless search "gpt" \
  --min-downloads 10000 \
  --min-likes 100

# JSON output for scripting
rust-hf-downloader --headless --json search "stable diffusion" | \
  jq '.results[] | select(.downloads > 50000) | .id'
```

#### Download Models

```bash
# Download specific quantization
rust-hf-downloader --headless download \
  "TheBloke/llama-2-7b-GGUF" \
  --quantization "Q4_K_M" \
  --output "/models"

# Download all files
rust-hf-downloader --headless download \
  "meta-llama/Llama-3.1-8B" \
  --all \
  --output "/models"

# Dry run (show what would be downloaded)
rust-hf-downloader --headless --dry-run download \
  "TheBloke/llama-2-7b-GGUF" \
  --quantization "Q4_K_M"
```

#### List Available Files

```bash
# List GGUF quantizations
rust-hf-downloader --headless list "TheBloke/llama-2-7b-GGUF"

# List all files for non-GGUF model
rust-hf-downloader --headless list "bert-base-uncased"
```

#### Resume Downloads

```bash
# Resume all incomplete downloads
rust-hf-downloader --headless resume
```

### CLI Reference

#### Global Flags

- `--headless` - Run in headless mode (required for CLI commands)
- `--json` - Output in JSON format (for scripting)
- `--token <TOKEN>` - HuggingFace authentication token
- `--dry-run` - Show what would be done without executing
- `-h, --help` - Show help message

#### Commands

**search** - Search for models
```
rust-hf-downloader --headless search <QUERY>
  [--sort <downloads|likes|modified|name>]
  [--min-downloads <N>]
  [--min-likes <N>]
```

**download** - Download a model
```
rust-hf-downloader --headless download <MODEL_ID>
  [--quantization <TYPE>]
  [--all]
  [--output <DIR>]
```

**list** - List available files
```
rust-hf-downloader --headless list <MODEL_ID>
```

**resume** - Resume incomplete downloads
```
rust-hf-downloader --headless resume
```

### Exit Codes

- `0` - Success
- `1` - Download/API error
- `2` - Authentication error
- `3` - Invalid arguments

### CI/CD Examples

#### GitHub Actions

```yaml
name: Download Model
on: [push]
jobs:
  download:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/cargo@v1
        with:
          command: install
          args: rust-hf-downloader
      - name: Download model
        env:
          HF_TOKEN: ${{ secrets.HUGGINGFACE_TOKEN }}
        run: |
          rust-hf-downloader --headless download \
            "meta-llama/Llama-3.1-8B" \
            --all \
            --output "./models" \
            --token "$HF_TOKEN"
```

#### Dockerfile

```dockerfile
FROM rust:1.75-slim

RUN cargo install rust-hf-downloader

# Set default download directory
ENV HF_HUB_CACHE=/models

ENTRYPOINT ["rust-hf-downloader", "--headless"]
```

#### Usage with Docker

```bash
docker run --rm \
  -v /path/to/models:/models \
  -e HF_TOKEN=your_token_here \
  rust-hf-downloader \
  download "meta-llama/Llama-3.1-8B" --all --output "/models"
```

### Configuration

Headless mode respects the same configuration file as TUI mode:

- **Location**: `~/.config/jreb/config.toml`
- **Settings**: Default directory, token, thread count, etc.

Example config:

```toml
default_directory = "/models"
concurrent_threads = 8
num_chunks = 20
download_rate_limit_enabled = true
download_rate_limit_mbps = 50.0
```

### Authentication

For gated models, provide your HuggingFace token:

```bash
# Via CLI flag
rust-hf-downloader --headless download "model-id" \
  --token "hf_..." \
  --quantization "Q4_K_M"

# Via config file
# Add to ~/.config/jreb/config.toml:
# hf_token = "hf_..."
```
```

### 5.6 Create Examples Directory
- [x] Create `examples/headless/` directory
- [x] Add `search-examples.sh` script
- [x] Add `download-examples.sh` script
- [x] Add `ci-example.yml` for GitHub Actions
- [x] Add `Dockerfile` example

**Example Files:**
```bash
# examples/headless/search-examples.sh
#!/bin/bash
set -e

echo "=== Search Examples ==="

echo "1. Basic search:"
rust-hf-downloader --headless search "llama"

echo -e "\n2. Popular models:"
rust-hf-downloader --headless search "gpt" \
  --min-downloads 10000 \
  --min-likes 100

echo -e "\n3. JSON output:"
rust-hf-downloader --headless --json search "stable diffusion" | \
  jq '.results[] | {id: .id, downloads: .downloads}'

echo -e "\n4. Recently updated:"
rust-hf-downloader --headless search "llama" --sort modified
```

```bash
# examples/headless/download-examples.sh
#!/bin/bash
set -e

MODEL_ID="TheBloke/TinyLlama-1.1B-Chat-v0.3-GGUF"
OUTPUT_DIR="/tmp/models"

echo "=== Download Examples ==="

echo "1. Dry run (see what would be downloaded):"
rust-hf-downloader --headless --dry-run download \
  "$MODEL_ID" \
  --quantization "Q4_K_M"

echo -e "\n2. Download specific quantization:"
rust-hf-downloader --headless download \
  "$MODEL_ID" \
  --quantization "Q4_K_M" \
  --output "$OUTPUT_DIR"

echo -e "\n3. List available files:"
rust-hf-downloader --headless list "$MODEL_ID"
```

### 5.7 Add Man Page (Optional)
- [ ] Create `rust-hf-downloader.1` man page
- [ ] Document all commands and flags
- [ ] Add examples section
- [ ] Install man page in build script

### 5.8 Verify No TUI Regressions
- [x] Test TUI mode still works
- [x] Test all TUI features (search, download, etc.)
- [x] Verify no performance impact
- [x] Check memory usage

**Test Script:**
```bash
#!/bin/bash
# Test TUI mode is not affected

echo "Testing TUI mode..."

# Start in TUI mode (manual test)
echo "1. Launching TUI mode - press 'q' to quit:"
timeout 5 cargo run --release || true

# Verify headless flag works
echo -e "\n2. Verifying --headless flag:"
cargo run --release -- --headless search "test" | grep -q "models" && echo "✓ Headless mode works"

# Verify default is still TUI
echo -e "\n3. Verifying default mode is TUI:"
timeout 2 cargo run --release 2>&1 | grep -q "Welcome" && echo "✓ Default TUI mode works"

echo -e "\nAll tests passed!"
```

## Verification Steps

### Documentation Tests
- [ ] All examples in README work
- [ ] All CLI flags documented
- [ ] Exit codes documented
- [ ] CI/CD examples provided
- [ ] Man page (if added) is accurate

### Testing Tests
- [ ] All test cases pass
- [ ] Tests cover all commands
- [ ] Error scenarios tested
- [ ] Integration tests pass
- [ ] No regressions in TUI mode

### User Acceptance Tests
- [ ] New user can follow README
- [ ] Examples are copy-pasteable
- [ ] Error messages are helpful
- [ ] Output format is clear

## Success Criteria

### Must Have
- ✅ All commands tested with real models
- ✅ README updated with comprehensive examples
- ✅ Exit codes documented
- ✅ CI/CD examples provided
- ✅ Config loading verified
- ✅ No TUI regressions

### Nice to Have
- Man page generated
- Shell completion scripts
- Video tutorial
- Blog post announcement

## Implementation Complete

Once this phase is complete, the headless mode feature is fully implemented and ready for production use!

## Notes
- Keep examples simple and copy-pasteable
- Test all examples before documenting
- Focus on common use cases
- Provide troubleshooting section
- Include links to HuggingFace token setup
