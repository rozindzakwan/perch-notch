//! Opens the default camera for a moment, checks that frames arrive, and
//! releases it. Frames are discarded. Opt-in because it switches the camera on:
//! `cargo test -p perch --features camera --test camera -- --ignored`.
#![cfg(feature = "camera")]

#[test]
#[ignore = "switches the webcam on briefly"]
fn the_default_camera_delivers_frames() {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
    let fmt = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut cam = nokhwa::Camera::new(CameraIndex::Index(0), fmt).unwrap_or_else(|e| panic!("{e}"));
    cam.open_stream().unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..5 {
        let img = cam
            .frame()
            .and_then(|f| f.decode_image::<RgbFormat>())
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(img.width() > 0 && img.height() > 0);
    }
    let _ = cam.stop_stream();
}
