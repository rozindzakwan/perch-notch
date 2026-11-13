//! Screen capture (GDI) and OCR (`Windows.Media.Ocr`) for the screenshot tool.

use perch_core::error::{Error, Result};
use perch_core::geometry::Rect;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    SRCCOPY,
};

/// Capture a screen rectangle (physical pixels, virtual-desktop coordinates)
/// as straight RGBA8.
pub fn capture(rect: Rect) -> Result<(Vec<u8>, u32, u32)> {
    let (w, h) = (rect.w as i32, rect.h as i32);
    if w <= 0 || h <= 0 {
        return Err(Error::backend("empty capture rectangle"));
    }
    // SAFETY: standard GDI capture sequence; every handle created here is
    // released before returning, and the output buffer is sized for the
    // 32-bit top-down DIB we request.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(Error::backend("GetDC failed"));
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let ok = BitBlt(
            mem,
            0,
            0,
            w,
            h,
            screen,
            rect.x,
            rect.y,
            SRCCOPY | CAPTUREBLT,
        );
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if ok.is_err() || lines == 0 {
            return Err(Error::backend("screen capture failed"));
        }
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok((bgra, w as u32, h as u32))
    }
}

/// Recognise text in an RGBA image with the OCR engine for the user's
/// languages (built into Windows 10/11; no network).
pub fn ocr(rgba: &[u8], w: u32, h: u32) -> Result<String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| Error::unavailable("ocr", format!("no OCR language installed: {e}")))?;
    let writer = DataWriter::new().map_err(|e| Error::backend(e.to_string()))?;
    writer
        .WriteBytes(rgba)
        .map_err(|e| Error::backend(e.to_string()))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|e| Error::backend(e.to_string()))?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Rgba8,
        w as i32,
        h as i32,
        BitmapAlphaMode::Straight,
    )
    .map_err(|e| Error::backend(format!("bitmap: {e}")))?;
    // OCR wants BGRA8.
    let bitmap = SoftwareBitmap::ConvertWithAlpha(
        &bitmap,
        BitmapPixelFormat::Bgra8,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(|e| Error::backend(format!("convert: {e}")))?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|op| op.get())
        .map_err(|e| Error::backend(format!("OCR: {e}")))?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(|e| Error::backend(e.to_string()))? {
        if let Ok(t) = line.Text() {
            lines.push(t.to_string());
        }
    }
    Ok(lines.join("\n"))
}
