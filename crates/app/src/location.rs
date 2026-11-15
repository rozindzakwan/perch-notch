//! Where the user is, for the Today tab (opt-in, uses the network).
//!
//! The OS location service gives coordinates (Windows `Geolocator`, Linux
//! GeoClue). They are rounded to about 100 m before anything leaves the
//! machine, then one OpenStreetMap Nominatim reverse-geocode request turns
//! them into a place name ("Dago, Bandung"). The model asks at most once an
//! hour; the answer lives in memory only, is never written to disk or logged,
//! and an unchanged position never repeats the request. The request carries
//! the coordinates and a generic user agent: nothing about the user.

use std::sync::Mutex;
use std::time::Duration;

/// How long the OS may take to find the position.
pub const LOCATE_TIMEOUT: Duration = Duration::from_secs(15);

/// Coordinates rounded to 3 decimals (about 110 m of latitude): enough for a
/// neighbourhood name, too coarse to pin a house.
pub fn round_coord(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The place name in a Nominatim `jsonv2` reverse answer: the local area and
/// the town ("Dago, Bandung"), else the town and region, else the first two
/// parts of the display name. Pure.
pub fn parse_place(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let addr = v.get("address");
    let first = |keys: &[&str]| -> Option<String> {
        let a = addr?;
        keys.iter()
            .filter_map(|k| a.get(*k).and_then(|s| s.as_str()))
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_string)
    };
    let local = first(&[
        "suburb",
        "city_district",
        "village",
        "neighbourhood",
        "quarter",
        "hamlet",
    ]);
    let town = first(&["city", "town", "municipality", "county"]);
    let region = first(&["state", "region", "country"]);
    let pair = |a: String, b: Option<String>| match b {
        Some(b) if b != a => format!("{a}, {b}"),
        _ => a,
    };
    match (local, town, region) {
        (Some(l), t, r) => Some(pair(l, t.or(r))),
        (None, Some(t), r) => Some(pair(t, r)),
        (None, None, Some(r)) => Some(r),
        (None, None, None) => {
            let name = v.get("display_name")?.as_str()?;
            let parts: Vec<&str> = name
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(2)
                .collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
    }
}

/// The last answer, keyed by rounded position (milli-degrees), so standing
/// still never repeats the network request. Memory only.
static LAST: Mutex<Option<((i64, i64), String)>> = Mutex::new(None);

fn key(lat: f64, lon: f64) -> (i64, i64) {
    ((lat * 1000.0).round() as i64, (lon * 1000.0).round() as i64)
}

#[cfg(windows)]
fn locate() -> perch_core::error::Result<(f64, f64)> {
    perch_platform_windows::location::locate(LOCATE_TIMEOUT)
}

#[cfg(target_os = "linux")]
fn locate() -> perch_core::error::Result<(f64, f64)> {
    perch_platform_linux::location::locate(LOCATE_TIMEOUT)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn locate() -> perch_core::error::Result<(f64, f64)> {
    Err(perch_core::error::Error::unavailable(
        "location",
        "no location service on this system",
    ))
}

#[cfg(feature = "location")]
fn reverse(lat: f64, lon: f64) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!(
            "Perch/",
            env!("CARGO_PKG_VERSION"),
            " (desktop notch app)"
        ))
        .build();
    let body = agent
        .get("https://nominatim.openstreetmap.org/reverse")
        .query("format", "jsonv2")
        .query("lat", &format!("{lat:.3}"))
        .query("lon", &format!("{lon:.3}"))
        .query("zoom", "14")
        .call()
        .map_err(|e| format!("place lookup: {e}"))?
        .into_string()
        .map_err(|e| format!("place lookup: {e}"))?;
    parse_place(&body).ok_or_else(|| "no place name for this position".into())
}

#[cfg(not(feature = "location"))]
fn reverse(_lat: f64, _lon: f64) -> Result<String, String> {
    Err("location is not compiled into this build".into())
}

/// Find the place on a worker thread and hand the answer to `done`.
pub fn locate_async(done: impl FnOnce(Result<String, String>) + Send + 'static) {
    let spawn = std::thread::Builder::new()
        .name("perch-location".into())
        .spawn(move || {
            let result = locate().map_err(|e| e.to_string()).and_then(|(lat, lon)| {
                let (lat, lon) = (round_coord(lat), round_coord(lon));
                let k = key(lat, lon);
                let cached = LAST.lock().ok().and_then(|l| {
                    l.as_ref()
                        .filter(|(at, _)| *at == k)
                        .map(|(_, p)| p.clone())
                });
                if let Some(place) = cached {
                    return Ok(place);
                }
                let place = reverse(lat, lon)?;
                if let Ok(mut l) = LAST.lock() {
                    *l = Some((k, place.clone()));
                }
                Ok(place)
            });
            done(result);
        });
    if let Err(e) = spawn {
        log::warn!("location thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_rounded_to_about_100_m() {
        assert_eq!(round_coord(-6.914_744), -6.915);
        assert_eq!(round_coord(107.609_81), 107.61);
    }

    #[test]
    fn place_names_prefer_the_local_area_and_town() {
        let j = r#"{"address":{"suburb":"Dago","city":"Bandung","state":"West Java","country":"Indonesia"}}"#;
        assert_eq!(parse_place(j).as_deref(), Some("Dago, Bandung"));
        let j = r#"{"address":{"town":"Lembang","state":"West Java"}}"#;
        assert_eq!(parse_place(j).as_deref(), Some("Lembang, West Java"));
        let j = r#"{"address":{"city":"Berlin","city_district":"Berlin","state":"Berlin"}}"#;
        assert_eq!(parse_place(j).as_deref(), Some("Berlin"));
        let j = r#"{"display_name":"Somewhere Road, Nowhere, Land"}"#;
        assert_eq!(parse_place(j).as_deref(), Some("Somewhere Road, Nowhere"));
        assert_eq!(parse_place(r#"{"error":"Unable to geocode"}"#), None);
        assert_eq!(parse_place("not json"), None);
    }
}
