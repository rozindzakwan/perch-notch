//! Screen capture (X11 `GetImage`) and OCR (`tesseract` CLI) for the
//! screenshot tool.
//!
//! Wayland: capture needs the xdg-desktop-portal Screenshot method, which is
//! not implemented; under XWayland only X11 windows are visible (documented).

use std::io::Write;
use std::process::{Command, Stdio};

use perch_core::error::{Error, Result};
use perch_core::geometry::Rect;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

/// Capture a screen rectangle (physical pixels) as straight RGBA8.
pub fn capture(rect: Rect) -> Result<(Vec<u8>, u32, u32)> {
    let (conn, screen) = crate::x11::connect()?;
    let setup = conn.setup();
    let s = setup
        .roots
        .get(screen)
        .ok_or_else(|| Error::backend("no such X screen"))?;
    let (w, h) = (
        rect.w.min(u32::from(u16::MAX)) as u16,
        rect.h.min(u32::from(u16::MAX)) as u16,
    );
    if w == 0 || h == 0 {
        return Err(Error::backend("empty capture rectangle"));
    }
    let reply = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            s.root,
            rect.x as i16,
            rect.y as i16,
            w,
            h,
            !0,
        )
        .map_err(|e| Error::backend(format!("GetImage: {e}")))?
        .reply()
        .map_err(|e| Error::backend(format!("GetImage reply: {e}")))?;
    let bpp = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == reply.depth)
        .map(|f| f.bits_per_pixel)
        .unwrap_or(0);
    if bpp != 32 {
        return Err(Error::unavailable(
            "screenshot",
            format!(
                "unsupported X pixel format ({}-bit, depth {})",
                bpp, reply.depth
            ),
        ));
    }
    let little = setup.image_byte_order == ImageOrder::LSB_FIRST;
    Ok((
        xpixels_to_rgba(&reply.data, u32::from(w), u32::from(h), little),
        u32::from(w),
        u32::from(h),
    ))
}

/// Convert 32-bit X pixels (0x00RRGGBB in the server's byte order) to RGBA.
/// Pure.
pub fn xpixels_to_rgba(data: &[u8], w: u32, h: u32, little_endian: bool) -> Vec<u8> {
    let n = (w * h) as usize;
    let mut out = Vec::with_capacity(n * 4);
    for px in data.chunks_exact(4).take(n) {
        let (r, g, b) = if little_endian {
            (px[2], px[1], px[0])
        } else {
            (px[1], px[2], px[3])
        };
        out.extend_from_slice(&[r, g, b, 255]);
    }
    out.resize(n * 4, 0);
    out
}

/// Recognise text with the `tesseract` CLI (Recommends: tesseract-ocr).
pub fn ocr(rgba: &[u8], w: u32, h: u32) -> Result<String> {
    let png = crate::clipboard::encode_png(rgba, w, h)
        .ok_or_else(|| Error::backend("encoding PNG for OCR"))?;
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::unavailable("ocr", "install tesseract-ocr to copy text from screenshots")
            } else {
                Error::io("running tesseract", e)
            }
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&png)
            .map_err(|e| Error::io("feeding tesseract", e))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::io("waiting for tesseract", e))?;
    if !out.status.success() {
        return Err(Error::backend("tesseract failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
