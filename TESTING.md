# Testing Guide for Rust HF Downloader

This document describes how to run tests, benchmarks, and quality checks for the project.

## Quick Reference

```bash
# Format code
cargo fmt

# Run linter
cargo clippy

# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Build release
cargo build --release

# Check for warnings
cargo build --release --all-features
```

## Test Suite

### Unit Tests

Located alongside the code they test:

- `src/config.rs` - Configuration loading/saving tests
- `src/api.rs` - API response parsing tests

Run unit tests:
```bash
cargo test --lib
```

### Integration Tests

Located in `tests/` directory (if present). Run with:
```bash
cargo test --test integration
```

### Documentation Tests

Examples in documentation are tested automatically:
```bash
cargo test --doc
```

## Quality Checks

### Code Formatting

```bash
# Check if formatted correctly
cargo fmt --check

# Auto-format
cargo fmt
```

### Linting

```bash
# Run clippy with all warnings as errors
cargo clippy --all-features -- -D warnings
```

### Compilation

```bash
# Check compilation without building
cargo check

# Check all features
cargo check --all-features
```

### Documentation

```bash
# Generate docs
cargo doc --no-deps

# Check docs compile
cargo doc --no-deps --check
```

## Benchmarking

To run performance benchmarks:

```bash
cargo bench
```

Common benchmarks include:
- Download chunk processing speed
- API response parsing
- SHA256 verification throughput

## CI/CD Checks

This project runs these checks on every PR:

1. Format check (`cargo fmt --check`)
2. Clippy linting (`cargo clippy`)
3. Compilation (`cargo check`)
4. All tests (`cargo test`)
5. Documentation build (`cargo doc`)

## Test Coverage

Generate test coverage reports:

```bash
# Using tarpaulin
cargo tarpaulin --out html

# Using grcov
cargo tarpaulin --out lcov
```

## Testing Specific Features

### Configuration Tests

```bash
cargo test --lib -- config
```

### API Tests

```bash
cargo test --lib -- api
```

### Download Manager Tests

```bash
cargo test -- download
```

### Verification Tests

```bash
cargo test -- verification
```

## Running Tests Without Network

Some tests require network access. To run only local tests:

```bash
cargo test --lib -- --skip api
```

## Troubleshooting

### Tests Failing

1. Check if tests need environment variables set
2. Ensure you have network connectivity for API tests
3. Try cleaning and rebuilding:
   ```bash
   cargo clean
   cargo test
   ```

### Slow Tests

Tests involving downloads or verification may take time. Use `timeout`:
```bash
timeout 300 cargo test --test integration
```

### Clippy Warnings

Address all clippy warnings before submitting:
```bash
cargo clippy --fix
```
