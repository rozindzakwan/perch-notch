//! Clipboard access on X11 (and XWayland) via `arboard`.
//!
//! Change detection and the privacy check live in the X11 watcher: it learns
//! about a new CLIPBOARD owner through XFixes, asks the owner for its
//! `TARGETS`, and only emits `ClipboardChanged` when the owner did not mark the
//! content as a password (`x-kde-passwordManagerHint`). The flags it computed
//! are shared here so the model applies the same rule.

use std::path::PathBuf;
use std::sync::Arc;

use perch_core::clipboard::{ClipboardContent, ContentFlags, ImageFormat};
use perch_core::error::{Error, Result};
use perch_core::platform::ClipboardAccess;

use crate::x11_watch::WatchFlags;

const MAX_IMAGE_PIXELS: usize = 4096 * 4096;

/// `arboard`-backed clipboard with watcher-provided privacy flags.
pub struct X11Clipboard {
    cb: arboard::Clipboard,
    flags: Arc<WatchFlags>,
}

impl X11Clipboard {
    /// Open the clipboard.
    pub fn new(flags: Arc<WatchFlags>) -> Result<Self> {
        arboard::Clipboard::new()
            .map(|cb| Self { cb, flags })
            .map_err(|e| Error::unavailable("clipboard", e.to_string()))
    }
}

/// Encode straight RGBA as PNG.
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

impl ClipboardAccess for X11Clipboard {
    fn read(&mut self) -> Result<Option<(ClipboardContent, ContentFlags)>> {
        let flags = self
            .flags
            .clipboard_flags
            .lock()
            .map(|f| f.clone())
            .unwrap_or_default();
        if flags.is_excluded() {
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
        if let Some(t) = content.as_text() {
            return self
                .cb
                .set_text(t.to_string())
                .map_err(|e| Error::backend(format!("clipboard text: {e}")));
        }
        if let ClipboardContent::Image { bytes, .. } = content {
            let img = image::load_from_memory(bytes)
                .map_err(|e| Error::backend(format!("decoding clipboard image: {e}")))?
                .to_rgba8();
            let (w, h) = img.dimensions();
            return self
                .cb
                .set_image(arboard::ImageData {
                    width: w as usize,
                    height: h as usize,
                    bytes: img.into_raw().into(),
                })
                .map_err(|e| Error::backend(format!("clipboard image: {e}")));
        }
        Ok(())
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
