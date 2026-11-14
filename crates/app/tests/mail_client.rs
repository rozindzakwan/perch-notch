//! The read-only IMAP client against a scripted in-process server (plain TCP
//! on 127.0.0.1, which is the only place unencrypted IMAP is allowed).
#![cfg(feature = "mail")]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_app::mail::{start, Options, Report};
use perch_core::config::MailAccount;
use perch_core::mail::{base64_decode, AccountStatus, MailUpdate, Security};
use perch_core::platform::{MemorySecrets, SecretStore};

#[derive(Clone)]
struct Script {
    idle: bool,
    password: &'static str,
}

struct Server {
    port: u16,
    commands: Arc<Mutex<Vec<String>>>,
    connections: Arc<Mutex<u32>>,
    unseen: Arc<Mutex<Vec<u32>>>,
    push: Sender<()>,
}

fn envelope(uid: u32) -> String {
    let (name, subject) = match uid {
        3 => ("\"Ann Lee\"", "\"=?utf-8?B?SGFsbyDwn5iA?=\""),
        5 => ("NIL", "\"Quarterly report\""),
        _ => (
            "\"=?iso-8859-1?Q?Ren=E9?=\"",
            format!("\"Message {uid}\"").leak() as &str,
        ),
    };
    format!(
        "(\"Sat, 14 Nov 2026 13:05:22 +0800\" {subject} (({name} NIL \"user{uid}\" \"example.com\")) NIL NIL NIL NIL NIL NIL \"<{uid}@example.com>\")"
    )
}

fn serve(script: Script) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let commands = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(Mutex::new(0));
    let unseen = Arc::new(Mutex::new(vec![3, 5]));
    let (push, pushed) = channel::<()>();
    let pushed = Arc::new(Mutex::new(pushed));
    let (c2, n2, u2) = (commands.clone(), connections.clone(), unseen.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            if let Ok(mut n) = n2.lock() {
                *n += 1;
            }
            let (c3, u3, p3, s3) = (c2.clone(), u2.clone(), pushed.clone(), script.clone());
            std::thread::spawn(move || {
                let _ = session(stream, &s3, &c3, &u3, &p3);
            });
        }
    });
    Server {
        port,
        commands,
        connections,
        unseen,
        push,
    }
}

fn session(
    stream: TcpStream,
    script: &Script,
    commands: &Mutex<Vec<String>>,
    unseen: &Mutex<Vec<u32>>,
    pushed: &Mutex<Receiver<()>>,
) -> std::io::Result<()> {
    let caps = if script.idle {
        "IMAP4rev1 AUTH=PLAIN SASL-IR IDLE"
    } else {
        "IMAP4rev1 AUTH=PLAIN SASL-IR"
    };
    let mut out = stream.try_clone()?;
    let mut lines = BufReader::new(stream);
    writeln!(out, "* OK [CAPABILITY {caps}] scripted server ready\r")?;
    loop {
        let mut line = String::new();
        if lines.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end().to_string();
        let (tag, cmd) = line.split_once(' ').unwrap_or((&line, ""));
        let upper = cmd.to_ascii_uppercase();
        if let Ok(mut c) = commands.lock() {
            // Never keep the credentials, even in a test log.
            c.push(if upper.starts_with("AUTHENTICATE") {
                "AUTHENTICATE PLAIN ***".into()
            } else {
                cmd.to_string()
            });
        }
        if upper == "CAPABILITY" {
            write!(out, "* CAPABILITY {caps}\r\n{tag} OK done\r\n")?;
        } else if let Some(b64) = cmd.strip_prefix("AUTHENTICATE PLAIN ") {
            let raw = base64_decode(b64).unwrap_or_default();
            if raw == format!("\0user@example.com\0{}", script.password).into_bytes() {
                write!(out, "{tag} OK [CAPABILITY {caps}] signed in\r\n")?;
            } else {
                write!(
                    out,
                    "{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n"
                )?;
            }
        } else if upper.starts_with("EXAMINE") {
            write!(
                out,
                "* 2 EXISTS\r\n* OK [UIDVALIDITY 42] UIDs valid\r\n{tag} OK [READ-ONLY] EXAMINE completed\r\n"
            )?;
        } else if upper == "UID SEARCH UNSEEN" {
            let u = unseen.lock().map(|u| u.clone()).unwrap_or_default();
            let list = u.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
            write!(out, "* SEARCH {list}\r\n{tag} OK search done\r\n")?;
        } else if let Some(rest) = upper.strip_prefix("UID FETCH ") {
            let set = rest.split(' ').next().unwrap_or("");
            for (seq, uid) in set
                .split(',')
                .filter_map(|s| s.parse::<u32>().ok())
                .enumerate()
            {
                write!(
                    out,
                    "* {} FETCH (UID {uid} INTERNALDATE \"14-Nov-2026 13:05:{:02} +0800\" ENVELOPE {})\r\n",
                    seq + 1,
                    uid % 60,
                    envelope(uid)
                )?;
            }
            write!(out, "{tag} OK fetch done\r\n")?;
        } else if upper == "IDLE" {
            write!(out, "+ idling\r\n")?;
            // Wait for the test to deliver mail, then announce it.
            let got = pushed
                .lock()
                .map(|p| p.recv_timeout(Duration::from_secs(20)).is_ok())
                .unwrap_or(false);
            if got {
                write!(out, "* 3 EXISTS\r\n")?;
            }
            let mut done = String::new();
            lines.read_line(&mut done)?;
            if let Ok(mut c) = commands.lock() {
                c.push(done.trim_end().to_string());
            }
            write!(out, "{tag} OK IDLE terminated\r\n")?;
        } else if upper == "LOGOUT" {
            write!(out, "* BYE\r\n{tag} OK bye\r\n")?;
            return Ok(());
        } else {
            write!(out, "{tag} BAD unexpected command\r\n")?;
        }
    }
}

