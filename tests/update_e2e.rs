//! End-to-end tests for the `update` subcommand.
//!
//! Serves a fake release directory (latest.json + platform asset) over a
//! local hyper server, then runs a **temp copy** of the real binary with
//! `RHD_UPDATE_BASE` pointed at it — the copy swaps *itself*, so the
//! cargo-built binary is never modified.
//!
//! Cross-platform trick for the swap tests: the served asset contains the
//! *same* real binary, and `--force` drives the full download → verify →
//! extract → swap path even though the version does not change. The
//! version-compare path is covered separately with a 99.0.0 manifest and
//! `--check` (no swap needed).

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Mock release server
// ---------------------------------------------------------------------------

struct ReleaseFixture {
    /// `version` field of latest.json.
    version: String,
    /// (asset name, bytes, sha256-to-advertise). `None` sha256 = real hash.
    assets: Vec<(String, Vec<u8>, Option<String>)>,
    /// Extra override of the manifest's assets map (e.g. omit the platform).
    manifest_override: Option<Value>,
}

async fn spawn_release_server(fixture: ReleaseFixture) -> String {
    let files: HashMap<String, (Vec<u8>, String)> = fixture
        .assets
        .iter()
        .map(|(name, bytes, advertised)| {
            let real = hex::encode(Sha256::digest(bytes));
            let sha = advertised.clone().unwrap_or(real);
            (name.clone(), (bytes.clone(), sha))
        })
        .collect();

    let triple = crate_target_triple();
    let mut assets = serde_json::Map::new();
    for (name, (_, sha)) in &files {
        let format = if name.ends_with(".zip") {
            "zip"
        } else {
            "tar-gz"
        };
        let asset_triple = name
            .strip_prefix("rust-hf-downloader-")
            .and_then(|n| n.strip_suffix(".tar.gz").or_else(|| n.strip_suffix(".zip")))
            .unwrap_or("unknown")
            .to_string();
        assets.insert(
            asset_triple,
            json!({ "name": name, "format": format, "sha256": sha }),
        );
    }
    if let Some(Value::Object(ov)) = fixture.manifest_override {
        for (k, v) in ov {
            assets.insert(k, v);
        }
    }
    let manifest = json!({
        "version": fixture.version,
        "released_at": "2026-01-01T00:00:00Z",
        "notes_url": format!("https://example.com/notes/{}", fixture.version),
        "assets": Value::Object(assets),
    })
    .to_string();

    let files = Arc::new(files);
    let manifest = Arc::new(manifest);
    let _ = triple;

    let make = make_service_fn(move |_| {
        let files = files.clone();
        let manifest = manifest.clone();
        async move {
            Ok::<_, std::convert::Infallible>(service_fn(move |req: Request<Body>| {
                let files = files.clone();
                let manifest = manifest.clone();
                async move {
                    let path = req.uri().path().trim_start_matches('/');
                    if path == "latest.json" {
                        return Ok::<_, hyper::Error>(Response::new(Body::from(
                            (*manifest).clone(),
                        )));
                    }
                    match files.get(path) {
                        Some((bytes, _)) => Ok(Response::new(Body::from(bytes.clone()))),
                        None => Ok(Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::from("not found"))
                            .unwrap()),
                    }
                }
            }))
        }
    });

    let srv = Server::bind(&([127, 0, 0, 1], 0).into()).serve(make);
    let addr = srv.local_addr();
    // Lives for the process; tests finish long before it matters.
    tokio::spawn(srv.with_graceful_shutdown(std::future::pending()));
    format!("http://{addr}")
}

fn crate_target_triple() -> String {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu".into()
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu".into()
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin".into()
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin".into()
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc".into()
    } else {
        panic!("tests must run on a published target triple")
    }
}

fn bin_name() -> &'static str {
    if cfg!(windows) {
        "rust-hf-downloader.exe"
    } else {
        "rust-hf-downloader"
    }
}

/// Packages the real binary into the platform archive format.
fn package_current_binary() -> Vec<u8> {
    let bin = std::fs::read(env!("CARGO_BIN_EXE_rust-hf-downloader")).expect("read test binary");

    #[cfg(unix)]
    {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(bin.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, bin_name(), bin.as_slice())
            .expect("tar append");
        let tar_bytes = tar.into_inner().expect("tar finish");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &tar_bytes).expect("gzip");
        enc.finish().expect("gzip finish")
    }

    #[cfg(windows)]
    {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file(bin_name(), options).expect("zip start");
            std::io::Write::write_all(&mut zip, &bin).expect("zip write");
            zip.finish().expect("zip finish");
        }
        cursor.into_inner()
    }
}

struct UpdateEnv {
    home: PathBuf,
}

