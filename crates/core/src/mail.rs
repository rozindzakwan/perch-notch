//! Read-only mail summary: per-account unread state, new-mail detection,
//! header decoding and provider presets. No I/O here; the IMAP client lives in
//! the app crate and feeds [`MailUpdate`]s in.

use serde::{Deserialize, Serialize};

/// One unread message, as shown in the Mail tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailItem {
    /// IMAP UID (unique within the folder while UIDVALIDITY holds).
    pub uid: u32,
    /// Sender display name (or address).
    pub from: String,
    /// Decoded subject.
    pub subject: String,
    /// When the server received it (unix seconds), if known.
    pub date_unix: Option<u64>,
}

/// Connection state of one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountStatus {
    /// Connecting or signing in.
    Connecting,
    /// Signed in; `push` = the server supports IDLE (no polling).
    Online {
        /// New mail is pushed (IMAP IDLE) rather than checked periodically.
        push: bool,
    },
    /// No password stored for this account.
    NoPassword,
    /// The server rejected the sign-in; Perch stops retrying until the
    /// password or the account settings change.
    AuthFailed(String),
    /// Network or protocol trouble; Perch retries with a growing delay.
    Error(String),
}

impl AccountStatus {
    /// Short text for the Mail tab and the settings window.
    pub fn label(&self) -> String {
        match self {
            AccountStatus::Connecting => "connecting…".into(),
            AccountStatus::Online { push: true } => "online".into(),
            AccountStatus::Online { push: false } => "online (checks every few minutes)".into(),
            AccountStatus::NoPassword => "no password saved".into(),
            AccountStatus::AuthFailed(m) => format!("sign-in failed: {m}"),
            AccountStatus::Error(m) => format!("offline: {m}"),
        }
    }

    /// Whether this is a problem worth showing above the messages.
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            AccountStatus::NoPassword | AccountStatus::AuthFailed(_) | AccountStatus::Error(_)
        )
    }
}

/// What the IMAP client reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailUpdate {
    /// Connection state changed.
    Status {
        /// Account name.
        account: String,
        /// New state.
        status: AccountStatus,
    },
    /// The unread set as of now.
    Snapshot {
        /// Account name.
        account: String,
        /// The folder's UIDVALIDITY; a change means UIDs were reassigned.
        uid_validity: u32,
        /// Total unread messages.
        unread: u32,
        /// The newest unread messages, newest first.
        items: Vec<MailItem>,
    },
}

/// One account's live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountState {
    /// Account name.
    pub name: String,
    /// Connection state.
    pub status: AccountStatus,
    /// Unread count.
    pub unread: u32,
    /// Newest unread messages, newest first.
    pub items: Vec<MailItem>,
    baseline: Option<(u32, u32)>, // (uid_validity, highest uid seen)
}

/// All accounts. Feeds the Mail tab, the pill and new-mail notifications.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailBoard {
    accounts: Vec<AccountState>,
}

impl MailBoard {
    /// Track exactly these accounts (keeps state for names that stay).
    pub fn set_accounts(&mut self, names: &[String]) {
        self.accounts.retain(|a| names.contains(&a.name));
        for n in names {
            if !self.accounts.iter().any(|a| &a.name == n) {
                self.accounts.push(AccountState {
                    name: n.clone(),
                    status: AccountStatus::Connecting,
                    unread: 0,
                    items: Vec::new(),
                    baseline: None,
                });
            }
        }
        self.accounts
            .sort_by_key(|a| names.iter().position(|n| n == &a.name));
    }

    /// Apply an update. Returns messages that arrived since the previous
    /// snapshot (never on the first snapshot of an account, so starting Perch
    /// does not announce the whole inbox).
    pub fn apply(&mut self, update: MailUpdate) -> Vec<(String, MailItem)> {
        match update {
            MailUpdate::Status { account, status } => {
                if let Some(a) = self.accounts.iter_mut().find(|a| a.name == account) {
                    a.status = status;
                }
                Vec::new()
            }
            MailUpdate::Snapshot {
                account,
                uid_validity,
                unread,
                items,
            } => {
                let Some(a) = self.accounts.iter_mut().find(|a| a.name == account) else {
                    return Vec::new();
                };
                let max = items.iter().map(|i| i.uid).max().unwrap_or(0);
                let fresh: Vec<(String, MailItem)> = match a.baseline {
                    Some((v, seen)) if v == uid_validity => items
                        .iter()
                        .filter(|i| i.uid > seen)
                        .map(|i| (a.name.clone(), i.clone()))
                        .collect(),
                    _ => Vec::new(),
                };
                let seen = match a.baseline {
                    Some((v, seen)) if v == uid_validity => seen.max(max),
                    _ => max,
                };
                a.baseline = Some((uid_validity, seen));
                a.unread = unread;
                a.items = items;
                fresh
            }
        }
    }

