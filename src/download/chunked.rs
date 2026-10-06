//! The chunked transport (W3.8): [`download_chunked`] probes the file
//! size, preallocates the `.incomplete` file, spawns one task per chunk
//! under a semaphore, and renames on success. The per-chunk worker
//! ([`download_chunk_with_progress`]) streams its byte range with rate
//! limiting and progress/speed pacing. Extracted verbatim from the old
//! monolithic `download.rs`; every `crate::download::X` path keeps
//! compiling through the facade re-exports.

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
    let (client, total_size, final_url) =
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

    {
        let mut prog = progress.lock().await;
        *prog = Some(DownloadProgress {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            downloaded: 0,
            total: total_size,
            speed_mbps: 0.0,
            chunks: Vec::new(), // Chunks will be added dynamically as they start
            verifying: false,
            num_chunks,
            chunk_completed: vec![false; num_chunks],
        });
    }

    // Phase 2a: create the file with proper size
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(incomplete_path)
        .await?;

    // Pre-allocate file space (optional, helps with fragmentation)
    file.set_len(total_size).await?;
    drop(file); // Close to allow multiple handles

    // Phase 2b: spawn the chunk tasks (semaphore + shared counters)
    let handles = spawn_chunk_tasks(
        client,
        final_url.clone(),
        incomplete_path.clone(),
        progress.clone(),
        num_chunks,
        chunk_size,
        total_size,
    );

    // Phase 2c: wait for all chunks; first failure aborts the await
    wait_for_chunks(handles).await?;

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
/// total (from `Content-Range`, falling back to `Content-Length`), and
/// the URL that will serve the bytes. Every error here predates file
/// creation — the caller's `?` skips no cleanup.
async fn probe_file_size(
    url: &str,
    filename: &str,
    hf_token: Option<&str>,
    status_tx: &mpsc::UnboundedSender<String>,
) -> Result<(reqwest::Client, u64, String), Box<dyn std::error::Error + Send + Sync>> {
    let timeout_secs = DOWNLOAD_CONFIG
        .download_timeout_secs
        .load(Ordering::Relaxed);
    let client = crate::http_client::build_client_with_token(
        hf_token,
        Some(std::time::Duration::from_secs(timeout_secs)),
    )?;

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

    Ok((client, total_size, final_url))
}

/// Phase 2b of [`download_chunked`] (W5.1b): create the shared
/// progress/speed state, the concurrency semaphore, and one task per
/// chunk. Each task registers itself in the progress struct, runs
/// [`download_chunk_with_progress`], then marks its chunk
/// completed/inactive — the loop body is the historical spawn closure,
/// verbatim.
fn spawn_chunk_tasks(
    client: reqwest::Client,
    final_url: String,
    incomplete_path: PathBuf,
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    num_chunks: usize,
    chunk_size: usize,
    total_size: u64,
) -> Vec<tokio::task::JoinHandle<ChunkTaskResult>> {
    // Step 3: Download chunks in parallel
    let max_concurrent = DOWNLOAD_CONFIG.concurrent_threads.load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    // Shared progress tracking (W5.6 audit): `progress_downloaded` is a
    // single monotonic u64 counter — the only state its old mutex guarded —
    // so it is an `AtomicU64` (fetch_add per stream item, relaxed load for
    // the speed snapshot). Rendered progress never reads it directly: the
    // HUD/CLI read `DownloadProgress` under its own lock. The two
    // speed-pacing mutexes stay mutexed — they are compound state (the
    // Instant gates the global recalculation window; the byte marker and
    // the timestamp update must move as one unit, under exclusion).
    let progress_downloaded = Arc::new(AtomicU64::new(0));
    let start_time = std::time::Instant::now();
    let last_update_time = Arc::new(Mutex::new(start_time));
    let last_downloaded_bytes = Arc::new(Mutex::new(0u64));

    for chunk_id in 0..num_chunks {
        let start = chunk_id as u64 * chunk_size as u64;
        let stop = std::cmp::min(start + chunk_size as u64 - 1, total_size - 1);
        let client = client.clone();
        let download_url = final_url.clone();
        let incomplete_path = incomplete_path.clone();
        let semaphore = semaphore.clone();
        let progress_downloaded = progress_downloaded.clone();
        let progress = progress.clone();
        let last_update_time = last_update_time.clone();
        let last_downloaded_bytes = last_downloaded_bytes.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            let chunk_total = stop - start + 1;

            // Add this chunk to active chunks
            {
                let mut prog = progress.lock().await;
                if let Some(p) = prog.as_mut() {
                    p.chunks.push(ChunkProgress {
                        chunk_id,
                        start,
                        end: stop,
                        downloaded: 0,
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
                chunk_id,
                start,
                stop,
                last_update: chunk_start_time,
                last_bytes: 0,
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

        handles.push(handle);
    }

    handles
}

/// Phase 2c of [`download_chunked`] (W5.1b): await every chunk task.
///
/// Wait for all chunks to complete. On the first failure, stop awaiting
/// the remaining chunk tasks: their handles would otherwise never be
/// awaited and the zombie tasks keep running while the retry loop
/// deletes and recreates the `.incomplete` file — a sporadic
/// ENOENT/offset race (incident #37: "download failed after retries:
/// No such file or directory", ~1/10 suite runs locally).
async fn wait_for_chunks(
    handles: Vec<tokio::task::JoinHandle<ChunkTaskResult>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut chunk_error: Option<Box<dyn std::error::Error + Send + Sync>> = None;
    for handle in handles {
        match handle.await {
            Ok(Ok(_size)) => {}
            Ok(Err(e)) => {
                chunk_error = Some(e);
                break;
            }
            Err(join_err) => {
                chunk_error = Some(format!("chunk task failed: {join_err}").into());
                break;
            }
        }
    }
    if let Some(e) = chunk_error {
        return Err(e);
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
    chunk_id: usize,
    start: u64,
    stop: u64,
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
        chunk_id,
        start,
        stop,
        mut last_update,
        mut last_bytes,
    } = ctx;

    let range = format!("bytes={}-{}", start, stop);

    let response = client
        .get(&download_url)
        .header("Range", range)
        .send()
        .await?
        .error_for_status()?;

    let mut chunk_downloaded = 0u64;

    // Open file for writing at offset
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&incomplete_path)
        .await?;

    file.seek(SeekFrom::Start(start)).await?;

    // Stream the response and update progress
    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(item) = stream.next().await {
        let bytes = item?;

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

            last_update = now;
            last_bytes = chunk_downloaded;
        }
    }

    file.flush().await?;

    Ok(())
}
