//! Clipboard access for the clipboard board and the shelf.
//!
//! Change notifications come from the event thread's
//! `AddClipboardFormatListener` (see [`crate::events`]); this module only reads
//! and writes. Before reading any content it checks the privacy formats that
//! password managers set (`ExcludeClipboardContentFromMonitorProcessing`,
//! `CanIncludeInClipboardHistory = 0`, `CanUploadToCloudClipboard = 0`), so a
//! secret is never even loaded into Perch's memory.

use std::path::PathBuf;

use perch_core::clipboard::{ClipboardContent, ContentFlags, ImageFormat};
use perch_core::error::{Error, Result};
use perch_core::platform::ClipboardAccess;
use windows::core::HSTRING;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

/// Largest image we keep (encoded); the model applies its own limit too.
const MAX_IMAGE_PIXELS: usize = 4096 * 4096;

/// Windows clipboard via `arboard`, plus the privacy-format check.
pub struct WindowsClipboard {
    cb: arboard::Clipboard,
}

impl WindowsClipboard {
    /// Open the clipboard API.
    pub fn new() -> Result<Self> {
        arboard::Clipboard::new()
            .map(|cb| Self { cb })
            .map_err(|e| Error::unavailable("clipboard", e.to_string()))
    }
}

fn format_id(name: &str) -> u32 {
    // SAFETY: registering (or looking up) a named clipboard format.
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) }
}

/// Read a DWORD-valued clipboard format, if present.
fn dword_format(id: u32) -> Option<u32> {
    // SAFETY: GetClipboardData is called with the clipboard open (caller); the
    // handle is locked only for the duration of the copy and checked for size.
    unsafe {
        let h: HANDLE = GetClipboardData(id).ok()?;
        let g = HGLOBAL(h.0);
        if GlobalSize(g) < 4 {
            return None;
        }
        let p = GlobalLock(g) as *const u32;
        if p.is_null() {
            return None;
        }
        let v = p.read_unaligned();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// Collect the privacy flags without touching the content.
pub fn privacy_flags() -> ContentFlags {
    let mut flags = ContentFlags::default();
    let exclude = format_id("ExcludeClipboardContentFromMonitorProcessing");
    // SAFETY: format availability can be queried without opening the clipboard.
    if exclude != 0 && unsafe { IsClipboardFormatAvailable(exclude) }.is_ok() {
        flags = flags.with_windows_format("ExcludeClipboardContentFromMonitorProcessing", 1);
    }
    let optional = ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"];
    let ids: Vec<(&str, u32)> = optional
        .iter()
        .map(|n| (*n, format_id(n)))
        .filter(|(_, id)| {
            // SAFETY: format availability can be queried without opening.
            *id != 0 && unsafe { IsClipboardFormatAvailable(*id) }.is_ok()
        })
        .collect();
    if !ids.is_empty() {
        // SAFETY: open/close bracket around the data reads; no owner window.
        if unsafe { OpenClipboard(None) }.is_ok() {
            for (name, id) in ids {
                if let Some(v) = dword_format(id) {
                    flags = flags.with_windows_format(name, v);
                }
            }
            // SAFETY: we opened it above.
            let _ = unsafe { CloseClipboard() };
        }
    }
    flags
}

impl ClipboardAccess for WindowsClipboard {
    fn read(&mut self) -> Result<Option<(ClipboardContent, ContentFlags)>> {
        let flags = privacy_flags();
        if flags.is_excluded() {
            // Do not read the payload at all; the model drops it on the flags.
            return Ok(Some((ClipboardContent::text(String::new()), flags)));
        }
        if let Ok(text) = self.cb.get_text() {
            if !text.is_empty() {
                return Ok(Some((ClipboardContent::text(text), flags)));
            }
        }
        if let Ok(img) = self.cb.get_image() {
            if img.width * img.height <= MAX_IMAGE_PIXELS {
                if let Some(png) = encode_png(&img.bytes, img.width as u32, img.height as u32) {
                    return Ok(Some((
                        ClipboardContent::image(
                            png,
                            img.width as u32,
                            img.height as u32,
                            ImageFormat::Png,
                        ),
                        flags,
                    )));
                }
            }
        }
        Ok(None)
    }

    fn write(&mut self, content: &ClipboardContent) -> Result<()> {
        write_content(&mut self.cb, content)
    }

    fn paste_text(&mut self) -> Result<Option<String>> {
        // An explicit paste: read the text even when a password manager marked
        // it as secret (that is usually why it was copied). Never recorded.
        Ok(self.cb.get_text().ok().filter(|t| !t.is_empty()))
    }

    fn write_files(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.cb
            .set()
            .file_list(paths)
            .map_err(|e| Error::backend(format!("clipboard file list: {e}")))
    }
}

/// Encode straight RGBA as PNG.
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// Put text or an image back on the clipboard.
pub fn write_content(cb: &mut arboard::Clipboard, content: &ClipboardContent) -> Result<()> {
    if let Some(t) = content.as_text() {
        return cb
            .set_text(t.to_string())
            .map_err(|e| Error::backend(format!("clipboard text: {e}")));
    }
    if let ClipboardContent::Image { bytes, .. } = content {
        let img = image::load_from_memory(bytes)
            .map_err(|e| Error::backend(format!("decoding clipboard image: {e}")))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        return cb
            .set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: img.into_raw().into(),
            })
            .map_err(|e| Error::backend(format!("clipboard image: {e}")));
    }
    Ok(())
}
