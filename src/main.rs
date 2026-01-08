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

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Parse CLI arguments
    use clap::Parser;
    let cli_args = cli::Cli::parse();

    // If --headless flag is present, run in headless mode
    if cli_args.headless {
        // TODO: Implement headless mode in Phase 2
        eprintln!("Headless mode is not yet implemented!");
        eprintln!();
        eprintln!("This feature is planned for implementation. See plans/ directory for details.");
        eprintln!();
        eprintln!("Available commands will include:");
        eprintln!("  search  - Search for models");
        eprintln!("  download - Download a model");
        eprintln!("  list    - List available files");
        eprintln!("  resume  - Resume incomplete downloads");
        return Ok(());
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
