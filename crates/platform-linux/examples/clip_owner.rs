//! Owns the X11 CLIPBOARD with a given text, optionally advertising the KDE
//! password-manager hint the way KeePassXC does. Used by the smoke test to
//! prove Perch records normal copies but never password-manager copies.
//!
//! Usage: `clip_owner TEXT [--secret]`; serves requests until another client
//! takes the clipboard over.

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = run() {
        eprintln!("clip_owner: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent,
        WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    let mut args = std::env::args().skip(1);
    let text = args.next().unwrap_or_else(|| "clip".into());
    let secret = args.any(|a| a == "--secret");

    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, n.as_bytes())?.reply()?.atom)
    };
    let clipboard = atom("CLIPBOARD")?;
    let targets = atom("TARGETS")?;
    let utf8 = atom("UTF8_STRING")?;
    let hint = atom("x-kde-passwordManagerHint")?;

    let win = conn.generate_id()?;
    conn.create_window(
        0,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?;
    conn.set_selection_owner(win, clipboard, x11rb::CURRENT_TIME)?;
    conn.flush()?;

    loop {
        match conn.wait_for_event()? {
            Event::SelectionRequest(req) => {
                let mut property = req.property;
                if req.target == targets {
                    let mut list = vec![targets, utf8, u32::from(AtomEnum::STRING)];
                    if secret {
                        list.push(hint);
                    }
                    conn.change_property32(
                        PropMode::REPLACE,
                        req.requestor,
                        req.property,
                        AtomEnum::ATOM,
                        &list,
                    )?;
                } else if req.target == utf8 || req.target == u32::from(AtomEnum::STRING) {
                    conn.change_property8(
                        PropMode::REPLACE,
                        req.requestor,
                        req.property,
                        req.target,
                        text.as_bytes(),
                    )?;
                } else if secret && req.target == hint {
                    conn.change_property8(
                        PropMode::REPLACE,
                        req.requestor,
                        req.property,
                        utf8,
                        b"secret",
                    )?;
                } else {
                    property = x11rb::NONE;
                }
                let ev = SelectionNotifyEvent {
                    response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: req.time,
                    requestor: req.requestor,
                    selection: req.selection,
                    target: req.target,
                    property,
                };
                conn.send_event(false, req.requestor, EventMask::NO_EVENT, ev)?;
                conn.flush()?;
            }
            Event::SelectionClear(_) => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("clip_owner is Linux-only");
}
