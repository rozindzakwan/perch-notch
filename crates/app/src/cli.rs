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
    /// Pin/unpin the notch open.
    Pin,
    /// Re-read the config file.
    Reload,
    /// Add a to-do item.
    Todo(String),
    /// Media transport: `play-pause`, `next` or `previous`.
    Media(String),
    /// Switch the expanded notch to a tab.
    Tab(String),
    /// Print version information and exit.
    Version,
    /// Print usage and exit.
    Help,
}

impl Command {
    /// The IPC verb for commands that are forwarded to a running instance.
    pub fn ipc_verb(&self) -> Option<String> {
        let s = |v: &str| Some(v.to_string());
        match self {
            Command::ToggleNotch => s("toggle-notch"),
            Command::Shot => s("shot"),
            Command::Dictate => s("dictate"),
            Command::Pomodoro => s("pomodoro"),
            Command::Settings => s("settings"),
            Command::Quit => s("quit"),
            Command::Pin => s("pin"),
            Command::Reload => s("reload"),
            // Newlines would split one command into several on the wire.
            Command::Todo(t) => Some(format!("todo {}", t.replace(['\n', '\r'], " "))),
            Command::Media(v) => Some(v.clone()),
            Command::Tab(t) => Some(format!("tab {t}")),
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
            "pin" => command = command.or(Some(Command::Pin)),
            "reload" => command = command.or(Some(Command::Reload)),
            "play-pause" | "next" | "previous" => {
                command = command.or(Some(Command::Media(arg.to_string())))
            }
            "todo" | "tab" if command.is_none() => {
                // Everything after the verb (up to the next option) is the text.
                let mut words = Vec::new();
                while let Some(next) = iter.peek() {
                    if next.as_ref().starts_with('-') {
                        break;
                    }
                    if let Some(w) = iter.next() {
                        words.push(w.as_ref().to_string());
                    }
                }
                let text = words.join(" ");
                if text.trim().is_empty() {
                    unknown.push(format!("{arg} (missing text)"));
                } else if arg == "todo" {
                    command = Some(Command::Todo(text));
                } else {
                    command = Some(Command::Tab(text));
                }
            }
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
         \u{20}   pin             Keep the notch open until pinned again\n\
         \u{20}   todo <TEXT>     Add a to-do item\n\
         \u{20}   play-pause, next, previous\n\
         \u{20}                   Control the current media player\n\
         \u{20}   tab <NAME>      Open a tab: music tasks clips shelf mixer sounds time tools\n\
         \u{20}   reload          Re-read the config file\n\
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
