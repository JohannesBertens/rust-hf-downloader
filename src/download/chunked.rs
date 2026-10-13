//! The chunked transport (W3.8): [`download_chunked`] probes the file
//! size, preallocates the `.incomplete` file, spawns one task per chunk
//! under a semaphore, and renames on success. The per-chunk worker
//! ([`download_chunk_with_progress`]) streams its byte range with rate
//! limiting and progress/speed pacing. Extracted verbatim from the old
//! monolithic `download.rs`; every `crate::download::X` path keeps
//! compiling through the facade re-exports.

use super::journal::{self, JournalWriter, ResumeJournal};
use super::{DOWNLOAD_CONFIG, RATE_LIMITER};
use crate::models::{
    ChunkProgress, CompleteDownloads, DownloadMetadata, DownloadProgress, DownloadStatus,
    VerificationQueueItem,
};
use crate::registry;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinSet;

/// Parameters for chunked download
pub(super) struct ChunkedDownloadParams<'a> {
    pub(super) url: &'a str,
    /// Git revision the URL points at; stored on registry entries (issue #28).
    pub(super) revision: &'a str,
    pub(super) incomplete_path: &'a PathBuf,
    pub(super) final_path: &'a PathBuf,
    pub(super) progress: &'a Arc<Mutex<Option<DownloadProgress>>>,
    pub(super) status_tx: &'a mpsc::UnboundedSender<String>,
    pub(super) complete_downloads: &'a Arc<Mutex<CompleteDownloads>>,
    pub(super) filename: &'a str,
    pub(super) expected_sha256: &'a Option<String>,
    pub(super) hf_token: &'a Option<String>,
}

fn calculate_chunk_size(file_size: u64) -> usize {
    let target_chunks = DOWNLOAD_CONFIG.target_chunks.load(Ordering::Relaxed) as u64;
    let min_size = DOWNLOAD_CONFIG.min_chunk_size.load(Ordering::Relaxed);
    let max_size = DOWNLOAD_CONFIG.max_chunk_size.load(Ordering::Relaxed);
    let ideal_size = file_size / target_chunks;
    ideal_size.clamp(min_size, max_size) as usize
}

