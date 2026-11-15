//! The device's position from GeoClue 2 (system D-Bus).
//!
//! GeoClue is the location service on Mint, Ubuntu, Fedora and most
//! desktops; it decides the source (Wi-Fi, GPS, IP) and asks the desktop's
//! agent whether Perch may know. A client is started, its first `Location`
//! read, and the client stopped again: no standing subscription.

use std::time::{Duration, Instant};

use perch_core::error::{Error, Result};
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const SERVICE: &str = "org.freedesktop.GeoClue2";

/// `GCLUE_ACCURACY_LEVEL_STREET`: enough for a neighbourhood name.
const ACCURACY_STREET: u32 = 6;

/// How often the client is checked for its first fix while waiting.
const FIX_CHECK: Duration = Duration::from_millis(250);

fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(conn)
        .destination(SERVICE)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
}

/// Latitude and longitude in degrees. Blocks for up to `timeout`.
pub fn locate(timeout: Duration) -> Result<(f64, f64)> {
    let unavailable = |m: String| Error::unavailable("location", m);
    let bus = |e: zbus::Error| unavailable(format!("GeoClue: {e}"));
    let conn = Connection::system().map_err(|e| unavailable(format!("no system bus: {e}")))?;
    let manager = proxy(
        &conn,
        "/org/freedesktop/GeoClue2/Manager",
        "org.freedesktop.GeoClue2.Manager",
    )
    .map_err(bus)?;
    let path: OwnedObjectPath = manager.call("GetClient", &()).map_err(bus)?;
    let client = proxy(&conn, path.as_str(), "org.freedesktop.GeoClue2.Client").map_err(bus)?;
    client
        .set_property("DesktopId", "perch")
        .map_err(|e| unavailable(format!("GeoClue: {e}")))?;
    client
        .set_property("RequestedAccuracyLevel", ACCURACY_STREET)
        .map_err(|e| unavailable(format!("GeoClue: {e}")))?;
    client
        .call_method("Start", &())
        .map_err(|e| unavailable(format!("GeoClue refused (location off?): {e}")))?;
    let deadline = Instant::now() + timeout;
    let found = loop {
        let at: OwnedObjectPath = match client.get_property("Location") {
            Ok(p) => p,
            Err(e) => break Err(unavailable(format!("GeoClue: {e}"))),
        };
        if at.as_str() != "/" {
            break Ok(at);
        }
        if Instant::now() >= deadline {
            break Err(unavailable("GeoClue found no position in time".into()));
        }
        std::thread::sleep(FIX_CHECK);
    };
    let read = found.and_then(|at| {
        let loc = proxy(&conn, at.as_str(), "org.freedesktop.GeoClue2.Location").map_err(bus)?;
        let lat: f64 = loc.get_property("Latitude").map_err(bus)?;
        let lon: f64 = loc.get_property("Longitude").map_err(bus)?;
        Ok((lat, lon))
    });
    let _ = client.call_method("Stop", &());
    let (lat, lon) = read?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(unavailable("position out of range".into()));
    }
    Ok((lat, lon))
}