fn account(port: u16) -> MailAccount {
    MailAccount {
        name: "Work".into(),
        username: "user@example.com".into(),
        host: "127.0.0.1".into(),
        port,
        security: Security::Plain,
        ..MailAccount::default()
    }
}

fn collector() -> (Report, Receiver<MailUpdate>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let report: Report = Arc::new(move |u| {
        if let Ok(t) = tx.lock() {
            let _ = t.send(u);
        }
    });
    (report, rx)
}

fn secrets(password: Option<&str>, acct: &MailAccount) -> Arc<dyn SecretStore> {
    let s = MemorySecrets::default();
    if let Some(p) = password {
        let _ = s.set(&acct.secret_key(), "test", p);
    }
    Arc::new(s)
}

/// Next update matching `pred`, within `secs`.
fn wait_for(
    rx: &Receiver<MailUpdate>,
    secs: u64,
    pred: impl Fn(&MailUpdate) -> bool,
) -> MailUpdate {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(u) if pred(&u) => return u,
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for a mail update"),
        }
    }
}

fn opts() -> Options {
    Options {
        max_items: 10,
        poll: Duration::from_millis(300),
    }
}

#[test]
fn reads_unread_mail_read_only_and_gets_new_mail_pushed() {
    let server = serve(Script {
        idle: true,
        password: "s3cret \"pw\"",
    });
    let acct = account(server.port);
    let (report, rx) = collector();
    let watcher = start(
        acct.clone(),
        secrets(Some("s3cret \"pw\""), &acct),
        opts(),
        report,
    );

    let online = wait_for(&rx, 10, |u| {
        matches!(
            u,
            MailUpdate::Status {
                status: AccountStatus::Online { .. },
                ..
            }
        )
    });
    assert_eq!(
        online,
        MailUpdate::Status {
            account: "Work".into(),
            status: AccountStatus::Online { push: true }
        }
    );
    let MailUpdate::Snapshot {
        uid_validity,
        unread,
        items,
        ..
    } = wait_for(&rx, 10, |u| matches!(u, MailUpdate::Snapshot { .. }))
    else {
        unreachable!()
    };
    assert_eq!((uid_validity, unread), (42, 2));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].uid, 5, "newest first");
    assert_eq!(
        items[0].from, "user5@example.com",
        "no display name -> address"
    );
    assert_eq!(items[0].subject, "Quarterly report");
    assert_eq!(items[1].from, "Ann Lee");
    assert_eq!(items[1].subject, "Halo 😀", "RFC 2047 subject decoded");
    assert_eq!(items[1].date_unix, Some(1_794_632_703));

    // New mail arrives while idling: pushed, not polled.
    if let Ok(mut u) = server.unseen.lock() {
        u.push(7);
    }
    let _ = server.push.send(());
    let MailUpdate::Snapshot { unread, items, .. } = wait_for(&rx, 10, |u| {
        matches!(u, MailUpdate::Snapshot { unread: 3, .. })
    }) else {
        unreachable!()
    };
    assert_eq!(unread, 3);
    assert_eq!(items[0].uid, 7);
    assert_eq!(items[0].from, "René");

    drop(watcher);
    std::thread::sleep(Duration::from_millis(200));
    let cmds = server
        .commands
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(cmds.iter().any(|c| c.starts_with("EXAMINE")), "{cmds:?}");
    assert!(
        cmds.iter().any(|c| c == "DONE"),
        "IDLE was ended with DONE: {cmds:?}"
    );
    for c in &cmds {
        let u = c.to_ascii_uppercase();
        assert!(
            !(u.starts_with("SELECT")
                || u.contains("STORE")
                || u.contains("BODY[")
                || u.contains("RFC822")),
            "the client must stay read-only and never download bodies: {c}"
        );
    }
    assert_eq!(server.connections.lock().map(|n| *n).unwrap_or(0), 1);
}

