//! Zoft - Professional Digital Audio Workstation

use daw_core::*;
use daw_engine::*;
use daw_ui::*;
use daw_plugins::*;
use anyhow::Result;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

fn main() -> Result<()> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    
    info!("Starting Zoft DAW v{}", env!("CARGO_PKG_VERSION"));
    
    // Initialize project
    let project = Project::new("Untitled");
    info!("Created project: {}", project.name);
    
    // Initialize audio engine
    let engine = AudioEngine::new()?;
    info!("Audio engine initialized");
    
    // Run UI
    let app = App::new(project, engine)?;
    app.run()?;
    
    info!("Shutting down Zoft");
    Ok(())
}