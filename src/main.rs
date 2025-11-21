mod models;
mod api;
mod download;
mod registry;
mod ui;
mod utils;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();
    result
}
