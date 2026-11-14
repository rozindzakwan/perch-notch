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
    /// Reconnect every mail account (e.g. after a password change).
    MailRetry,
    /// Open the add-mail-account dialog.
    MailAdd,
    /// Store a mail account's password (read from stdin) in the keychain.
    MailPassword(String),
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
            Command::MailRetry => s("mail-retry"),
            Command::MailAdd => s("mail-add"),
            // Newlines would split one command into several on the wire.
            Command::Todo(t) => Some(format!("todo {}", t.replace(['\n', '\r'], " "))),
            Command::Media(v) => Some(v.clone()),
            Command::Tab(t) => Some(format!("tab {t}")),
            Command::Run | Command::Version | Command::Help | Command::MailPassword(_) => None,
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
            "mail-retry" => command = command.or(Some(Command::MailRetry)),
            "mail-add" => command = command.or(Some(Command::MailAdd)),
            "mail-password" if command.is_none() => {
                let name = iter
                    .next()
                    .map(|n| n.as_ref().to_string())
                    .unwrap_or_default();
                if name.trim().is_empty() {
                    unknown.push("mail-password (missing account name)".into());
                } else {
                    command = Some(Command::MailPassword(name));
                }
            }
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
         \u{20}   todo [DUE] <TEXT>\n\
         \u{20}                   Add a to-do item; DUE is +30s, +10m, +2h or HH:MM\n\
         \u{20}   play-pause, next, previous\n\
         \u{20}                   Control the current media player\n\
         \u{20}   tab <NAME>      Open a tab: music tasks clips shelf mixer sounds time tools mail\n\
         \u{20}   reload          Re-read the config file\n\
         \u{20}   mail-add        Open the add-mail-account dialog\n\
         \u{20}   mail-retry      Reconnect mail accounts (e.g. after a password change)\n\
         \u{20}   mail-password <ACCOUNT>\n\
         \u{20}                   Store that mail account's password (read from stdin)\n\
         \u{20}                   in the system keychain\n\
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

/// The password line read by `perch mail-password`: without the trailing
/// newline, and without the UTF-8 byte-order mark that Windows PowerShell puts
/// in front of text piped to a program (it would otherwise become part of the
/// stored password).
pub fn password_from_line(line: &str) -> &str {
    line.trim_start_matches('\u{feff}')
        .trim_end_matches(['\r', '\n'])
}

#[cfg(test)]
mod password_line_tests {
    use super::password_from_line;

    #[test]
    fn strips_bom_and_newline_but_keeps_spaces() {
        assert_eq!(
            password_from_line("\u{feff}correct horse\r\n"),
            "correct horse"
        );
        assert_eq!(password_from_line("  spaced pw \n"), "  spaced pw ");
        assert_eq!(password_from_line("plain"), "plain");
    }
}
