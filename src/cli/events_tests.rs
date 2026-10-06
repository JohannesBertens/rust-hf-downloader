//! NDJSON event-schema tests (M6/T1 split of `cli/tests.rs`): the insta
//! goldens of every `cli::events::Event` shape (`src/cli/snapshots/`,
//! byte-stable — additive-only wire contract) and the literal wire-bytes
//! tables for the `error`/`file_complete` variants. Run:
//! `cargo test events_tests`

use super::events::{Event, FileDto, FileStatus, OverallProgress, Summary};
use super::testutil::snap;

#[test]
fn snapshot_event_resolved() {
    snap!(
        &Event::Resolved {
            model: "org/model".to_string(),
            files: vec![FileDto {
                filename: "model-Q4_K_M.gguf".to_string(),
                size_bytes: 4_947_802_324,
                sha256: Some("a".repeat(64)),
            }],
            total_bytes: 4_947_802_324,
        },
        "event-resolved",
    );
}

#[test]
fn snapshot_event_progress() {
    snap!(
        &Event::Progress {
            filename: "model-Q4_K_M.gguf".to_string(),
            downloaded_bytes: 1_048_576,
            total_bytes: 4_947_802_324,
            speed_mbps: 62.4,
            percent: 0.021_183,
            overall: None,
        },
        "event-progress",
    );
}

#[test]
fn snapshot_event_progress_overall() {
    snap!(
        &Event::Progress {
            filename: "model-00003-of-00017.safetensors".to_string(),
            downloaded_bytes: 1_048_576,
            total_bytes: 4_947_802_324,
            speed_mbps: 62.4,
            percent: 0.021_183,
            overall: Some(OverallProgress {
                files_done: 2,
                files_total: 17,
                downloaded_bytes: 10_485_760,
                total_bytes: 84_102_439_308,
            }),
        },
        "event-progress-overall",
    );
}

#[test]
fn snapshot_event_file_complete() {
    snap!(
        &Event::FileComplete {
            filename: "model-Q4_K_M.gguf".to_string(),
            status: FileStatus::Downloaded,
            bytes: 4_947_802_324,
        },
        "event-file-complete",
    );
}

#[test]
fn snapshot_event_verification_result_ok_omits_hashes() {
    snap!(
        &Event::VerificationResult {
            filename: "model-Q4_K_M.gguf".to_string(),
            ok: true,
            expected_sha256: None,
            actual_sha256: None,
        },
        "event-verification-ok",
    );
}

#[test]
fn snapshot_event_verification_result_mismatch_includes_hashes() {
    snap!(
        &Event::VerificationResult {
            filename: "model-Q4_K_M.gguf".to_string(),
            ok: false,
            expected_sha256: Some("e".repeat(64)),
            actual_sha256: Some("a".repeat(64)),
        },
        "event-verification-mismatch",
    );
}

#[test]
fn snapshot_event_done() {
    snap!(
        &Event::Done {
            summary: Summary {
                files: 3,
                downloaded: 2,
                skipped: 1,
                verified: 3,
                failed: 0,
                hash_mismatch: 0,
                total_bytes: 4_947_802_324,
            },
        },
        "event-done",
    );
}

#[test]
fn snapshot_event_error_ambiguous_lists_available() {
    snap!(
        &Event::Error {
            code: "ambiguous".to_string(),
            message:
                "model has 2 downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all"
                    .to_string(),
            available: Some(vec![
                FileDto {
                    filename: "model-Q4_K_M.gguf".to_string(),
                    size_bytes: 1,
                    sha256: None,
                },
                FileDto {
                    filename: "model-Q8_0.gguf".to_string(),
                    size_bytes: 2,
                    sha256: None,
                },
            ]),
        },
        "event-error-ambiguous",
    );
}

#[test]
fn snapshot_event_sync_planned() {
    snap!(
        &Event::SyncPlanned {
            model: "org/model".to_string(),
            sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            files: vec![FileDto {
                filename: "model-00001-of-00002.safetensors".to_string(),
                size_bytes: 4_947_802_324,
                sha256: Some("a".repeat(64)),
            }],
            skipped: 3,
            total_bytes: 4_947_802_324,
        },
        "event-sync-planned",
    );
}

#[test]
fn snapshot_event_file_published() {
    snap!(
        &Event::FilePublished {
            path: "model-00001-of-00002.safetensors".to_string(),
            blob: "a".repeat(64),
        },
        "event-file-published",
    );
}

#[test]
fn snapshot_event_sync_complete() {
    snap!(
        &Event::SyncComplete {
            snapshot_path: "/home/u/.cache/huggingface/hub/models--org--model/snapshots/f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            revision: "main".to_string(),
            sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
        },
        "event-sync-complete",
    );
}

