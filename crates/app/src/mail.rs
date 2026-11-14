//! Read-only IMAP client for the Mail tab.
//!
//! One thread per account. It signs in, opens the folder with `EXAMINE`
//! (read-only: the server will not let this session change flags), asks for
//! the unread UIDs and fetches only `UID INTERNALDATE ENVELOPE` for the newest
//! few. Envelope fetches never set `\Seen`, so Perch cannot mark mail as read.
//!
//! New mail is pushed with IMAP IDLE (re-issued every 25 minutes, as RFC 2177
//! asks); servers without IDLE are checked every `poll_minutes`. Network
//! errors reconnect with a growing delay. A rejected sign-in stops the thread:
//! retrying a wrong password could lock the account. The thread is restarted
//! when the account settings or the stored password change.
//!
//! The password comes from the OS keychain, is read on this thread, and is
//! never logged.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use imap_proto::{AttributeValue, Capability, MailboxDatum, Response, ResponseCode, Status};
use perch_core::config::{MailAccount, MailConfig};
use perch_core::mail::{
    base64_encode, decode_header, display_address, parse_internal_date, AccountStatus, MailItem,
    MailUpdate, Security,
};
use perch_core::platform::SecretStore;

/// Where updates go (the shell forwards them to the event loop).
pub type Report = Arc<dyn Fn(MailUpdate) + Send + Sync>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Re-issue IDLE before the 30-minute server timeout (RFC 2177).
const IDLE_KEEPALIVE: Duration = Duration::from_secs(25 * 60);
const BACKOFF: [u64; 5] = [5, 15, 60, 180, 300];

/// Tuning that is not per account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Unread messages fetched per account.
    pub max_items: usize,
    /// Check interval without IDLE.
    pub poll: Duration,
}

impl From<&MailConfig> for Options {
    fn from(c: &MailConfig) -> Self {
        Self {
            max_items: c.max_items,
            poll: Duration::from_secs(u64::from(c.poll_minutes) * 60),
        }
    }
}

/// Stop signal shared with the worker.
#[derive(Default)]
struct Ctl {
    stopped: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
    wait: (Mutex<()>, Condvar),
}

impl Ctl {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Sleep up to `d`; returns false if stopped meanwhile.
    fn sleep(&self, d: Duration) -> bool {
        let (m, cv) = &self.wait;
        let deadline = Instant::now() + d;
        let Ok(mut guard) = m.lock() else {
            return false;
        };
        while !self.stopped() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            match cv.wait_timeout(guard, left) {
                Ok((g, _)) => guard = g,
                Err(_) => return false,
            }
        }
        false
    }
}

/// A running watcher; dropping it stops the thread (without waiting for it).
pub struct MailWatcher {
    ctl: Arc<Ctl>,
}

