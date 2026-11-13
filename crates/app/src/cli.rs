//! Command-line parsing, hand-rolled so the binary pulls in no argument
//! parser (binary size is a hard requirement).
//!
//! Every command except `run` is forwarded over single-instance IPC to the
//! running Perch, which is what lets users bind keys in Cinnamon's Keyboard
//! settings when native hotkeys are unavailable.

/// What the user asked for on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start (or focus) the notch. The default.
    Run,
    /// Toggle the notch open/closed.
    ToggleNotch,
    /// Take a screenshot.
    Shot,
    /// Start push-to-talk dictation.
    Dictate,
    /// Start/stop the Pomodoro.
    Pomodoro,
    /// Open the settings window.
    Settings,
    /// Ask the running instance to quit.
    Quit,
    /// Print version information and exit.
    Version,
    /// Print usage and exit.
    Help,
}

impl Command {
    /// The IPC verb for commands that are forwarded to a running instance.
    pub fn ipc_verb(&self) -> Option<&'static str> {
        match self {
            Command::ToggleNotch => Some("toggle-notch"),
            Command::Shot => Some("shot"),
            Command::Dictate => Some("dictate"),
            Command::Pomodoro => Some("pomodoro"),
            Command::Settings => Some("settings"),
            Command::Quit => Some("quit"),
            Command::Run | Command::Version | Command::Help => None,
        }
    }
}

/// How the command line was interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The command to carry out.
    pub command: Command,
    /// An explicit config path from `--config`.
    pub config: Option<String>,
    /// Extra diagnostics (`--verbose`).
    pub verbose: bool,
    /// Unrecognised arguments, reported rather than silently ignored.
    pub unknown: Vec<String>,
}

/// Parse an argument list (excluding the program name).
pub fn parse<I, S>(args: I) -> Invocation
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut command = None;
    let mut config = None;
    let mut verbose = false;
    let mut unknown = Vec::new();
    let mut iter = args.into_iter().peekable();

    while let Some(raw) = iter.next() {
        let arg = raw.as_ref();
        match arg {
            "run" => command = command.or(Some(Command::Run)),
            "toggle-notch" | "toggle" => command = command.or(Some(Command::ToggleNotch)),
            "shot" | "screenshot" => command = command.or(Some(Command::Shot)),
            "dictate" => command = command.or(Some(Command::Dictate)),
            "pomodoro" => command = command.or(Some(Command::Pomodoro)),
            "settings" => command = command.or(Some(Command::Settings)),
            "quit" | "exit" => command = command.or(Some(Command::Quit)),
            "-V" | "--version" | "version" => command = Some(Command::Version),
            "-h" | "--help" | "help" => command = Some(Command::Help),
            "-v" | "--verbose" => verbose = true,
            "--config" => {
                config = iter.next().map(|s| s.as_ref().to_string());
                if config.is_none() {
                    unknown.push("--config (missing a path)".to_string());
                }
            }
            other if other.starts_with("--config=") => {
                config = Some(other.trim_start_matches("--config=").to_string());
            }
            other => unknown.push(other.to_string()),
        }
    }

    Invocation {
        command: command.unwrap_or(Command::Run),
        config,
        verbose,
        unknown,
    }
}

/// The usage text, also used by the `.deb`'s man-page-less `--help`.
pub fn usage() -> String {
    format!(
        "{name} {version}\n\
         A lightweight dynamic notch for Windows and Linux.\n\
         \n\
         USAGE:\n\
         \u{20}   perch [COMMAND] [OPTIONS]\n\
         \n\
         COMMANDS:\n\
         \u{20}   run             Start Perch (default)\n\
         \u{20}   toggle-notch    Open/close the notch in the running instance\n\
         \u{20}   shot            Take a screenshot\n\
         \u{20}   dictate         Start push-to-talk dictation\n\
         \u{20}   pomodoro        Start or stop the Pomodoro timer\n\
         \u{20}   settings        Open the settings window\n\
         \u{20}   quit            Ask the running instance to exit\n\
         \n\
         OPTIONS:\n\
         \u{20}   --config <PATH> Use a specific config file\n\
         \u{20}   -v, --verbose   Verbose logging\n\
         \u{20}   -V, --version   Print the version\n\
         \u{20}   -h, --help      Print this help\n\
         \n\
         Bind the subcommands to keys in your desktop's keyboard settings if\n\
         native global hotkeys are unavailable.\n",
        name = perch_core::APP_NAME,
        version = env!("CARGO_PKG_VERSION"),
    )
}
