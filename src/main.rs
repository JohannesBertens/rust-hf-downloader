mod api;
mod cli;
mod config;
mod download;
mod headless;
mod http_client;
mod models;
mod rate_limiter;
mod registry;
mod ui;
mod utils;
mod verification;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Parse CLI arguments
    use clap::Parser;
    let cli_args = cli::Cli::parse();

    // If --headless flag is present, run in CLI mode
    if cli_args.headless {
        let json_mode = cli_args.json;
        let reporter = headless::ProgressReporter::new(json_mode);

        // Create channels for download manager
        let (download_tx, download_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let download_rx = std::sync::Arc::new(tokio::sync::Mutex::new(download_rx));

        // Create shutdown signal
        let shutdown_signal = std::sync::Arc::new(tokio::sync::Mutex::new(false));
        let shutdown_signal_clone = shutdown_signal.clone();

        // Spawn download manager task
        let download_progress = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let complete_downloads =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let verification_queue = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let verification_queue_size = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let download_queue = std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::models::QueueState::new(0, 0),
        ));
        let download_registry = std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::models::DownloadRegistry::default(),
        ));

        // Create verification progress tracking
        let verification_progress = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

        // Clone Arcs for the download manager task
        let download_progress_clone = download_progress.clone();
        let complete_downloads_clone = complete_downloads.clone();
        let verification_queue_clone = verification_queue.clone();
        let verification_queue_size_clone = verification_queue_size.clone();
        let download_queue_clone = download_queue.clone();
        let progress_tx_clone = progress_tx.clone();
        let _download_registry_clone = download_registry.clone();

        // Spawn verification worker
        let verification_queue_worker = verification_queue.clone();
        let verification_progress_worker = verification_progress.clone();
        let verification_queue_size_worker = verification_queue_size.clone();
        let progress_tx_verify = progress_tx.clone();
        let download_registry_verify = download_registry.clone();
        tokio::spawn(async move {
            verification::verification_worker(
                verification_queue_worker,
                verification_progress_worker,
                verification_queue_size_worker,
                progress_tx_verify,
                download_registry_verify,
            )
            .await;
        });

        tokio::spawn(async move {
            use crate::download::DownloadParams;

            loop {
                // Lock only when receiving, release immediately after
                // This prevents deadlock by not holding download_rx while acquiring other locks
                let (model_id, filename, path, sha256, hf_token, total_size) = {
                    let mut rx = download_rx.lock().await;
                    match rx.recv().await {
                        Some(msg) => msg,
                        None => break, // Channel closed
                    }
                };

                // Spawn download task
                let params = DownloadParams {
                    model_id,
                    filename,
                    base_path: path,
                    progress: download_progress_clone.clone(),
                    status_tx: progress_tx_clone.clone(),
                    complete_downloads: complete_downloads_clone.clone(),
                    expected_sha256: sha256,
                    verification_queue: verification_queue_clone.clone(),
                    verification_queue_size: verification_queue_size_clone.clone(),
                    hf_token,
                };

                let queue = download_queue_clone.clone();
                tokio::spawn(async move {
                    download::start_download(params).await;
                    let mut queue = queue.lock().await;
                    queue.remove(1, total_size);
                });
            }
        });

        // Spawn progress reporter task
        let json_mode = cli_args.json;
        tokio::spawn(async move {
            use std::io::Write;

            while let Some(msg) = progress_rx.recv().await {
                if !json_mode {
                    print!("\r\x1b[2K");
                    let _ = std::io::stdout().flush();
                }
                eprintln!("{}", msg);
            }
        });

        // Spawn signal handler for graceful shutdown
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            tokio::spawn(async move {
                let mut sigint =
                    signal(SignalKind::interrupt()).expect("Failed to setup SIGINT handler");
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("Failed to setup SIGTERM handler");

                tokio::select! {
                    _ = sigint.recv() => {
                        eprintln!("\nReceived interrupt signal (Ctrl+C), shutting down gracefully...");
                        *shutdown_signal_clone.lock().await = true;
                    }
                    _ = sigterm.recv() => {
                        eprintln!("\nReceived termination signal, shutting down gracefully...");
                        *shutdown_signal_clone.lock().await = true;
                    }
                }
            });
        }

        #[cfg(windows)]
        {
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                eprintln!("\nReceived interrupt signal (Ctrl+C), shutting down gracefully...");
                *shutdown_signal_clone.lock().await = true;
            });
        }

        // Execute command
        let result = match cli_args.command {
            Some(cli::Commands::Search {
                query,
                sort: _,
                min_downloads,
                min_likes,
            }) => {
                headless::run_search(
                    &query,
                    None, // sort_field
                    min_downloads,
                    min_likes,
                    cli_args.token.as_ref(),
                    &reporter,
                )
                .await
            }
            Some(cli::Commands::Download {
                model_id,
                quantization,
                all,
                output,
            }) => {
                let output_dir = output.unwrap_or_else(|| {
                    let options = config::load_config();
                    options.default_directory
                });

                if cli_args.dry_run {
                    headless::run_download_dry_run(
                        &model_id,
                        quantization.as_deref(),
                        all,
                        &output_dir,
                        cli_args.token,
                        &reporter,
                    )
                    .await
                } else {
                    headless::run_download(
                        &model_id,
                        quantization.as_deref(),
                        all,
                        &output_dir,
                        cli_args.token,
                        &reporter,
                        download_tx,
                        progress_tx,
                        download_queue,
                        download_progress,
                        verification_queue_size,
                        verification_progress,
                        shutdown_signal,
                    )
                    .await
                }
            }
            Some(cli::Commands::List { model_id }) => {
                headless::run_list(&model_id, cli_args.token.as_ref(), &reporter).await
            }
            Some(cli::Commands::Resume) => {
                headless::run_resume(
                    &reporter,
                    download_tx,
                    progress_tx,
                    download_queue,
                    download_progress,
                    verification_queue_size,
                    verification_progress,
                    shutdown_signal,
                )
                .await
            }
            None => {
                eprintln!("Error: No command specified");
                std::process::exit(headless::EXIT_INVALID_ARGS);
            }
        };

        match result {
            Ok(_) => std::process::exit(headless::EXIT_SUCCESS),
            Err(e) => {
                reporter.report_error(&e.to_string());
                std::process::exit(e.exit_code());
            }
        }
    }

    // Original TUI flow (unchanged)
    // Enable mouse capture for the terminal
    use crossterm::event::EnableMouseCapture;
    use crossterm::execute;
    use std::io::stdout;
    execute!(stdout(), EnableMouseCapture)?;

    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();

    // Disable mouse capture when exiting
    use crossterm::event::DisableMouseCapture;
    execute!(stdout(), DisableMouseCapture)?;

    result
}
