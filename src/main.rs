mod models;
mod api;
mod config;
mod download;
mod verification;
mod registry;
mod ui;
mod utils;
mod http_client;
mod rate_limiter;
mod cli;
mod headless;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Parse CLI arguments
    use clap::Parser;
    let cli_args = cli::Cli::parse();

    // If --headless flag is present, run in headless mode
    if cli_args.headless {
        let json_mode = cli_args.json;
        let reporter = headless::ProgressReporter::new(json_mode);

        // Create channels for download manager
        let (download_tx, download_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let download_rx = std::sync::Arc::new(tokio::sync::Mutex::new(download_rx));

        // Spawn download manager task
        let download_progress = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let complete_downloads = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let verification_queue = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let verification_queue_size = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let download_queue_size = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let download_queue_bytes = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let download_registry = std::sync::Arc::new(tokio::sync::Mutex::new(crate::models::DownloadRegistry::default()));

        // Clone Arcs for the download manager task
        let download_progress_clone = download_progress.clone();
        let complete_downloads_clone = complete_downloads.clone();
        let verification_queue_clone = verification_queue.clone();
        let verification_queue_size_clone = verification_queue_size.clone();
        let download_queue_size_clone = download_queue_size.clone();
        let download_queue_bytes_clone = download_queue_bytes.clone();
        let progress_tx_clone = progress_tx.clone();
        let _download_registry_clone = download_registry.clone();

        tokio::spawn(async move {
            use crate::download::DownloadParams;

            let mut rx = download_rx.lock().await;
            while let Some((model_id, filename, path, sha256, hf_token, total_size)) = rx.recv().await {
                // Update queue size
                {
                    let mut size = download_queue_size_clone.lock().await;
                    *size += 1;
                }
                {
                    let mut bytes = download_queue_bytes_clone.lock().await;
                    *bytes += total_size;
                }

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

                tokio::spawn(async move {
                    download::start_download(params).await;
                });
            }
        });

        // Spawn progress reporter task
        tokio::spawn(async move {
            while let Some(msg) = progress_rx.recv().await {
                eprintln!("{}", msg);
            }
        });

        // Execute command
        let result = match cli_args.command {
            Some(cli::Commands::Search { query, sort: _, min_downloads, min_likes }) => {
                let models = headless::search_models(
                    &query,
                    None, // sort
                    None, // direction
                    min_downloads,
                    min_likes,
                    cli_args.token.as_ref(),
                ).await?;
                reporter.report_search(&models);
                Ok(())
            }
            Some(cli::Commands::Download { model_id, quantization, all, output }) => {
                let output_dir = output.unwrap_or_else(|| {
                    let options = config::load_config();
                    options.default_directory
                });
                headless::download_model(
                    &model_id,
                    quantization.as_deref(),
                    all,
                    &output_dir,
                    cli_args.token,
                    progress_tx,
                    download_tx,
                ).await
            }
            Some(cli::Commands::List { model_id }) => {
                let (quantizations, metadata) = headless::list_quantizations(
                    &model_id,
                    cli_args.token.as_ref(),
                ).await?;
                reporter.report_list_quantizations(&quantizations, &metadata);
                Ok(())
            }
            Some(cli::Commands::Resume) => {
                let resumed = headless::resume_downloads(
                    download_tx,
                    progress_tx,
                ).await?;
                reporter.report_resume(&resumed);
                Ok(())
            }
            None => {
                eprintln!("Error: No command specified");
                std::process::exit(1);
            }
        };

        match result {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                reporter.report_error(&e.to_string());
                std::process::exit(1);
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
