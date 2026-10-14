//! Text selection follows the drawn layout, including automatic wrapping and glyph overhangs.
//! Fixtures are synthetic and use CADCraft Stroke, so no installed font or external DXF is needed.

use cadcraft_engine::doc::{Common, Drawing, EntityKind, HAlign, Handle, MText, Space, Text, VAlign};
use cadcraft_engine::geom::{Bounds2, Vec2, Vec3};
use cadcraft_engine::{select, spatial};

fn wrapped() -> MText {
    MText {
        insert: Vec3::ZERO,
        height: 1.0,
        width: 5.0,
        attach: 1,
        rotation: 0.0,
        style: "Standard".into(),
        contents: "AAAA AAAA AAAA".into(),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
    }
}

fn text(value: &str) -> Text {
    Text {
        insert: Vec3::ZERO,
        align_pt: None,
        height: 1.0,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    }
}

fn drawing(kind: EntityKind, indexed: bool) -> (Drawing, Handle) {
    let mut d = Drawing::new_metric();
    let h = d.add(&Space::Model, Common::default(), kind).unwrap();
    if indexed {
        for i in 0..spatial::MIN_INDEXED {
            d.add(
                &Space::Model,
                Common::default(),
                EntityKind::Point(cadcraft_engine::doc::Point { p: Vec3::new(1000.0 + i as f64, 1000.0, 0.0), angle: 0.0 }),
            )
            .unwrap();
        }
    }
    (d, h)
}

fn tiny(p: Vec2) -> Bounds2 {
    Bounds2::new(p - Vec2::new(0.01, 0.01), p + Vec2::new(0.01, 0.01))
}

#[test]
fn crossing_and_pick_find_automatically_wrapped_last_line() {
    // Each word occupies a line; the third A's left stroke is below the old one-line box.
    let p = Vec2::new(0.2, -4.0);
    for indexed in [false, true] {
        let (d, h) = drawing(EntityKind::MText(wrapped()), indexed);
        assert_eq!(spatial::index(&d, &Space::Model).is_some(), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(p), true), vec![h]);
        assert_eq!(select::pick(&d, &Space::Model, p, 0.01), Some(h));
        assert!(select::select_window(&d, &Space::Model, tiny(p), false).is_empty());
        // A left-to-right window containing only the first line must not select the whole text.
        assert!(select::select_window(&d, &Space::Model, Bounds2::new(Vec2::new(-1.0, -2.0), Vec2::new(6.0, 1.0)), false).is_empty());
        assert_eq!(select::select_window(&d, &Space::Model, Bounds2::new(Vec2::new(-1.0, -6.0), Vec2::new(6.0, 1.0)), false), vec![h]);
        assert!(select::select_window(&d, &Space::Model, tiny(Vec2::new(10.0, -4.0)), true).is_empty());
    }
}

#[test]
fn crossing_finds_inline_height_and_rotated_wrapping() {
    for indexed in [false, true] {
        let mut t = wrapped();
        t.rotation = std::f64::consts::FRAC_PI_2;
        let (d, h) = drawing(EntityKind::MText(t), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(Vec2::new(4.0, 0.2)), true), vec![h]);

        let mut t = wrapped();
        t.contents = "\\H4x;A".into();
        t.width = 0.0;
        let (d, h) = drawing(EntityKind::MText(t), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(Vec2::new(0.5, -3.0)), true), vec![h]);
    }
}

#[test]
fn crossing_finds_text_overline_oblique_and_fit() {
    for indexed in [false, true] {
        let (d, h) = drawing(EntityKind::Text(text("%%oA")), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(Vec2::new(0.2, 1.2)), true), vec![h]);
        assert_eq!(select::pick(&d, &Space::Model, Vec2::new(0.2, 1.2), 0.01), Some(h));

        let mut t = text("A");
        t.oblique = 1.0;
        let (d, h) = drawing(EntityKind::Text(t), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(Vec2::new(1.8, 0.9)), true), vec![h]);

        let mut t = text("ABC");
        t.halign = HAlign::Fit;
        t.align_pt = Some(Vec3::new(12.0, 0.0, 0.0));
        let (d, h) = drawing(EntityKind::Text(t), indexed);
        assert_eq!(select::select_window(&d, &Space::Model, tiny(Vec2::new(11.5, 0.5)), true), vec![h]);
    }
}

#[test]
fn changing_text_style_rebuilds_cached_bounds() {
    let (mut d, h) = drawing(EntityKind::MText(wrapped()), true);
    let p = Vec2::new(6.0, -0.5);
    let before = spatial::index(&d, &Space::Model).unwrap();
    assert!(select::select_window(&d, &Space::Model, tiny(p), true).is_empty());
    // No entity edits: the widened words exceed the reference width and reach further in X.
    d.text_styles.first_mut().unwrap().width_factor = 2.0;
    let after = spatial::index(&d, &Space::Model).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    assert_eq!(select::select_window(&d, &Space::Model, tiny(p), true), vec![h]);
}

#[test]
fn blank_text_remains_unselectable() {
    for indexed in [false, true] {
        for kind in [EntityKind::Text(text("   ")), EntityKind::MText(MText { contents: " \\~ ".into(), ..wrapped() })] {
            let (d, _) = drawing(kind, indexed);
            assert!(select::select_window(&d, &Space::Model, Bounds2::new(Vec2::new(-10.0, -10.0), Vec2::new(10.0, 10.0)), true).is_empty());
        }
    }
}
