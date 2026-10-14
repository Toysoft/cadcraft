//! A layout viewport's view centre is in display coordinates, relative to the view target (DXF
//! Reference, VIEWPORT groups 12/22 and 17/27/37). DWG files commonly store the view in the
//! target with the centre at 0,0; reading only 12/22 pointed every such viewport at the model
//! origin.

use cadcraft_doc::*;
use cadcraft_geom::{Vec2, Vec3};

/// A metric drawing with one viewport on its first layout, looking at `view`.
fn drawing(view: Vec2) -> (Drawing, Handle) {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(5000.0, 3000.0, 0.0) })).unwrap();
    let sheet = Space::Paper(d.layouts[0].name.clone());
    let vp = Viewport {
        center: Vec3::new(100.0, 100.0, 0.0),
        width: 150.0,
        height: 150.0,
        view_center: view,
        view_height: 6000.0,
        id: 2,
        ..Viewport::default()
    };
    let h = d.add(&sheet, Common::default(), EntityKind::Viewport(vp)).unwrap();
    (d, h)
}

/// The DXF text as (code, value) pairs, and back.
fn groups(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.to_string(), c.get(1)?.to_string()))).collect()
}
fn text_of(gs: &[(String, String)]) -> String {
    gs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

/// Rewrites the viewport `h` as DWG files store it: centre 12/22 = `center`, target 17/27 = `target`.
fn with_target(text: &str, h: Handle, center: [&str; 2], target: [&str; 2]) -> String {
    let mut gs = groups(text);
    let start = gs.iter().position(|(c, v)| c == "5" && *v == h.hex()).expect("viewport handle");
    let end = start + gs[start..].iter().position(|(c, _)| c == "0").expect("entity end");
    for (c, v) in &mut gs[start..end] {
        match c.as_str() {
            "12" => *v = center[0].into(),
            "22" => *v = center[1].into(),
            _ => {}
        }
    }
    let at = start + gs[start..end].iter().position(|(c, _)| c == "45").expect("view height");
    gs.insert(at, ("17".into(), target[0].into()));
    gs.insert(at + 1, ("27".into(), target[1].into()));
    gs.insert(at + 2, ("37".into(), "0.0".into()));
    text_of(&gs)
}

fn view_center(d: &Drawing, h: Handle) -> Vec2 {
    match d.entity(h).map(|e| &e.kind) {
        Some(EntityKind::Viewport(v)) => v.view_center,
        other => panic!("not a viewport: {other:?}"),
    }
}

#[test]
fn view_in_the_target_is_read_as_the_model_centre() {
    let (d, h) = drawing(Vec2::ZERO);
    let text = with_target(&cadcraft_io::write_dxf(&d), h, ["0.0", "0.0"], ["3269.0", "6654.5"]);
    let back = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(view_center(&back, h), Vec2::new(3269.0, 6654.5));
}

#[test]
fn centre_is_relative_to_the_target() {
    let (d, h) = drawing(Vec2::ZERO);
    let text = with_target(&cadcraft_io::write_dxf(&d), h, ["10.0", "-20.0"], ["1000.0", "2000.0"]);
    let back = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(view_center(&back, h), Vec2::new(1010.0, 1980.0));
}

#[test]
fn view_survives_a_round_trip() {
    let (d, h) = drawing(Vec2::ZERO);
    let text = with_target(&cadcraft_io::write_dxf(&d), h, ["0.0", "0.0"], ["3269.0", "6654.5"]);
    let once = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    let twice = cadcraft_io::read_dxf(cadcraft_io::write_dxf(&once).as_bytes()).unwrap();
    assert_eq!(view_center(&twice, h), Vec2::new(3269.0, 6654.5));
}

#[test]
fn non_finite_target_falls_back_to_the_centre() {
    let (d, h) = drawing(Vec2::ZERO);
    let text = with_target(&cadcraft_io::write_dxf(&d), h, ["5.0", "6.0"], ["1e308", "NaN"]);
    let back = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    let got = view_center(&back, h);
    assert!(got.is_finite(), "{got:?}");
    // The sum overflows to infinity: the centre alone is kept.
    let text = with_target(&cadcraft_io::write_dxf(&d), h, ["1e308", "6.0"], ["1e308", "0.0"]);
    let back = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(view_center(&back, h), Vec2::new(1e308, 6.0));
}
