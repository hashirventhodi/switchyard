//! The desktop shell. All behaviour lives in switchyard-core; this file only bridges the
//! core's command/event stream onto Tauri IPC: one `dispatch` command in, every core event
//! out on `core://event`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{mpsc, Mutex};

use switchyard_core::Core;
use switchyard_schema::Command;
use tauri::{Emitter, Manager, State};

const CORE_EVENT: &str = "core://event";

#[tauri::command]
fn dispatch(command: Command, core: State<'_, Mutex<Core>>) {
    core.lock().expect("core lock").dispatch(command);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let (tx, rx) = mpsc::channel();
            let core = Core::open(&data_dir, tx).map_err(|e| e.0)?;
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
