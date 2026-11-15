//! The device's position from Windows location services (`Geolocator`).
//!
//! An unpackaged desktop app may read it when the user allows it in
//! Settings > Privacy & security > Location ("Let desktop apps access your
//! location"). Windows uses Wi-Fi, GPS or IP as it sees fit; Perch only
//! receives the coordinates, once per call, on the calling thread.

use std::time::Duration;

use perch_core::error::{Error, Result};
use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
use windows::Foundation::TimeSpan;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Latitude and longitude in degrees. Blocks for up to `timeout`.
pub fn locate(timeout: Duration) -> Result<(f64, f64)> {
    let unavailable = |m: String| Error::unavailable("location", m);
    // SAFETY: joins this (worker) thread to the multithreaded apartment for
    // the WinRT calls below; harmless if it already is.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let access = Geolocator::RequestAccessAsync()
        .and_then(|op| op.get())
        .map_err(|e| unavailable(format!("{e}")))?;
    if access != GeolocationAccessStatus::Allowed {
        return Err(unavailable(
            "location is off for desktop apps: Settings > Privacy & security > Location".into(),
        ));
    }
    let g = Geolocator::new().map_err(|e| unavailable(format!("{e}")))?;
    let _ = g.SetDesiredAccuracy(PositionAccuracy::High);
    // A fix up to 10 minutes old is fine for a place name.
    let ticks = |d: Duration| TimeSpan {
        Duration: i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX),
    };
    let pos = g
        .GetGeopositionAsyncWithAgeAndTimeout(ticks(Duration::from_secs(600)), ticks(timeout))
        .and_then(|op| op.get())
        .and_then(|p| p.Coordinate())
        .and_then(|c| c.Point())
        .and_then(|p| p.Position())
        .map_err(|e| unavailable(format!("no position: {e}")))?;
    if !(-90.0..=90.0).contains(&pos.Latitude) || !(-180.0..=180.0).contains(&pos.Longitude) {
        return Err(unavailable("position out of range".into()));
    }
    Ok((pos.Latitude, pos.Longitude))
}
