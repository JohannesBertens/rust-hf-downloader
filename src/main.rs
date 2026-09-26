mod api;
mod cli;
mod config;
mod download;
mod engine;
mod http_client;
mod models;
mod paths;
mod rate_limiter;
mod registry;
mod ui;
mod utils;
mod verification;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // CLI mode: a `download` subcommand runs headless; no arguments (or a
    // help/version flag) never enters the terminal setup below.
    use clap::Parser;
    match cli::Cli::try_parse() {
        Ok(parsed) => match parsed.command {
            Some(command) => {
                let code = cli::run(command).await;
                // Explicit flush: stdout is block-buffered when piped, and
                // process::exit does not run destructors.
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                std::process::exit(code);
            }
            None => tui_main().await,
        },
        Err(e) => {
            // Help/version go to stdout with code 0; usage errors get our
            // EX_USAGE code so exit code 2 stays reserved for auth.
            let _ = e.print();
            std::process::exit(if e.use_stderr() { cli::EXIT_USAGE } else { 0 });
        }
    }
}

async fn tui_main() -> color_eyre::Result<()> {
    color_eyre::install()?;

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