pub(super) async fn download_chunked(
    params: ChunkedDownloadParams<'_>,
    model_id: &str,
) -> Result<
    (u64, u64, Option<VerificationQueueItem>, String),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let ChunkedDownloadParams {
        url,
        revision,
        incomplete_path,
        final_path,
        progress,
        status_tx,
        complete_downloads: _complete_downloads,
        filename,
        expected_sha256,
        hf_token,
    } = params;

    let local_path_str = final_path.to_string_lossy().to_string();

    // Phase 1: build the client and probe the file size (with the raw
    // fallback). Every early return in this phase fires BEFORE the
    // `.incomplete` file is created, so the extracted `?` skips no cleanup.
    let (client, total_size, final_url, etag) =
        probe_file_size(url, filename, hf_token.as_deref(), status_tx).await?;

    // Update metadata entry in registry
    registry::upsert_metadata(DownloadMetadata {
        model_id: model_id.to_string(),
        filename: filename.to_string(),
        url: url.to_string(),
        local_path: local_path_str.clone(),
        total_size,
        downloaded_size: 0,
        status: DownloadStatus::Incomplete,
        expected_sha256: expected_sha256.clone(),
        revision: if revision == crate::api::DEFAULT_REVISION {
            None
        } else {
            Some(revision.to_string())
        },
    });

    // Calculate dynamic chunk size based on file size
    let chunk_size = calculate_chunk_size(total_size);

    // Initialize progress with chunk tracking
    let num_chunks = total_size.div_ceil(chunk_size as u64) as usize;

    // Resume decision (issue #50): trust an existing journal only when it
    // validates against THIS attempt's freshly probed identity (url, size,
    // layout, validators) and the on-disk `.incomplete` covers every byte
    // it claims. Any mismatch discards both files and restarts fresh —
    // fail-safe, never partial trust.
    let jpath = journal::journal_path(incomplete_path);
    let incomplete_len = tokio::fs::metadata(incomplete_path)
        .await
        .map(|m| m.len())
        .ok();
    let foreign_file = incomplete_len.is_some_and(|len| len > total_size);
    let resume_offsets: Option<Vec<u64>> = if foreign_file {
        None
    } else {
        journal::load(&jpath).and_then(|j| {
            j.validate(
                url,
                total_size,
                chunk_size as u64,
                etag.as_deref(),
                expected_sha256.as_deref(),
                incomplete_len,
            )
            .then_some(j.chunk_offsets)
        })
    };
    let (resume, resumed_bytes) = match resume_offsets {
        Some(offsets) => {
            let bytes = offsets.iter().sum();
            let _ = status_tx.send(format!(
                "Resuming {filename} at {}/{} bytes ({:.0}%)",
                bytes,
                total_size,
                if total_size > 0 {
                    bytes as f64 / total_size as f64 * 100.0
                } else {
                    0.0
                }
            ));
            (offsets, bytes)
        }
        None => {
            // No journal, invalid journal, or foreign data file: restart
            // from scratch (delete both, recreate below).
            let _ = tokio::fs::remove_file(incomplete_path).await;
            journal::remove(&jpath);
            (vec![0; num_chunks], 0)
        }
    };

    // The journal writer every chunk task records into. The header always
    // carries THIS attempt's validators (etag/sha256 as probed now), with
    // the resumed offsets grafted in; persisted eagerly so the journal
    // exists even for runs that fail before any chunk completes.
    let revision_field = if revision == crate::api::DEFAULT_REVISION {
        None
    } else {
        Some(revision.to_string())
    };
    let mut journal_doc = ResumeJournal::fresh(
        url,
        revision_field,
        total_size,
        chunk_size as u64,
        etag.clone(),
        expected_sha256.clone(),
        num_chunks,
    );
    journal_doc.chunk_offsets = resume.clone();
    let journal_writer = std::sync::Arc::new(JournalWriter::new(jpath.clone(), journal_doc));
    journal_writer.flush();

    {
        let mut prog = progress.lock().await;
        *prog = Some(DownloadProgress {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            downloaded: resumed_bytes,
            total: total_size,
            speed_mbps: 0.0,
            chunks: Vec::new(), // Chunks will be added dynamically as they start
            verifying: false,
            num_chunks,
            chunk_completed: resume
                .iter()
                .enumerate()
                .map(|(i, &off)| {
                    let start = i as u64 * chunk_size as u64;
                    off == (total_size - start).min(chunk_size as u64)
                })
                .collect(),
        });
    }

    // Phase 2a: create the file with proper size. A resumed download
    // opens WITHOUT truncate (its bytes are the whole point) and only
    // ever EXTENDS via set_len; a fresh download truncates any leftover.
    let file = if resumed_bytes > 0 {
        // Deliberate no-truncate open: the existing bytes ARE the resume
        // state (issue #50) — clippy's suspicious_open_options fires on
        // `create` without an explicit truncate decision, which is exactly
        // the decision being made here.
        #[allow(clippy::suspicious_open_options)]
        {
            tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(incomplete_path)
                .await?
        }
    } else {
        tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(incomplete_path)
            .await?
    };

    // Pre-allocate file space (optional, helps with fragmentation); on
    // resume this is a no-op when the file is already full-length.
    file.set_len(total_size).await?;
    drop(file); // Close to allow multiple handles

    // Phase 2b: spawn the chunk tasks (semaphore + shared counters).
    // Completed chunks (journal says every byte is on disk) are skipped
    // entirely — no request, no write.
    let tasks = spawn_chunk_tasks(
        client,
        final_url.clone(),
        incomplete_path.clone(),
        progress.clone(),
        ChunkPlan {
            num_chunks,
            chunk_size,
            total_size,
            resume_offsets: resume,
        },
        journal_writer,
    );

    // Phase 2c: wait for all chunks; first failure aborts the rest
    wait_for_chunks(tasks).await?;

    // Final progress update
    {
        let mut prog = progress.lock().await;
        if let Some(p) = prog.as_mut() {
            p.downloaded = total_size;
            // All chunks are complete once every handle has joined
            p.chunk_completed.iter_mut().for_each(|c| *c = true);
        }
    }

    // Rename to final path immediately after download completes. Retried
    // with backoff: antivirus and search indexers can briefly hold the
    // freshly-written `.incomplete` file open on Windows (sharing
    // violation / access denied), which would otherwise fail an otherwise
    // complete download (incident #37 symptom B). Policy: 1 initial try +
    // 4 retries, 100ms base delay with linear backoff (100/200/300/400ms).
    // Async twin: retries sleep via tokio so the worker thread never blocks.
    crate::utils::atomic_rename_with_retry_async(
        incomplete_path,
        final_path,
        4,
        std::time::Duration::from_millis(100),
    )
    .await?;

    // Success consumes the journal (a crash between rename and remove
    // leaves an orphan, cleaned by the next attempt of this file: a
    // journal without its `.incomplete` is untrusted and deleted).
    journal::remove(&jpath);

    // Prepare verification data if hash is available
    let verification_item = expected_sha256
        .as_ref()
        .map(|expected_hash| VerificationQueueItem {
            filename: filename.to_string(),
            local_path: final_path.to_string_lossy().to_string(),
            expected_sha256: expected_hash.clone(),
            total_size,
            is_manual: false,
        });

    Ok((total_size, total_size, verification_item, final_url))
}

/// One chunk task's join result: the chunk's byte count, or the transport
/// error that failed it.
type ChunkTaskResult = Result<u64, Box<dyn std::error::Error + Send + Sync>>;

