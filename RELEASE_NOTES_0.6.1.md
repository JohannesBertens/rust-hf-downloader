# Release Notes - Rust HF Downloader v0.6.1

**Release Date**: November 21, 2025  
**Focus**: Rust Edition Update

## 🔄 Changes

### Rust Edition Migration
- **Downgraded from Rust 2024 to Rust 2021 edition**
  - Updated `Cargo.toml` edition field from `"2024"` to `"2021"`
  - Ensures broader compatibility with stable Rust toolchains
  - All features and functionality remain unchanged
  - No breaking changes to the codebase or API

## ✅ Verification

- ✅ Compilation: `cargo check` passed
- ✅ Build: `cargo build` successful
- ✅ Full compatibility with Rust 2021 edition confirmed

## 📦 Installation

```bash
git clone https://github.com/JohannesBertens/rust-hf-downloader.git
cd rust-hf-downloader
cargo build --release
```

## 🔄 Breaking Changes

None. This release is fully backward compatible with v0.6.0.

## 📝 Upgrade Instructions

Users on v0.6.0 can upgrade directly:

```bash
git pull origin main
cargo build --release
```

No configuration changes required. All downloads and registry files remain compatible.

## 📄 Full Changelog

### Version 0.6.1 (2025-11-21)
- Changed Rust edition from 2024 to 2021 in Cargo.toml

### Version 0.6.0 (2024-11-21)
- **Security**: Fixed HIGH severity path traversal vulnerability
- Added comprehensive path validation and sanitization
- Created SECURITY.md and SECURITY_FIX.md documentation

### Version 0.5.0
- Added download resume on startup
- Multi-part GGUF file support
- Progress tracking with speed indicators
- TOML-based download registry

---

**Project**: Rust HF Downloader  
**License**: MIT  
**Author**: Johannes Bertens
