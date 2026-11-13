//! The app icon, rendered from the same original vector mascot as the pill.
//! No bitmap assets are shipped; packaging exports PNG/ICO from here
//! (`perch --export-icon <file.png> <size>`).

use perch_ui::art;
use tiny_skia::{Color, FillRule, Paint, Pixmap, Transform};

/// Render the icon into a premultiplied pixmap.
pub fn icon_pixmap(size: u32) -> Option<Pixmap> {
    let size = size.clamp(16, 1024);
    let mut pm = Pixmap::new(size, size)?;
    let s = size as f32;
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(Color::from_rgba8(22, 23, 28, 255));
    if let Some(p) = art::rounded_rect(0.0, 0.0, s, s, s * 0.24) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    let inset = s * 0.16;
    paint.set_color(Color::from_rgba8(242, 243, 247, 255));
    if let Some(p) = art::mascot(inset, inset, s - 2.0 * inset) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    paint.set_color(Color::from_rgba8(255, 176, 66, 255));
    if let Some(p) = art::mascot_eye(inset, inset, s - 2.0 * inset) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    Some(pm)
}

/// Straight (non-premultiplied) RGBA8, as tray-icon and PNG encoders want.
pub fn icon_rgba(size: u32) -> (Vec<u8>, u32) {
    match icon_pixmap(size) {
        Some(pm) => {
            let side = pm.width();
            let mut out = Vec::with_capacity((side * side * 4) as usize);
            for p in pm.pixels() {
                let c = p.demultiply();
                out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
            }
            (out, side)
        }
        None => (Vec::new(), 0),
    }
}

/// ARGB32 in network byte order, as the StatusNotifierItem spec wants.
pub fn icon_argb32(size: u32) -> (Vec<u8>, u32) {
    let (rgba, side) = icon_rgba(size);
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    (out, side)
}

/// Write the icon as a PNG (used by the packaging scripts).
pub fn export_png(path: &std::path::Path, size: u32) -> Result<(), String> {
    let (rgba, side) = icon_rgba(size);
    if side == 0 {
        return Err("could not render the icon".into());
    }
    image::save_buffer(path, &rgba, side, side, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}