/// Phase 1 of [`download_chunked`] (W5.1b): build the authenticated
/// client and determine the total file size with a `bytes=0-0` range
/// probe, falling back to the `/raw/` endpoint when the primary URL
/// 404s. Returns the client (reused by every chunk task), the parsed
/// total (from `Content-Range`, falling back to `Content-Length`), the
/// URL that will serve the bytes, and the response's `ETag` (when the
/// server sent one — a secondary resume validator; the LFS oid is the
/// primary one). Every error here predates file creation — the caller's
/// `?` skips no cleanup.
async fn probe_file_size(
    url: &str,
    filename: &str,
    hf_token: Option<&str>,
    status_tx: &mpsc::UnboundedSender<String>,
) -> Result<(reqwest::Client, u64, String, Option<String>), Box<dyn std::error::Error + Send + Sync>>
{
    let timeout_secs = DOWNLOAD_CONFIG
        .download_timeout_secs
        .load(Ordering::Relaxed);
    let (client, token_warning) = crate::http_client::build_client_with_token(
        hf_token,
        Some(std::time::Duration::from_secs(timeout_secs)),
    )?;
    // B5 owner revision 2026-10-07: a malformed token is dropped WITH A
    // WARNING through the run's status channel; the transport proceeds
    // unauthenticated (explicit, not the pre-M4 silent drop).
    if let Some(w) = token_warning {
        let _ = status_tx.send(format!("Warning: {}", w.message()));
    }

    // Step 1: Get file size using a range request
    // Try the primary URL first, fallback to raw endpoint on 404
    let (response, final_url) = match client.get(url).header("Range", "bytes=0-0").send().await {
        Ok(resp) => match resp.error_for_status() {
            Ok(r) => (r, url.to_string()),
            Err(e) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                // Try raw endpoint as fallback (revision-agnostic rewrite:
                // `/resolve/{rev}/` -> `/raw/{rev}/`)
                let raw_url = url.replacen("/resolve/", "/raw/", 1);
                let _ = status_tx.send(format!("404 error, trying raw endpoint for: {}", filename));

                let raw_response = client
                    .get(&raw_url)
                    .header("Range", "bytes=0-0")
                    .send()
                    .await?
                    .error_for_status()?;

                (raw_response, raw_url)
            }
            Err(e) => return Err(Box::new(e)),
        },
        Err(e) => return Err(Box::new(e)),
    };

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let total_size = if let Some(content_range) = response.headers().get("content-range") {
        // Parse "bytes 0-0/TOTAL" to get TOTAL
        if let Ok(range_str) = content_range.to_str() {
            if let Some(total_str) = range_str.split('/').nth(1) {
                total_str.parse::<u64>().unwrap_or(0)
            } else {
                return Err("Invalid Content-Range header".into());
            }
        } else {
            return Err("Invalid Content-Range header encoding".into());
        }
    } else {
        // Fallback: try Content-Length
        response.content_length().unwrap_or(0)
    };

    if total_size == 0 {
        return Err("Could not determine file size".into());
    }

    Ok((client, total_size, final_url, etag))
}

/// Phase 2b of [`download_chunked`] (W5.1b): create the shared
/// progress/speed state, the concurrency semaphore, and one task per
/// chunk. Each task registers itself in the progress struct, runs
/// [`download_chunk_with_progress`], then marks its chunk
/// completed/inactive — the loop body is the historical spawn closure,
/// verbatim. The tasks live in a [`JoinSet`] (plan M4/B4) so a failure
/// in one chunk can abort every still-running sibling (see
/// [`wait_for_chunks`]).
/// The chunk layout one `download_chunked` run settles on (W5.1b): the
/// computed chunking of the probed total plus, since issue #50, the
/// resume offsets per chunk (all-zero for a fresh download; a validated
/// journal's offsets otherwise). Bundled so `spawn_chunk_tasks` keeps a
/// small signature.
struct ChunkPlan {
    num_chunks: usize,
    chunk_size: usize,
    total_size: u64,
    /// Per-chunk bytes already on disk (length == num_chunks).
    resume_offsets: Vec<u64>,
}

