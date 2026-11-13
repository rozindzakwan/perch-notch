//! Now playing via MPRIS (`org.mpris.MediaPlayer2.*` on the session bus).
//!
//! Event-driven: two threads block on D-Bus signal iterators, one for
//! `NameOwnerChanged` (players appearing/disappearing) and one for the
//! players' `PropertiesChanged` / `Seeked` signals. On a signal the player's
//! state is re-read with one `GetAll` call. The "active" player is the most
//! recently changed one that is playing, else the most recently changed one.
//!
//! Cover art is only read from local `file://` URLs. Fetching a player's
//! `https://` art URL would be a network request the user did not opt into.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use perch_core::error::{Error, Result};
use perch_core::platform::{
    EventSink, MediaControl, MediaEvent, PlatformEvent, Track, TransportCommand,
};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const MAX_COVER: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
struct Player {
    unique: String,
    track: Option<Track>,
    track_id: Option<String>,
    changed: u64,
}

#[derive(Debug, Default)]
struct State {
    players: BTreeMap<String, Player>,
    tick: u64,
    active: Option<String>,
    last_reported: Option<Option<Track>>,
}

impl State {
    fn pick_active(&self) -> Option<String> {
        let playing = self
            .players
            .iter()
            .filter(|(_, p)| p.track.as_ref().is_some_and(|t| t.playing))
            .max_by_key(|(_, p)| p.changed);
        let any = self
            .players
            .iter()
            .filter(|(_, p)| p.track.is_some())
            .max_by_key(|(_, p)| p.changed);
        playing.or(any).map(|(n, _)| n.clone())
    }

    fn name_for_unique(&self, unique: &str) -> Option<String> {
        self.players
            .iter()
            .find(|(_, p)| p.unique == unique)
            .map(|(n, _)| n.clone())
    }
}

/// MPRIS-backed [`MediaControl`].
pub struct MprisMedia {
    conn: Connection,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for MprisMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MprisMedia").finish_non_exhaustive()
    }
}

impl MprisMedia {
    /// Connect to the session bus, scan for players and start listening.
    pub fn start(sink: EventSink) -> Result<Self> {
        let conn = Connection::session()
            .map_err(|e| Error::unavailable("media", format!("session bus: {e}")))?;
        let state = Arc::new(Mutex::new(State::default()));
        Self::start_on(conn, state, sink)
    }

    fn start_on(conn: Connection, state: Arc<Mutex<State>>, sink: EventSink) -> Result<Self> {
        // Initial scan.
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)
            .map_err(|e| Error::backend(format!("D-Bus proxy: {e}")))?;
        let names = dbus
            .list_names()
            .map_err(|e| Error::backend(format!("ListNames: {e}")))?;
        for name in names
            .iter()
            .map(|n| n.to_string())
            .filter(|n| n.starts_with(PREFIX))
        {
            if let Some(unique) = owner_of(&dbus, &name) {
                refresh_player(&conn, &state, &name, &unique);
            }
        }
        report(&state, &sink);

