//! The desktop shell. All behaviour lives in switchyard-core; this file only bridges the
//! core's command/event stream onto Tauri IPC: one `dispatch` command in, every core event
//! out on `core://event`. It also tells the core where the Claude Code sidecar and the
//! `codex` executable live, and
//! opens the core's install and update links in the user's browser (`open_link`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use switchyard_core::agents::ByAgent;
use switchyard_core::codex::CodexLauncher;
use switchyard_core::compat::SystemProbe;
use switchyard_core::forge::GhForge;
use switchyard_core::sidecar::SidecarLauncher;
use switchyard_core::processes::SystemProcesses;
use switchyard_core::secrets::SystemSecrets;
use switchyard_core::watcher::{FsWatcher, FALLBACK_EVERY};
use switchyard_core::{default_export_dir, default_worktree_root, system_now_ms, Core, Runtime};
use switchyard_schema::Command;
use tauri::path::BaseDirectory;
use tauri::{App, Emitter, Manager, State};

const CORE_EVENT: &str = "core://event";
const SIDECAR: &str = "claude-sidecar.mjs";
/// One Forge poll tick; a pull request with no news is polled every 1, 2, 4 then 8 ticks.
const FORGE_POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// Commands go to the core off the main thread, in the order the renderer sent them, on
/// one worker; those that wait on the network (pull requests, issue sync, gh detection)
/// get a thread of their own so they never hold up the rest.
struct Dispatcher {
    core: Arc<Core>,
    queue: Mutex<mpsc::Sender<Command>>,
}

impl Dispatcher {
    fn new(core: Core) -> Dispatcher {
        let core = Arc::new(core);
        let (tx, rx) = mpsc::channel::<Command>();
        let worker = core.clone();
        std::thread::spawn(move || {
            for command in rx {
                worker.dispatch(command);
            }
        });
        Dispatcher { core, queue: Mutex::new(tx) }
    }

    fn send(&self, command: Command) {
        let slow = matches!(
            command,
            Command::OpenPullRequest { .. }
                | Command::PruneMerged { .. }
                | Command::SyncTasks { .. }
                | Command::CheckForge
                | Command::PollForgeNow
                | Command::WindowVisibility { .. }
        );
        if slow {
            let core = self.core.clone();
            std::thread::spawn(move || core.dispatch(command));
        } else {
            let _ = self.queue.lock().expect("dispatch queue").send(command);
        }
    }
}

#[tauri::command]
fn dispatch(command: Command, dispatcher: State<'_, Dispatcher>) {
    dispatcher.send(command);
}

/// Open an install or update link in the default browser. Only https links: the renderer
/// passes links the core reported, never arbitrary targets.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("not an https link: {url}"));
    }
    std::process::Command::new("open").arg(&url).spawn().map(|_| ()).map_err(|e| e.to_string())
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

/// `codex app-server`: `SWITCHYARD_CODEX`, else the first `codex` in the probe's search
/// directories (PATH plus the usual install places). `SWITCHYARD_RECORD_DIR` records raw
/// JSON-RPC traffic for new conformance fixtures.
fn codex(search_dirs: &[PathBuf]) -> CodexLauncher {
    let codex = std::env::var_os("SWITCHYARD_CODEX").map(PathBuf::from).unwrap_or_else(|| {
        search_dirs.iter().map(|d| d.join("codex")).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("codex"))
    });
    CodexLauncher { codex, record_dir: std::env::var_os("SWITCHYARD_RECORD_DIR").map(PathBuf::from) }
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
            let launcher = sidecar(app);
            let search_dirs = SystemProbe::default_search_dirs();
            let codex = codex(&search_dirs);
            let probe = SystemProbe {
                search_dirs,
                node: launcher.node.clone(),
                sidecar: launcher.script.clone(),
            };
            let runtime = Runtime {
                agents: Arc::new(ByAgent { claude_code: Arc::new(launcher), codex: Arc::new(codex) }),
                now_ms: system_now_ms,
                probe: Arc::new(probe),
                worktree_root: default_worktree_root(),
                processes: Arc::new(SystemProcesses),
                export_dir: default_export_dir(),
                secrets: Arc::new(SystemSecrets),
                forge: Arc::new(GhForge::find()),
                forge_poll_every: Some(FORGE_POLL_EVERY),
                claude_home: switchyard_core::context::default_claude_home(),
                sandbox_evidence: switchyard_core::detect_sandbox,
                watcher: Arc::new(FsWatcher::new()),
                unwatched_every: Some(FALLBACK_EVERY),
            };
            let core = Core::open_with(&data_dir, tx, runtime).map_err(|e| e.0)?;
            app.manage(Dispatcher::new(core));
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for event in rx {
                    let _ = handle.emit(CORE_EVENT, event);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![dispatch, open_link])
        .run(tauri::generate_context!())
        .expect("error while running Switchyard");
}
