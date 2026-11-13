//! Single-instance enforcement plus the CLI channel, over a named pipe.
//!
//! The first instance creates `\\.\pipe\perch-<user-sid-free-name>` and serves
//! it on a background thread; later invocations connect, write one command line
//! and exit. A named mutex guards the race so two simultaneous launches cannot
//! both decide they are first.

use std::io::{Read, Write};
use std::sync::mpsc::Sender;

use perch_core::error::{Error, Result};
use perch_core::platform::SingleInstance;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

const PIPE_NAME: &str = r"\\.\pipe\perch-ipc";
const MUTEX_NAME: &str = r"Local\perch-single-instance";

/// Named-pipe + named-mutex single instance.
#[derive(Debug, Default)]
pub struct NamedPipeInstance {
    mutex: Option<HANDLE>,
}

impl Drop for NamedPipeInstance {
    fn drop(&mut self) {
        if let Some(h) = self.mutex.take() {
            // SAFETY: `h` is a handle we created with CreateMutexW and have not
            // closed; this is the matching close.
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

impl NamedPipeInstance {
    /// Start serving CLI commands, forwarding each received line to `tx`.
    ///
    /// Spawns one blocking thread that waits on client connections; it costs no
    /// CPU while idle because `ConnectNamedPipe` blocks in the kernel.
    pub fn serve(&self, tx: Sender<String>) -> Result<()> {
        std::thread::Builder::new()
            .name("perch-ipc".into())
            .spawn(move || {
                loop {
                    match serve_one(&tx) {
                        Ok(()) => {}
                        Err(e) => {
                            log::debug!("ipc server: {e}");
                            // Avoid a hot loop if the pipe is permanently broken.
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                    }
                }
            })
            .map_err(|e| Error::io("spawning the IPC thread", e))?;
        Ok(())
    }
}

fn serve_one(tx: &Sender<String>) -> Result<()> {
    use std::os::windows::io::FromRawHandle;

    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let name = HSTRING::from(PIPE_NAME);
    // SAFETY: `name` is a valid wide string that outlives the call; the flags
    // are documented constants and we own the returned handle.
    let handle = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            8,    // max instances
            1024, // out buffer
            1024, // in buffer
            0,    // default timeout
            None, // default security: current user only
        )
    }
    .map_err(|e| Error::backend(format!("CreateNamedPipeW: {e}")))?;

    // SAFETY: `handle` is a pipe handle we just created and have not closed.
    let connected = unsafe { ConnectNamedPipe(handle, None) };
    if connected.is_err() {
        // SAFETY: closing the handle we own after a failed connect.
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(Error::backend("ConnectNamedPipe failed"));
    }

    // SAFETY: we transfer ownership of the raw handle into a File exactly once;
    // the File's Drop closes it, and we do not touch `handle` for closing after
    // this point.
    let mut file = unsafe { std::fs::File::from_raw_handle(handle.0 as *mut _) };
    let mut buf = String::new();
    let read = file.read_to_string(&mut buf);

    // SAFETY: the handle is still valid here (owned by `file`, not yet dropped),
    // and disconnecting a server pipe end is the documented teardown.
    unsafe {
        let _ = DisconnectNamedPipe(handle);
    }

    match read {
        Ok(_) => {
            for line in buf.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    let _ = tx.send(line.to_string());
                }
            }
            Ok(())
        }
        Err(e) => Err(Error::io("reading an IPC command", e)),
    }
}

impl SingleInstance for NamedPipeInstance {
    fn acquire(&mut self) -> Result<bool> {
        let name = HSTRING::from(MUTEX_NAME);
        // SAFETY: `name` outlives the call; we own and later close the returned
        // handle. A named mutex has no other side effects.
        let handle = unsafe { CreateMutexW(None, true, &name) }
            .map_err(|e| Error::backend(format!("CreateMutexW: {e}")))?;
        // SAFETY: GetLastError only reads this thread's last-error value.
        let already = unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS;
        if already {
            // SAFETY: closing the handle we just created; another instance owns
            // the mutex, so we must not hold a reference to it.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(false);
        }
        self.mutex = Some(handle);
        Ok(true)
    }

    fn forward(&mut self, command: &str) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(PIPE_NAME)
            .map_err(|e| {
                Error::unavailable("ipc", format!("no running Perch to talk to: {e}"))
            })?;
        file.write_all(command.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|e| Error::io("sending an IPC command", e))
    }
}
