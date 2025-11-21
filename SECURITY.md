# Security Considerations for Rust HF Downloader

This document outlines security considerations and recommendations for the Rust HF Downloader application.

## Status of Known Vulnerabilities

### ✅ Fixed Vulnerabilities

#### 1. Path Traversal Vulnerability (Fixed)
**Severity**: HIGH  
**Status**: FIXED in current version  
**Details**: See [SECURITY_FIX.md](SECURITY_FIX.md)

Comprehensive path validation and sanitization now prevents:
- Directory traversal attacks (`../` sequences)
- Malicious model IDs or filenames
- Symbolic link exploitation
- Null byte injection

## Remaining Security Concerns

### 2. Unvalidated Remote Content from HuggingFace API
**Severity**: MEDIUM-HIGH  
**Status**: NOT FIXED

**Issue**: The application fetches model metadata and file listings from HuggingFace without signature verification or integrity checks.

**Risks**:
- Man-in-the-middle attacks (partially mitigated by HTTPS)
- Compromised API responses could serve malicious payloads
- Downloaded GGUF model files are not verified for integrity
- No validation of Content-Type headers
- File sizes from API are trusted without verification

**Recommendations**:
```rust
// 1. Implement SHA256 hash verification
async fn verify_download(path: &Path, expected_hash: &str) -> Result<bool, Error> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path).await?;
    let mut buffer = [0u8; 8192];
    while let Ok(n) = file.read(&mut buffer).await {
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let hash = format!("{:x}", hasher.finalize());
    Ok(hash == expected_hash)
}

// 2. Validate Content-Type headers
if let Some(content_type) = response.headers().get("content-type") {
    if !content_type.to_str()?.contains("application/octet-stream") {
        return Err("Unexpected content type".into());
    }
}

// 3. Add HuggingFace API signature verification if available
// Check for X-Signature or similar headers
```

**Mitigation Steps**:
1. Add SHA256 hash verification for downloaded files
2. Validate HTTP response headers (Content-Type, Content-Length)
3. Implement certificate pinning for HuggingFace API
4. Add checksum validation from `.sha256` files if available
5. Verify file magic bytes for GGUF format

### 3. Unsafe File System Operations in Download Manager
**Severity**: MEDIUM  
**Status**: NOT FIXED

**Issue**: The download manager creates directories recursively without limits and lacks proper error handling for edge cases.

**Risks**:
- Unlimited directory depth could exhaust inodes
- Race conditions between file operations (check-then-act patterns)
- `.incomplete` files can be deleted without backup (potential data loss)
- No file locking mechanism for concurrent downloads
- Directory creation doesn't validate total path length

**Current Vulnerable Code**:
```rust
// Unlimited recursive directory creation
tokio::fs::create_dir_all(&base_path).await

// No atomic operations
tokio::fs::rename(incomplete_path, final_path).await

// No file locking
let mut file = tokio::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&incomplete_path)
    .await?;
```

**Recommendations**:
```rust
// 1. Limit directory depth
const MAX_PATH_DEPTH: usize = 10;
fn validate_path_depth(path: &Path) -> Result<(), Error> {
    if path.components().count() > MAX_PATH_DEPTH {
        return Err("Path depth exceeds limit".into());
    }
    Ok(())
}

// 2. Implement file locking
use tokio::fs::File;
use fs2::FileExt;

async fn lock_download_file(path: &Path) -> Result<File, Error> {
    let file = File::create(path).await?;
    file.try_lock_exclusive()?;
    Ok(file)
}

// 3. Atomic file operations with temp files
let temp_path = final_path.with_extension("tmp");
// Write to temp_path first
tokio::fs::rename(&temp_path, &final_path).await?;

// 4. Validate total path length (most filesystems have 4096 byte limit)
const MAX_PATH_LENGTH: usize = 4096;
if path.as_os_str().len() > MAX_PATH_LENGTH {
    return Err("Path too long".into());
}
```

**Mitigation Steps**:
1. Add maximum path depth validation (suggest 10 levels)
2. Implement file locking for concurrent download protection
3. Use atomic file operations (write to temp, then rename)
4. Add total path length validation (4096 bytes max)
5. Implement proper cleanup on errors

### 4. Insecure Credential and Environment Variable Handling
**Severity**: MEDIUM  
**Status**: NOT FIXED

