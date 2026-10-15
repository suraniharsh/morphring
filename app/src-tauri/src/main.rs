// Release builds on Windows would otherwise pop a console next to the window.
// Harmless on Linux, and it saves future me a confused afternoon in v0.4.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // Same deal as the CLI: logs go to stderr, quiet by default, RUST_LOG turns them up.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    morphring_app::run()
}
