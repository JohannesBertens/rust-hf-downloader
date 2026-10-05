//! W2.4a golden fixtures: byte-exact pins of every inline load-modify-save
//! registry mutation the engine performs today (download.rs x5,
//! verification.rs x1, engine.rs `register_pending`), plus the concurrency
//! and failure contracts the W2.4b typed ops must reproduce unchanged:
//!
//! - **Disk is the source of truth.** Every op loads the on-DISK registry,
//!   mutates, saves (non-atomic `fs::File::create`, errors silently
//!   swallowed — deliberately, see plan §8.5), and only then updates the
//!   in-memory mirror — never the reverse.
//! - **The mirror is updated regardless of whether the save succeeded**
//!   (today's silent-failure behavior).
//! - **No lock is held across load-modify-save.** The lost-update race
//!   between concurrent writers is a known deferred defect (plan §8) that
//!   these tests PIN, not fix.
//!
//! The golden tests drive the real typed ops (`super::mark_complete`,
//! `mark_failed`, `upsert_metadata`, `mark_mismatch`, and the real
//! `engine::register_pending`). In W2.4a the very same assertions pinned
//! byte-for-byte identical replicas of the pre-refactor inline sequences —
//! passing unchanged through the W2.4b swap is the migration's
//! behavior-preservation proof.

use super::*;
use crate::models::{CompleteDownloads, DownloadMetadata, DownloadRegistry, DownloadStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

// -------------------------------------------------------------------------
// Env plumbing
//
// Every test in this module redirects the process-global registry path via
// `RUST_HF_DOWNLOADER_DATA_DIR`. They serialize on the shared
// `paths::ENV_MUTEX` (the same mutex the registry-touching engine tests
// take) so no sibling test in this test binary observes a redirected
// registry path mid-test.
// -------------------------------------------------------------------------

/// RAII guard restoring `ENV_DATA_DIR` on drop (same pattern as the
/// `EnvGuard`s in the `paths.rs` and `engine.rs` tests).
struct DataDirGuard {
    original: Option<std::ffi::OsString>,
}

impl DataDirGuard {
    fn install(dir: &Path) -> Self {
        let original = std::env::var_os(crate::paths::ENV_DATA_DIR);
        std::env::set_var(crate::paths::ENV_DATA_DIR, dir);
        Self { original }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(v) => std::env::set_var(crate::paths::ENV_DATA_DIR, v),
            None => std::env::remove_var(crate::paths::ENV_DATA_DIR),
        }
    }
}

fn tmp(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rhd-registry-golden-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn read_registry_file() -> String {
    std::fs::read_to_string(crate::paths::registry_path()).expect("registry file readable")
}

// -------------------------------------------------------------------------
// Fixture + expected-byte builders
//
// `block`/`expected_file` emit exactly what `save_registry` writes
// (`toml::to_string_pretty`): `[[downloads]]` tables in struct-field order,
// `None` optionals omitted, one blank line between tables, one trailing
// newline. Fixture files are written in this same normalized shape so each
// assertion's diff is exactly the mutated fields. The format itself is
// anchored by one fully-inline literal in
// `serialization_format_is_pinned_by_a_literal_anchor`.
// -------------------------------------------------------------------------

const MODEL: &str = "org/model";
const URL_COMPLETE: &str = "https://huggingface.co/org/model/resolve/main/complete.gguf";
const URL_INCOMPLETE: &str = "https://huggingface.co/org/model/resolve/main/incomplete.bin";
const URL_STAGING: &str =
    "https://huggingface.co/org/model/resolve/main/legacy-staging.safetensors";
const URL_QUANT: &str = "https://huggingface.co/org/model/resolve/main/quant.gguf";
const URL_RAW_QUANT: &str = "https://huggingface.co/org/model/raw/main/quant.gguf";

/// TOML basic-string escaping for path values (Windows separators).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
}

fn local(tmp: &Path, rel: &str) -> String {
    tmp.join(rel).to_string_lossy().to_string()
}