**Issue**: The application uses environment variables without proper validation and lacks secure credential storage.

**Risks**:
- `HOME` environment variable used without validation (falls back to `/tmp`)
- Future token support could leak credentials in logs or errors
- No protection against environment variable injection
- Registry file (`hf-downloads.toml`) stored in plaintext
- No sanitization of status messages that may contain paths

**Current Vulnerable Code**:
```rust
let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
let default_path = format!("{}/models", home);

// Status messages might leak sensitive paths
self.status = format!("Downloading {} to {}", filename, path);
```

**Recommendations**:
```rust
// 1. Validate HOME directory
fn get_safe_home_dir() -> Result<PathBuf, Error> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE")) // Windows support
        .map_err(|_| "No HOME directory found")?;
    
    let home_path = PathBuf::from(home);
    
    // Validate it exists and is a directory
    if !home_path.exists() || !home_path.is_dir() {
        return Err("HOME path is invalid".into());
    }
    
    // Canonicalize to prevent symlink attacks
    home_path.canonicalize().map_err(Into::into)
}

// 2. Sanitize paths in status messages
fn sanitize_path_for_display(path: &Path) -> String {
    // Only show relative to home or last 3 components
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    
    // Show last 3 components only
    path.components()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .iter()
        .rev()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// 3. Add future token support with secure storage
#[cfg(target_os = "linux")]
fn get_hf_token() -> Result<Option<String>, Error> {
    use keyring::Entry;
    let entry = Entry::new("rust-hf-downloader", "huggingface-token")?;
    entry.get_password().ok()
}

// 4. Encrypt registry file
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit};

fn save_encrypted_registry(registry: &DownloadRegistry, key: &[u8; 32]) -> Result<(), Error> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(b"unique nonce"); // Should be random
    
    let toml_string = toml::to_string_pretty(registry)?;
    let encrypted = cipher.encrypt(nonce, toml_string.as_bytes())?;
    
    std::fs::write(Self::get_registry_path(), encrypted)?;
    Ok(())
}
```

**Mitigation Steps**:
1. Add robust HOME directory validation with Windows support
2. Sanitize all paths in status messages (show relative to home)
3. Never log full absolute paths in error messages
4. Use keyring/keychain for future token storage
5. Encrypt the registry file with user-derived key
6. Add `RUST_LOG` filtering to prevent credential leaks in debug logs

### 5. Lack of Download Size and Rate Limiting
**Severity**: MEDIUM  
**Status**: NOT FIXED

**Issue**: No limits on file sizes, download quotas, or concurrent operations could lead to resource exhaustion.

**Risks**:
- Users can queue unlimited downloads (`download_queue_size` has no upper bound)
- No individual file size limits (could download TB-sized files)
- No disk space checks before starting downloads
- Could lead to disk exhaustion or system instability
- No session-based download quotas
- Retry logic exists but no overall rate limiting

**Current Vulnerable Code**:
```rust
// Unlimited queue size
{
    let mut queue_size = self.download_queue_size.lock().await;
    *queue_size += num_files; // No upper bound check
}

// No size validation
let total_size = response.content_length().unwrap_or(0) + resume_from;
// No check if total_size > MAX_FILE_SIZE
```

**Recommendations**:
```rust
// 1. Add file size limits
const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024 * 1024; // 50GB
const MAX_QUEUE_SIZE: usize = 20;
const MAX_SESSION_DOWNLOAD: u64 = 200 * 1024 * 1024 * 1024; // 200GB per session

// 2. Check disk space before download
fn check_available_space(path: &Path, required: u64) -> Result<bool, Error> {
    use nix::sys::statvfs::statvfs;
    
    let stat = statvfs(path)?;
    let available = stat.blocks_available() * stat.block_size();
    
    // Require 110% of file size to be safe
    Ok(available > (required * 11 / 10))
}

// 3. Enforce queue limits
async fn queue_download(&mut self, download: DownloadRequest) -> Result<(), Error> {
    let mut queue_size = self.download_queue_size.lock().await;
    
    if *queue_size >= MAX_QUEUE_SIZE {
        return Err("Download queue is full. Please wait for current downloads to complete.".into());
    }
    
    *queue_size += 1;
    self.download_tx.send(download)?;
    Ok(())
}

// 4. Validate file size from API
if total_size > MAX_FILE_SIZE {
    return Err(format!(
        "File size ({}) exceeds maximum allowed ({})",
        format_size(total_size),
        format_size(MAX_FILE_SIZE)
    ).into());
}

// 5. Track session download total
struct DownloadStats {
    session_total: Arc<Mutex<u64>>,
}

impl DownloadStats {
    async fn add_download(&self, size: u64) -> Result<(), Error> {
        let mut total = self.session_total.lock().await;
        
        if *total + size > MAX_SESSION_DOWNLOAD {
            return Err("Session download quota exceeded".into());
        }
        
        *total += size;
        Ok(())
    }
}

// 6. Add concurrent download limit
const MAX_CONCURRENT_DOWNLOADS: usize = 3;

// Use semaphore to limit concurrency
let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));

async fn download_with_limit(semaphore: Arc<Semaphore>, /* ... */) {
    let _permit = semaphore.acquire().await?;
    // Perform download
}
```

