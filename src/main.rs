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
    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();
    result
}
