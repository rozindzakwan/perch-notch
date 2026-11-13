//! Small persisted state (to-dos) and the test status file.
//!
//! Clipboard history is deliberately *not* persisted: it can contain things
//! the user copied once and would not want written to disk.

use std::path::{Path, PathBuf};

use perch_core::todo::TodoList;
use serde::Serialize;

/// Per-user data directory (`%APPDATA%\perch\data` / `~/.local/share/perch`).
/// `PERCH_DATA_DIR` overrides it, so smoke tests never touch real user data.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("PERCH_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    directories::ProjectDirs::from("", "", perch_core::APP_ID).map(|d| d.data_dir().to_path_buf())
}

/// Per-user cache directory (lyrics, speech models); under `PERCH_DATA_DIR`
/// when that is set.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("PERCH_DATA_DIR") {
        return Some(PathBuf::from(d).join("cache"));
    }
    directories::ProjectDirs::from("", "", perch_core::APP_ID).map(|d| d.cache_dir().to_path_buf())
}

/// Write bytes atomically (temp file + rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Load the to-do list; a missing or corrupt file yields an empty list (the
/// corrupt file is kept aside as `todos.json.bad` so nothing is silently lost).
pub fn load_todos(dir: &Path) -> TodoList {
    let path = dir.join("todos.json");
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(list) => list,
            Err(e) => {
                log::warn!("todos.json is unreadable ({e}); starting empty and keeping a copy");
                let _ = std::fs::rename(&path, dir.join("todos.json.bad"));
                TodoList::new()
            }
        },
        Err(_) => TodoList::new(),
    }
}

/// Save the to-do list.
pub fn save_todos(dir: &Path, todos: &TodoList) {
    match serde_json::to_vec_pretty(todos) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(&dir.join("todos.json"), &bytes) {
                log::warn!("saving to-dos: {e}");
            }
        }
        Err(e) => log::warn!("serialising to-dos: {e}"),
    }
}

/// Machine-readable snapshot written to `$PERCH_STATUS_FILE` after state
/// changes. Only used by the smoke/soak scripts; off unless the variable is set.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Status {
    /// Notch state machine state.
    pub state: String,
    /// Pill window rect `[x, y, w, h]` in physical pixels.
    pub pill: [i64; 4],
    /// Whether the pill is visible (false while a fullscreen app hides it).
    pub visible: bool,
    /// Selected tab.
    pub tab: String,
    /// Current track label.
    pub track: String,
    /// Playing.
    pub playing: bool,
    /// Clipboard history length.
    pub clipboard: usize,
    /// Shelf length.
    pub shelf: usize,
    /// To-do count.
    pub todos: usize,
    /// Live features.
    pub features: Vec<String>,
    /// Frames presented.
    pub frames: u64,
    /// Milliseconds from process start to the first presented frame.
    pub first_frame_ms: Option<u64>,
    /// Noise audible.
    pub noise: bool,
    /// Lyrics line currently shown.
    pub lyric: String,
}

/// Write the status file if `$PERCH_STATUS_FILE` is set and it changed.
pub fn write_status(path: &Path, status: &Status) {
    if let Ok(json) = serde_json::to_vec_pretty(status) {
        let _ = write_atomic(path, &json);
    }
}

/// Most bytes written to `perch.log` per run; beyond this the log goes quiet
/// rather than growing without bound (it is truncated at every start).
const LOG_CAP: u64 = 2 * 1024 * 1024;

/// A size-capped log file writer.
pub struct CappedLog {
    file: std::fs::File,
    written: u64,
}

impl std::io::Write for CappedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written >= LOG_CAP {
            return Ok(buf.len());
        }
        self.written += buf.len() as u64;
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Open (and truncate) `<data dir>/perch.log` for the running instance.
pub fn log_file() -> Option<CappedLog> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let file = std::fs::File::create(dir.join("perch.log")).ok()?;
    Some(CappedLog { file, written: 0 })
}