        // Players appearing / disappearing.
        let (c, s, k) = (conn.clone(), state.clone(), sink.clone());
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")
            .and_then(|b| b.interface("org.freedesktop.DBus"))
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg0ns("org.mpris.MediaPlayer2"))
            .map(|b| b.build())
            .map_err(|e| Error::backend(format!("match rule: {e}")))?;
        let names_iter = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64))
            .map_err(|e| Error::backend(format!("subscribe NameOwnerChanged: {e}")))?;
        std::thread::Builder::new()
            .name("perch-mpris-names".into())
            .spawn(move || {
                for msg in names_iter {
                    let Ok(msg) = msg else { continue };
                    let Ok((name, _old, new)) =
                        msg.body().deserialize::<(String, String, String)>()
                    else {
                        continue;
                    };
                    if !name.starts_with(PREFIX) {
                        continue;
                    }
                    if new.is_empty() {
                        if let Ok(mut st) = s.lock() {
                            st.players.remove(&name);
                        }
                    } else {
                        refresh_player(&c, &s, &name, &new);
                    }
                    report(&s, &k);
                }
            })
            .map_err(|e| Error::io("spawning the MPRIS thread", e))?;

        // Property changes and seeks from any player.
        let (c, s, k) = (conn.clone(), state.clone(), sink);
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .path(PATH)
            .map(|b| b.build())
            .map_err(|e| Error::backend(format!("match rule: {e}")))?;
        let props_iter = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64))
            .map_err(|e| Error::backend(format!("subscribe player signals: {e}")))?;
        std::thread::Builder::new()
            .name("perch-mpris-props".into())
            .spawn(move || {
                for msg in props_iter {
                    let Ok(msg) = msg else { continue };
                    let header = msg.header();
                    let Some(sender) = header.sender().map(|s| s.to_string()) else {
                        continue;
                    };
                    let name = s.lock().ok().and_then(|st| st.name_for_unique(&sender));
                    let Some(name) = name else { continue };
                    refresh_player(&c, &s, &name, &sender);
                    report(&s, &k);
                }
            })
            .map_err(|e| Error::io("spawning the MPRIS thread", e))?;

        Ok(Self { conn, state })
    }

    fn active(&self) -> Option<(String, Option<String>)> {
        let st = self.state.lock().ok()?;
        let name = st.active.clone()?;
        let tid = st.players.get(&name).and_then(|p| p.track_id.clone());
        Some((name, tid))
    }
}

fn owner_of(dbus: &zbus::blocking::fdo::DBusProxy<'_>, name: &str) -> Option<String> {
    let bus: zbus::names::BusName<'_> = name.try_into().ok()?;
    dbus.get_name_owner(bus).ok().map(|u| u.to_string())
}

/// Re-read one player's state with `GetAll`.
fn refresh_player(conn: &Connection, state: &Arc<Mutex<State>>, name: &str, unique: &str) {
    let reply = conn.call_method(
        Some(name),
        PATH,
        Some("org.freedesktop.DBus.Properties"),
        "GetAll",
        &(PLAYER_IFACE,),
    );
    let props: Option<HashMap<String, OwnedValue>> =
        reply.ok().and_then(|m| m.body().deserialize().ok());
    let (track, track_id) = match props {
        Some(p) => parse_player(&p),
        None => (None, None),
    };
    if let Ok(mut st) = state.lock() {
        st.tick += 1;
        let tick = st.tick;
        let entry = st.players.entry(name.to_string()).or_default();
        entry.unique = unique.to_string();
        // Only bump recency when something a user would notice changed.
        let notable = match (&entry.track, &track) {
            (Some(a), Some(b)) => a.title != b.title || a.playing != b.playing,
            (None, None) => false,
            _ => true,
        };
        entry.track = track;
        entry.track_id = track_id;
        if notable {
            entry.changed = tick;
        }
    }
}

/// Emit what changed for the active player.
fn report(state: &Arc<Mutex<State>>, sink: &EventSink) {
    let event = {
        let Ok(mut st) = state.lock() else { return };
        let active = st.pick_active();
        st.active = active.clone();
        let track = active.and_then(|n| st.players.get(&n).and_then(|p| p.track.clone()));
        if st.last_reported.as_ref() == Some(&track) {
            return;
        }
        st.last_reported = Some(track.clone());
        match track {
            Some(t) => MediaEvent::TrackChanged(t),
            None => MediaEvent::Gone,
        }
    };
    sink(PlatformEvent::Media(event));
}

