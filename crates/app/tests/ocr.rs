//! OCR round trip: render a known sentence, recognise it.
//! Windows uses Windows.Media.Ocr; Linux needs `tesseract-ocr` installed
//! (the CI image has it).

#[test]
fn rendered_text_is_recognised() {
    let (w, h) = (900u32, 140u32);
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| panic!("pixmap"));
    pm.fill(tiny_skia::Color::WHITE);
    let mut text = perch_ui::text::TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
    text.draw(
        &mut pm,
        "PERCH READS 42 WORDS",
        30.0,
        90.0,
        64.0,
        tiny_skia::Color::BLACK,
        860.0,
    );
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in pm.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), 255]);
    }
    match perch_app::sys::ocr(&rgba, w, h) {
        Ok(t) => {
            eprintln!("recognised: {t:?}");
            let t = t.to_uppercase();
            for word in ["PERCH", "READS", "42", "WORDS"] {
                assert!(t.contains(word), "missing {word} in {t:?}");
            }
        }
        // A machine without an OCR engine must degrade, not fail hard.
        Err(e) => {
            eprintln!("OCR unavailable here: {e}");
            assert!(e.is_degradable(), "{e}");
        }
    }
}