fn spawn_chunk_tasks(
    client: reqwest::Client,
    final_url: String,
    incomplete_path: PathBuf,
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    plan: ChunkPlan,
    journal: Arc<JournalWriter>,
) -> JoinSet<ChunkTaskResult> {
    // Step 3: Download chunks in parallel
    let max_concurrent = DOWNLOAD_CONFIG.concurrent_threads.load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    // Shared progress tracking (W5.6 audit): `progress_downloaded` is a
    // single monotonic u64 counter — the only state its old mutex guarded —
    // so it is an `AtomicU64` (fetch_add per stream item, relaxed load for
    // the speed snapshot). Rendered progress never reads it directly: the
    // HUD/CLI read `DownloadProgress` under its own lock. The two
    // speed-pacing mutexes stay mutexed — they are compound state (the
    // Instant gates the global recalculation window; the byte marker and
    // the timestamp update must move as one unit, under exclusion).
    // Seeded with the resumed byte count so speeds/HUD reflect only NEW
    // bytes (plan-review finding: seeding just the counter would count
    // all resumed bytes as fresh throughput in the first interval).
    let resumed_bytes: u64 = plan.resume_offsets.iter().sum();
    let progress_downloaded = Arc::new(AtomicU64::new(resumed_bytes));
    let start_time = std::time::Instant::now();
    let last_update_time = Arc::new(Mutex::new(start_time));
    let last_downloaded_bytes = Arc::new(Mutex::new(resumed_bytes));

    let mut tasks: JoinSet<ChunkTaskResult> = JoinSet::new();

    for chunk_id in 0..plan.num_chunks {
        let start = chunk_id as u64 * plan.chunk_size as u64;
        let stop = std::cmp::min(start + plan.chunk_size as u64 - 1, plan.total_size - 1);
        let chunk_total = stop - start + 1;
        let resume_offset = plan.resume_offsets[chunk_id];
        if resume_offset >= chunk_total {
            // Journaled complete (== chunk_total; > was rejected by
            // validation): skip entirely — no request, no write.
            continue;
        }
        let client = client.clone();
        let download_url = final_url.clone();
        let incomplete_path = incomplete_path.clone();
        let semaphore = semaphore.clone();
        let progress_downloaded = progress_downloaded.clone();
        let progress = progress.clone();
        let last_update_time = last_update_time.clone();
        let last_downloaded_bytes = last_downloaded_bytes.clone();
        let journal = journal.clone();

        tasks.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            // Add this chunk to active chunks (a resumed chunk enters with
            // its already-on-disk bytes as its starting progress).
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    p.chunks.push(ChunkProgress {
                        chunk_id,
                        start,
                        end: stop,
                        downloaded: resume_offset,
                        total: chunk_total,
                        speed_mbps: 0.0,
                        is_active: true,
                    });
                }
            }

            let chunk_start_time = std::time::Instant::now();

            // W5.6: everything the chunk task needs, bundled — this used
            // to be a 12-argument function signature. The pacing fields
            // start exactly where the old caller initialized them (after
            // the chunk registers itself in the progress struct).
            let ctx = ChunkContext {
                client,
                download_url,
                incomplete_path,
                progress: progress.clone(),
                progress_downloaded,
                last_update_time,
                last_downloaded_bytes,
                journal,
                chunk_id,
                start,
                stop,
                resume_offset,
                last_update: chunk_start_time,
                last_bytes: resume_offset,
            };

            // Download this chunk with progress tracking
            let result = download_chunk_with_progress(ctx).await;

            let chunk_size = stop - start + 1;
            let chunk_ok = result.is_ok();

            // Remove this chunk from active list (mark as inactive) and
            // record completion in the bitmap for monotonic progress display
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    if let Some(chunk) = p.chunks.iter_mut().find(|c| c.chunk_id == chunk_id) {
                        chunk.is_active = false;
                        chunk.downloaded = chunk_total;
                    }
                    if chunk_ok {
                        if let Some(done) = p.chunk_completed.get_mut(chunk_id) {
                            *done = true;
                        }
                    }
                }
            }

            // Clean up inactive chunks older than 1 second
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    p.chunks.retain(|c| c.is_active);
                }
            }

            result?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(chunk_size)
        });
    }

    tasks
}

/// Phase 2c of [`download_chunked`] (W5.1b): await every chunk task
/// until the [`JoinSet`] runs dry.
///
/// On the first failure, [`JoinSet::abort_all`] cancels every remaining
/// chunk task BEFORE the error propagates (plan M4/B4): their futures are
/// dropped at their await points, so no zombie keeps streaming bytes at
/// stale offsets into the `.incomplete` file while
/// `execute_download_with_retry` deletes and recreates it — the sporadic
/// ENOENT/offset race of incident #37 ("download failed after retries:
/// No such file or directory", ~1/10 suite runs locally). Pinned by
/// `abort_tests::first_chunk_error_aborts_parked_sibling_no_writes_after_recreate`.
async fn wait_for_chunks(
    mut tasks: JoinSet<ChunkTaskResult>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(_size)) => {}
            Ok(Err(e)) => {
                tasks.abort_all();
                return Err(e);
            }
            Err(join_err) => {
                tasks.abort_all();
                return Err(format!("chunk task failed: {join_err}").into());
            }
        }
    }
    Ok(())
}

/// Everything one chunk task needs (W5.6): the shared per-download
/// handles plus this chunk's byte span and per-chunk speed-pacing state —
/// the bundle replaces what used to be a 12-argument
/// `download_chunk_with_progress` signature.
struct ChunkContext {
    client: reqwest::Client,
    /// URL that serves the bytes (the possibly-raw-fallback endpoint).
    download_url: String,
    /// The `.incomplete` file all chunks write into at their offsets.
    incomplete_path: PathBuf,
    /// The engine-wide progress struct the HUD/CLI render from.
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    /// Monotonic sum of every chunk's downloaded bytes (single u64
    /// counter — see the W5.6 audit note at its creation site).
    progress_downloaded: Arc<AtomicU64>,
    /// Global speed-pacing compound state (Instant gate + byte marker);
    /// mutexed on purpose — only one chunk task may run the global
    /// recalculation per interval window.
    last_update_time: Arc<Mutex<std::time::Instant>>,
    last_downloaded_bytes: Arc<Mutex<u64>>,
    /// The resume journal writer (terminal-leaf lock; `record` is called
    /// OUTSIDE every other lock scope — see `journal.rs` module docs).
    journal: Arc<JournalWriter>,
    chunk_id: usize,
    start: u64,
    stop: u64,
    /// Bytes of this chunk already on disk from a previous attempt
    /// (issue #50): the worker seeks to `start + resume_offset` and asks
    /// the server for `bytes=(start+resume_offset)-stop`.
    resume_offset: u64,
    /// This chunk's last speed sample (when/what it last reported).
    last_update: std::time::Instant,
    last_bytes: u64,
}