/// One `[[downloads]]` table block (with trailing newline), in the exact
/// field order `save_registry` emits.
#[allow(clippy::too_many_arguments)]
fn block(
    filename: &str,
    url: &str,
    local_path: &str,
    total_size: u64,
    downloaded_size: u64,
    status: &str,
    expected_sha256: Option<&str>,
    revision: Option<&str>,
) -> String {
    let mut s = format!(
        "[[downloads]]\nmodel_id = \"{MODEL}\"\nfilename = \"{filename}\"\nurl = \"{url}\"\nlocal_path = \"{}\"\ntotal_size = {total_size}\ndownloaded_size = {downloaded_size}\nstatus = \"{status}\"\n",
        esc(local_path)
    );
    if let Some(sha) = expected_sha256 {
        s.push_str(&format!("expected_sha256 = \"{sha}\"\n"));
    }
    if let Some(rev) = revision {
        s.push_str(&format!("revision = \"{rev}\"\n"));
    }
    s
}

/// Concatenate table blocks the way `toml::to_string_pretty` separates them
/// (single blank line between tables, single trailing newline).
fn expected_file(blocks: &[String]) -> String {
    blocks.join("\n")
}

/// Fixture entry 1: full-field `Complete` (hash + revision recorded).
fn b_complete(tmp: &Path) -> String {
    block(
        "complete.gguf",
        URL_COMPLETE,
        &local(tmp, "org/model/complete.gguf"),
        1000,
        1000,
        "Complete",
        Some("deadbeefcafe"),
        Some("v2.7-tag"),
    )
}

/// Fixture entry 2: legacy-minimal `Incomplete` (optional fields absent),
/// still carrying a stale partial `downloaded_size`.
fn b_incomplete(tmp: &Path) -> String {
    block(
        "incomplete.bin",
        URL_INCOMPLETE,
        &local(tmp, "org/model/incomplete.bin"),
        500,
        200,
        "Incomplete",
        None,
        None,
    )
}

/// Fixture entry 3: `HashMismatch` living in an hf-cache staging directory.
fn b_staging(tmp: &Path) -> String {
    block(
        "legacy-staging.safetensors",
        URL_STAGING,
        &local(tmp, ".rhd-staging/pub/legacy-staging.safetensors"),
        7,
        3,
        "HashMismatch",
        Some("aa11"),
        None,
    )
}

/// Fixture entry 4: `Incomplete` with a stale `downloaded_size`.
fn b_quant(tmp: &Path) -> String {
    block(
        "quant.gguf",
        URL_QUANT,
        &local(tmp, "org/model/quant.gguf"),
        42,
        42,
        "Incomplete",
        None,
        None,
    )
}

/// The standard four-entry fixture. Returns the file content so tests can
/// assert "unchanged" against it.
fn write_fixture(tmp: &Path) -> String {
    let content = expected_file(&[
        b_complete(tmp),
        b_incomplete(tmp),
        b_staging(tmp),
        b_quant(tmp),
    ]);
    std::fs::write(crate::paths::registry_path(), &content).expect("write fixture");
    content
}

/// The four fixture entries as an in-memory registry (for seeding mirrors
/// the way `engine::seed_registry_mirror` would).
fn fixture_registry(tmp: &Path) -> DownloadRegistry {
    toml::from_str(&write_fixture(tmp)).expect("fixture parses")
}

// -------------------------------------------------------------------------
// Golden tests
//
// (W2.4a pinned these against `*_seq` replicas of the inline sequences;
// W2.4b swapped the replicas for the typed ops in `registry` — the byte
// assertions are UNCHANGED, and that is the migration's
// behavior-preservation proof.)
// -------------------------------------------------------------------------

