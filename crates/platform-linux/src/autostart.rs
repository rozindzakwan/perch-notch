//! XDG autostart: a `.desktop` file in `~/.config/autostart`.

use std::path::PathBuf;

use perch_core::error::{Error, Result};
use perch_core::platform::Autostart;

const FILE_NAME: &str = "perch.desktop";

/// Writes and removes the XDG autostart entry.
#[derive(Debug, Default)]
pub struct XdgAutostart;

fn autostart_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("autostart"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| Error::NoDirectory { kind: "config" })?;
    Ok(PathBuf::from(home).join(".config").join("autostart"))
}

fn entry_path() -> Result<PathBuf> {
    Ok(autostart_dir()?.join(FILE_NAME))
}

/// The `.desktop` body, kept here so the packaging scripts and this code cannot
/// drift apart.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Perch\n\
         Comment=Dynamic notch for your desktop\n\
         Exec={exec}\n\
         Icon=perch\n\
         Terminal=false\n\
         Categories=Utility;\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=false\n"
    )
}

impl Autostart for XdgAutostart {
    fn is_enabled(&self) -> Result<bool> {
        Ok(entry_path()?.exists())
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        let path = entry_path()?;
        if enabled {
            let dir = autostart_dir()?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
            let exe = std::env::current_exe()
                .map_err(|e| Error::io("determining the Perch executable path", e))?;
            std::fs::write(&path, desktop_entry(&exe.display().to_string()))
                .map_err(|e| Error::io(format!("writing {}", path.display()), e))
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
            }
        }
    }
}
