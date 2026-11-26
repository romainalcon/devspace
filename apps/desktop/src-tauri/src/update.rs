//! Updates of the desktop application (`tauri-plugin-updater`).
//!
//! The update manifest and bundles are published with the GitHub releases;
//! the updater verifies the minisign signature of every bundle with the
//! public key of `tauri.conf.json` before installing it. Installing stops
//! the daemon first (it runs from the bundle being replaced), then restarts
//! the application, which starts the new daemon.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::daemon::{Daemon, DaemonView, DesktopError};

/// An available update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    /// Version of the running application.
    pub current_version: String,
    /// Version offered.
    pub version: String,
    /// Release notes.
    pub notes: Option<String>,
}

fn updater_error(err: impl std::fmt::Display) -> DesktopError {
    DesktopError::Shell(format!("update failed: {err}"))
}

/// Checks the release manifest; `None` when the application is up to date.
///
/// # Errors
///
/// [`DesktopError::Shell`] if the manifest cannot be fetched or verified.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, DesktopError> {
    let update = app
        .updater()
        .map_err(updater_error)?
        .check()
        .await
        .map_err(updater_error)?;
    Ok(update.map(|update| UpdateInfo {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        notes: update.body.clone(),
    }))
}

/// Downloads, verifies and installs the update, stopping the daemon first,
/// then restarts the application. Returns only on failure.
///
/// # Errors
///
/// [`DesktopError::Shell`] if there is no update any more or it cannot be
/// installed; the daemon errors if it cannot be stopped.
pub async fn install(app: &AppHandle, daemon: &Daemon) -> Result<(), DesktopError> {
    let update = app
        .updater()
        .map_err(updater_error)?
        .check()
        .await
        .map_err(updater_error)?
        .ok_or_else(|| DesktopError::Shell("no update is available any more".to_owned()))?;
    // Downloaded and verified before anything stops.
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(updater_error)?;
    if matches!(daemon.view().await, DaemonView::Running { .. }) {
        daemon.stop().await?;
    }
    tracing::info!(version = %update.version, "installing the update");
    update.install(bytes).map_err(updater_error)?;
    app.restart();
}
