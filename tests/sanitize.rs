//! Integration tests for path-traversal hardening in the download layer.
//!
//! These guard the security-critical sanitization logic so that the
//! structural refactors (shared runtime, config ownership) can't silently
//! weaken path validation.

use rust_hf_downloader::download::{sanitize_path_component, validate_and_sanitize_path};

#[test]
fn rejects_traversal_components() {
    assert!(sanitize_path_component("..").is_none());
    assert!(sanitize_path_component(".").is_none());
    assert!(sanitize_path_component("").is_none());
    assert!(sanitize_path_component("a/b").is_none());
    assert!(sanitize_path_component("a\\b").is_none());
    assert!(sanitize_path_component("a\0b").is_none());
}

#[test]
fn accepts_valid_components() {
    assert_eq!(sanitize_path_component("meta-llama").as_deref(), Some("meta-llama"));
    // Leading dots (dotfiles) are preserved; only trailing dots are trimmed.
    assert_eq!(sanitize_path_component(".gitattributes").as_deref(), Some(".gitattributes"));
    // Whitespace is trimmed.
    assert_eq!(sanitize_path_component("  model.bin  ").as_deref(), Some("model.bin"));
}

#[test]
fn validate_rejects_bad_model_id_format() {
    let tmp = tempfile_dir();
    let err = validate_and_sanitize_path(&tmp, "no-slash-here", "model.bin");
    assert!(err.is_err());
}

#[test]
fn validate_rejects_traversal_in_model_id() {
    let tmp = tempfile_dir();
    assert!(validate_and_sanitize_path(&tmp, "../etc", "model.bin").is_err());
    assert!(validate_and_sanitize_path(&tmp, "author/..", "model.bin").is_err());
}

#[test]
fn validate_rejects_traversal_in_filename() {
    let tmp = tempfile_dir();
    assert!(validate_and_sanitize_path(&tmp, "author/model", "../../../etc/passwd").is_err());
}

#[test]
fn validate_accepts_subdir_filename() {
    let tmp = tempfile_dir();
    let path = validate_and_sanitize_path(&tmp, "author/model", "Q4_K_M/file.gguf");
    assert!(path.is_ok());
    let path = path.unwrap();
    assert!(path.to_string_lossy().ends_with("author/model/Q4_K_M/file.gguf"));
}

/// Build a unique temporary directory for each test. Uses a real temp dir so
/// `base.canonicalize()` succeeds the same way it does in production.
fn tempfile_dir() -> String {
    let dir = std::env::temp_dir().join(format!(
        "rust-hf-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}