async fn download_chunk_with_progress(
    ctx: ChunkContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ChunkContext {
        client,
        download_url,
        incomplete_path,
        progress,
        progress_downloaded,
        last_update_time,
        last_downloaded_bytes,
        journal,
        chunk_id,
        start,
        stop,
        resume_offset,
        mut last_update,
        mut last_bytes,
    } = ctx;

    let chunk_total = stop - start + 1;
    // Resume: only the missing tail of the chunk is requested.
    let range = format!("bytes={}-{}", start + resume_offset, stop);

    let response = client
        .get(&download_url)
        .header("Range", range)
        .send()
        .await?
        .error_for_status()?;

    // A server that ignores Range and answers 200 with the FULL body
    // would be written at the seek offset — silent corruption. Partial
    // Content is the only acceptable answer to a range request.
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(format!(
            "server ignored Range request (expected 206, got {})",
            response.status()
        )
        .into());
    }

    let mut chunk_downloaded = resume_offset;

    // Open file for writing at offset
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&incomplete_path)
        .await?;

    file.seek(SeekFrom::Start(start + resume_offset)).await?;

    // Stream the response and update progress
    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(item) = stream.next().await {
        let bytes = item?;

        // Over-delivery cap (fix review): a body longer than the
        // requested range would write past `stop` into the NEXT chunk's
        // byte territory — the corruption mirror of the short-stream
        // hole below. Fail instead of silently clobbering a sibling.
        if chunk_downloaded + bytes.len() as u64 > chunk_total {
            return Err(format!(
                "chunk {} over-delivery: {} bytes exceed the requested range",
                chunk_id,
                bytes.len()
            )
            .into());
        }

        // Rate limiting: acquire tokens before writing
        if DOWNLOAD_CONFIG.rate_limit_enabled.load(Ordering::Relaxed) {
            RATE_LIMITER.acquire(bytes.len()).await?;
        }

        file.write_all(&bytes).await?;

        let bytes_len = bytes.len() as u64;
        chunk_downloaded += bytes_len;

        // Update total downloaded bytes immediately (single u64 counter —
        // atomic fetch_add; the reader below takes a relaxed snapshot).
        progress_downloaded.fetch_add(bytes_len, Ordering::Relaxed);

        // Update chunk progress and total speed at configured interval
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_update).as_secs_f64();
        let interval_secs = DOWNLOAD_CONFIG
            .progress_update_interval_ms
            .load(Ordering::Relaxed) as f64
            / 1000.0;

        if elapsed >= interval_secs {
            let bytes_since_last = chunk_downloaded - last_bytes;
            let chunk_speed_mbps = (bytes_since_last as f64 / elapsed) / 1_048_576.0;

            // Calculate total download speed
            let mut last_update_global = last_update_time.lock().await;
            let elapsed_global = now.duration_since(*last_update_global).as_secs_f64();

            let total_speed_mbps = if elapsed_global >= interval_secs {
                let total_downloaded = progress_downloaded.load(Ordering::Relaxed);

                let mut last_bytes_global = last_downloaded_bytes.lock().await;
                let bytes_since_last_global = total_downloaded - *last_bytes_global;
                let speed = (bytes_since_last_global as f64 / elapsed_global) / 1_048_576.0;

                *last_bytes_global = total_downloaded;
                *last_update_global = now;

                Some((speed, total_downloaded))
            } else {
                None
            };
            drop(last_update_global);

            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    if let Some(chunk) = p.chunks.iter_mut().find(|c| c.chunk_id == chunk_id) {
                        chunk.downloaded = chunk_downloaded;
                        chunk.speed_mbps = chunk_speed_mbps;
                    }

                    // Update total speed and downloaded if calculated
                    if let Some((speed, total)) = total_speed_mbps {
                        p.speed_mbps = speed;
                        p.downloaded = total;
                    }
                }
            } // progress guard released BEFORE journaling (fix-review
              // finding, unanimous): the journal mutex is a terminal leaf
              // and its sync write must never run under (and so stall
              // renders waiting on) the progress lock.

            journal.record(chunk_id, chunk_downloaded);

            last_update = now;
            last_bytes = chunk_downloaded;
        }
    }

    // Short-stream detection (plan review): a stream that ends cleanly
    // BEFORE delivering the requested range (clean-FIN mid-body, a
    // range-clamping server) used to be miscounted as chunk success —
    // leaving a hole the file-level size check cannot see (set_len made
    // the file full-length). That is silent corruption; fail the chunk.
    if chunk_downloaded < chunk_total {
        return Err(format!(
            "chunk {} short read: got {} of {} bytes",
            chunk_id,
            chunk_downloaded - resume_offset,
            chunk_total - resume_offset
        )
        .into());
    }

    // Completion is power-loss safe before it is journaled: flush the
    // data, fsync it, THEN record (data-before-journal keeps the journal
    // under-reporting only; see `journal.rs` module docs).
    file.flush().await?;
    file.sync_all().await?;
    journal.record(chunk_id, chunk_total);

    Ok(())
}

#[cfg(test)]
mod abort_tests {
    //! Plan M4/B4: the abort contract of [`wait_for_chunks`]. A failed
    //! chunk must ABORT its still-running siblings before the error
    //! propagates to the retry loop — a merely-detached task keeps its
    //! open file handle and writes at its stale offset into the file the
    //! retry recreates (incident #37's zombie writer).
    //!
    //! Deterministic by construction (no flake loop): the parked chunk
    //! parks on an in-process [`tokio::sync::Notify`] barrier, never on
    //! network timing. The failing chunk errors only after the parked one
    //! has registered, so both tasks are provably alive when the first
    //! error lands; the parked task records — in an atomic the test reads
    //! — every resumption after the barrier. Tokio's abort semantics make
    //! the pass deterministic: a task parked at an await point is dropped
    //! without its body ever being polled again, so it cannot observe the
    //! barrier release. Against the pre-B4 `Vec<JoinHandle>` early-break
    //! (remaining handles detached) the same test fails: the detached task
    //! resumes on release and performs the stale write.