    /// Every account.
    pub fn accounts(&self) -> &[AccountState] {
        &self.accounts
    }

    /// Unread messages across accounts.
    pub fn total_unread(&self) -> u32 {
        self.accounts.iter().map(|a| a.unread).sum()
    }

    /// Unread messages of every account, newest first.
    pub fn items(&self) -> Vec<(&str, &MailItem)> {
        let mut all: Vec<(&str, &MailItem)> = self
            .accounts
            .iter()
            .flat_map(|a| a.items.iter().map(move |i| (a.name.as_str(), i)))
            .collect();
        all.sort_by(|x, y| {
            y.1.date_unix
                .cmp(&x.1.date_unix)
                .then(y.1.uid.cmp(&x.1.uid))
        });
        all
    }

    /// Forget everything (feature turned off).
    pub fn clear(&mut self) {
        self.accounts.clear();
    }
}

/// How to secure the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// TLS from the first byte (port 993). The default.
    #[default]
    Tls,
    /// Plain connection upgraded with STARTTLS (port 143).
    Starttls,
    /// No encryption. Only allowed to localhost (for testing).
    Plain,
}

/// A known provider's IMAP settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailPreset {
    /// Stable id.
    pub id: &'static str,
    /// Shown in the add-account dialog.
    pub label: &'static str,
    /// IMAP host (empty for "Other").
    pub host: &'static str,
    /// IMAP port.
    pub port: u16,
    /// What to put in the password field.
    pub hint: &'static str,
}

/// Providers offered in the add-account dialog. All of them accept a password
/// (or app password) over TLS; Microsoft 365 / Outlook.com need OAuth and are
/// not listed (see the README).
pub const PRESETS: &[MailPreset] = &[
    MailPreset {
        id: "spacemail",
        label: "Spacemail (Namecheap)",
        host: "mail.spacemail.com",
        port: 993,
        hint: "your mailbox password",
    },
    MailPreset {
        id: "gmail",
        label: "Gmail / Google Workspace",
        host: "imap.gmail.com",
        port: 993,
        hint: "an app password (Google Account > Security > App passwords)",
    },
    MailPreset {
        id: "yahoo",
        label: "Yahoo Mail",
        host: "imap.mail.yahoo.com",
        port: 993,
        hint: "an app password from Yahoo account security",
    },
    MailPreset {
        id: "icloud",
        label: "iCloud Mail",
        host: "imap.mail.me.com",
        port: 993,
        hint: "an app-specific password (appleid.apple.com)",
    },
    MailPreset {
        id: "fastmail",
        label: "Fastmail",
        host: "imap.fastmail.com",
        port: 993,
        hint: "an app password (Settings > Privacy & Security)",
    },
    MailPreset {
        id: "zoho",
        label: "Zoho Mail",
        host: "imap.zoho.com",
        port: 993,
        hint: "your password or an app password",
    },
    MailPreset {
        id: "other",
        label: "Other (IMAP over TLS)",
        host: "",
        port: 993,
        hint: "your mailbox password",
    },
];

/// Guess a preset from an address's domain (`None` = unknown, use "Other").
pub fn preset_for_address(address: &str) -> Option<&'static MailPreset> {
    let domain = address.rsplit_once('@')?.1.trim().to_ascii_lowercase();
    let id = match domain.as_str() {
        "gmail.com" | "googlemail.com" => "gmail",
        "yahoo.com" | "ymail.com" => "yahoo",
        "icloud.com" | "me.com" | "mac.com" => "icloud",
        "fastmail.com" | "fastmail.fm" => "fastmail",
        "zoho.com" | "zohomail.com" => "zoho",
        _ => return None,
    };
    PRESETS.iter().find(|p| p.id == id)
}

