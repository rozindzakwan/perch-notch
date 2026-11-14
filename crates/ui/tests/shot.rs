use perch_ui::shot::{Mark, ShotEvent, ShotState, Tool};

/// A 200x100 capture: left half red, right half blue.
fn state() -> ShotState {
    let mut rgba = Vec::new();
    for _y in 0..100 {
        for x in 0..200 {
            if x < 100 {
                rgba.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    ShotState::new(&rgba, 200, 100).unwrap_or_else(|| panic!("state"))
}

#[test]
fn dragging_selects_and_crop_returns_the_pixels() {
    let mut s = state();
    s.press(110.0, 10.0);
    s.drag(150.0, 50.0);
    s.release(150.0, 50.0);
    assert_eq!(s.selection(), Some((110.0, 10.0, 40.0, 40.0)));
    let (rgba, w, h) = s.crop(false).unwrap_or_else(|| panic!("crop"));
    assert_eq!((w, h), (40, 40));
    assert_eq!(&rgba[0..4], &[0, 0, 255, 255], "inside the blue half");
}

#[test]
fn a_click_selects_the_whole_monitor_and_drag_direction_does_not_matter() {
    let mut s = state();
    s.press(50.0, 50.0);
    s.release(51.0, 50.0);
    assert_eq!(s.selection(), Some((0.0, 0.0, 200.0, 100.0)));
    let mut s = state();
    s.press(150.0, 80.0);
    s.drag(500.0, -20.0); // past the edges, up-right
    s.release(500.0, -20.0);
    assert_eq!(
        s.selection(),
        Some((150.0, 0.0, 50.0, 80.0)),
        "clamped to the capture"
    );
}

#[test]
fn annotations_change_the_copy_but_not_the_ocr_input() {
    let mut s = state();
    s.press(0.0, 0.0);
    s.drag(100.0, 100.0);
    s.release(100.0, 100.0);
    // Pen is the default tool: draw a stroke across the selection.
    s.press(10.0, 50.0);
    s.drag(50.0, 50.0);
    s.drag(90.0, 50.0);
    s.release(90.0, 50.0);
    assert!(matches!(s.marks(), [Mark::Pen(p)] if p.len() == 3));
    let (plain, _, _) = s.crop(false).unwrap_or_else(|| panic!());
    let (inked, w, _) = s.crop(true).unwrap_or_else(|| panic!());
    let i = ((50 * w + 50) * 4) as usize;
    assert_eq!(&plain[i..i + 3], &[255, 0, 0]);
    assert_ne!(&inked[i..i + 3], &[255, 0, 0], "the stroke is in the copy");
}

#[test]
fn text_tool_types_commits_and_escape_cancels() {
    let mut s = state();
    s.press(0.0, 0.0);
    s.drag(200.0, 100.0);
    s.release(200.0, 100.0);
    // Pick the text tool through the toolbar by painting first (to lay out
    // buttons) is window-specific; drive the state directly instead.
    let mut text = perch_ui::text::TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = tiny_skia::Pixmap::new(200, 100).unwrap_or_else(|| panic!());
    s.paint(&mut pm, &mut text);
    assert_eq!(s.tool(), Tool::Pen);
    s.press(20.0, 20.0);
    s.release(20.0, 20.0); // a dot is not kept as a stroke
    assert!(s.marks().is_empty());
    assert_eq!(
        s.escape(),
        vec![ShotEvent::Cancel],
        "Esc with nothing typed cancels"
    );
}

#[test]
fn enter_copies_after_selection_and_cancel_is_explicit() {
    let mut s = state();
    assert!(s.enter().is_empty(), "nothing to copy before a selection");
    s.press(0.0, 0.0);
    s.drag(20.0, 10.0);
    s.release(20.0, 10.0);
    match s.enter().as_slice() {
        [ShotEvent::Copy(rgba, 20, 10)] => assert_eq!(rgba.len(), 20 * 10 * 4),
        other => panic!("{other:?}"),
    }
}

#[test]
fn text_labels_are_typed_and_committed_on_enter() {
    let mut s = state();
    s.press(0.0, 0.0);
    s.drag(200.0, 100.0);
    s.release(200.0, 100.0);
    s.set_tool(Tool::Text);
    s.press(30.0, 40.0);
    s.release(30.0, 40.0);
    s.type_text("Hi\u{7}!");
    s.backspace();
    assert!(
        s.enter().is_empty(),
        "Enter while typing commits, it does not copy"
    );
    assert!(matches!(s.marks(), [Mark::Text(_, t)] if t == "Hi"));
    s.press(60.0, 60.0); // start another label, then abandon it
    s.type_text("draft");
    assert!(
        s.escape().is_empty(),
        "Esc while typing only drops the label"
    );
    assert_eq!(s.marks().len(), 1);
}

#[test]
fn mirror_flips_horizontally_and_crops_to_fill() {
    use perch_ui::mirror::mirror_fill;
    // 4x2 RGB: left column red, everything else blue.
    let mut src = Vec::new();
    for _y in 0..2 {
        for x in 0..4 {
            if x == 0 {
                src.extend_from_slice(&[255, 0, 0]);
            } else {
                src.extend_from_slice(&[0, 0, 255]);
            }
        }
    }
    let out = mirror_fill(&src, 4, 2, 4, 2);
    assert_eq!(out[3], 0xff0000, "red moves to the right edge");
    assert_eq!(out[0], 0x0000ff);
    assert_eq!(
        mirror_fill(&src, 4, 2, 2, 2).len(),
        4,
        "crops to the target aspect"
    );
    assert!(
        mirror_fill(&[], 0, 0, 3, 3).iter().all(|p| *p == 0),
        "no frame paints black"
    );
}

/// A 600x300 capture, all red, with the whole screen selected and painted.
fn annotating() -> (ShotState, tiny_skia::Pixmap) {
    let rgba: Vec<u8> = [255u8, 0, 0, 255].repeat(600 * 300);
    let mut s = ShotState::new(&rgba, 600, 300).unwrap_or_else(|| panic!("state"));
    s.press(10.0, 10.0);
    s.release(10.0, 10.0);
    let mut text = perch_ui::text::TextRenderer::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pm = tiny_skia::Pixmap::new(600, 300).unwrap_or_else(|| panic!());
    s.paint(&mut pm, &mut text);
    (s, pm)
}

#[test]
fn toolbar_is_a_glass_pill_with_icon_buttons() {
    let (mut s, pm) = annotating();
    let (tx, ty, tw, th) = s.toolbar_rect().unwrap_or_else(|| panic!("a toolbar"));
    let buttons = s.toolbar_buttons();
    let names: Vec<&str> = buttons.iter().map(|b| b.1).collect();
    assert_eq!(
        names,
        ["pen", "arrow", "text", "ocr", "copy", "save", "cancel"]
    );
    for ((bx, by, bw, bh), name) in &buttons {
        assert!((bw - bh).abs() < 0.5, "{name} is a square icon button");
        assert!(*bx >= tx && bx + bw <= tx + tw && *by >= ty && by + bh <= ty + th);
    }
    // Glass fill: the dark palette surface over the capture (the toolbar sits
    // inside a full-screen selection, which is not dimmed).
    let pal = perch_ui::theme::Palette::dark(false);
    let a = pal.surface.alpha();
    let want = pal.surface.red() * 255.0 * a + 255.0 * (1.0 - a);
    let p = pm
        .pixel((tx + 4.0) as u32, (ty + th / 2.0) as u32)
        .unwrap_or_else(|| panic!());
    assert!(
        (f32::from(p.red()) - want).abs() < 8.0,
        "toolbar red {} vs {want}",
        p.red()
    );
    // The buttons keep their actions.
    let centre = |name: &str| {
        let ((x, y, w, h), _) = *buttons
            .iter()
            .find(|b| b.1 == name)
            .unwrap_or_else(|| panic!("{name}"));
        (x + w / 2.0, y + h / 2.0)
    };
    let (x, y) = centre("text");
    s.press(x, y);
    s.release(x, y);
    assert_eq!(s.tool(), Tool::Text);
    let (x, y) = centre("cancel");
    assert_eq!(s.press(x, y), vec![ShotEvent::Cancel]);
    let (x, y) = centre("copy");
    assert!(matches!(
        s.press(x, y).as_slice(),
        [ShotEvent::Copy(_, 600, 300)]
    ));
}

#[test]
fn mirror_frame_has_rounded_corners() {
    use perch_ui::mirror::mirror_frame;
    let src = [200u8, 100, 50].repeat(64 * 48);
    let out = mirror_frame(&src, 64, 48, 64, 48, 12.0);
    assert_eq!(out.len(), 64 * 48);
    assert_eq!(out[0] >> 24, 0, "top-left corner is transparent");
    assert_eq!(out[63] >> 24, 0, "top-right corner is transparent");
    assert_eq!(out[64 * 47] >> 24, 0, "bottom-left corner is transparent");
    let centre = out[24 * 64 + 32];
    assert_eq!(centre >> 24, 0xFF, "the middle is opaque");
    assert_eq!(centre & 0x00FF_FFFF, 0x00C8_6432, "and shows the frame");
}
