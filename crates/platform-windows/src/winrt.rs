//! Waiting for WinRT async operations with a time limit.
//!
//! `IAsyncOperation::get()` blocks until the operation finishes, however long
//! that is: an app logo that never loads, or a notification list read that
//! stalls, would hold the calling thread forever. Shutdown then hangs on
//! joining that thread and Windows reports Perch as not responding. These
//! helpers poll the operation's status instead, and cancel it when the limit
//! passes.

use std::time::{Duration, Instant};

use windows::core::{Interface, RuntimeType};
use windows::Foundation::{AsyncStatus, IAsyncInfo, IAsyncOperation};

/// How often a pending operation is checked.
const POLL: Duration = Duration::from_millis(10);

/// The operation's result, or `None` if it failed, was cancelled or took
/// longer than `limit` (then it is cancelled).
pub fn wait<T: RuntimeType + 'static>(op: &IAsyncOperation<T>, limit: Duration) -> Option<T> {
    let deadline = Instant::now() + limit;
    loop {
        match op.Status() {
            Ok(AsyncStatus::Completed) => return op.GetResults().ok(),
            Ok(AsyncStatus::Started) => {}
            _ => return None,
        }
        if Instant::now() >= deadline {
            if let Ok(info) = op.cast::<IAsyncInfo>() {
                let _ = info.Cancel();
            }
            return None;
        }
        std::thread::sleep(POLL);
    }
}