/// Whether a host is the local machine (the only place plaintext IMAP is
/// allowed to go).
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Decode RFC 2047 encoded words (`=?utf-8?B?...?=`, `=?iso-8859-1?Q?...?=`)
/// in a header value. Unknown charsets and malformed words are kept as is.
/// Whitespace between two adjacent encoded words is dropped, as the RFC says.
pub fn decode_header(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        match parse_word(tail) {
            Some((decoded, used)) => {
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                rest = &tail[used..];
                last_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &tail[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `=?charset?enc?text?=` at the start of `s` → (decoded, bytes consumed).
fn parse_word(s: &str) -> Option<(String, usize)> {
    let body = s.strip_prefix("=?")?;
    let (charset, body) = body.split_once('?')?;
    let (enc, body) = body.split_once('?')?;
    let end = body.find("?=")?;
    let text = &body[..end];
    if text.contains(' ') {
        return None;
    }
    let used = 2 + charset.len() + 1 + enc.len() + 1 + end + 2;
    let bytes = match enc.to_ascii_uppercase().as_str() {
        "B" => base64_decode(text)?,
        "Q" => q_decode(text),
        _ => return None,
    };
    // RFC 2231 language suffix: "utf-8*en".
    let charset = charset.split('*').next().unwrap_or(charset);
    Some((decode_charset(charset, &bytes)?, used))
}

fn decode_charset(charset: &str, bytes: &[u8]) -> Option<String> {
    match charset.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => {
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        // Latin-1 maps byte-for-byte to U+0000..U+00FF; windows-1252 differs
        // only in 0x80..0x9F, which are rare in headers.
        "iso-8859-1" | "latin1" | "iso-8859-15" | "windows-1252" | "cp1252" => {
            Some(bytes.iter().map(|&b| char::from(b)).collect())
        }
        _ => None,
    }
}

fn q_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => out.push(b' '),
            b'=' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 2;
                }
                _ => out.push(b'='),
            },
            c => out.push(c),
        }
        i += 1;
    }
    out
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Standard base64 (padding optional). `None` on invalid input.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.len();
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let v = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= n {
                out.push(char::from(T[(v >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How a sender is shown: the decoded display name, else `mailbox@host`.
pub fn display_address(name: Option<&str>, mailbox: Option<&str>, host: Option<&str>) -> String {
    if let Some(n) = name.map(decode_header) {
        let n = n.trim().trim_matches('"').trim();
        if !n.is_empty() {
            return n.to_string();
        }
    }
    match (mailbox, host) {
        (Some(m), Some(h)) => format!("{m}@{h}"),
        (Some(m), None) => m.to_string(),
        _ => "(unknown sender)".to_string(),
    }
}

/// Parse an IMAP INTERNALDATE (`"14-Nov-2026 13:05:22 +0800"`) to unix seconds.
pub fn parse_internal_date(s: &str) -> Option<u64> {
    let s = s.trim().trim_matches('"').trim();
    let (date, rest) = s.split_once(' ')?;
    let (time, zone) = rest.trim().split_once(' ')?;
    let mut d = date.split('-');
    let day: i64 = d.next()?.trim().parse().ok()?;
    let mon = d.next()?;
    let year: i64 = d.next()?.parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| m.eq_ignore_ascii_case(mon))? as i64
        + 1;
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let se: i64 = t.next()?.parse().ok()?;
    let sign = match zone.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let zh: i64 = zone.get(1..3)?.parse().ok()?;
    let zm: i64 = zone.get(3..5)?.parse().ok()?;
    if !(1..=31).contains(&day) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let local = days * 86_400 + h * 3600 + mi * 60 + se;
    u64::try_from(local - sign * (zh * 3600 + zm * 60)).ok()
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(uid: u32) -> MailItem {
        MailItem {
            uid,
            from: format!("Sender {uid}"),
            subject: format!("Subject {uid}"),
            date_unix: Some(1_700_000_000 + u64::from(uid)),
        }
    }

    fn snap(uidv: u32, uids: &[u32]) -> MailUpdate {
        MailUpdate::Snapshot {
            account: "Work".into(),
            uid_validity: uidv,
            unread: uids.len() as u32,
            items: uids.iter().rev().map(|u| item(*u)).collect(),
        }
    }

    #[test]
    fn first_snapshot_announces_nothing_later_ones_announce_new_uids() {
        let mut b = MailBoard::default();
        b.set_accounts(&["Work".into()]);
        assert!(b.apply(snap(7, &[1, 2, 3])).is_empty());
        assert_eq!(b.total_unread(), 3);
        let new = b.apply(snap(7, &[1, 2, 3, 5]));
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].1.uid, 5);
        // Reading mail elsewhere shrinks the set; nothing to announce.
        assert!(b.apply(snap(7, &[5])).is_empty());
        assert_eq!(b.total_unread(), 1);
        // A message marked unread again (lower UID) is not "new".
        assert!(b.apply(snap(7, &[2, 5])).is_empty());
    }

    #[test]
    fn uidvalidity_change_resets_the_baseline_quietly() {
        let mut b = MailBoard::default();
        b.set_accounts(&["Work".into()]);
        b.apply(snap(7, &[10]));
        assert!(b.apply(snap(8, &[1, 2])).is_empty());
        assert_eq!(b.apply(snap(8, &[1, 2, 3])).len(), 1);
    }

    #[test]
    fn accounts_follow_the_config_and_items_merge_newest_first() {
        let mut b = MailBoard::default();
        b.set_accounts(&["Work".into(), "Home".into()]);
        b.apply(MailUpdate::Snapshot {
            account: "Home".into(),
            uid_validity: 1,
            unread: 1,
            items: vec![item(50)],
        });
        b.apply(snap(1, &[1]));
        let items = b.items();
        assert_eq!(items[0].0, "Home");
        assert_eq!(b.total_unread(), 2);
        b.set_accounts(&["Work".into()]);
        assert_eq!(b.accounts().len(), 1);
        assert_eq!(b.total_unread(), 1);
        b.apply(MailUpdate::Status {
            account: "Work".into(),
            status: AccountStatus::AuthFailed("bad password".into()),
        });
        assert!(b.accounts()[0].status.is_problem());
    }

    #[test]
    fn decodes_encoded_words() {
        assert_eq!(decode_header("=?UTF-8?B?SGFsbyDwn5iA?="), "Halo 😀");
        assert_eq!(
            decode_header("=?iso-8859-1?Q?Caf=E9_cr=E8me?="),
            "Café crème"
        );
        assert_eq!(
            decode_header("=?utf-8?q?Hello?= =?utf-8?q?_World?="),
            "Hello World",
            "whitespace between encoded words is dropped"
        );
        assert_eq!(
            decode_header("Re: =?utf-8?B?w6l0w6k=?= plans"),
            "Re: été plans"
        );
        assert_eq!(decode_header("plain subject"), "plain subject");
        assert_eq!(
            decode_header("=?x-unknown?B?QUJD?= ok"),
            "=?x-unknown?B?QUJD?= ok"
        );
        assert_eq!(decode_header("odd =? text"), "odd =? text");
    }

    #[test]
    fn base64_round_trips() {
        for s in ["", "a", "ab", "abc", "\0user\0p@ss wörd"] {
            let enc = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(&enc).as_deref(), Some(s.as_bytes()), "{s:?}");
        }
        assert_eq!(base64_encode(b"\0u\0p"), "AHUAcA==");
    }

    #[test]
    fn sender_display() {
        assert_eq!(
            display_address(Some("\"Ann Lee\""), Some("ann"), Some("x.com")),
            "Ann Lee"
        );
        assert_eq!(
            display_address(Some("=?utf-8?Q?J=C3=BCrgen?="), None, None),
            "Jürgen"
        );
        assert_eq!(
            display_address(None, Some("bob"), Some("y.org")),
            "bob@y.org"
        );
        assert_eq!(
            display_address(Some("  "), Some("bob"), Some("y.org")),
            "bob@y.org"
        );
    }

    #[test]
    fn internal_date() {
        // 2026-11-14 13:05:22 +08:00 = 05:05:22 UTC.
        assert_eq!(
            parse_internal_date("14-Nov-2026 13:05:22 +0800"),
            Some(1_794_632_722)
        );
        assert_eq!(parse_internal_date(" 1-Jan-1970 00:00:00 +0000"), Some(0));
        assert_eq!(
            parse_internal_date("\"31-Dec-1999 23:59:59 -0500\""),
            Some(946_702_799)
        );
        assert_eq!(parse_internal_date("garbage"), None);
    }

    #[test]
    fn presets_and_loopback() {
        assert_eq!(
            preset_for_address("me@gmail.com").map(|p| p.host),
            Some("imap.gmail.com")
        );
        assert!(preset_for_address("someone@example.org").is_none());
        assert!(PRESETS.iter().any(|p| p.host == "mail.spacemail.com"));
        assert!(is_loopback("localhost") && is_loopback("127.0.0.1") && is_loopback("[::1]"));
        assert!(!is_loopback("mail.spacemail.com") && !is_loopback("10.0.0.1"));
    }
}
