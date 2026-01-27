# Troubleshooting Guide

This document covers common issues, their causes, and solutions.

## Table of Contents

- [Installation Issues](#installation-issues)
- [Download Problems](#download-problems)
- [Authentication Errors](#authentication-errors)
- [Display Issues](#display-issues)
- [Performance Issues](#performance-issues)
- [File Path Issues](#file-path-issues)
- [Configuration Issues](#configuration-issues)

## Installation Issues

### Rust Version Too Old

**Error**: `error: package requires Rust Edition 2021 but 1.75.0 is below that`

**Solution**: Install a newer Rust version:

```bash
rustup update stable
rustup default stable
```

Or install from source:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Compilation Fails with Missing Dependencies

**Error**: Various linker or dependency errors

**Solution**: Install development dependencies:

```bash
# Ubuntu/Debian
sudo apt-get update
sudo apt-get install build-essential pkg-config libssl-dev

# Fedora/RHEL
sudo dnf install gcc openssl-devel
```

### Crates.io Installation Fails

**Error**: `error: could not find `rust-hf-downloader` in registry`

**Solution**: Build from source instead:

```bash
git clone https://github.com/JohannesBertens/rust-hf-downloader.git
cd rust-hf-downloader
cargo install --path .
```

## Download Problems

### Downloads Fail Silently

**Symptom**: Download starts but never completes, no error shown

**Solutions**:

1. Check disk space:
   ```bash
   df -h
   ```

2. Check download directory permissions:
   ```bash
   ls -la ~/models/
   ```

3. Enable rate limiting to prevent network issues:
   - Press `o` to open options
   - Navigate to "Rate Limit" and enable it
   - Set "Max Download Speed" to 25 MB/s

### Cannot Resume Download

**Symptom**: Resume prompts appear but download doesn't continue

**Solutions**:

1. Delete incomplete files and start fresh:
   - On resume popup, press `D` to delete and skip

2. Check registry for corruption:
   ```bash
   cat ~/models/hf-downloads.toml
   ```
   If corrupted, delete it:
   ```bash
   rm ~/models/hf-downloads.toml
   ```

### Multi-part GGUF Files Not Downloading Completely

**Symptom**: Only some parts of a multi-file model download

**Solution**: The application should auto-queue all parts. If not:

1. Press `d` on the quantization group, not individual files
2. Check that all parts have the same base name

### Download Speed Very Slow

**Symptom**: Downloads at <1 MB/s on fast connection

**Solutions**:

1. Increase concurrent threads:
   - Press `o` → Options
   - Navigate to "Concurrent Threads"
   - Increase from default (4) to 8

2. Increase chunk size:
   - Navigate to "Chunk Size (MB)"
   - Increase from default (10) to 50

3. Disable rate limiting if enabled

## Authentication Errors

### 401 Unauthorized for Gated Models

**Error Popup**: Authentication required, shows link to get token

**Solution**:

1. Get token from: https://huggingface.co/settings/tokens
2. Accept model terms on the model's page
3. Press `o` to open Options
4. Navigate to "HuggingFace Token"
5. Press Enter, paste token, Enter again
6. Token saved to `~/.config/jreb/config.toml`

### Token Not Saved

**Symptom**: Must re-enter token on every launch

**Solution**: Check config file permissions:

```bash
chmod 600 ~/.config/jreb/config.toml
```

### Still Getting 401 After Adding Token

**Solutions**:

1. Verify token is valid:
   ```bash
   curl -H "Authorization: Bearer YOUR_TOKEN" https://huggingface.co/api/user
   ```

2. Ensure you accepted model terms on the model page

3. Check for extra spaces in token field in Options

## Display Issues

### Screen is Blank/Empty

**Symptom**: No content displayed after search or initial load

**Solution**:

1. Try with different terminal themes (dark vs light)
2. Increase terminal font size
3. Check terminal dimensions:
   ```bash
   echo $LINES $COLUMNS
   ```
   Minimum: 24 lines, 80 columns

### Text Contrast Issues

**Symptom**: Text hard to read or invisible

**Solution**:

1. Use terminal's default colors
2. Avoid custom color schemes while running
3. Set terminal background to dark

### UI Elements Misaligned

**Symptom**: Borders, tables, or panels don't line up

**Solution**: Resize terminal to at least 120x40:

```bash
# Check current size
echo $LINES $COLUMNS
# If less than 40x120, resize window
```

## Performance Issues

### High CPU Usage

**Symptom**: Fan spins up, system slows down during downloads

**Solution**: Limit concurrent operations:

1. Reduce concurrent threads (Options → Concurrent Threads)
2. Enable rate limiting with lower value
3. Limit verification concurrency

### Slow Search Results

**Symptom**: Search takes >5 seconds to return

**Solutions**:

1. Check internet connection
2. Use more specific search terms
3. Increase API timeout in config (not exposed in UI)

### Memory Usage Grows

**Symptom**: Application uses more RAM over time

**Solution**: Restart application periodically. Known issue with long-running sessions.

## File Path Issues

### Path Not Found Error

**Symptom**: "Invalid path" or "Path traversal" errors

**Solution**: Use absolute paths:

```bash
# Instead of:
~/models/

# Use:
/home/username/models/
```

### Permission Denied

**Symptom**: Cannot write to download directory

**Solution**: Fix directory permissions:

```bash
mkdir -p ~/models
chmod 755 ~/models
```

### Downloads Go to Wrong Location

**Symptom**: Files saved in unexpected subdirectories

**Solution**: Use simple download path:

1. Press `d` on quantization
2. Edit path to simple location: `/home/user/models`
3. Files will be organized as: `/home/user/models/author/model-name/filename`

## Configuration Issues

### Settings Not Persisting

**Symptom**: Options reset on restart

**Solutions**:

1. Check config file exists:
   ```bash
   cat ~/.config/jreb/config.toml
   ```

2. Fix config file permissions:
   ```bash
   chmod 600 ~/.config/jreb/config.toml
   ```

3. Check disk space isn't full

### Options Screen Doesn't Open

**Symptom**: Pressing `o` does nothing

**Solution**: Ensure you're in the main view (not in a popup):
1. Press `Esc` to close any open popups
2. Press `o` again

### Cannot Find Configuration File

**Location**: `~/.config/jreb/config.toml`

If missing, the application will regenerate defaults on next start.

## Still Having Issues?

1. Check the [GitHub Issues](https://github.com/JohannesBertens/rust-hf-downloader/issues)
2. Search existing issues for your problem
3. Open a new issue with:
   - Operating system and version
   - Rust version (`rustc --version`)
   - Terminal emulator used
   - Steps to reproduce
   - Error messages (exact text)
