mod application;
mod domain;
mod error;
mod infrastructure;
mod ui;

use std::env;

use application::AppCore;
use error::{AppError, Result};
use infrastructure::ConfigRepository;
use reqwest::Client;
use ui::run_tui;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {}", error);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cwd = env::current_dir().map_err(AppError::CurrentDir)?;
    let config = ConfigRepository::load()?;
    let mut core = AppCore::new(cwd, config, Client::new());
    core.vram_mb = infrastructure::detect_vram();
    run_tui(core).await
}
