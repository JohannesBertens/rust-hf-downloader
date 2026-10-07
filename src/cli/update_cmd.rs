//! `update` subcommand (self-update; see plans/self-update.md).

use super::args::UpdateArgs;
use super::{EXIT_CHECKSUM, EXIT_FAILURE, EXIT_OK, EXIT_UPDATE_AVAILABLE};
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// NDJSON event stream for `update --json` (mirrors `Event`'s style).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UpdateEvent {
    Checking,
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes_url: Option<String>,
    },
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
        percent: f64,
    },
    Verified {
        sha256: String,
    },
    Updated {
        from: String,
        to: String,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Minimum interval between JSON `downloading` events.
const UPDATE_JSON_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

fn update_error_code(err: &crate::update::UpdateError) -> &'static str {
    use crate::update::UpdateError;
    match err {
        UpdateError::Network(_) => "network",
        UpdateError::UnsupportedPlatform => "unsupported_platform",
        UpdateError::NoAssetForPlatform { .. } => "no_asset_for_platform",
        UpdateError::MalformedManifest(_) => "manifest",
        UpdateError::Checksum { .. } => "checksum",
        UpdateError::Archive(_) => "archive",
        UpdateError::Swap(_) => "swap",
        UpdateError::Io(_) => "io",
    }
}

fn update_error_exit(err: &crate::update::UpdateError) -> i32 {
    match err {
        crate::update::UpdateError::Checksum { .. } => EXIT_CHECKSUM,
        _ => EXIT_FAILURE,
    }
}

pub async fn run_update(args: UpdateArgs) -> i32 {
    use crate::update;

    let human = !args.json && std::io::stderr().is_terminal();
    let mut last_emit = Instant::now();
    let emit = |event: UpdateEvent| {
        if args.json {
            // Same flush discipline as the download reporter.
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", serde_json::to_string(&event).unwrap());
            let _ = out.flush();
        }
    };

    if !args.json {
        eprintln!("Checking for updates…");
    }
    emit(UpdateEvent::Checking);

    let client = reqwest::Client::builder().build();
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            let err = update::UpdateError::Network(format!("building HTTP client: {e}"));
            return update_fail(&err, &emit);
        }
    };
    let base = update::base_url();

    let (manifest, latest) = match update::fetch_manifest(&client, &base).await {
        Ok(v) => v,
        Err(e) => return update_fail(&e, &emit),
    };
    let current = update::VersionTriple::current();
    let triple = match update::target_triple() {
        Some(t) => t,
        None => return update_fail(&update::UpdateError::UnsupportedPlatform, &emit),
    };
    let asset = match update::manifest_asset(&manifest, triple) {
        Ok(a) => a.clone(),
        Err(e) => return update_fail(&e, &emit),
    };

    if latest <= current && !args.force {
        if args.json {
            emit(UpdateEvent::UpToDate {
                current: current.to_string(),
            });
        } else {
            eprintln!("rust-hf-downloader is up to date (v{current})");
        }
        return EXIT_OK;
    }

    if args.json {
        emit(UpdateEvent::Available {
            current: current.to_string(),
            latest: latest.to_string(),
            notes_url: manifest.notes_url.clone(),
        });
    } else {
        eprintln!("Update available: v{current} → v{latest}");
    }
    if args.check {
        if !args.json {
            eprintln!("(check only; nothing was installed — run without --check)");
        }
        return EXIT_UPDATE_AVAILABLE;
    }

    // Download + verify. Progress: humans get the Reporter-shaped line
    // (bar/percent/sizes from `cli::report`, M4/C5) with the same
    // `\r`…`\x1b[K` rewrite discipline the download reporter uses; JSON
    // gets throttled `downloading` events.
    let mut progress = |downloaded: u64, total: u64| {
        if args.json {
            if last_emit.elapsed() >= UPDATE_JSON_PROGRESS_INTERVAL || downloaded == total {
                last_emit = Instant::now();
                emit(UpdateEvent::Downloading {
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    percent: if total > 0 {
                        downloaded as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    },
                });
            }
        } else if human {
            eprint!(
                "\r{}\x1b[K",
                crate::cli::report::format_update_progress(downloaded, total)
            );
            let _ = std::io::stderr().flush();
        }
    };
    let archive_path = match update::download_asset(&client, &base, &asset, &mut progress).await {
        Ok(p) => p,
        Err(e) => return update_fail(&e, &emit),
    };
    if !args.json {
        // Same erase-to-end-of-line clear the download reporter uses
        // (`clear_progress_line`'s shape, M4/C5).
        eprint!("\r\x1b[2K");
    }
    emit(UpdateEvent::Verified {
        sha256: asset.sha256.clone(),
    });
    if !args.json {
        eprintln!("  checksum ok");
    }

    let binary_path = match update::extract_binary(&archive_path, asset.format) {
        Ok(p) => p,
        Err(e) => return update_fail(&e, &emit),
    };
    // `swap` takes only the staged binary: `self_replace` resolves the
    // running executable itself (the former `current_exe` argument was
    // ignored — dropped in M4/C5 as the smaller change).
    if let Err(e) = update::swap(&binary_path) {
        return update_fail(&e, &emit);
    }
    // Best-effort temp cleanup; self_replace consumed the staged binary.
    if let Some(parent) = archive_path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }

    if args.json {
        emit(UpdateEvent::Updated {
            from: current.to_string(),
            to: latest.to_string(),
        });
    } else {
        eprintln!("Updated rust-hf-downloader v{current} → v{latest}");
        eprintln!("Restart any running instance to pick up the new version.");
    }
    EXIT_OK
}

fn update_fail(err: &crate::update::UpdateError, emit: &dyn Fn(UpdateEvent)) -> i32 {
    eprintln!("update failed: {err}");
    emit(UpdateEvent::Error {
        code: update_error_code(err).to_string(),
        message: err.to_string(),
    });
    update_error_exit(err)
}
