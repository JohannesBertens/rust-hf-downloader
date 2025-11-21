# Release Notes - Rust HF Downloader v0.6.2

**Release Date**: November 21, 2025  
**Focus**: TLS Backend Update for Ubuntu 22.04 Compatibility

## 🔄 Changes

### TLS Backend Migration
- **Switched from native-tls to rustls for HTTPS connections**
  - Updated `reqwest` dependency to use `rustls-tls` feature instead of default `native-tls`
  - Enables compatibility with older Rust versions (1.75.0+) on Ubuntu 22.04 LTS
  - Pure Rust TLS implementation - no system OpenSSL dependencies
  - All HTTPS functionality remains unchanged
  - No breaking changes to the codebase or API

### Technical Details
- Changed `Cargo.toml`: `reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }`
- Previous version required Rust 1.80.0+ due to `native-tls` dependency constraints
- Now compatible with Rust 1.75.0+ (Ubuntu 22.04 default compiler)

## ✅ Verification

- ✅ Compilation: `cargo check` passed
- ✅ Build: `cargo build --release` successful
- ✅ TLS functionality verified with HuggingFace API calls
- ✅ Full compatibility with Rust 1.75.0 confirmed

## 📦 Installation

```bash
git clone https://github.com/JohannesBertens/rust-hf-downloader.git
cd rust-hf-downloader
cargo build --release
```

## 🔄 Breaking Changes

None. This release is fully backward compatible with v0.6.1.

## 📝 Upgrade Instructions

Users on v0.6.1 or earlier can upgrade directly:

```bash
git pull origin main
cargo build --release
```

No configuration changes required. All downloads and registry files remain compatible.

## 📄 Full Changelog

### Version 0.6.2 (2025-11-21)
- Switched from native-tls to rustls for TLS implementation
- Added support for Rust 1.75.0+ (Ubuntu 22.04 compatibility)

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