fn unwrap_variant<'a>(v: &'a Value<'a>) -> &'a Value<'a> {
    match v {
        Value::Value(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn as_str(v: &Value<'_>) -> Option<String> {
    match unwrap_variant(v) {
        Value::Str(s) => Some(s.as_str().to_string()),
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        _ => None,
    }
}

fn as_i64(v: &Value<'_>) -> Option<i64> {
    match unwrap_variant(v) {
        Value::I64(x) => Some(*x),
        Value::U64(x) => i64::try_from(*x).ok(),
        Value::I32(x) => Some(i64::from(*x)),
        Value::U32(x) => Some(i64::from(*x)),
        Value::F64(x) => Some(*x as i64),
        _ => None,
    }
}

fn first_str(v: &Value<'_>) -> Option<String> {
    match unwrap_variant(v) {
        Value::Array(a) => a.iter().find_map(as_str),
        other => as_str(other),
    }
}

/// Turn a `GetAll` result into a [`Track`] plus the MPRIS track id. Pure.
pub fn parse_player_props(props: &HashMap<String, Value<'_>>) -> (Option<Track>, Option<String>) {
    let playing = props
        .get("PlaybackStatus")
        .and_then(as_str)
        .is_some_and(|s| s == "Playing");
    let position = props
        .get("Position")
        .and_then(as_i64)
        .map(|us| Duration::from_micros(us.max(0) as u64));
    let mut title = String::new();
    let mut artist = String::new();
    let mut album = String::new();
    let mut length = None;
    let mut art = None;
    let mut track_id = None;
    if let Some(Value::Dict(d)) = props.get("Metadata").map(unwrap_variant) {
        for (k, v) in d.iter() {
            let Some(key) = as_str(k) else { continue };
            match key.as_str() {
                "xesam:title" => title = as_str(v).unwrap_or_default(),
                "xesam:artist" => artist = first_str(v).unwrap_or_default(),
                "xesam:album" => album = as_str(v).unwrap_or_default(),
                "mpris:length" => {
                    length = as_i64(v)
                        .filter(|us| *us > 0)
                        .map(|us| Duration::from_micros(us as u64))
                }
                "mpris:artUrl" => art = as_str(v),
                "mpris:trackid" => track_id = as_str(v),
                _ => {}
            }
        }
    }
    if title.is_empty() && artist.is_empty() {
        return (None, track_id);
    }
    let cover = art.as_deref().and_then(read_local_art);
    (
        Some(Track {
            title,
            artist,
            album,
            duration: length,
            position,
            playing,
            cover,
        }),
        track_id,
    )
}

fn parse_player(props: &HashMap<String, OwnedValue>) -> (Option<Track>, Option<String>) {
    let borrowed: HashMap<String, Value<'_>> = props
        .iter()
        .filter_map(|(k, v)| v.try_clone().ok().map(|v| (k.clone(), Value::from(v))))
        .collect();
    parse_player_props(&borrowed)
}

/// Read cover art from a local `file://` URL only. Pure apart from the read.
pub fn read_local_art(url: &str) -> Option<Vec<u8>> {
    let path = url.strip_prefix("file://")?;
    let path = percent_decode(path);
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_COVER {
        return None;
    }
    std::fs::read(path).ok()
}

/// Decode `%xx` escapes in a file URL path. Pure.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl MediaControl for MprisMedia {
    fn now_playing(&self) -> Option<Track> {
        let st = self.state.lock().ok()?;
        let name = st.active.clone()?;
        st.players.get(&name).and_then(|p| p.track.clone())
    }

    fn send(&mut self, cmd: TransportCommand) -> Result<()> {
        let (name, track_id) = self
            .active()
            .ok_or_else(|| Error::unavailable("media", "no player"))?;
        let call = |method: &str| {
            self.conn
                .call_method(Some(name.as_str()), PATH, Some(PLAYER_IFACE), method, &())
                .map(drop)
        };
        let r = match cmd {
            TransportCommand::Play => call("Play"),
            TransportCommand::Pause => call("Pause"),
            TransportCommand::PlayPause => call("PlayPause"),
            TransportCommand::Next => call("Next"),
            TransportCommand::Previous => call("Previous"),
            TransportCommand::SeekTo(pos) => {
                let us = i64::try_from(pos.as_micros()).unwrap_or(i64::MAX);
                match track_id
                    .as_deref()
                    .and_then(|t| zbus::zvariant::ObjectPath::try_from(t).ok())
                {
                    Some(tid) => self
                        .conn
                        .call_method(
                            Some(name.as_str()),
                            PATH,
                            Some(PLAYER_IFACE),
                            "SetPosition",
                            &(tid, us),
                        )
                        .map(drop),
                    None => Err(zbus::Error::Failure(
                        "player has no track id to seek in".into(),
                    )),
                }
            }
        };
        r.map_err(|e| Error::backend(format!("MPRIS {cmd:?}: {e}")))
    }
}
