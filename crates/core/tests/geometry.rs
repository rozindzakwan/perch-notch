//! RED-phase tests for notch placement geometry.

use perch_core::geometry::{
    hot_zone_rect, notch_rect, Edge, HotZone, Monitor, NotchGeometry, NotchPosition, Rect,
};

fn mon_1080p() -> Monitor {
    Monitor {
        id: 0,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        primary: true,
    }
}

#[test]
fn collapsed_pill_sits_centered_on_the_top_edge() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let r = notch_rect(&mon_1080p(), NotchPosition::TopCenter, &geo, false);
    assert_eq!(r.w, 200, "collapsed width");
    assert_eq!(r.h, 32, "collapsed height");
    assert_eq!(r.x, (1920 - 200) / 2, "horizontally centered");
    assert_eq!(r.y, 0, "flush with the top edge");
}

#[test]
fn expanded_pill_keeps_its_center_and_grows_downward() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let collapsed = notch_rect(&mon_1080p(), NotchPosition::TopCenter, &geo, false);
    let expanded = notch_rect(&mon_1080p(), NotchPosition::TopCenter, &geo, true);
    assert_eq!(expanded.w, 420);
    assert_eq!(expanded.h, 120);
    assert_eq!(
        expanded.center_x(),
        collapsed.center_x(),
        "center preserved"
    );
    assert_eq!(expanded.y, 0, "still anchored to the top edge");
}

#[test]
fn all_eight_positions_stay_inside_the_monitor() {
    let geo = NotchGeometry::default();
    let m = mon_1080p();
    for pos in NotchPosition::ALL {
        for expanded in [false, true] {
            let r = notch_rect(&m, *pos, &geo, expanded);
            assert!(r.x >= m.x, "{pos:?} expanded={expanded} left edge: {r:?}");
            assert!(r.y >= m.y, "{pos:?} expanded={expanded} top edge: {r:?}");
            assert!(
                r.x + r.w as i32 <= m.x + m.width as i32,
                "{pos:?} expanded={expanded} right edge: {r:?}"
            );
            assert!(
                r.y + r.h as i32 <= m.y + m.height as i32,
                "{pos:?} expanded={expanded} bottom edge: {r:?}"
            );
        }
    }
}

#[test]
fn bottom_positions_anchor_to_the_bottom_edge() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let m = mon_1080p();
    let r = notch_rect(&m, NotchPosition::BottomCenter, &geo, false);
    assert_eq!(r.y + r.h as i32, 1080, "flush with the bottom edge");
    let e = notch_rect(&m, NotchPosition::BottomCenter, &geo, true);
    assert_eq!(e.y + e.h as i32, 1080, "grows upward when expanding");
}

#[test]
fn left_and_right_positions_respect_the_margin() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 24,
        ..NotchGeometry::default()
    };
    let m = mon_1080p();
    let l = notch_rect(&m, NotchPosition::TopLeft, &geo, false);
    assert_eq!(l.x, 24, "left margin applied");
    let r = notch_rect(&m, NotchPosition::TopRight, &geo, false);
    assert_eq!(r.x + r.w as i32, 1920 - 24, "right margin applied");
}

#[test]
fn position_maps_to_the_edge_its_hot_zone_lives_on() {
    assert_eq!(NotchPosition::TopCenter.edge(), Edge::Top);
    assert_eq!(NotchPosition::TopLeft.edge(), Edge::Top);
    assert_eq!(NotchPosition::TopRight.edge(), Edge::Top);
    assert_eq!(NotchPosition::BottomCenter.edge(), Edge::Bottom);
    assert_eq!(NotchPosition::LeftCenter.edge(), Edge::Left);
    assert_eq!(NotchPosition::RightCenter.edge(), Edge::Right);
}

#[test]
fn hot_zone_is_a_thin_strip_on_the_notch_edge() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        hot_zone_thickness: 2,
        hot_zone_padding: 40,
    };
    let m = mon_1080p();
    let HotZone { rect, edge } = hot_zone_rect(&m, NotchPosition::TopCenter, &geo);
    assert_eq!(edge, Edge::Top);
    assert_eq!(rect.h, 2, "1-2px thin so it never eats real clicks");
    assert_eq!(rect.y, 0, "sits on the very top row of pixels");
    assert_eq!(
        rect.w,
        200 + 2 * 40,
        "slightly wider than the collapsed pill for comfortable aiming"
    );
    assert_eq!(
        rect.center_x(),
        notch_rect(&m, NotchPosition::TopCenter, &geo, false).center_x()
    );
}

