//! DevSpace desktop entry point.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

/// Overrides the log filter, as for the daemon and the CLI.
const LOG_ENV: &str = "DEVSPACE_LOG";

fn main() -> ExitCode {
    let filter =
        EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info,tao=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match devspace_desktop_lib::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "devspace-desktop failed");
            eprintln!("devspace-desktop: {err}");
            ExitCode::FAILURE
        }
    }
}