fn empty_complete_map() -> Arc<Mutex<CompleteDownloads>> {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Anchors the whole module: the fixture bytes are pinned by one fully
/// inline literal (no `block` helpers), the `block` helpers reproduce those
/// exact bytes, and a no-op load->save round trip is byte-stable (the
/// serialization is idempotent, so every later "unchanged entries" reuse of
/// the helpers is sound).
#[test]
fn serialization_format_is_pinned_by_a_literal_anchor() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("format-anchor");
    let _guard = DataDirGuard::install(&tmp);
    let content = write_fixture(&tmp);

    let inline = format!(
        "[[downloads]]\nmodel_id = \"{MODEL}\"\nfilename = \"complete.gguf\"\nurl = \"{URL_COMPLETE}\"\nlocal_path = \"{}\"\ntotal_size = 1000\ndownloaded_size = 1000\nstatus = \"Complete\"\nexpected_sha256 = \"deadbeefcafe\"\nrevision = \"v2.7-tag\"\n\n\
         [[downloads]]\nmodel_id = \"{MODEL}\"\nfilename = \"incomplete.bin\"\nurl = \"{URL_INCOMPLETE}\"\nlocal_path = \"{}\"\ntotal_size = 500\ndownloaded_size = 200\nstatus = \"Incomplete\"\n\n\
         [[downloads]]\nmodel_id = \"{MODEL}\"\nfilename = \"legacy-staging.safetensors\"\nurl = \"{URL_STAGING}\"\nlocal_path = \"{}\"\ntotal_size = 7\ndownloaded_size = 3\nstatus = \"HashMismatch\"\nexpected_sha256 = \"aa11\"\n\n\
         [[downloads]]\nmodel_id = \"{MODEL}\"\nfilename = \"quant.gguf\"\nurl = \"{URL_QUANT}\"\nlocal_path = \"{}\"\ntotal_size = 42\ndownloaded_size = 42\nstatus = \"Incomplete\"\n",
        esc(&local(&tmp, "org/model/complete.gguf")),
        esc(&local(&tmp, "org/model/incomplete.bin")),
        esc(&local(&tmp, ".rhd-staging/pub/legacy-staging.safetensors")),
        esc(&local(&tmp, "org/model/quant.gguf")),
    );
    assert_eq!(content, inline, "fixture bytes drifted from the pin");

    assert_eq!(
        content,
        expected_file(&[
            b_complete(&tmp),
            b_incomplete(&tmp),
            b_staging(&tmp),
            b_quant(&tmp),
        ]),
        "block() helpers drifted from the pinned bytes"
    );

    // Idempotent round trip: load -> save without mutation rewrites the
    // exact same bytes (this is what every op does to untouched entries).
    let registry = load_registry();
    save_registry(&registry);
    assert_eq!(read_registry_file(), content);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// download.rs "file already exists" site: ONLY the status flips — the site
/// does not touch downloaded_size (still 200), url, or any other field; the
/// complete-map mirror gains the mutated entry; an unknown url still
/// re-saves (byte-stable) and never touches the map.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn golden_mark_complete_already_exists_pins_bytes_and_complete_map() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("mark-complete");
    let _guard = DataDirGuard::install(&tmp);
    write_fixture(&tmp);

    let complete = empty_complete_map();
    mark_complete(&complete, URL_INCOMPLETE, "incomplete.bin").await;

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            b_complete(&tmp),
            block(
                "incomplete.bin",
                URL_INCOMPLETE,
                &local(&tmp, "org/model/incomplete.bin"),
                500,
                200, // untouched by this site
                "Complete",
                None,
                None,
            ),
            b_staging(&tmp),
            b_quant(&tmp),
        ])
    );

    // Complete-map mirror: gained the mutated entry.
    let map = complete.lock().await;
    let entry = map.get("incomplete.bin").expect("entry inserted into map");
    assert_eq!(entry.status, DownloadStatus::Complete);
    assert_eq!(entry.downloaded_size, 200);
    assert_eq!(entry.url, URL_INCOMPLETE);
    drop(map);

    // No matching url: the site still saves (load-modify-save round trip,
    // byte-stable) and never touches the map.
    let before = read_registry_file();
    mark_complete(
        &complete,
        "https://huggingface.co/no/such/resolve/main/x.bin",
        "x.bin",
    )
    .await;
    assert_eq!(read_registry_file(), before);
    assert_eq!(complete.lock().await.len(), 1);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// download.rs success site: Complete + downloaded_size = final size + url
