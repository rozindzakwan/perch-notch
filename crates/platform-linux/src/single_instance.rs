//! Single-instance enforcement plus the CLI channel, over a Unix socket in
//! `$XDG_RUNTIME_DIR`.
//!
//! Binding an `AF_UNIX` socket is itself the lock: a second process cannot bind
//! the same path. A stale socket (left by a crash) is detected by trying to
//! connect first, which is the standard safe recovery.

use perch_core::platform::{EventSink, PlatformEvent};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use perch_core::error::{Error, Result};
use perch_core::platform::SingleInstance;

const SOCKET_NAME: &str = "perch.sock";

/// Unix-socket single instance and IPC endpoint.
#[derive(Debug, Default)]
pub struct UnixSocketInstance {
    listener: Option<UnixListener>,
}

fn socket_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join(SOCKET_NAME));
        }
    }
    // Fall back to a per-user path under /tmp when the session has no runtime
    // dir (some minimal setups, and CI).
    let uid = std::env::var("UID").unwrap_or_else(|_| "0".into());
    Ok(PathBuf::from(format!("/tmp/perch-{uid}")).join(SOCKET_NAME))
}

impl UnixSocketInstance {
    /// Serve CLI commands, delivering each received line to `sink` as a
    /// [`PlatformEvent::Command`].
    ///
    /// One blocking accept loop on its own thread; idle cost is zero because
    /// `accept` blocks in the kernel.
    pub fn serve(&mut self, sink: EventSink) -> Result<()> {
        let listener = match self.listener.take() {
            Some(l) => l,
            None => {
                return Err(Error::backend(
                    "serve() called before a successful acquire()",
                ))
            }
        };
        std::thread::Builder::new()
            .name("perch-ipc".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let reader = BufReader::new(stream);
                            for line in reader.lines() {
                                match line {
                                    Ok(line) => {
                                        let line = line.trim().to_string();
                                        if !line.is_empty() {
                                            sink(PlatformEvent::Command(line));
                                        }
                                    }
                                    Err(e) => {
                                        log::debug!("ipc read: {e}");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => log::debug!("ipc accept: {e}"),
                    }
                }
            })
            .map_err(|e| Error::io("spawning the IPC thread", e))?;
        Ok(())
    }
}

impl SingleInstance for UnixSocketInstance {
    fn acquire(&mut self) -> Result<bool> {
        let path = socket_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
        }
        // A live socket means another instance owns the lock.
        if UnixStream::connect(&path).is_ok() {
            return Ok(false);
        }
        // Otherwise any file there is stale; removing it is safe because the
        // connect above proved nobody is listening.
        match std::fs::remove_file(&path) {
            Ok(()) => log::info!("removed a stale Perch socket at {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(format!("removing {}", path.display()), e)),
        }
        let listener = UnixListener::bind(&path)
            .map_err(|e| Error::io(format!("binding {}", path.display()), e))?;
        self.listener = Some(listener);
        Ok(true)
    }

    fn forward(&mut self, command: &str) -> Result<()> {
        let path = socket_path()?;
        let mut stream = UnixStream::connect(&path)
            .map_err(|e| Error::unavailable("ipc", format!("no running Perch to talk to: {e}")))?;
        stream
            .write_all(command.as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .map_err(|e| Error::io("sending an IPC command", e))
    }
}
