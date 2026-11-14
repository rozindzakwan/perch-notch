//! Camera capture for the mirror (feature `camera`): Media Foundation on
//! Windows, V4L2 on Linux, via `nokhwa`. The device is opened only while the
//! mirror is shown, and released as soon as it closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use perch_ui::mirror::FrameSlot;
use winit::event_loop::EventLoopProxy;

use crate::shell::UserEvent;

/// Largest frame width kept; bigger frames are downscaled to save CPU.
const MAX_WIDTH: u32 = 640;

/// A running capture thread. Dropping it stops capture and frees the camera.
pub struct Capture {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Open the default camera and stream frames into `slot`.
pub fn start(slot: FrameSlot, proxy: EventLoopProxy<UserEvent>) -> Result<Capture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let join = std::thread::Builder::new()
        .name("perch-camera".into())
        .spawn(move || {
            if let Err(e) = run(&slot, &proxy, &flag) {
                let _ = proxy.send_event(UserEvent::CameraStopped(e));
            }
        })
        .map_err(|e| format!("cannot start the camera thread: {e}"))?;
    Ok(Capture {
        stop,
        join: Some(join),
    })
}

fn run(
    slot: &FrameSlot,
    proxy: &EventLoopProxy<UserEvent>,
    stop: &AtomicBool,
) -> Result<(), String> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
    let fmt = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut cam =
        nokhwa::Camera::new(CameraIndex::Index(0), fmt).map_err(|e| format!("no camera: {e}"))?;
    cam.open_stream()
        .map_err(|e| format!("camera busy or blocked: {e}"))?;
    while !stop.load(Ordering::Relaxed) {
        let frame = cam.frame().map_err(|e| format!("camera stopped: {e}"))?;
        let img = frame
            .decode_image::<RgbFormat>()
            .map_err(|e| format!("camera frame: {e}"))?;
        let img = if img.width() > MAX_WIDTH {
            let h = img.height() * MAX_WIDTH / img.width().max(1);
            image::imageops::resize(
                &img,
                MAX_WIDTH,
                h.max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };
        let (w, h) = img.dimensions();
        if let Ok(mut s) = slot.lock() {
            *s = Some((img.into_raw(), w, h));
        }
        if proxy.send_event(UserEvent::CameraFrame).is_err() {
            break; // event loop gone
        }
    }
    let _ = cam.stop_stream();
    Ok(())
}
