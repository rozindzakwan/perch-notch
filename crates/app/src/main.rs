#![forbid(unsafe_code)]
// A GUI app must not flash a console window on Windows. Debug builds keep the
// console so logs are visible while developing.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Perch's entry point: parse the command line, enforce a single instance,
//! forward CLI verbs to a running instance, or start the event loop.

use std::sync::mpsc::channel;

use perch_app::app::PerchApp;
use perch_app::cli::{self, Command};
use perch_core::config::ConfigStore;
use perch_core::platform::SingleInstance;
use winit::event_loop::EventLoop;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = cli::parse(&args);

    for unknown in &invocation.unknown {
        eprintln!("perch: ignoring unrecognised argument {unknown}");
    }

    init_logging(invocation.verbose);

    match invocation.command {
        Command::Help => {
            print!("{}", cli::usage());
            return std::process::ExitCode::SUCCESS;
        }
        Command::Version => {
            println!(
                "{} {} ({} renderer)",
                perch_core::APP_NAME,
                env!("CARGO_PKG_VERSION"),
                perch_ui::RENDERER
            );
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }

    let store = match &invocation.config {
        Some(path) => ConfigStore::at(path),
        None => match ConfigStore::discover() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("perch: {e}");
                return std::process::ExitCode::FAILURE;
            }
        },
    };

    let mut instance = new_single_instance();

    // A forwardable verb goes to the running instance; if there is none, the
    // user gets a clear message rather than a second notch.
    if let Some(verb) = invocation.command.ipc_verb() {
        return match instance.forward(verb) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("perch: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    match instance.acquire() {
        Ok(true) => {}
        Ok(false) => {
            // Already running: surface the notch instead of starting twice.
            let _ = instance.forward("toggle-notch");
            eprintln!("perch: already running; toggled the existing notch");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("perch: cannot determine whether Perch is already running: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }

    let (tx, rx) = channel::<String>();
    if let Err(e) = serve_ipc(&mut instance, tx) {
        log::warn!("CLI commands will be unavailable: {e}");
    }

    let (cfg, problem) = store.load_or_default();
    if let Some(p) = problem {
        log::warn!("config problem, using defaults where needed: {p}");
    }

    #[cfg(windows)]
    perch_platform_windows::WindowsMonitors::enable_per_monitor_dpi_v2();

    let monitors = new_monitor_source();
    let fullscreen = new_fullscreen_watch(&monitors);

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("perch: cannot create an event loop: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut app = PerchApp::new(cfg, store, monitors, fullscreen).with_commands(rx);
    match event_loop.run_app(&mut app) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("perch: event loop failed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn init_logging(verbose: bool) {
    let default = if verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default))
        .format_timestamp_secs()
        .init();
}

#[cfg(windows)]
fn new_single_instance() -> perch_platform_windows::NamedPipeInstance {
    perch_platform_windows::NamedPipeInstance::default()
}

#[cfg(target_os = "linux")]
fn new_single_instance() -> perch_platform_linux::UnixSocketInstance {
    perch_platform_linux::UnixSocketInstance::default()
}

#[cfg(windows)]
fn serve_ipc(
    instance: &mut perch_platform_windows::NamedPipeInstance,
    tx: std::sync::mpsc::Sender<String>,
) -> perch_core::Result<()> {
    instance.serve(tx)
}

#[cfg(target_os = "linux")]
fn serve_ipc(
    instance: &mut perch_platform_linux::UnixSocketInstance,
    tx: std::sync::mpsc::Sender<String>,
) -> perch_core::Result<()> {
    instance.serve(tx)
}

#[cfg(windows)]
fn new_monitor_source() -> perch_platform_windows::WindowsMonitors {
    perch_platform_windows::WindowsMonitors
}

#[cfg(target_os = "linux")]
fn new_monitor_source() -> perch_platform_linux::x11::X11Monitors {
    perch_platform_linux::x11::X11Monitors
}

#[cfg(windows)]
fn new_fullscreen_watch(
    monitors: &perch_platform_windows::WindowsMonitors,
) -> perch_platform_windows::FullscreenDetector {
    use perch_core::platform::MonitorSource;
    let list = monitors.monitors().unwrap_or_default();
    perch_platform_windows::FullscreenDetector::new(list)
}

#[cfg(target_os = "linux")]
fn new_fullscreen_watch(
    monitors: &perch_platform_linux::x11::X11Monitors,
) -> perch_platform_linux::x11::X11Fullscreen {
    use perch_core::platform::MonitorSource;
    let list = monitors.monitors().unwrap_or_default();
    perch_platform_linux::x11::X11Fullscreen::new(list)
}
