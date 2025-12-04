mod models;
mod api;
mod config;
mod download;
mod verification;
mod registry;
mod ui;
mod utils;
mod http_client;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
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