    use super::{wait_for_chunks, ChunkTaskResult};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::task::JoinSet;

    type ChunkError = Box<dyn std::error::Error + Send + Sync>;

    /// Unique temp dir per test run (pid + atomic counter; no env vars
    /// touched, so no ENV_MUTEX is needed).
    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("chunk-abort-{}-{}-{}", tag, std::process::id(), n))
    }

    /// The stale write a zombie chunk task performs if it resumes after
    /// the retry recreated the file: seek to its OLD offset and write —
    /// exactly the corruption shape from incident #37.
    fn zombie_write(path: &std::path::Path) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("recreated .incomplete exists");
        file.seek(SeekFrom::Start(1)).unwrap();
        file.write_all(b"ZOMBIE").unwrap();
    }

    #[tokio::test]
    async fn first_chunk_error_aborts_parked_sibling_no_writes_after_recreate() {
        let dir = temp_dir("parked");
        std::fs::create_dir_all(&dir).unwrap();
        let incomplete = dir.join("file.incomplete");

        // The stale attempt's bytes — what a zombie would corrupt.
        std::fs::write(&incomplete, b"OLD-DATA").unwrap();

        let barrier = Arc::new(tokio::sync::Notify::new());
        let parked_registered = Arc::new(AtomicBool::new(false));
        let resumed_after_recreate = Arc::new(AtomicBool::new(false));

        // Chunk 1 — the parked one: registers, then parks on the barrier.
        // If it EVER resumes, it flags the atomic and performs the stale
        // write into whatever file sits at the path at that moment.
        let parked = {
            let barrier = barrier.clone();
            let parked_registered = parked_registered.clone();
            let resumed = resumed_after_recreate.clone();
            let path = incomplete.clone();
            async move {
                parked_registered.store(true, Ordering::SeqCst);
                barrier.notified().await;
                resumed.store(true, Ordering::SeqCst);
                zombie_write(&path);
                Ok::<u64, ChunkError>(7)
            }
        };

        // Chunk 0 — the failing one: errors only after its sibling has
        // parked, so both are provably alive when the first error lands.
        let failing = {
            let parked_registered = parked_registered.clone();
            async move {
                while !parked_registered.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
                Err::<u64, ChunkError>("chunk 0: injected".into())
            }
        };

        let mut tasks: JoinSet<ChunkTaskResult> = JoinSet::new();
        tasks.spawn(failing);
        tasks.spawn(parked);
        let result = wait_for_chunks(tasks).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "chunk 0: injected",
            "the first error must propagate verbatim"
        );

        // The retry loop's behavior between attempts: delete the stale
        // attempt and recreate the file from scratch.
        std::fs::remove_file(&incomplete).unwrap();
        std::fs::write(&incomplete, b"NEW").unwrap();

        // Release the barrier the parked chunk waits on. If it was
        // aborted, the release is a no-op (its future is gone); if it was
        // merely detached (pre-B4), it resumes within this poll loop and
        // the flag trips — polling for up to 1 s means a detached zombie
        // fails fast and deterministically instead of on a fixed sleep.
        barrier.notify_waiters();
        for _ in 0..200 {
            assert!(
                !resumed_after_recreate.load(Ordering::SeqCst),
                "parked chunk resumed after the retry recreated the file"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // No stale write landed: the recreated file is byte-identical.
        assert_eq!(std::fs::read(&incomplete).unwrap(), b"NEW");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn all_success_tasks_are_awaited() {
        // Sanity pin for the success path: every task's completion is
        // observed before Ok(()) — the drain must run to exhaustion, not
        // stop at the first join.
        let mut tasks: JoinSet<ChunkTaskResult> = JoinSet::new();
        for _ in 0..4 {
            tasks.spawn(async { Ok::<u64, ChunkError>(1) });
        }
        wait_for_chunks(tasks)
            .await
            .expect("all-success chunks must join cleanly");
    }
}

#[cfg(test)]
mod resume_worker_tests {
    //! Issue #50 core-mechanism coverage (plan review finding): a
    //! NON-ZERO, NON-COMPLETE `resume_offset` driving the worker's seek +
    //! Range construction + short-stream detection. The e2e suite's 4 KiB
    //! loopback chunks finish inside the journal's 1 s coalescing window,
    //! so mid-chunk resume is invisible there; here the worker runs
    //! against an in-process hyper server (no registry, no engine).
    //!
    //! Layout: a 3_000-byte file with chunk size 1024 splits into chunks
    //! (0,1023) / (1024,2047) / (2048,2999).

    use super::journal::{journal_path, JournalWriter, ResumeJournal};
    use super::{download_chunk_with_progress, ChunkContext};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// The server-side knobs the tests read/set: every Range header seen,
    /// and an optional end-clamp for short-serve injection.
    #[derive(Clone)]
    struct ServerKnobs {
        seen: Arc<std::sync::Mutex<Vec<String>>>,
        clamp_end: Arc<std::sync::Mutex<Option<u64>>>,
    }

    /// A one-path Range-aware hyper server over a fixed buffer.
    async fn spawn_range_server(content: &'static [u8]) -> (String, ServerKnobs) {
        use hyper::service::{make_service_fn, service_fn};
        use hyper::{Body, Request, Response, Server, StatusCode};

        let knobs = ServerKnobs {
            seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            clamp_end: Arc::new(std::sync::Mutex::new(None)),
        };
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let knobs_ret = knobs.clone();
        let make_service = make_service_fn(move |_| {
            let knobs = knobs.clone();
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req: Request<Body>| {
                    let knobs = knobs.clone();
                    async move {
                        let header = req
                            .headers()
                            .get("range")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        if let Some(h) = &header {
                            knobs.seen.lock().unwrap().push(h.clone());
                        }
                        let (start, end) = header
                            .as_deref()
                            .and_then(|s| s.strip_prefix("bytes="))
                            .and_then(|s| s.split_once('-'))
                            .map(|(s, e)| {
                                (
                                    s.parse::<u64>().unwrap_or(0),
                                    e.parse::<u64>().unwrap_or(content.len() as u64 - 1),
                                )
                            })
                            .unwrap_or((0, content.len() as u64 - 1));
                        // Short-serve injection: clamp `end` when set.
                        let end = knobs
                            .clamp_end
                            .lock()
                            .unwrap()
                            .map(|max| end.min(max))
                            .unwrap_or(end);
                        let slice = content[start as usize..=(end as usize)].to_vec();
                        Ok::<Response<Body>, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::PARTIAL_CONTENT)
                                .header(
                                    "content-range",
                                    format!("bytes {}-{}/{}", start, end, content.len()),
                                )
                                .body(Body::from(slice))
                                .unwrap(),
                        )
                    }
                }))
            }
        });
        let server = Server::bind(&addr).serve(make_service);
        let url = format!("http://{}/a/b/resolve/main/model.gguf", server.local_addr());
        tokio::spawn(server);
        (url, knobs_ret)
    }

    fn content_3000() -> &'static [u8] {
        Box::leak(
            (0..3000usize)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<u8>>()
                .into_boxed_slice(),
        )
    }

    #[tokio::test]
    async fn worker_resumes_mid_chunk_from_journal_offset() {
        let content = content_3000();
        let (url, knobs) = spawn_range_server(content).await;

        // Chunk 1 = (1024, 2047); the journal says 512 of its bytes are
        // already on disk (bytes 1024..1536 hold valid content).
        let dir = std::env::temp_dir().join(format!("rhd-resume-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let incomplete = dir.join("model.gguf.incomplete");
        let mut disk = vec![0u8; 3000];
        disk[1024..1536].copy_from_slice(&content[1024..1536]);
        std::fs::write(&incomplete, &disk).unwrap();

        let jpath = journal_path(&incomplete);
        let journal = Arc::new(JournalWriter::new(
            jpath.clone(),
            ResumeJournal::fresh(&url, None, 3000, 1024, None, None, 3),
        ));

        let ctx = ChunkContext {
            client: reqwest::Client::new(),
            download_url: url,
            incomplete_path: incomplete.clone(),
            progress: Arc::new(Mutex::new(None)),
            progress_downloaded: Arc::new(std::sync::atomic::AtomicU64::new(1536)),
            last_update_time: Arc::new(Mutex::new(std::time::Instant::now())),
            last_downloaded_bytes: Arc::new(Mutex::new(1536u64)),
            journal: journal.clone(),
            chunk_id: 1,
            start: 1024,
            stop: 2047,
            resume_offset: 512,
            last_update: std::time::Instant::now(),
            last_bytes: 512,
        };

        download_chunk_with_progress(ctx)
            .await
            .expect("resumed chunk completes");

        // The wire asked for exactly the missing tail of the chunk.
        assert_eq!(
            knobs.seen.lock().unwrap().last().unwrap(),
            "bytes=1536-2047"
        );
        // The file now holds the full chunk-1 bytes; everything else is
        // untouched (chunk 0/2 are not this worker's business).
        let after = std::fs::read(&incomplete).unwrap();
        assert_eq!(&after[1024..2048], &content[1024..2048]);
        assert_eq!(&after[0..1024], &[0u8; 1024]);
        // The journal recorded the chunk complete.
        assert_eq!(
            super::journal::load(&jpath).unwrap().chunk_offsets,
            vec![0, 1024, 0]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn worker_errors_on_short_stream_instead_of_faking_success() {
        let content = content_3000();
        let (url, knobs) = spawn_range_server(content).await;

        let dir = std::env::temp_dir().join(format!("rhd-short-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let incomplete = dir.join("model.gguf.incomplete");
        std::fs::write(&incomplete, vec![0u8; 3000]).unwrap();

        // Serve only through byte 1800 (clamped end): a clean-FIN short
        // body the old worker miscounted as chunk success.
        *knobs.clamp_end.lock().unwrap() = Some(1800);

        let jpath = journal_path(&incomplete);
        let journal = Arc::new(JournalWriter::new(
            jpath.clone(),
            ResumeJournal::fresh(&url, None, 3000, 1024, None, None, 3),
        ));

        let ctx = ChunkContext {
            client: reqwest::Client::new(),
            download_url: url,
            incomplete_path: incomplete.clone(),
            progress: Arc::new(Mutex::new(None)),
            progress_downloaded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_update_time: Arc::new(Mutex::new(std::time::Instant::now())),
            last_downloaded_bytes: Arc::new(Mutex::new(0u64)),
            journal,
            chunk_id: 1,
            start: 1024,
            stop: 2047,
            resume_offset: 0,
            last_update: std::time::Instant::now(),
            last_bytes: 0,
        };

        let err = download_chunk_with_progress(ctx)
            .await
            .expect_err("short stream must fail the chunk");
        assert!(
            err.to_string().contains("short read"),
            "error should name the short read: {err}"
        );
        assert_eq!(
            knobs.seen.lock().unwrap().last().unwrap(),
            "bytes=1024-2047"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn worker_rejects_range_ignoring_200_response() {
        // A server that answers a Range request with 200 + the FULL body
        // would have its bytes written at the seek offset — silent
        // corruption. The worker must fail with a clear error instead
        // (fix-review finding: pin the 206-assert branch; note this is a
        // deliberate capability change — range-incapable servers are no
        // longer supported by the chunked transport; HF always honors
        // Range).
        use hyper::service::{make_service_fn, service_fn};
        use hyper::{Body, Response, Server, StatusCode};

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let make_service = make_service_fn(move |_| async move {
            Ok::<_, hyper::Error>(service_fn(move |_req| async move {
                Ok::<Response<Body>, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from(vec![0u8; 3000]))
                        .unwrap(),
                )
            }))
        });
        let server = Server::bind(&addr).serve(make_service);
        let url = format!("http://{}/a/b/resolve/main/model.gguf", server.local_addr());
        tokio::spawn(server);

        let dir = std::env::temp_dir().join(format!("rhd-200-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let incomplete = dir.join("model.gguf.incomplete");
        std::fs::write(&incomplete, vec![0u8; 3000]).unwrap();

        let jpath = journal_path(&incomplete);
        let journal = Arc::new(JournalWriter::new(
            jpath.clone(),
            ResumeJournal::fresh(&url, None, 3000, 1024, None, None, 3),
        ));

        let ctx = ChunkContext {
            client: reqwest::Client::new(),
            download_url: url,
            incomplete_path: incomplete.clone(),
            progress: Arc::new(Mutex::new(None)),
            progress_downloaded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_update_time: Arc::new(Mutex::new(std::time::Instant::now())),
            last_downloaded_bytes: Arc::new(Mutex::new(0u64)),
            journal,
            chunk_id: 1,
            start: 1024,
            stop: 2047,
            resume_offset: 0,
            last_update: std::time::Instant::now(),
            last_bytes: 0,
        };

        let err = download_chunk_with_progress(ctx)
            .await
            .expect_err("a 200 answer to a Range request must fail the chunk");
        assert!(
            err.to_string().contains("ignored Range"),
            "error should name the ignored Range: {err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn worker_rejects_over_delivery_past_chunk_end() {
        // Over-delivery mirror of the short-stream hole: a 206 body LONGER
        // than the requested range would write past `stop` into the next
        // chunk's territory. The over-delivery cap must fail the chunk
        // before any write lands (fix-review finding).
        use hyper::service::{make_service_fn, service_fn};
        use hyper::{Body, Response, Server, StatusCode};

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let make_service = make_service_fn(move |_| {
            async move {
                Ok::<_, hyper::Error>(service_fn(move |_req| async move {
                    // A 206 status with a body LONGER than any requested
                    // range (3000 bytes to every request).
                    Ok::<Response<Body>, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header("content-range", "bytes 1024-2047/3000")
                            .body(Body::from(vec![7u8; 3000]))
                            .unwrap(),
                    )
                }))
            }
        });
        let server = Server::bind(&addr).serve(make_service);
        let url = format!("http://{}/a/b/resolve/main/model.gguf", server.local_addr());
        tokio::spawn(server);

        let dir = std::env::temp_dir().join(format!("rhd-over-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let incomplete = dir.join("model.gguf.incomplete");
        std::fs::write(&incomplete, vec![0u8; 3000]).unwrap();

        let jpath = journal_path(&incomplete);
        let journal = Arc::new(JournalWriter::new(
            jpath.clone(),
            ResumeJournal::fresh(&url, None, 3000, 1024, None, None, 3),
        ));

        let ctx = ChunkContext {
            client: reqwest::Client::new(),
            download_url: url,
            incomplete_path: incomplete.clone(),
            progress: Arc::new(Mutex::new(None)),
            progress_downloaded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_update_time: Arc::new(Mutex::new(std::time::Instant::now())),
            last_downloaded_bytes: Arc::new(Mutex::new(0u64)),
            journal,
            chunk_id: 1,
            start: 1024,
            stop: 2047,
            resume_offset: 0,
            last_update: std::time::Instant::now(),
            last_bytes: 0,
        };

        let err = download_chunk_with_progress(ctx)
            .await
            .expect_err("an over-delivering body must fail the chunk");
        assert!(
            err.to_string().contains("over-delivery"),
            "error should name the over-delivery: {err}"
        );
        // Nothing was written (the first stream item already exceeded the
        // range; the file is untouched).
        assert_eq!(std::fs::read(&incomplete).unwrap(), vec![0u8; 3000]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
