#![forbid(unsafe_code)]
// A GUI app must not flash a console window on Windows. Debug builds keep the
// console so logs are visible while developing; release builds re-attach to
// the parent console when run from a terminal (see `attach_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Perch's entry point: parse the command line, enforce a single instance,
//! forward CLI verbs to a running instance, or start the event loop.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use perch_app::cli::{self, Command};
use perch_app::glue::Services;
use perch_app::shell::{Shell, UserEvent};
use perch_core::config::ConfigStore;
use perch_core::platform::{EventSink, PlatformEvent, SingleInstance};
use winit::event_loop::EventLoop;

fn main() -> ExitCode {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Hidden helper for the packaging scripts: render the app icon to a PNG.
    if args.first().map(String::as_str) == Some("--export-icon") {
        let path = args.get(1).map(String::as_str).unwrap_or("perch.png");
        let size = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
        return match perch_app::icon::export_png(std::path::Path::new(path), size) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("perch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let invocation = cli::parse(&args);
    let gui = invocation.command == Command::Run;
    // Only short-lived CLI invocations talk to the parent terminal. The
    // long-running instance must never write to a console or pipe that
    // nobody drains: a blocked write would freeze the UI thread.
    if !gui {
        attach_console();
    }
    for unknown in &invocation.unknown {
        eprintln!("perch: ignoring unrecognised argument {unknown}");
    }
    init_logging(invocation.verbose, gui);

    match invocation.command {
        Command::Help => {
            print!("{}", cli::usage());
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!(
                "{} {} ({} renderer)",
                perch_core::APP_NAME,
                env!("CARGO_PKG_VERSION"),
                perch_ui::RENDERER
            );
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let store = match &invocation.config {
        Some(path) => ConfigStore::at(path),
        None => match ConfigStore::discover() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("perch: {e}");
                return ExitCode::FAILURE;
            }
        },
    };

    let mut instance = new_single_instance();

    // A forwardable verb goes to the running instance; if there is none, the
    // user gets a clear message rather than a second notch.
    if let Some(verb) = invocation.command.ipc_verb() {
        return match instance.forward(&verb) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("perch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    match instance.acquire() {
        Ok(true) => {}
        Ok(false) => {
            // Already running: surface the notch instead of starting twice.
            let _ = instance.forward("toggle-notch");
            eprintln!("perch: already running; toggled the existing notch");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("perch: cannot determine whether Perch is already running: {e}");
            return ExitCode::FAILURE;
        }
    }

    let (cfg, problem) = store.load_or_default();
    if let Some(p) = problem {
        log::warn!("config problem, using defaults where needed: {p}");
    }

    let event_loop = match build_event_loop() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("perch: cannot create an event loop: {e}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let sink_proxy = proxy.clone();
    // Every backend thread delivers through this; sending wakes the loop.
    let sink: EventSink = Arc::new(move |ev: PlatformEvent| {
        let _ = sink_proxy.send_event(UserEvent::Platform(ev));
    });

    if let Err(e) = serve_ipc(&mut instance, sink.clone()) {
        log::warn!("CLI commands will be unavailable: {e}");
    }

    let services = Services::start(sink);
    let mut shell = Shell::new(cfg, store, services, proxy, started);
    match event_loop.run_app(&mut shell) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("perch: event loop failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_event_loop() -> Result<EventLoop<UserEvent>, winit::error::EventLoopError> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    {
        // On Wayland sessions without wlr-layer-shell support, run through
        // XWayland so the dock/above/sticky hints still apply (documented
        // limitation; see README). `PERCH_BACKEND=wayland` opts out.
        use winit::platform::x11::EventLoopBuilderExtX11;
        let session = perch_platform_linux::detect_session();
        log::info!(
            "session: {session:?}; {}",
            perch_platform_linux::session::support_note(session)
        );
        let force_wayland = std::env::var("PERCH_BACKEND").ok().as_deref() == Some("wayland");
        let has_display = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
        if !force_wayland && has_display {
            builder.with_x11();
        }
    }
    builder.build()
}

fn init_logging(verbose: bool, gui: bool) {
    use std::io::IsTerminal;
    let default = if verbose { "debug" } else { "info" };
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default));
    builder.format_timestamp_millis();
    let to_stderr = !gui
        || std::io::stderr().is_terminal()
        || std::env::var("PERCH_LOG").ok().as_deref() == Some("stderr");
    if !to_stderr {
        if let Some(w) = perch_app::persist::log_file() {
            builder.target(env_logger::Target::Pipe(Box::new(w)));
        }
    }
    builder.init();
}

/// Release builds use the GUI subsystem; when started from a terminal, attach
/// to its console so `perch --help` and CLI errors are visible.
fn attach_console() {
    #[cfg(windows)]
    perch_platform_windows::services::attach_parent_console();
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
    sink: EventSink,
) -> perch_core::Result<()> {
    instance.serve(sink)
}

#[cfg(target_os = "linux")]
fn serve_ipc(
    instance: &mut perch_platform_linux::UnixSocketInstance,
    sink: EventSink,
) -> perch_core::Result<()> {
    instance.serve(sink)
}
