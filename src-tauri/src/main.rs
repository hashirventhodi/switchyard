//! The desktop shell. All behaviour lives in switchyard-core; this file only bridges the
//! core's command/event stream onto Tauri IPC: one `dispatch` command in, every core event
//! out on `core://event`. It also tells the core where the Claude Code sidecar lives.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use switchyard_core::sidecar::SidecarLauncher;
use switchyard_core::{system_now_ms, Core, Runtime};
use switchyard_schema::Command;
use tauri::path::BaseDirectory;
use tauri::{App, Emitter, Manager, State};

const CORE_EVENT: &str = "core://event";
const SIDECAR: &str = "claude-sidecar.mjs";

#[tauri::command]
fn dispatch(command: Command, core: State<'_, Mutex<Core>>) {
    core.lock().expect("core lock").dispatch(command);
}

/// The bundled sidecar script (a resource in builds, `sidecar/dist` in development) and the
/// Node that runs it: `SWITCHYARD_NODE`, else `node` on PATH or in the usual install places.
fn sidecar(app: &App) -> SidecarLauncher {
    let bundled = app.path().resolve(SIDECAR, BaseDirectory::Resource).ok().filter(|p| p.is_file());
    let script = std::env::var_os("SWITCHYARD_SIDECAR")
        .map(PathBuf::from)
        .or(bundled)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../sidecar/dist/claude-sidecar.mjs")));
    let node = std::env::var_os("SWITCHYARD_NODE").map(PathBuf::from).unwrap_or_else(find_node);
    SidecarLauncher { node, script, claude: None }
}

fn find_node() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
        .map(|dir| dir.join("node"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("node"))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let (tx, rx) = mpsc::channel();
            let runtime = Runtime { agents: Arc::new(sidecar(app)), now_ms: system_now_ms, ..Runtime::default() };
            let core = Core::open_with(&data_dir, tx, runtime).map_err(|e| e.0)?;
            app.manage(Mutex::new(core));
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for event in rx {
                    let _ = handle.emit(CORE_EVENT, event);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![dispatch])
        .run(tauri::generate_context!())
        .expect("error while running Switchyard");
}