impl Drop for MailWatcher {
    fn drop(&mut self) {
        self.ctl.stopped.store(true, Ordering::Relaxed);
        self.ctl.wait.1.notify_all();
        if let Ok(mut s) = self.ctl.socket.lock() {
            if let Some(sock) = s.take() {
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// Start watching one account.
pub fn start(
    account: MailAccount,
    secrets: Arc<dyn SecretStore>,
    opts: Options,
    report: Report,
) -> MailWatcher {
    let ctl = Arc::new(Ctl::default());
    let c = ctl.clone();
    let name = account.name.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("perch-mail-{}", account.name))
        .spawn(move || run(&account, secrets.as_ref(), opts, &report, &c));
    if let Err(e) = spawned {
        log::warn!("mail: cannot start the thread for {name}: {e}");
    }
    MailWatcher { ctl }
}

fn status(report: &Report, account: &MailAccount, status: AccountStatus) {
    report(MailUpdate::Status {
        account: account.name.clone(),
        status,
    });
}

fn run(
    account: &MailAccount,
    secrets: &dyn SecretStore,
    opts: Options,
    report: &Report,
    ctl: &Ctl,
) {
    if let Some(p) = account.problem() {
        status(report, account, AccountStatus::Error(p));
        return;
    }
    let password = match secrets.get(&account.secret_key()) {
        Ok(Some(p)) if !p.is_empty() => p,
        Ok(_) => {
            status(report, account, AccountStatus::NoPassword);
            return;
        }
        Err(e) => {
            status(
                report,
                account,
                AccountStatus::Error(format!("keychain: {e}")),
            );
            return;
        }
    };
    let mut attempt = 0usize;
    while !ctl.stopped() {
        status(report, account, AccountStatus::Connecting);
        let began = Instant::now();
        match session(account, &password, opts, report, ctl) {
            Ok(()) => return,
            Err(_) if ctl.stopped() => return,
            Err(SessErr::Auth(msg)) => {
                log::info!("mail {}: sign-in rejected: {msg}", account.name);
                status(report, account, AccountStatus::AuthFailed(msg));
                return;
            }
            Err(e @ (SessErr::Fail(_) | SessErr::Timeout)) => {
                let msg = match e {
                    SessErr::Fail(m) => m,
                    _ => "the server stopped answering".into(),
                };
                log::info!("mail {}: {msg}", account.name);
                status(report, account, AccountStatus::Error(msg));
                if began.elapsed() > Duration::from_secs(120) {
                    attempt = 0;
                }
                let wait = BACKOFF[attempt.min(BACKOFF.len() - 1)];
                attempt += 1;
                if !ctl.sleep(Duration::from_secs(wait)) {
                    return;
                }
            }
        }
    }
}

#[derive(Debug)]
enum SessErr {
    /// The server refused the credentials.
    Auth(String),
    /// Anything else; worth retrying.
    Fail(String),
    /// A read timed out (the IDLE keepalive uses this).
    Timeout,
}

impl From<io::Error> for SessErr {
    fn from(e: io::Error) -> Self {
        if matches!(
            e.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            return SessErr::Timeout;
        }
        SessErr::Fail(match e.kind() {
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => {
                "the connection was closed".into()
            }
            _ => e.to_string(),
        })
    }
}

type SessResult<T> = std::result::Result<T, SessErr>;

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

struct Conn {
    stream: Stream,
    tcp: TcpStream,
    buf: Vec<u8>,
    tag: u32,
}

/// What a command's responses told us.
enum Seen {
    /// The tagged completion arrived.
    Done,
    /// Keep reading.
    More,
}

impl Conn {
    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("p{}", self.tag)
    }

    fn send_line(&mut self, line: &str) -> io::Result<()> {
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\r\n")?;
        self.stream.flush()
    }

    /// Read one response and hand it to `f` (parsed in place, no copies).
    fn read<T>(&mut self, f: impl FnOnce(&Response<'_>) -> T) -> SessResult<T> {
        let mut f = Some(f);
        let mut chunk = [0u8; 8192];
        loop {
            if !self.buf.is_empty() {
                let parsed = match Response::parse(&self.buf) {
                    Ok((rest, resp)) => match f.take() {
                        Some(f) => Some((self.buf.len() - rest.len(), f(&resp))),
                        None => {
                            return Err(SessErr::Fail("internal: response consumed twice".into()))
                        }
                    },
                    Err(e) if e.is_incomplete() => None,
                    Err(_) => {
                        let line = self.buf.split(|&b| b == b'\n').next().unwrap_or_default();
                        let line = String::from_utf8_lossy(&line[..line.len().min(120)]);
                        return Err(SessErr::Fail(format!(
                            "unexpected server reply: {}",
                            line.trim()
                        )));
                    }
                };
                if let Some((used, out)) = parsed {
                    self.buf.drain(..used);
                    return Ok(out);
                }
            }
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(SessErr::Fail("the connection was closed".into()));
            }
            if self.buf.len() + n > 16 * 1024 * 1024 {
                return Err(SessErr::Fail("server reply too large".into()));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Run a command; `untagged` sees every untagged response. A tagged NO or
    /// BAD becomes `Err(no(text))`.
    fn command(
        &mut self,
        cmd: &str,
        no: fn(String) -> SessErr,
        mut untagged: impl FnMut(&Response<'_>),
    ) -> SessResult<()> {
        let tag = self.next_tag();
        self.send_line(&format!("{tag} {cmd}"))?;
        self.finish(&tag, no, &mut untagged)
    }

    fn finish(
        &mut self,
        tag: &str,
        no: fn(String) -> SessErr,
        untagged: &mut impl FnMut(&Response<'_>),
    ) -> SessResult<()> {
        loop {
            let r = self.read(|r| match r {
                Response::Done {
                    tag: t,
                    status,
                    outcome,
                } if t.0 == tag => {
                    let text = outcome
                        .information
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    match status {
                        Status::Ok => Ok(Seen::Done),
                        _ => Err(no(if text.is_empty() {
                            "the server refused".into()
                        } else {
                            text
                        })),
                    }
                }
                Response::Data {
                    status: Status::Bye,
                    outcome,
                } => Err(SessErr::Fail(format!(
                    "the server closed the session: {}",
                    outcome.information.as_deref().unwrap_or("").trim()
                ))),
                other => {
                    untagged(other);
                    Ok(Seen::More)
                }
            })??;
            if let Seen::Done = r {
                return Ok(());
            }
        }
    }
}

fn caps_of(list: &[Capability<'_>]) -> Vec<String> {
    list.iter()
        .map(|c| match c {
            Capability::Imap4rev1 => "IMAP4REV1".to_string(),
            Capability::Auth(a) => format!("AUTH={}", a.to_ascii_uppercase()),
            Capability::Atom(a) => a.to_ascii_uppercase(),
        })
        .collect()
}

fn caps_in(r: &Response<'_>) -> Option<Vec<String>> {
    match r {
        Response::Capabilities(c) => Some(caps_of(c)),
        Response::Data { outcome, .. } | Response::Done { outcome, .. } => match &outcome.code {
            Some(ResponseCode::Capabilities(c)) => Some(caps_of(c)),
            _ => None,
        },
        _ => None,
    }
}

/// IMAP quoted string; `None` if it needs a literal (CR/LF or 8-bit).
fn quoted(s: &str) -> Option<String> {
    if s.bytes()
        .any(|b| b == b'\r' || b == b'\n' || b >= 0x80 || b == 0)
    {
        return None;
    }
    Some(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn tls_config(account: &MailAccount) -> SessResult<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if !account.ca_file.trim().is_empty() {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::CertificateDer;
        let certs = CertificateDer::pem_file_iter(account.ca_file.trim())
            .map_err(|e| SessErr::Fail(format!("reading ca_file: {e}")))?;
        for cert in certs {
            let cert = cert.map_err(|e| SessErr::Fail(format!("reading ca_file: {e}")))?;
            roots
                .add(cert)
                .map_err(|e| SessErr::Fail(format!("ca_file: {e}")))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SessErr::Fail(format!("TLS setup: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

fn wrap_tls(tcp: TcpStream, account: &MailAccount) -> SessResult<Stream> {
    let cfg = tls_config(account)?;
    let name = rustls::pki_types::ServerName::try_from(account.host.trim().to_string())
        .map_err(|e| SessErr::Fail(format!("bad server name: {e}")))?;
    let conn = rustls::ClientConnection::new(cfg, name)
        .map_err(|e| SessErr::Fail(format!("TLS setup: {e}")))?;
    let mut s = rustls::StreamOwned::new(conn, tcp);
    // Drive the handshake now so certificate problems surface clearly.
    while s.conn.is_handshaking() {
        s.conn
            .complete_io(&mut s.sock)
            .map_err(|e| SessErr::Fail(format!("secure connection failed: {e}")))?;
    }
    Ok(Stream::Tls(Box::new(s)))
}

fn connect(account: &MailAccount, ctl: &Ctl) -> SessResult<Conn> {
    let host = account.host.trim();
    let addrs = (host, account.port)
        .to_socket_addrs()
        .map_err(|e| SessErr::Fail(format!("cannot find {host}: {e}")))?;
    let mut last = None;
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let tcp = tcp.ok_or_else(|| {
        SessErr::Fail(match last {
            Some(e) => format!("cannot reach {host}:{}: {e}", account.port),
            None => format!("cannot find {host}"),
        })
    })?;
    tcp.set_read_timeout(Some(COMMAND_TIMEOUT))?;
    tcp.set_write_timeout(Some(COMMAND_TIMEOUT))?;
    let _ = tcp.set_nodelay(true);
    if let Ok(mut s) = ctl.socket.lock() {
        *s = tcp.try_clone().ok();
    }
    if ctl.stopped() {
        return Err(SessErr::Fail("stopped".into()));
    }
    let handle = tcp.try_clone()?;
    let stream = match account.security {
        Security::Tls => wrap_tls(tcp, account)?,
        Security::Starttls | Security::Plain => Stream::Plain(tcp),
    };
    Ok(Conn {
        stream,
        tcp: handle,
        buf: Vec::new(),
        tag: 0,
    })
}

fn capabilities(conn: &mut Conn) -> SessResult<Vec<String>> {
    let mut caps = Vec::new();
    conn.command("CAPABILITY", SessErr::Fail, |r| {
        if let Some(c) = caps_in(r) {
            caps = c;
        }
    })?;
    Ok(caps)
}

fn login(conn: &mut Conn, caps: &[String], user: &str, password: &str) -> SessResult<()> {
    let has = |c: &str| caps.iter().any(|x| x == c);
    if has("AUTH=PLAIN") {
        let ir = base64_encode(format!("\0{user}\0{password}").as_bytes());
        if has("SASL-IR") {
            return conn.command(&format!("AUTHENTICATE PLAIN {ir}"), SessErr::Auth, |_| {});
        }
        let tag = conn.next_tag();
        conn.send_line(&format!("{tag} AUTHENTICATE PLAIN"))?;
        let cont = conn.read(|r| matches!(r, Response::Continue(_)))?;
        if !cont {
            return Err(SessErr::Fail(
                "the server did not accept AUTHENTICATE".into(),
            ));
        }
        conn.send_line(&ir)?;
        return conn.finish(&tag, SessErr::Auth, &mut |_| {});
    }
    if has("LOGINDISABLED") {
        return Err(SessErr::Auth(
            "this server does not allow password sign-in (it may need OAuth)".into(),
        ));
    }
    match (quoted(user), quoted(password)) {
        (Some(u), Some(p)) => conn.command(&format!("LOGIN {u} {p}"), SessErr::Auth, |_| {}),
        _ => Err(SessErr::Auth(
            "the password has characters this server cannot accept with LOGIN".into(),
        )),
    }
}

/// Sign in, open the folder read-only, then report and wait for changes until
/// stopped or the connection fails.
fn session(
    account: &MailAccount,
    password: &str,
    opts: Options,
    report: &Report,
    ctl: &Ctl,
) -> SessResult<()> {
    let mut conn = connect(account, ctl)?;
    let mut caps = conn.read(|r| match r {
        Response::Data {
            status: Status::Ok | Status::PreAuth,
            ..
        } => Ok(caps_in(r).unwrap_or_default()),
        Response::Data {
            status: Status::Bye,
            outcome,
        } => Err(SessErr::Fail(format!(
            "the server refused the connection: {}",
            outcome.information.as_deref().unwrap_or("").trim()
        ))),
        _ => Err(SessErr::Fail("unexpected greeting".into())),
    })??;
    if caps.is_empty() {
        caps = capabilities(&mut conn)?;
    }
    if account.security == Security::Starttls {
        if !caps.iter().any(|c| c == "STARTTLS") {
            return Err(SessErr::Fail("the server does not offer STARTTLS".into()));
        }
        conn.command("STARTTLS", SessErr::Fail, |_| {})?;
        let Conn { stream, tcp, .. } = conn;
        let plain = match stream {
            Stream::Plain(s) => s,
            Stream::Tls(_) => return Err(SessErr::Fail("already encrypted".into())),
        };
        conn = Conn {
            stream: wrap_tls(plain, account)?,
            tcp,
            buf: Vec::new(),
            tag: 100,
        };
        caps = capabilities(&mut conn)?;
    }
    login(&mut conn, &caps, account.username.trim(), password)?;
    caps = capabilities(&mut conn)?;
    let push = caps.iter().any(|c| c == "IDLE");

    let folder = quoted(account.folder.trim())
        .ok_or_else(|| SessErr::Fail("folder names must be ASCII".into()))?;
    let mut uid_validity = 0u32;
    conn.command(&format!("EXAMINE {folder}"), SessErr::Fail, |r| {
        if let Response::Data { outcome, .. } = r {
            if let Some(ResponseCode::UidValidity(v)) = outcome.code {
                uid_validity = v;
            }
        }
    })?;
    status(report, account, AccountStatus::Online { push });

    loop {
        refresh(&mut conn, account, uid_validity, opts, report)?;
        if ctl.stopped() {
            return Ok(());
        }
        if push {
            idle(&mut conn, ctl)?;
        } else if !ctl.sleep(opts.poll) {
            return Ok(());
        }
        if ctl.stopped() {
            return Ok(());
        }
    }
}

fn refresh(
    conn: &mut Conn,
    account: &MailAccount,
    uid_validity: u32,
    opts: Options,
    report: &Report,
) -> SessResult<()> {
    let mut uids: Vec<u32> = Vec::new();
    conn.command("UID SEARCH UNSEEN", SessErr::Fail, |r| {
        if let Response::MailboxData(MailboxDatum::Search(v)) = r {
            uids.extend(v);
        }
    })?;
    uids.sort_unstable();
    uids.dedup();
    let newest: Vec<u32> = uids.iter().rev().take(opts.max_items).copied().collect();
    let mut items: Vec<MailItem> = Vec::new();
    if !newest.is_empty() {
        let set = newest
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        conn.command(
            &format!("UID FETCH {set} (UID INTERNALDATE ENVELOPE)"),
            SessErr::Fail,
            |r| {
                if let Response::Fetch(_, attrs) = r {
                    if let Some(item) = item_from(attrs) {
                        items.push(item);
                    }
                }
            },
        )?;
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.uid));
    items.dedup_by_key(|i| i.uid);
    report(MailUpdate::Snapshot {
        account: account.name.clone(),
        uid_validity,
        unread: u32::try_from(uids.len()).unwrap_or(u32::MAX),
        items,
    });
    Ok(())
}

fn lossy(b: &Option<std::borrow::Cow<'_, [u8]>>) -> Option<String> {
    b.as_ref().map(|b| String::from_utf8_lossy(b).into_owned())
}

fn item_from(attrs: &[AttributeValue<'_>]) -> Option<MailItem> {
    let mut uid = None;
    let mut date = None;
    let mut from = String::new();
    let mut subject = String::new();
    for a in attrs {
        match a {
            AttributeValue::Uid(u) => uid = Some(*u),
            AttributeValue::InternalDate(d) => date = parse_internal_date(d),
            AttributeValue::Envelope(env) => {
                subject = lossy(&env.subject)
                    .map(|s| decode_header(&s))
                    .unwrap_or_default();
                if let Some(a) = env.from.as_ref().and_then(|f| f.first()) {
                    from = display_address(
                        lossy(&a.name).as_deref(),
                        lossy(&a.mailbox).as_deref(),
                        lossy(&a.host).as_deref(),
                    );
                }
            }
            _ => {}
        }
    }
    let subject = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(MailItem {
        uid: uid?,
        from: if from.is_empty() {
            "(unknown sender)".into()
        } else {
            from
        },
        subject: if subject.is_empty() {
            "(no subject)".into()
        } else {
            subject
        },
        date_unix: date,
    })
}

/// IDLE until the mailbox changes, the keepalive interval passes, or we are
/// stopped (the socket is shut down, which ends the read with an error).
fn idle(conn: &mut Conn, ctl: &Ctl) -> SessResult<()> {
    let tag = conn.next_tag();
    conn.send_line(&format!("{tag} IDLE"))?;
    let accepted =
        conn.read(|r| !matches!(r, Response::Done { status, .. } if *status != Status::Ok))?;
    if !accepted {
        return Err(SessErr::Fail("the server refused IDLE".into()));
    }
    conn.tcp.set_read_timeout(Some(IDLE_KEEPALIVE))?;
    let waited = loop {
        let r = conn.read(|r| match r {
            Response::MailboxData(MailboxDatum::Exists(_) | MailboxDatum::Recent(_))
            | Response::Expunge(_)
            | Response::Fetch(..)
            | Response::Vanished { .. } => Ok(true),
            Response::Data {
                status: Status::Bye,
                ..
            } => Err(SessErr::Fail("the server closed the session".into())),
            _ => Ok(false),
        });
        match r {
            Ok(Ok(true)) => break Ok(()),
            Ok(Ok(false)) => continue,
            Ok(Err(e)) => break Err(e),
            Err(_) if ctl.stopped() => return Ok(()),
            // Keepalive: the read timed out with nothing to report.
            Err(SessErr::Timeout) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    waited?;
    conn.tcp.set_read_timeout(Some(COMMAND_TIMEOUT))?;
    conn.send_line("DONE")?;
    conn.finish(&tag, SessErr::Fail, &mut |_| {})
}

/// Runs one watcher per configured account and restarts exactly the ones whose
/// settings (or password) changed.
#[derive(Default)]
pub struct MailHub {
    running: Vec<(MailAccount, Options, MailWatcher)>,
}

impl MailHub {
    /// Match the running watchers to `cfg` (`active` = feature on).
    pub fn sync(
        &mut self,
        cfg: &MailConfig,
        active: bool,
        secrets: &Arc<dyn SecretStore>,
        report: &Report,
    ) {
        let opts = Options::from(cfg);
        let want: Vec<&MailAccount> = if active {
            cfg.accounts.iter().collect()
        } else {
            Vec::new()
        };
        self.running
            .retain(|(a, o, _)| *o == opts && want.contains(&a));
        for a in want {
            if !self.running.iter().any(|(r, _, _)| r == a) {
                let w = start(a.clone(), secrets.clone(), opts, report.clone());
                self.running.push((a.clone(), opts, w));
            }
        }
    }

    /// Restart one account (its password changed).
    pub fn restart(&mut self, name: &str, secrets: &Arc<dyn SecretStore>, report: &Report) {
        if let Some(i) = self.running.iter().position(|(a, _, _)| a.name == name) {
            let (a, o, w) = self.running.remove(i);
            drop(w);
            let nw = start(a.clone(), secrets.clone(), o, report.clone());
            self.running.push((a, o, nw));
        }
    }

    /// Number of running watchers.
    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// No watchers.
    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }
}