#[test]
fn hot_zone_never_exceeds_the_monitor_width() {
    let geo = NotchGeometry {
        collapsed: (1900, 32),
        expanded: (1910, 120),
        margin: 0,
        hot_zone_thickness: 2,
        hot_zone_padding: 200,
    };
    let m = mon_1080p();
    let hz = hot_zone_rect(&m, NotchPosition::TopCenter, &geo);
    assert!(hz.rect.w <= 1920, "clamped: {:?}", hz.rect);
    assert!(hz.rect.x >= 0);
}

#[test]
fn vertical_edges_get_a_vertical_hot_zone() {
    let geo = NotchGeometry {
        hot_zone_thickness: 2,
        ..NotchGeometry::default()
    };
    let m = mon_1080p();
    let hz = hot_zone_rect(&m, NotchPosition::LeftCenter, &geo);
    assert_eq!(hz.edge, Edge::Left);
    assert_eq!(hz.rect.w, 2, "thin vertical strip");
    assert_eq!(hz.rect.x, 0);
    assert!(hz.rect.h > hz.rect.w);
}

#[test]
fn geometry_scales_with_hidpi_factor() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let hidpi = Monitor {
        id: 1,
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
        scale: 2.0,
        primary: true,
    };
    let r = notch_rect(&hidpi, NotchPosition::TopCenter, &geo, false);
    assert_eq!(r.w, 400, "logical 200 at scale 2 = 400 physical px");
    assert_eq!(r.h, 64);
    assert_eq!(r.x, (3840 - 400) / 2);
}

#[test]
fn secondary_monitor_offset_is_honored() {
    let geo = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let right = Monitor {
        id: 2,
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        primary: false,
    };
    let r = notch_rect(&right, NotchPosition::TopCenter, &geo, false);
    assert_eq!(r.x, 1920 + (1920 - 200) / 2);
}

#[test]
fn oversized_pill_is_clamped_into_the_monitor() {
    // A user can configure an expanded size larger than the screen, or a margin
    // that pushes the pill off the right edge. Both must be clamped, not drawn
    // off-screen (mutation guard for the clamp in notch_rect).
    let m = mon_1080p();

    let too_wide = NotchGeometry {
        collapsed: (3000, 32),
        expanded: (4000, 120),
        margin: 0,
        ..NotchGeometry::default()
    };
    let r = notch_rect(&m, NotchPosition::TopCenter, &too_wide, false);
    assert_eq!(r.x, 0, "clamped to the left edge: {r:?}");
    assert_eq!(r.w, 1920, "clamped to the monitor width: {r:?}");

    let huge_margin = NotchGeometry {
        collapsed: (200, 32),
        expanded: (420, 120),
        margin: 5000,
        ..NotchGeometry::default()
    };
    let r = notch_rect(&m, NotchPosition::TopLeft, &huge_margin, false);
    assert!(
        r.x + r.w as i32 <= 1920,
        "left-anchored with absurd margin stays on screen: {r:?}"
    );
    let r = notch_rect(&m, NotchPosition::TopRight, &huge_margin, false);
    assert!(
        r.x >= 0,
        "right-anchored with absurd margin stays on screen: {r:?}"
    );

    let too_tall = NotchGeometry {
        collapsed: (200, 2000),
        expanded: (420, 3000),
        margin: 0,
        ..NotchGeometry::default()
    };
    let r = notch_rect(&m, NotchPosition::BottomCenter, &too_tall, true);
    assert_eq!(r.y, 0, "clamped to the top: {r:?}");
    assert_eq!(r.h, 1080, "clamped to the monitor height: {r:?}");
}

#[test]
fn rect_contains_point_is_inclusive_of_the_origin_only() {
    let r = Rect {
        x: 10,
        y: 10,
        w: 5,
        h: 5,
    };
    assert!(r.contains(10, 10));
    assert!(r.contains(14, 14));
    assert!(!r.contains(15, 15), "exclusive on the far edge");
    assert!(!r.contains(9, 10));
}

#[test]
fn notch_position_round_trips_through_its_config_string() {
    for pos in NotchPosition::ALL {
        let s = pos.as_str();
        assert_eq!(
            NotchPosition::from_str_opt(s),
            Some(*pos),
            "round trip for {s}"
        );
    }
    assert_eq!(NotchPosition::from_str_opt("nonsense"), None);
}
