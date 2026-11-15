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
        } else if upper.starts_with("SELECT") {
            write!(
                out,
                "* 2 EXISTS\r\n* OK [UIDVALIDITY 42] UIDs valid\r\n{tag} OK [READ-WRITE] SELECT completed\r\n"
            )?;
        } else if let Some(rest) = upper.strip_prefix("UID STORE ") {
            // Only "+FLAGS.SILENT (\Seen)" is accepted: anything else would
            // be the client changing more than the read flag.
            let (set, change) = rest.split_once(' ').unwrap_or((rest, ""));
            if change != "+FLAGS.SILENT (\\SEEN)" {
                write!(out, "{tag} BAD only the read flag may change\r\n")?;
                continue;
            }
            if let Ok(mut u) = unseen.lock() {
                if set == "1:*" {
                    u.clear();
                } else {
                    let marked: Vec<u32> = set.split(',').filter_map(|s| s.parse().ok()).collect();
                    u.retain(|x| !marked.contains(x));
                }
            }
            write!(out, "{tag} OK store done\r\n")?;
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

/// Run a mark and wait for its answer.
fn mark(acct: &MailAccount, uid_validity: u32, uids: Option<Vec<u32>>) -> Result<(), String> {
    let (tx, rx) = channel();
    perch_app::mail::mark_read(
        acct.clone(),
        secrets(Some("pw"), acct),
        uid_validity,
        uids,
        move |r| {
            let _ = tx.send(r);
        },
    );
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("no answer from mark_read: {e}"))
}

#[test]
fn marking_read_sets_only_the_seen_flag_over_its_own_connection() {
    let server = serve(Script {
        idle: false,
        password: "pw",
    });
    let acct = account(server.port);
    assert_eq!(mark(&acct, 42, Some(vec![5])), Ok(()));
    assert_eq!(
        server.unseen.lock().map(|u| u.clone()).unwrap_or_default(),
        [3]
    );
    let cmds = server
        .commands
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(cmds.iter().any(|c| c == "SELECT \"INBOX\""), "{cmds:?}");
    assert!(
        cmds.iter()
            .any(|c| c == r"UID STORE 5 +FLAGS.SILENT (\Seen)"),
        "{cmds:?}"
    );
    assert_eq!(cmds.last().map(String::as_str), Some("LOGOUT"), "{cmds:?}");
    for never in ["CLOSE", "EXPUNGE", "UID EXPUNGE", "COPY", "MOVE"] {
        assert!(
            !cmds
                .iter()
                .any(|c| c.to_ascii_uppercase().starts_with(never)),
            "{never} sent: {cmds:?}"
        );
    }
    assert_eq!(server.connections.lock().map(|n| *n).unwrap_or(0), 1);

    // "Mark all read" covers every message of the folder.
    assert_eq!(mark(&acct, 42, None), Ok(()));
    assert!(server.unseen.lock().map(|u| u.is_empty()).unwrap_or(false));
    let cmds = server
        .commands
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(
        cmds.iter()
            .any(|c| c == r"UID STORE 1:* +FLAGS.SILENT (\Seen)"),
        "{cmds:?}"
    );
}

#[test]
fn a_changed_uidvalidity_marks_nothing() {
    let server = serve(Script {
        idle: false,
        password: "pw",
    });
    let acct = account(server.port);
    // The UIDs came from UIDVALIDITY 41; the server now says 42, so they
    // may name other messages.
    let r = mark(&acct, 41, Some(vec![5]));
    assert!(r.is_err(), "{r:?}");
    assert_eq!(
        server.unseen.lock().map(|u| u.clone()).unwrap_or_default(),
        [3, 5]
    );
    let cmds = server
        .commands
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(!cmds.iter().any(|c| c.starts_with("UID STORE")), "{cmds:?}");
    // A wrong password is an error too, never a retry loop.
    let mut wrong = acct.clone();
    wrong.username = "someone@example.com".into();
    assert!(mark(&wrong, 42, Some(vec![5])).is_err());
}

/// Against a real server (`scripts/mail-test.sh` runs it against Dovecot):
/// the read-only watcher lists the unread mail, one message is marked read
/// over the separate connection, and the watcher (still read-only, waiting
/// in IDLE) sees it go. Configured by `PERCH_TEST_IMAP_*`.
#[test]
#[ignore = "needs a real IMAP server: run by scripts/mail-test.sh"]
fn marks_one_message_read_on_a_real_server() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
    let acct = MailAccount {
        name: "Real".into(),
        username: env("PERCH_TEST_IMAP_USER"),
        host: "localhost".into(),
        port: env("PERCH_TEST_IMAP_PORT").parse().unwrap_or(1993),
        security: Security::Tls,
        ca_file: env("PERCH_TEST_IMAP_CA"),
        ..MailAccount::default()
    };
    let secrets = secrets(Some(&env("PERCH_TEST_IMAP_PASSWORD")), &acct);
    let (report, rx) = collector();
    let _watcher = start(acct.clone(), secrets.clone(), opts(), report);
    let (validity, unread, uid) = match wait_for(
        &rx,
        15,
        |u| matches!(u, MailUpdate::Snapshot { items, .. } if !items.is_empty()),
    ) {
        MailUpdate::Snapshot {
            uid_validity,
            unread,
            items,
            ..
        } => (uid_validity, unread, items[0].uid),
        other => panic!("{other:?}"),
    };
    let (tx, done) = channel();
    perch_app::mail::mark_read(acct, secrets, validity, Some(vec![uid]), move |r| {
        let _ = tx.send(r);
    });
    let r = done
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|e| panic!("no answer: {e}"));
    assert_eq!(r, Ok(()));
    // Pushed to the read-only session as a flag change.
    wait_for(&rx, 15, |u| {
        matches!(u, MailUpdate::Snapshot { unread: n, items, .. }
            if *n == unread - 1 && items.iter().all(|i| i.uid != uid))
    });
    println!("marked uid {uid} read; unread {unread} -> {}", unread - 1);
}
