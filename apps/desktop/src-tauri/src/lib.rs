//! DevSpace desktop application shell.
//!
//! The shell never embeds the core engine: every operation goes to the
//! daemon through `devspace-ipc` ([`daemon::Daemon`]). It exposes a few
//! commands to the Vue interface, starts the daemon when the application
//! starts, and keeps an icon in the menu bar (notification area) whose menu
//! shows the daemon state. Closing the window hides it; quitting the
//! application leaves the daemon running.

pub mod daemon;

use std::sync::Arc;
use std::time::Duration;

use devspace_ipc::{Request, Response};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent, Wry};
use tauri_plugin_opener::OpenerExt;

use crate::daemon::{Daemon, DaemonView, DesktopError};

/// Label of the main window (`tauri.conf.json`).
const MAIN_WINDOW: &str = "main";

/// Refresh period of the tray menu.
const TRAY_REFRESH: Duration = Duration::from_secs(3);

type Shared<'a> = State<'a, Arc<Daemon>>;

/// State of the daemon.
#[tauri::command]
async fn daemon_status(daemon: Shared<'_>) -> Result<DaemonView, String> {
    Ok(daemon.view().await)
}

/// Starts the daemon unless it runs.
#[tauri::command]
async fn daemon_start(daemon: Shared<'_>) -> Result<DaemonView, String> {
    daemon.start().await.map_err(|err| err.report())
}

/// Stops the daemon.
#[tauri::command]
async fn daemon_stop(daemon: Shared<'_>) -> Result<DaemonView, String> {
    daemon.stop().await.map_err(|err| err.report())
}

/// Sends a request of the IPC protocol to the daemon; error responses are
/// returned as `{"kind": "error", …}`.
#[tauri::command]
async fn daemon_request(daemon: Shared<'_>, request: Request) -> Result<Response, String> {
    daemon.request(request).await.map_err(|err| err.report())
}

/// Opens the site of a project in the default browser. The URL comes from
/// the daemon, never from the interface.
#[tauri::command]
async fn open_site(app: AppHandle, daemon: Shared<'_>, project: String) -> Result<(), String> {
    let url = daemon
        .site_url(&project)
        .await
        .map_err(|err| err.report())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| DesktopError::Shell(err.to_string()).report())
}

/// Shows the directory of a project in the file manager.
#[tauri::command]
async fn reveal_project(app: AppHandle, daemon: Shared<'_>, project: String) -> Result<(), String> {
    let project = daemon.project(&project).await.map_err(|err| err.report())?;
    app.opener()
        .reveal_item_in_dir(&project.path)
        .map_err(|err| DesktopError::Shell(err.to_string()).report())
}

/// Builds and runs the Tauri application until it quits.
///
/// # Errors
///
/// Returns the Tauri error if the application fails to start, and refuses
/// to run with elevated privileges or without the DevSpace directories.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    if devspace_platform::privileges::is_elevated()? {
        return Err(
            "DevSpace must not run with elevated privileges (root or administrator); \
                    run it as a regular user"
                .into(),
        );
    }
    let daemon = Arc::new(Daemon::new().map_err(|err| err.report())?);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::clone(&daemon))
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_start,
            daemon_stop,
            daemon_request,
            open_site,
            reveal_project,
        ])
        .setup(move |app| {
            setup_tray(app.handle(), Arc::clone(&daemon))?;
            // The interface shows "starting" meanwhile, then the result.
            let autostart = Arc::clone(&daemon);
            tauri::async_runtime::spawn(async move {
                // A failure is kept in the state and shown by the interface.
                let _ = autostart.start().await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the application in the menu bar.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                api.prevent_close();
                if let Err(err) = window.hide() {
                    tracing::warn!(error = %err, "failed to hide the window");
                }
            }
        })
        .build(tauri::generate_context!())?;

    app.run(|app, event| {
        // macOS: a click on the Dock icon brings the hidden window back.
        if let RunEvent::Reopen { .. } = event {
            show_main_window(app);
        }
    });
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    for result in [window.unminimize(), window.show(), window.set_focus()] {
        if let Err(err) = result {
            tracing::warn!(error = %err, "failed to show the window");
        }
    }
}

/// Menu of the tray icon, refreshed every [`TRAY_REFRESH`].
struct TrayMenu {
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
}

impl TrayMenu {
    fn update(&self, view: &DaemonView) {
        let (status, toggle, enabled) = match view {
            DaemonView::Running { status } => (
                format!("Daemon running (pid {})", status.pid),
                "Stop daemon",
                true,
            ),
            DaemonView::Stopped { last_error: None } => {
                ("Daemon stopped".to_owned(), "Start daemon", true)
            }
            DaemonView::Stopped {
                last_error: Some(_),
            } => ("Daemon failed to start".to_owned(), "Start daemon", true),
            DaemonView::Starting => ("Daemon starting…".to_owned(), "Start daemon", false),
            DaemonView::Stopping => ("Daemon stopping…".to_owned(), "Stop daemon", false),
            DaemonView::Unknown { .. } => {
                ("Daemon not responding".to_owned(), "Start daemon", false)
            }
        };
        for result in [
            self.status.set_text(status),
            self.toggle.set_text(toggle),
            self.toggle.set_enabled(enabled),
        ] {
            if let Err(err) = result {
                tracing::warn!(error = %err, "failed to update the tray menu");
            }
        }
    }
}

fn setup_tray(app: &AppHandle, daemon: Arc<Daemon>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open DevSpace", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Daemon…", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Start daemon", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit DevSpace", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let tray_menu = Arc::new(TrayMenu { status, toggle });

    let mut builder = TrayIconBuilder::with_id("devspace")
        .tooltip("DevSpace")
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let on_menu = {
        let daemon = Arc::clone(&daemon);
        let tray_menu = Arc::clone(&tray_menu);
        move |app: &AppHandle, event: tauri::menu::MenuEvent| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            "toggle" => {
                let daemon = Arc::clone(&daemon);
                let tray_menu = Arc::clone(&tray_menu);
                tauri::async_runtime::spawn(async move {
                    let result = match daemon.view().await {
                        DaemonView::Running { .. } => daemon.stop().await,
                        _ => daemon.start().await,
                    };
                    if let Err(err) = result {
                        tracing::warn!(error = %err.report(), "daemon control from the tray failed");
                    }
                    tray_menu.update(&daemon.view().await);
                });
            }
            _ => {}
        }
    };
    builder.on_menu_event(on_menu).build(app)?;

    tauri::async_runtime::spawn(async move {
        loop {
            tray_menu.update(&daemon.view().await);
            tokio::time::sleep(TRAY_REFRESH).await;
        }
    });
    Ok(())
}