#[test]
fn sync_events_serialize_with_type_tags() {
    // Smoke: serde tagging mirrors the existing events (plans/hf-cache-sync.md §2.4).
    let value = serde_json::to_value(&Event::SyncPlanned {
        model: "org/model".to_string(),
        sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        files: vec![FileDto {
            filename: "model.safetensors".to_string(),
            size_bytes: 42,
            sha256: Some("a".to_string()),
        }],
        skipped: 2,
        total_bytes: 42,
    })
    .unwrap();
    assert_eq!(value["type"], "sync_planned");
    assert_eq!(value["model"], "org/model");
    assert_eq!(value["skipped"], 2);

    let value = serde_json::to_value(&Event::FilePublished {
        path: "config.json".to_string(),
        blob: "deadbeef".to_string(),
    })
    .unwrap();
    assert_eq!(value["type"], "file_published");
    assert_eq!(value["path"], "config.json");
    assert_eq!(value["blob"], "deadbeef");

    let value = serde_json::to_value(&Event::SyncComplete {
        snapshot_path: "/x/snapshots/abc".to_string(),
        revision: "main".to_string(),
        sha: "abc".to_string(),
    })
    .unwrap();
    assert_eq!(value["type"], "sync_complete");
    assert_eq!(value["snapshot_path"], "/x/snapshots/abc");
    assert_eq!(value["revision"], "main");
}

// --- H6: error-event wire contract table ---------------------------------
// Additive-only NDJSON contract (plan H6). Every `code: "…"` literal in
// src/cli/*.rs (26 construction sites: download_cmd 11, hf_cache 13,
// search_cmd 2) collapses to the 13 distinct codes below; each entry pins
// the exact serialized bytes of `Event::Error` with that code. A new code
// MUST be added here; renaming or dropping one fails this table.

#[test]
fn error_event_code_wire_contract_table() {
    const EXPECTED: &[(&str, &str)] = &[
        // download_cmd.rs
        ("usage", r#"{"type":"error","code":"usage","message":"m"}"#),
        (
            "invalid_path",
            r#"{"type":"error","code":"invalid_path","message":"m"}"#,
        ),
        (
            "interrupted",
            r#"{"type":"error","code":"interrupted","message":"m"}"#,
        ),
        (
            "download_failed",
            r#"{"type":"error","code":"download_failed","message":"m"}"#,
        ),
        (
            "hash_mismatch",
            r#"{"type":"error","code":"hash_mismatch","message":"m"}"#,
        ),
        (
            "auth_required",
            r#"{"type":"error","code":"auth_required","message":"m"}"#,
        ),
        (
            "verification_error",
            r#"{"type":"error","code":"verification_error","message":"m"}"#,
        ),
        // hf_cache/sync.rs
        (
            "plan_failed",
            r#"{"type":"error","code":"plan_failed","message":"m"}"#,
        ),
        ("io", r#"{"type":"error","code":"io","message":"m"}"#),
        (
            "publish_failed",
            r#"{"type":"error","code":"publish_failed","message":"m"}"#,
        ),
        (
            "sync_lock",
            r#"{"type":"error","code":"sync_lock","message":"m"}"#,
        ),
        // search_cmd.rs
        (
            "internal",
            r#"{"type":"error","code":"internal","message":"m"}"#,
        ),
        (
            "network",
            r#"{"type":"error","code":"network","message":"m"}"#,
        ),
    ];
    assert_eq!(EXPECTED.len(), 13, "distinct error codes drifted");
    for (code, expected) in EXPECTED {
        let event = Event::Error {
            code: (*code).to_string(),
            message: "m".to_string(),
            available: None,
        };
        let actual = serde_json::to_string(&event).unwrap();
        assert_eq!(
            &actual, *expected,
            "error code {code:?} changed its wire bytes"
        );
    }
}

#[test]
fn error_event_available_variant_wire_contract() {
    // Real usage shape (download_cmd.rs ambiguity path): `available` lists
    // FileDtos of the repo; `sha256: null` is serialized, not omitted.
    let event = Event::Error {
        code: "ambiguous".to_string(),
        message: "model has 2 downloadable file(s)".to_string(),
        available: Some(vec![FileDto {
            filename: "model-Q4_K_M.gguf".to_string(),
            size_bytes: 4_947_802_324,
            sha256: None,
        }]),
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"error","code":"ambiguous","message":"model has 2 downloadable file(s)","available":[{"filename":"model-Q4_K_M.gguf","size_bytes":4947802324,"sha256":null}]}"#
    );
}

#[test]
fn file_complete_status_wire_contract() {
    // The `status` field is a FileStatus with exactly two variants, both
    // produced at download_cmd.rs — pin their serialized bytes.
    for (status, expected) in [
        (
            FileStatus::Downloaded,
            r#"{"type":"file_complete","filename":"model-Q4_K_M.gguf","status":"downloaded","bytes":4947802324}"#,
        ),
        (
            FileStatus::AlreadyExists,
            r#"{"type":"file_complete","filename":"model-Q4_K_M.gguf","status":"already_exists","bytes":4947802324}"#,
        ),
    ] {
        let event = Event::FileComplete {
            filename: "model-Q4_K_M.gguf".to_string(),
            status,
            bytes: 4_947_802_324,
        };
        let actual = serde_json::to_string(&event).unwrap();
        assert_eq!(
            actual, expected,
            "FileComplete status {status:?} changed its wire bytes"
        );
    }
}

#[test]
fn error_with_available_constructor_wire_shape() {
    let event = Event::error_with_available(
        super::events::ErrorCode::Usage,
        "m",
        vec![FileDto {
            filename: "f.gguf".to_string(),
            size_bytes: 1,
            sha256: None,
        }],
    );
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"error","code":"usage","message":"m","available":[{"filename":"f.gguf","size_bytes":1,"sha256":null}]}"#
    );
}