impl UpdateEnv {
    fn new(tag: &str) -> Self {
        let home =
            std::env::temp_dir().join(format!("rhd-update-e2e-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create temp home");
        UpdateEnv { home }
    }

    /// Copies the real binary here; the copy is what gets self-replaced.
    fn binary_copy(&self) -> PathBuf {
        let dest = self.home.join(bin_name());
        std::fs::copy(env!("CARGO_BIN_EXE_rust-hf-downloader"), &dest).expect("copy binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dest
    }

    async fn run(&self, base: &str, args: &[&str]) -> (i32, String, String) {
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(self.home.join(bin_name()))
                .args(args)
                .env("RHD_UPDATE_BASE", base)
                .current_dir(&self.home)
                .output(),
        )
        .await
        .expect("child timed out")
        .expect("failed to spawn binary");

        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }
}

impl Drop for UpdateEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Up-to-date: manifest version older than the running binary → exit 0.
#[tokio::test]
async fn update_reports_up_to_date_for_older_manifest() {
    let base = spawn_release_server(ReleaseFixture {
        version: "0.0.1".into(),
        assets: vec![(platform_asset_name(), package_current_binary(), None)],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("up-to-date");
    let _ = env.binary_copy();
    let (code, _out, err) = env.run(&base, &["update", "--check"]).await;
    assert_eq!(code, 0, "stderr: {err}");
    assert!(err.contains("up to date"), "stderr: {err}");
}

/// Newer manifest + `--check` → exit 70, nothing installed.
#[tokio::test]
async fn update_check_exits_70_when_newer_available() {
    let base = spawn_release_server(ReleaseFixture {
        version: "99.0.0".into(),
        assets: vec![(platform_asset_name(), package_current_binary(), None)],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("check-newer");
    let bin = env.binary_copy();
    let before = std::fs::read(&bin).unwrap();

    let (code, _out, err) = env.run(&base, &["update", "--check"]).await;
    assert_eq!(code, 70, "stderr: {err}");
    assert!(err.contains("99.0.0"), "stderr: {err}");

    let after = std::fs::read(&bin).unwrap();
    assert_eq!(before, after, "--check must not touch the binary");
}

/// Full path with `--force`: same-version asset is downloaded, verified,
/// extracted, and swapped in; the copy still runs afterwards.
#[tokio::test]
async fn update_force_swaps_binary_in_place() {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let base = spawn_release_server(ReleaseFixture {
        version: current.clone(),
        assets: vec![(platform_asset_name(), package_current_binary(), None)],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("force-swap");
    let bin = env.binary_copy();

    let (code, _out, err) = env.run(&base, &["update", "--force"]).await;
    assert_eq!(code, 0, "stderr: {err}");
    assert!(err.contains("checksum ok"), "stderr: {err}");

    // The swapped copy still executes and reports the same version.
    let ver = tokio::process::Command::new(&bin)
        .arg("--version")
        .output()
        .await
        .expect("run swapped copy");
    assert!(ver.status.success());
    assert!(String::from_utf8_lossy(&ver.stdout).contains(&current));
}

/// Tampered checksum → exit 71, binary untouched.
#[tokio::test]
async fn update_rejects_checksum_mismatch() {
    let base = spawn_release_server(ReleaseFixture {
        version: "99.0.0".into(),
        assets: vec![(
            platform_asset_name(),
            package_current_binary(),
            Some("deadbeef".repeat(32)),
        )],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("bad-sum");
    let bin = env.binary_copy();
    let before = std::fs::read(&bin).unwrap();

    let (code, _out, err) = env.run(&base, &["update"]).await;
    assert_eq!(code, 71, "stderr: {err}");
    assert!(err.to_lowercase().contains("checksum"), "stderr: {err}");

    let after = std::fs::read(&bin).unwrap();
    assert_eq!(before, after, "mismatch must not touch the binary");
}

/// JSON mode: checking → available event sequence, stable schema.
#[tokio::test]
async fn update_json_events_are_wellformed() {
    let base = spawn_release_server(ReleaseFixture {
        version: "99.0.0".into(),
        assets: vec![(platform_asset_name(), package_current_binary(), None)],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("json");
    let _ = env.binary_copy();
    let (code, out, _err) = env.run(&base, &["update", "--check", "--json"]).await;
    assert_eq!(code, 70);

    let events: Vec<Value> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect();
    let types: Vec<&str> = events
        .iter()
        .map(|e| e["type"].as_str().expect("type tag"))
        .collect();
    assert_eq!(types, vec!["checking", "available"], "events: {out}");
    assert_eq!(events[1]["latest"], "99.0.0");
    assert!(events[1]["notes_url"].as_str().unwrap().contains("99.0.0"));
}

/// Manifest without an asset for this platform → clear failure, exit 1.
#[tokio::test]
async fn update_fails_cleanly_without_platform_asset() {
    let base = spawn_release_server(ReleaseFixture {
        version: "99.0.0".into(),
        assets: vec![],
        manifest_override: None,
    })
    .await;
    let env = UpdateEnv::new("no-asset");
    let _ = env.binary_copy();
    let (code, _out, err) = env.run(&base, &["update"]).await;
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("no asset"), "stderr: {err}");
}

fn platform_asset_name() -> String {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("rust-hf-downloader-{}.{ext}", crate_target_triple())
}

use std::sync::Arc;
