//! Pyro Porch: a lean, secure, config-driven button deck. A play on the idea of a deck by a stream:
//! a porch, with a fire.
//!
//! `pyroporch`                         opens the app
//! `pyroporch mcp`                     runs the built-in MCP server for an AI assistant
//! `pyroporch --screenshot <file.png>` draws 120 frames, saves a picture and exits
//! `pyroporch --quit`                  closes a copy running in the background

// No console window on Windows. Pipes to the MCP server still work without one.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod hardware;
mod icon;
mod instance;
#[cfg(target_os = "macos")]
mod macos;
mod tray;
mod ui;

use instance::Gate;
use pyroporch_core::config::Workspace;
use pyroporch_core::engine::Engine;
use pyroporch_core::mcp::McpServer;
use pyroporch_core::paths::Paths;
use pyroporch_core::sample_plugin;
use pyroporch_core::security::harden_process;
use pyroporch_core::vault::Vault;
use pyroporch_sdk::Plugin;
use std::path::PathBuf;
use std::sync::Arc;

/// The modules compiled into this build.
#[allow(clippy::vec_init_then_push, unused_mut)] // Which pushes exist depends on the features.
fn natives() -> Vec<Arc<dyn Plugin>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
    #[cfg(feature = "desktop-actions")]
    plugins.push(Arc::new(pyroporch_desktop::DesktopActions));
    plugins
}

fn start_core() -> Result<(Workspace, Arc<Engine>), String> {
    let paths = Paths::from_env();
    let mut ws = Workspace::load_with(paths.clone(), cfg!(feature = "desktop-actions"))?;
    sample_plugin::sync(&mut ws)?;
    let vault = Vault::open(paths.vault())?;
    let vault = demo_vault(vault);
    Ok((ws, Arc::new(Engine::new(paths, vault, natives()))))
}

/// Debug builds only: opens a vault for checking the vault screen in a picture. It needs the real
/// passphrase, so it gets around nothing, and release builds don't contain it.
#[cfg(debug_assertions)]
fn demo_vault(mut vault: Vault) -> Vault {
    use pyroporch_core::vault::VaultState;
    if let Ok(passphrase) = std::env::var("PYROPORCH_DEMO_PASSPHRASE") {
        let opened = match vault.state() {
            VaultState::NotCreated => vault.create(&passphrase),
            _ => vault.unlock(&passphrase),
        };
        if opened.is_ok() && vault.list().is_empty() {
            let _ = vault.set("demo_token", "not-a-real-value", &["api.example.com".into()]);
        }
    }
    vault
}

#[cfg(not(debug_assertions))]
fn demo_vault(vault: Vault) -> Vault {
    vault
}

fn run_mcp() -> Result<(), String> {
    let paths = Paths::from_env();
    let mut ws = Workspace::load_with(paths.clone(), cfg!(feature = "desktop-actions"))?;
    sample_plugin::sync(&mut ws)?;
    let vault = Vault::open(paths.vault())?;
    let engine = Engine::new(paths, vault, natives());
    let stdin = std::io::stdin();
    McpServer::new(ws, engine)
        .serve(stdin.lock(), std::io::stdout().lock())
        .map_err(|e| e.to_string())
}

fn window_icon() -> eframe::egui::IconData {
    const SIZE: u32 = 256;
    eframe::egui::IconData {
        rgba: icon::render(SIZE).into_raw(),
        width: SIZE,
        height: SIZE,
    }
}

fn run_app(screenshot: Option<PathBuf>, view: Option<String>) -> Result<(), String> {
    // Only a normal start takes the lock: a screenshot doesn't touch a copy that's running.
    let lock = if screenshot.is_none() {
        match instance::acquire(&Paths::from_env(), env!("PYROPORCH_BUILD"))? {
            Gate::First(lock) => Some(lock),
            Gate::AlreadyRunning => return Ok(()),
        }
    } else {
        None
    };
    let (ws, engine) = start_core()?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Pyro Porch")
            .with_app_id("pyroporch")
            .with_icon(window_icon())
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pyro Porch",
        options,
        Box::new(move |cc| {
            let mut app = ui::App::new(&cc.egui_ctx, ws, engine, ui::Launch { screenshot, lock });
            if let Some(view) = view {
                app.start_on(&view);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

const HELP: &str = "Pyro Porch: a lean, secure, config-driven button deck

  pyroporch                          open the app
  pyroporch mcp                      run the MCP server for an AI assistant (stdin/stdout)
  pyroporch --screenshot <file.png>  draw 120 frames, save a picture of the window and exit
  pyroporch --view <screen>          start on plugins, vault, assistant or settings
  pyroporch --quit                   close a copy running in the background
  pyroporch --list-decks             list connected decks and their key sizes
  pyroporch --version                show the version

Config lives in the platform's config folder, or in PYROPORCH_HOME if set.";

fn main() {
    harden_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--view` can go with the other options, so it's taken out first.
    let mut args = args;
    let view = args.iter().position(|a| a == "--view").and_then(|at| {
        args.remove(at);
        (at < args.len()).then(|| args.remove(at))
    });
    // On a Mac a normal start goes through the app bundle, so the system sees Pyro Porch.
    #[cfg(target_os = "macos")]
    if args.is_empty()
        && let Some(code) = macos::relaunch_in_bundle(&view.iter().flat_map(|v| ["--view".to_string(), v.clone()]).collect::<Vec<_>>())
    {
        std::process::exit(code);
    }
    let result = match args.first().map(String::as_str) {
        None => run_app(None, view),
        Some("--quit") => instance::request_quit(&Paths::from_env()).map(|running| {
            println!("{}", if running { "Asked Pyro Porch to quit." } else { "Pyro Porch isn't running." });
        }),
        Some("--list-decks") => {
            let decks = hardware::describe_connected();
            if decks.is_empty() {
                println!("No decks found.");
            }
            decks.iter().for_each(|deck| println!("{deck}"));
            Ok(())
        }
        Some("mcp") => run_mcp(),
        Some("--screenshot") => match args.get(1) {
            Some(path) => run_app(Some(PathBuf::from(path)), view),
            None => Err("--screenshot needs a file name".into()),
        },
        Some("--version") => {
            println!("pyroporch {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => Err(format!("unknown option: {other}\n\n{HELP}")),
    };
    if let Err(error) = result {
        eprintln!("pyroporch: {error}");
        std::process::exit(1);
    }
}