/// rewritten to the successful (raw-fallback) endpoint; the complete-map
/// mirror holds the rewritten entry.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn golden_mark_complete_with_url_rewrite_pins_bytes_and_complete_map() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("mark-complete-url");
    let _guard = DataDirGuard::install(&tmp);
    write_fixture(&tmp);

    let complete = empty_complete_map();
    mark_complete_with_url(&complete, URL_QUANT, URL_RAW_QUANT, 999_999, "quant.gguf").await;

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            b_complete(&tmp),
            b_incomplete(&tmp),
            b_staging(&tmp),
            block(
                "quant.gguf",
                URL_RAW_QUANT, // rewritten
                &local(&tmp, "org/model/quant.gguf"),
                42,
                999_999,
                "Complete",
                None,
                None,
            ),
        ])
    );

    let map = complete.lock().await;
    let entry = map.get("quant.gguf").expect("entry inserted into map");
    assert_eq!(entry.status, DownloadStatus::Complete);
    assert_eq!(entry.downloaded_size, 999_999);
    assert_eq!(entry.url, URL_RAW_QUANT);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The two download.rs failure sites (401 and final failure) run the
/// identical sequence: find by url -> Incomplete + downloaded_size = 0; no
/// mirror update of any kind.
#[test]
fn golden_mark_failed_pins_bytes() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("mark-failed");
    let _guard = DataDirGuard::install(&tmp);
    write_fixture(&tmp);

    mark_failed(URL_COMPLETE);

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            block(
                "complete.gguf",
                URL_COMPLETE,
                &local(&tmp, "org/model/complete.gguf"),
                1000,
                0,
                "Incomplete",
                Some("deadbeefcafe"),
                Some("v2.7-tag"),
            ),
            b_incomplete(&tmp),
            b_staging(&tmp),
            b_quant(&tmp),
        ])
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// download.rs `download_chunked` metadata-upsert site, both branches:
/// update (ONLY total_size and downloaded_size change; downloaded_size is
/// hardcoded to 0 regardless of the passed value) and append (fresh entry
/// lands at the END of the table with every field the site constructs).
#[test]
fn golden_upsert_metadata_updates_and_appends_pins_bytes() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("upsert-metadata");
    let _guard = DataDirGuard::install(&tmp);
    write_fixture(&tmp);

    // Update branch.
    let refresh = DownloadMetadata {
        model_id: MODEL.to_string(),
        filename: "incomplete.bin".to_string(),
        url: URL_INCOMPLETE.to_string(),
        local_path: "/ignored/on/update".to_string(),
        total_size: 777,
        downloaded_size: 555, // ignored: the site hardcodes 0
        status: DownloadStatus::Incomplete,
        expected_sha256: None,
        revision: None,
    };
    upsert_metadata(refresh);

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            b_complete(&tmp),
            block(
                "incomplete.bin",
                URL_INCOMPLETE,
                &local(&tmp, "org/model/incomplete.bin"),
                777,
                0,
                "Incomplete",
                None,
                None,
            ),
            b_staging(&tmp),
            b_quant(&tmp),
        ])
    );

    // Append branch (unknown url -> entry at the end).
    let after_update = read_registry_file();
    let fresh = DownloadMetadata {
        model_id: MODEL.to_string(),
        filename: "brand-new.gguf".to_string(),
        url: "https://huggingface.co/org/model/resolve/nightly/brand-new.gguf".to_string(),
        local_path: local(&tmp, "org/model/brand-new.gguf"),
        total_size: 64000,
        downloaded_size: 0,
        status: DownloadStatus::Incomplete,
        expected_sha256: Some("ff00".to_string()),
        revision: Some("nightly".to_string()),
    };
    upsert_metadata(fresh);

    assert_eq!(
        read_registry_file(),
        format!(
            "{after_update}\
             \n{}",
            block(
                "brand-new.gguf",
                "https://huggingface.co/org/model/resolve/nightly/brand-new.gguf",
                &local(&tmp, "org/model/brand-new.gguf"),
                64000,
                0,
                "Incomplete",
                Some("ff00"),
                Some("nightly"),
            )
        )
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// verification.rs mismatch site: disk entry flips to HashMismatch; the
/// engine's registry mirror is patched after the save — independently of
/// the disk (an empty mirror stays empty; a mirror lacking the entry is
/// untouched while the disk still updates: disk is the source of truth).
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn golden_mark_mismatch_pins_bytes_and_registry_mirror() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("mark-mismatch");
    let _guard = DataDirGuard::install(&tmp);

    // Phase 1: seeded mirror (the TUI case) — both disk and mirror update.
    let mirror = Arc::new(Mutex::new(fixture_registry(&tmp)));
    mark_mismatch(&mirror, Path::new(&local(&tmp, "org/model/complete.gguf"))).await;

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            block(
                "complete.gguf",
                URL_COMPLETE,
                &local(&tmp, "org/model/complete.gguf"),
                1000,
                1000,
                "HashMismatch",
                Some("deadbeefcafe"),
                Some("v2.7-tag"),
            ),
            b_incomplete(&tmp),
            b_staging(&tmp),
            b_quant(&tmp),
        ])
    );
    {
        let m = mirror.lock().await;
        assert_eq!(m.downloads[0].status, DownloadStatus::HashMismatch);
        assert_eq!(m.downloads[1].status, DownloadStatus::Incomplete);
    }

    // Phase 2: EMPTY mirror (the CLI-before-bootstrap case) — the disk still
    // updates (source of truth), the mirror has nothing to patch.
    let empty_mirror = Arc::new(Mutex::new(DownloadRegistry::default()));
    mark_mismatch(
        &empty_mirror,
        Path::new(&local(&tmp, "org/model/incomplete.bin")),
    )
    .await;

    assert_eq!(
        read_registry_file(),
        expected_file(&[
            block(
                "complete.gguf",
                URL_COMPLETE,
                &local(&tmp, "org/model/complete.gguf"),
                1000,
                1000,
                "HashMismatch",
                Some("deadbeefcafe"),
                Some("v2.7-tag"),
            ),
            block(
                "incomplete.bin",
                URL_INCOMPLETE,
                &local(&tmp, "org/model/incomplete.bin"),
                500,
                200,
                "HashMismatch",
                None,
                None,
            ),
            b_staging(&tmp),
            b_quant(&tmp),
        ])
    );
    assert!(
        empty_mirror.lock().await.downloads.is_empty(),
        "mirror untouched when it has no matching entry"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// engine.rs `register_pending` (driven through the real fn): appends
/// entries for urls not yet present, skips urls already recorded, records
/// the revision only for non-default revisions, and saves exactly once.
#[test]
fn golden_register_pending_appends_only_missing_urls_and_pins_bytes() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("register-pending");
    let _guard = DataDirGuard::install(&tmp);
    let fixture = write_fixture(&tmp);

    let url_new = crate::api::resolve_url(MODEL, "brand-new.gguf", crate::api::DEFAULT_REVISION);
    crate::engine::register_pending(
        MODEL,
        crate::api::DEFAULT_REVISION,
        &[
            // url already recorded (fixture entry 2) -> skipped, even though
            // the sizes/hashes differ; the site never updates existing urls.
            ("incomplete.bin".to_string(), 500, None),
            // fresh url -> appended at the end; DEFAULT_REVISION -> no
            // revision field.
            (
                "brand-new.gguf".to_string(),
                64000,
                Some("ff00".to_string()),
            ),
        ],
        &tmp.to_string_lossy(),
    )
    .expect("register_pending succeeds");

    assert_eq!(
        read_registry_file(),
        format!(
            "{fixture}\
             \n{}",
            block(
                "brand-new.gguf",
                &url_new,
                &local(&tmp, "org/model/brand-new.gguf"),
                64000,
                0,
                "Incomplete",
                Some("ff00"),
                None,
            )
        )
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// `register_pending` aborts on the FIRST invalid filename and saves
/// nothing: valid files earlier in the list must not leak to disk.
#[test]
fn register_pending_aborts_on_first_invalid_file_without_saving() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("register-pending-abort");
    let _guard = DataDirGuard::install(&tmp);
    let fixture = write_fixture(&tmp);

    let err = crate::engine::register_pending(
        MODEL,
        crate::api::DEFAULT_REVISION,
        &[
            ("brand-new.gguf".to_string(), 1, None), // valid — must NOT be saved
            ("../evil".to_string(), 1, None),        // invalid — aborts here
            ("CON".to_string(), 1, None),            // also invalid — never reached
        ],
        &tmp.to_string_lossy(),
    )
    .expect_err("invalid filename must abort");

    assert!(matches!(
        &err,
        crate::paths::sanitize::PathError::InvalidFilenameComponent(p) if p == ".."
    ));
    assert_eq!(
        read_registry_file(),
        fixture,
        "no partial save: the registry file is byte-identical to the fixture"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// -------------------------------------------------------------------------
// Save-failure pins (redirectable via ENV_DATA_DIR)
//
// The data dir is pointed at a regular FILE, so the registry path
// `<file>/hf-downloads.toml` fails with ENOTDIR on every read AND write —
// deterministic on every platform and for every user (unlike chmod, which
// root ignores). This is the strongest redirectable save failure.
// -------------------------------------------------------------------------

/// verification.rs mismatch site under save failure: no panic, the error
/// stays silent, and the mirror is STILL patched — today's behavior of
/// updating the mirror regardless of the (swallowed) save outcome.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn save_failure_mismatch_mirror_still_patched_and_error_silent() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let parent = tmp("save-fail-mismatch");
    let not_a_dir = parent.join("registry-sits-under-a-file");
    std::fs::write(&not_a_dir, "x").expect("create blocking file");
    let _guard = DataDirGuard::install(&not_a_dir);

    // Mirror seeded with one entry whose local_path raw-equals the (dead)
    // registry path — the mismatch site would target exactly it.
    let dead_path = not_a_dir
        .join("hf-downloads.toml")
        .to_string_lossy()
        .to_string();
    let mirror = Arc::new(Mutex::new(DownloadRegistry {
        downloads: vec![DownloadMetadata {
            model_id: MODEL.to_string(),
            filename: "f.bin".to_string(),
            url: "https://huggingface.co/org/model/resolve/main/f.bin".to_string(),
            local_path: dead_path.clone(),
            total_size: 1,
            downloaded_size: 0,
            status: DownloadStatus::Incomplete,
            expected_sha256: None,
            revision: None,
        }],
    }));

    // Must not panic; the io error is silently swallowed by save_registry.
    mark_mismatch(&mirror, Path::new(&dead_path)).await;

    // The save failed: no registry file can exist at the dead path.
    assert!(!crate::paths::registry_path().exists());
    // The mirror was patched regardless of the failed save.
    let m = mirror.lock().await;
    assert_eq!(m.downloads[0].status, DownloadStatus::HashMismatch);

    let _ = std::fs::remove_dir_all(&parent);
}

/// download.rs mark-complete site under save failure: no panic, and the
/// complete-map insert is CONDITIONAL on finding the entry on disk — an
/// unreadable registry yields an empty default, so the map stays empty.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn save_failure_complete_map_insert_requires_disk_entry() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let parent = tmp("save-fail-complete");
    let not_a_dir = parent.join("registry-sits-under-a-file");
    std::fs::write(&not_a_dir, "x").expect("create blocking file");
    let _guard = DataDirGuard::install(&not_a_dir);

    let complete = empty_complete_map();
    mark_complete(&complete, URL_COMPLETE, "complete.gguf").await;

    assert!(
        !crate::paths::registry_path().exists(),
        "save silently failed"
    );
    assert!(
        complete.lock().await.is_empty(),
        "complete map untouched: the disk entry could not be loaded"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

// -------------------------------------------------------------------------
// Two-writer interleaving (plan H7)
//
// Both writers replicate the verification-site sequence against the same
// registry file, with a rendezvous BETWEEN load and save that forces the
// un-serialized load-modify-save race deterministically: both writers load
// the pre-state, then overwrite each other's save. The typed op exposes
// no such rendezvous seam, so this test pins the SHAPE the ops must keep
// (no cross-op lock); `two_writers_through_the_real_ops_stay_safe` below
// drives the real `mark_mismatch` op concurrently. The pins:
//
// - both ops complete (no panic, no deadlock),
// - the final file parses,
// - the last completed write wins on disk (exactly one entry updated —
//   the lost-update race of plan §8, pinned, not fixed),
// - the mirror reflects every writer's patch (it is updated per write,
//   regardless of the disk outcome — so after a race the mirror can hold
//   MORE than the disk: today's divergence, pinned).
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn two_writers_complete_ops_final_file_parses_mirror_matches_last_write() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("two-writers");
    let _guard = DataDirGuard::install(&tmp);

    // Two plain entries, both Incomplete.
    let content = expected_file(&[
        block(
            "a.bin",
            "https://huggingface.co/org/model/resolve/main/a.bin",
            &local(&tmp, "a.bin"),
            10,
            10,
            "Incomplete",
            None,
            None,
        ),
        block(
            "b.bin",
            "https://huggingface.co/org/model/resolve/main/b.bin",
            &local(&tmp, "b.bin"),
            20,
            20,
            "Incomplete",
            None,
            None,
        ),
    ]);
    std::fs::write(crate::paths::registry_path(), &content).expect("write fixture");
    let mirror = Arc::new(Mutex::new(load_registry()));
    let barrier = Arc::new(std::sync::Barrier::new(2));

    let writer = |mirror: Arc<Mutex<DownloadRegistry>>,
                  barrier: Arc<std::sync::Barrier>,
                  local_path: String| async move {
        // Exact load-modify-save of the mismatch site, with the rendezvous
        // placed between load and save.
        let mut registry = load_registry();
        barrier.wait();
        if let Some(entry) = registry
            .downloads
            .iter_mut()
            .find(|d| d.local_path == local_path)
        {
            entry.status = DownloadStatus::HashMismatch;
        }
        save_registry(&registry);

        let mut m = mirror.lock().await;
        if let Some(entry) = m.downloads.iter_mut().find(|d| d.local_path == local_path) {
            entry.status = DownloadStatus::HashMismatch;
        }
    };

    let a = tokio::spawn(writer(
        mirror.clone(),
        barrier.clone(),
        local(&tmp, "a.bin"),
    ));
    let b = tokio::spawn(writer(
        mirror.clone(),
        barrier.clone(),
        local(&tmp, "b.bin"),
    ));
    a.await.expect("writer A completed without panic");
    b.await.expect("writer B completed without panic");

    // Final file parses.
    let disk: DownloadRegistry =
        toml::from_str(&read_registry_file()).expect("final registry parses");

    // Lost-update race pinned: both writers loaded the two-Incomplete
    // pre-state, so the LAST completed write's view is on disk — exactly
    // one of the two entries survived as HashMismatch.
    let mismatches = disk
        .downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::HashMismatch)
        .count();
    assert_eq!(
        mismatches, 1,
        "last completed write must win (lost-update race of plan §8, pinned)"
    );

    // Mirror reflects BOTH writers' patches — each writer updated it after
    // its own save, regardless of the disk outcome.
    let m = mirror.lock().await;
    assert_eq!(m.downloads[0].status, DownloadStatus::HashMismatch);
    assert_eq!(m.downloads[1].status, DownloadStatus::HashMismatch);

    let _ = std::fs::remove_dir_all(&tmp);
}

// -------------------------------------------------------------------------
// path_matches (moved from verification.rs with the mark_mismatch op)
// -------------------------------------------------------------------------

fn small_temp_file(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rhd-registry-pathmatch-{name}-{}",
        std::process::id()
    ));
    std::fs::write(&path, [1u8; 4]).expect("create temp file");
    path
}

