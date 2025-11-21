# Release Notes - hf-search v0.6.0

**Release Date**: November 21, 2024  
**Focus**: Security hardening and path traversal vulnerability fix

## 🔒 Security Fixes

### Critical: Path Traversal Vulnerability (HIGH Severity)
Fixed a critical security vulnerability that could allow malicious model IDs or filenames to write files outside the intended download directory.

**Impact**: This vulnerability could have allowed attackers to:
- Write arbitrary files to system directories via crafted model IDs (e.g., `../../etc/passwd`)
- Escape download directories using path traversal sequences in filenames
- Exploit symbolic link attacks

**Resolution**: Comprehensive path validation and sanitization system implemented.

## ✨ New Features

### Path Validation System
- **`sanitize_path_component()`**: Validates individual path components
  - Rejects `.`, `..`, empty strings, and null bytes
  - Blocks forward and backward slashes in components
  - Strips dangerous leading/trailing characters
  
- **`validate_and_sanitize_path()`**: End-to-end path validation
  - Validates and canonicalizes base paths
  - Enforces model_id format validation (`author/model-name`)
  - Sanitizes all filename components
  - Performs final canonicalization check to ensure paths stay within base directory

### Enhanced Download Security
- All user-provided paths validated before use
- API-provided filenames sanitized before file operations
- Canonicalization checks in download manager
- Multi-part file downloads fully validated
- Descriptive error messages for invalid paths

## 📚 Documentation

### New Files
- **SECURITY.md**: Comprehensive security documentation
  - Lists all security considerations (fixed and remaining)
  - Provides code examples for recommended fixes
  - Includes vulnerability reporting process
  - Documents security update policy
  
- **SECURITY_FIX.md**: Detailed analysis of the path traversal fix
  - Attack scenarios and examples
  - Before/after comparison
  - Security guarantees provided
  - Testing recommendations

### Updated Files
- **README.md**: 
  - Added security notice at the top
  - New "Security" section with key features
  - New "Changelog" section
  - Reference to SECURITY.md
  
- **Cargo.toml**: Version bumped to 0.6.0

## 🛡️ Security Guarantees

### Before v0.6.0
❌ User input paths accepted without validation  
❌ API-provided filenames used directly in file operations  
❌ No canonicalization or traversal checks  
❌ Vulnerable to `../` and symbolic link attacks  

### After v0.6.0
✅ All path components individually sanitized  
✅ Paths canonicalized and verified against base directory  
✅ Model IDs validated to match expected format  
✅ Multi-level protection: input validation → component sanitization → canonicalization checks  
✅ Descriptive error messages without information leakage  

## ⚠️ Remaining Security Considerations

While this release addresses the critical path traversal vulnerability, the following security enhancements are recommended for future releases:

1. **Unvalidated Remote Content** (MEDIUM-HIGH) - Add hash verification for downloads
2. **File System Operations** (MEDIUM) - Implement file locking and atomic operations
3. **Credential Handling** (MEDIUM) - Add secure token storage for private models
4. **Resource Limits** (MEDIUM) - Implement download size/rate limiting

See [SECURITY.md](SECURITY.md) for detailed recommendations and code examples.

## 🔄 Breaking Changes

None. This release is fully backward compatible with v0.5.0.

## 📦 Installation

```bash
git clone https://github.com/username/hf-search.git
cd hf-search
cargo build --release
```

## 🧪 Testing

All existing functionality verified:
- ✅ Compilation: `cargo check` passed
- ✅ Unit tests: `cargo test` passed
- ✅ Release build: `cargo build --release` successful
- ✅ Path validation tested with malicious inputs

## 📝 Upgrade Instructions

Users on v0.5.0 can upgrade directly:

```bash
git pull origin main
cargo build --release
```

No configuration changes required. All downloads and registry files remain compatible.

## 🙏 Acknowledgments

Security analysis and fix implemented to address the top security concerns identified in codebase review.

## 📮 Reporting Security Issues

If you discover a security vulnerability, please report it to:
- **Email**: 
- **Subject**: [SECURITY] hf-search vulnerability report

Do not create public GitHub issues for security vulnerabilities.

## 📄 Full Changelog

### Version 0.6.0 (2024-11-21)
- **Security**: Fixed HIGH severity path traversal vulnerability
- Added comprehensive path validation and sanitization
- Added `validate_and_sanitize_path()` function for safe path handling
- Added `sanitize_path_component()` helper function
- Updated download manager with canonicalization checks
- Updated `App::confirm_download()` with path validation
- Updated `start_download()` with runtime validation
- Created SECURITY.md with remaining security considerations
- Created SECURITY_FIX.md with detailed vulnerability analysis
- Updated README.md with security notice and changelog
- Version bumped to 0.6.0 in Cargo.toml

### Version 0.5.0 (Previous)
- Added download resume on startup
- Multi-part GGUF file support
- Progress tracking with speed indicators
- TOML-based download registry

---

**Project**: hf-search  
**License**: MIT  
**Author**: Johannes Bertens