**Mitigation Steps**:
1. Add maximum file size limit (suggest 50GB)
2. Implement maximum queue size (suggest 20 items)
3. Check available disk space before each download
4. Add per-session download quota (suggest 200GB)
5. Limit concurrent downloads (suggest 3 simultaneous)
6. Display warnings when approaching limits

## Additional Security Recommendations

### 6. HTTPS Certificate Pinning
**Priority**: LOW-MEDIUM

Consider pinning HuggingFace's certificates to prevent MITM attacks:
```rust
use reqwest::tls::Certificate;

let cert = Certificate::from_pem(HUGGINGFACE_CERT_PEM)?;
let client = reqwest::Client::builder()
    .add_root_certificate(cert)
    .build()?;
```

### 7. Input Validation for Model IDs
**Priority**: MEDIUM

Add validation for model IDs before making API requests:
```rust
fn validate_model_id(id: &str) -> Result<(), Error> {
    // Format: author/model-name
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() != 2 {
        return Err("Invalid model ID format".into());
    }
    
    // Author: alphanumeric, dash, underscore only
    if !parts[0].chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid author name".into());
    }
    
    // Model name: similar rules
    if !parts[1].chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err("Invalid model name".into());
    }
    
    Ok(())
}
```

### 8. Error Message Sanitization
**Priority**: LOW-MEDIUM

Avoid leaking system information in error messages:
```rust
// Bad
Err(format!("Failed to access {}: {}", path.display(), e))

// Good
Err(format!("Failed to access file: operation not permitted"))

// Better - log full error, show sanitized message
log::error!("Failed to access {}: {}", path.display(), e);
Err("Failed to access file. Check logs for details.".into())
```

## Security Testing Checklist

Before each release, verify:

- [ ] Path traversal tests with malicious inputs (`../../etc/passwd`)
- [ ] Large file download tests (>10GB)
- [ ] Disk space exhaustion scenarios
- [ ] Concurrent download stress tests
- [ ] Invalid model ID injection tests
- [ ] Special character handling in filenames (Unicode, null bytes)
- [ ] Symbolic link attack tests
- [ ] Network interruption and resume tests
- [ ] Registry file corruption recovery
- [ ] Error message information disclosure review

## Reporting Security Vulnerabilities

If you discover a security vulnerability in Rust HF Downloader, please report it via GitHub Issues.

**Subject**: [SECURITY] Rust HF Downloader vulnerability report

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if available)

**Do not** create public GitHub issues for security vulnerabilities.

## Security Update Policy

- **Critical vulnerabilities** (RCE, arbitrary file write): Patched within 24-48 hours
- **High severity** (path traversal, DoS): Patched within 1 week
- **Medium severity** (information disclosure): Patched within 2 weeks
- **Low severity** (minor issues): Patched in next regular release

## Dependencies Security

Regularly audit dependencies for known vulnerabilities:

```bash
# Install cargo-audit
cargo install cargo-audit

# Check for vulnerable dependencies
cargo audit

# Update dependencies
cargo update
```

## References

- OWASP Top 10: https://owasp.org/www-project-top-ten/
- Rust Security Guidelines: https://anssi-fr.github.io/rust-guide/
- CWE Top 25: https://cwe.mitre.org/top25/
- NIST Secure Software Development Framework: https://csrc.nist.gov/projects/ssdf

---

**Last Updated**: 2024-11-21  
**Version**: 0.6.0
