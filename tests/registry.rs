//! Integration tests for the on-disk download registry format and filters.
//!
//! These exercise the TOML (de)serialization contract and the incomplete/
//! complete filters without touching the real HOME-based registry path,
//! so they're hermetic.

use rust_hf_downloader::models::{
    DownloadMetadata, DownloadRegistry, DownloadStatus,
};
use rust_hf_downloader::registry::{get_complete_downloads, get_incomplete_downloads};

fn sample(status: DownloadStatus, filename: &str) -> DownloadMetadata {
    DownloadMetadata {
        model_id: "author/model".into(),
        filename: filename.into(),
        url: format!("https://huggingface.co/author/model/resolve/main/{}", filename),
        local_path: format!("/tmp/{}", filename),
        total_size: 1024,
        downloaded_size: 1024,
        status,
        expected_sha256: Some("abc123".into()),
    }
}

#[test]
fn registry_roundtrips_through_toml() {
    let mut reg = DownloadRegistry::default();
    reg.downloads.push(sample(DownloadStatus::Complete, "a.bin"));
    reg.downloads.push(sample(DownloadStatus::Incomplete, "b.bin"));
    reg.downloads.push(sample(DownloadStatus::HashMismatch, "c.bin"));

    let toml_str = toml::to_string_pretty(&reg).expect("serialize");
    let back: DownloadRegistry = toml::from_str(&toml_str).expect("deserialize");

    assert_eq!(back.downloads.len(), 3);
    assert_eq!(back.downloads[0].status, DownloadStatus::Complete);
    assert_eq!(back.downloads[2].expected_sha256.as_deref(), Some("abc123"));
}

#[test]
fn incomplete_filter_includes_incomplete_and_mismatch() {
    let mut reg = DownloadRegistry::default();
    reg.downloads.push(sample(DownloadStatus::Complete, "ok.bin"));
    reg.downloads.push(sample(DownloadStatus::Incomplete, "half.bin"));
    reg.downloads.push(sample(DownloadStatus::HashMismatch, "bad.bin"));

    let incomplete = get_incomplete_downloads(&reg);
    assert_eq!(incomplete.len(), 2);
    let names: Vec<&str> = incomplete.iter().map(|d| d.filename.as_str()).collect();
    assert!(names.contains(&"half.bin"));
    assert!(names.contains(&"bad.bin"));
}

#[test]
fn complete_filter_keys_by_filename() {
    let mut reg = DownloadRegistry::default();
    reg.downloads.push(sample(DownloadStatus::Complete, "ok.bin"));
    reg.downloads.push(sample(DownloadStatus::Incomplete, "half.bin"));

    let complete = get_complete_downloads(&reg);
    assert_eq!(complete.len(), 1);
    assert!(complete.contains_key("ok.bin"));
}
