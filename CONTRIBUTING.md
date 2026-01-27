# Contributing to Rust HF Downloader

Thank you for your interest in contributing! This document outlines the process for contributing to this project.

## Getting Started

### Prerequisites

- **Rust**: 1.75.0 or newer
- **Cargo**: Latest stable version
- **Git**: For version control

### Development Setup

1. **Fork the repository** on GitHub
2. **Clone your fork**:
   ```bash
   git clone https://github.com/YOUR_USERNAME/rust-hf-downloader.git
   cd rust-hf-downloader
   ```

3. **Set up development environment**:
   ```bash
   cargo build --dev
   ```

4. **Create a feature branch**:
   ```bash
   git checkout -b feature/your-feature-name
   ```

## Development Workflow

### Code Style

This project uses standard Rust formatting:

```bash
# Check formatting
cargo fmt --check

# Auto-format code
cargo fmt
```

### Linting

```bash
cargo clippy
```

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_name
```

### Building

```bash
# Development build (faster, no optimizations)
cargo build

# Release build
cargo build --release

# Build for testing
cargo test --no-run
```

## Submitting Changes

### Pull Request Process

1. **Ensure all checks pass**:
   ```bash
   cargo fmt
   cargo clippy
   cargo test
   ```

2. **Commit your changes** with a descriptive message:
   ```bash
   git add .
   git commit -m "feat: add feature description"
   ```

   Follow [Conventional Commits](https://www.conventionalcommits.org/) format:
   - `feat:` for new features
   - `fix:` for bug fixes
   - `refactor:` for code refactoring
   - `docs:` for documentation changes
   - `chore:` for maintenance tasks

3. **Push to your fork**:
   ```bash
   git push origin feature/your-feature-name
   ```

4. **Open a Pull Request** against the `main` branch

### Commit Message Format

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

Examples:
```
feat(api): add support for new quantization types

Fixes: #123
```

## Area-Specific Guidelines

### UI Changes

- Follow existing patterns in `src/ui/render.rs`
- Update AGENTS.md if adding new modules or significant logic
- Test mouse and keyboard interactions

### API Changes

- Update AGENTS.md documentation
- Add unit tests for new functions
- Ensure backward compatibility

### Configuration Changes

- Update `src/config.rs` for persisted options
- Update `src/models.rs` AppOptions struct
- Update TUI options screen in `src/ui/app/downloads.rs`

### Documentation

- Update README.md for user-facing changes
- Update AGENTS.md for developer-facing changes
- Add changelog entry for notable changes

## Reporting Issues

### Bug Reports

Include:
1. Steps to reproduce
2. Expected behavior
3. Actual behavior
4. Environment (OS, Rust version, etc.)
5. Relevant logs or screenshots

### Feature Requests

Include:
1. Clear description of the feature
2. Use case or motivation
3. Suggested implementation (optional)
4. Alternative solutions considered (optional)

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](https://www.contributor-covenant.org/version/2/1/code_of_conduct.html).

## Questions?

Open an issue for discussion or reach out via GitHub.