#[test]
fn path_matches_accepts_raw_string_equality() {
    let f = small_temp_file("path-eq");
    let s = f.to_string_lossy().to_string();
    assert!(path_matches(&s, &f));
    let _ = std::fs::remove_file(&f);
}

/// Registry entries record the user-facing path while download internals
/// canonicalize; on macOS the temp dir lives behind the /var ->
/// /private/var symlink, on Windows canonicalize adds a \\?\ prefix. A
/// symlinked alias reproduces the divergence on any Unix: the raw strings
/// differ but both resolve to the same file.
#[cfg(unix)]
#[test]
fn path_matches_resolves_symlinked_aliases() {
    let f = small_temp_file("path-symlink");
    let dir = std::env::temp_dir().join(format!("rhd-registry-alias-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let alias_dir = dir.join("alias");
    std::os::unix::fs::symlink(f.parent().unwrap(), &alias_dir).unwrap();
    let alias_path = alias_dir.join(f.file_name().unwrap());
    let recorded = alias_path.to_string_lossy().to_string();
    assert_ne!(recorded, f.to_string_lossy().to_string());
    assert!(path_matches(&recorded, &f));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&f);
}

// -------------------------------------------------------------------------
// Two-writer interleaving through the REAL ops (plan H7)
//
// Unlike the barrier-forced test above (which drives the load-modify-save
// shape directly, with a rendezvous the typed op cannot expose), this test
// runs the actual `mark_mismatch` op concurrently from two tasks. The
// un-serialized race means either writer's save may be lost — the pins are
// the safety properties, not a specific interleaving: both ops complete,
// the final file parses, the last completed write is on disk (at least one
// entry updated), and the mirror reflects every writer's patch.
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::await_holding_lock)] // serializes env-mutating tests
async fn two_writers_through_the_real_ops_stay_safe() {
    let _env = crate::paths::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tmp("two-writers-ops");
    let _guard = DataDirGuard::install(&tmp);

    std::fs::write(
        crate::paths::registry_path(),
        expected_file(&[
            block(
                "a.bin",
                "https://huggingface.co/org/model/resolve/main/a.bin",
                &local(&tmp, "a.bin"),
                10,
                10,
                "Incomplete",
                None,
                None,
            ),
            block(
                "b.bin",
                "https://huggingface.co/org/model/resolve/main/b.bin",
                &local(&tmp, "b.bin"),
                20,
                20,
                "Incomplete",
                None,
                None,
            ),
        ]),
    )
    .expect("write fixture");

    let mirror = Arc::new(Mutex::new(load_registry()));
    let a = tokio::spawn({
        let mirror = mirror.clone();
        let path = PathBuf::from(local(&tmp, "a.bin"));
        async move { mark_mismatch(&mirror, &path).await }
    });
    let b = tokio::spawn({
        let mirror = mirror.clone();
        let path = PathBuf::from(local(&tmp, "b.bin"));
        async move { mark_mismatch(&mirror, &path).await }
    });
    a.await.expect("op A completed without panic");
    b.await.expect("op B completed without panic");

    // Final file parses.
    let disk: DownloadRegistry =
        toml::from_str(&read_registry_file()).expect("final registry parses");

    // The last completed write is on disk: whichever op saved last always
    // includes its own entry's patch, so at least one entry is marked (the
    // other may or may not be — that is the pinned race).
    let mismatches = disk
        .downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::HashMismatch)
        .count();
    assert!(
        (1..=2).contains(&mismatches),
        "expected the last write (1) or both writes (2) to survive, got {mismatches}"
    );

    // The mirror reflects every writer's patch.
    let m = mirror.lock().await;
    assert_eq!(m.downloads[0].status, DownloadStatus::HashMismatch);
    assert_eq!(m.downloads[1].status, DownloadStatus::HashMismatch);

    let _ = std::fs::remove_dir_all(&tmp);
}
