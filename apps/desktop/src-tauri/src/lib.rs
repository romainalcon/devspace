//! DevSpace desktop application shell.
//!
//! **Status (M0):** this is the unmodified Tauri + Vue starter (a `greet`
//! demo command). The graphical interface is developed in a later milestone;
//! it will never embed the core engine and will talk to the daemon through
//! `devspace-ipc` only.

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}

/// Builds and runs the Tauri application until the last window closes.
///
/// # Errors
///
/// Returns the Tauri error if the application fails to start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
}