#[test]
fn a_rejected_password_is_reported_and_not_retried() {
    let server = serve(Script {
        idle: true,
        password: "right",
    });
    let acct = account(server.port);
    let (report, rx) = collector();
    let _w = start(acct.clone(), secrets(Some("wrong"), &acct), opts(), report);
    let u = wait_for(&rx, 10, |u| {
        matches!(
            u,
            MailUpdate::Status {
                status: AccountStatus::AuthFailed(_),
                ..
            }
        )
    });
    let MailUpdate::Status {
        status: AccountStatus::AuthFailed(msg),
        ..
    } = u
    else {
        unreachable!()
    };
    assert!(msg.contains("Invalid credentials"), "{msg}");
    std::thread::sleep(Duration::from_secs(7)); // longer than the first backoff
    assert_eq!(
        server.connections.lock().map(|n| *n).unwrap_or(0),
        1,
        "no retries with a wrong password (could lock the account)"
    );
}

#[test]
fn a_missing_password_is_reported_without_connecting() {
    let server = serve(Script {
        idle: true,
        password: "x",
    });
    let acct = account(server.port);
    let (report, rx) = collector();
    let _w = start(acct.clone(), secrets(None, &acct), opts(), report);
    wait_for(&rx, 5, |u| {
        matches!(
            u,
            MailUpdate::Status {
                status: AccountStatus::NoPassword,
                ..
            }
        )
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.connections.lock().map(|n| *n).unwrap_or(0), 0);
}

#[test]
fn servers_without_idle_are_checked_periodically() {
    let server = serve(Script {
        idle: false,
        password: "pw",
    });
    let acct = account(server.port);
    let (report, rx) = collector();
    let _w = start(acct.clone(), secrets(Some("pw"), &acct), opts(), report);
    wait_for(&rx, 10, |u| {
        matches!(
            u,
            MailUpdate::Status {
                status: AccountStatus::Online { push: false },
                ..
            }
        )
    });
    wait_for(&rx, 10, |u| {
        matches!(u, MailUpdate::Snapshot { unread: 2, .. })
    });
    if let Ok(mut u) = server.unseen.lock() {
        u.push(9);
    }
    wait_for(&rx, 10, |u| {
        matches!(u, MailUpdate::Snapshot { unread: 3, .. })
    });
}

#[test]
fn plaintext_to_a_remote_host_is_refused() {
    let mut acct = account(143);
    acct.host = "mail.example.com".into();
    let (report, rx) = collector();
    let _w = start(acct.clone(), secrets(Some("pw"), &acct), opts(), report);
    let u = wait_for(&rx, 5, |u| {
        matches!(
            u,
            MailUpdate::Status {
                status: AccountStatus::Error(_),
                ..
            }
        )
    });
    assert!(
        format!("{u:?}").contains("only allowed to localhost"),
        "{u:?}"
    );
}
