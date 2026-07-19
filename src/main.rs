use rust_hf_downloader::{cli, config, headless, runtime, ui};

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

        // Shared download/verification runtime (channels + state + workers).
        let rt = runtime::DownloadRuntime::new();

        // Graceful-shutdown signal shared with the long-running commands.
        let shutdown_signal = std::sync::Arc::new(tokio::sync::Mutex::new(false));
        let shutdown_signal_clone = shutdown_signal.clone();

        // Background workers: verification (SHA256) + concurrent download manager.
        rt.spawn_verification_worker();
        rt.spawn_download_manager_concurrent();

        // Progress reporter: drain the shared status channel to the terminal.
        let status_rx = rt.status_rx.clone();
        tokio::spawn(async move {
            use std::io::Write;
            let mut rx = status_rx.lock().await;
            while let Some(msg) = rx.recv().await {
                if !json_mode {
                    print!("\r\x1b[2K");
                    let _ = std::io::stdout().flush();
                }
                eprintln!("{}", msg);
            }
        });

        // Auth-required reporter: drain the typed auth channel.
        let auth_rx = rt.auth_rx.clone();
        tokio::spawn(async move {
            let mut rx = auth_rx.lock().await;
            while let Some(model_id) = rx.recv().await {
                eprintln!("Authentication required for {}", model_id);
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
                        &rt,
                        shutdown_signal,
                    )
                    .await
                }
            }
            Some(cli::Commands::List { model_id }) => {
                headless::run_list(&model_id, cli_args.token.as_ref(), &reporter).await
            }
            Some(cli::Commands::Resume) => {
                headless::run_resume(&reporter, &rt, shutdown_signal).await
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

    // TUI flow
    use crossterm::event::EnableMouseCapture;
    use crossterm::execute;
    use std::io::stdout;
    execute!(stdout(), EnableMouseCapture)?;

    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();

    use crossterm::event::DisableMouseCapture;
    execute!(stdout(), DisableMouseCapture)?;

    result
}
