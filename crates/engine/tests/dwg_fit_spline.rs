//! Issue #1179: a fit-point spline saved in a DWG keeps only its fit points, so the control net
//! is rebuilt on read. The rebuilt net must be the one the SPLINE command builds, and that one
//! the net AutoCAD builds (two control points more than fit points, the chord lengths as knots),
//! so the curve read back from the DWG is the curve that was drawn.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{Drawing, EntityKind};
use cadcraft_geom::{KnotParam, Spline, Vec2};
use serde_json::json;

/// The fit points of the issue: 0,100 / 30,130 / 60,90 / 100,120.
const FIT: [[f64; 2]; 4] = [[0.0, 100.0], [30.0, 130.0], [60.0, 90.0], [100.0, 120.0]];

/// AutoCAD's knots for this spline (its DXF): 0 ×4, 42.43, 92.43, 142.43 ×4 — the chord lengths
/// of the fit points. CADCraft's parameter runs from 0 to 1, so its knots are these divided by
/// the total chord length.
const KNOTS: [f64; 10] = [0.0, 0.0, 0.0, 0.0, 42.43, 92.43, 142.43, 142.43, 142.43, 142.43];
const CHORD: f64 = 142.43;

/// The SPLINE as the DWG bridge renders a fit-point DWG spline into DXF: fit points, tolerance
/// and no tangents; no knots and no control points.
const FIT_ONLY_DXF: &str = "0\nSECTION\n2\nENTITIES\n\
0\nSPLINE\n5\n2A\n8\n0\n100\nAcDbEntity\n100\nAcDbSpline\n210\n0\n220\n0\n230\n1\n70\n8\n71\n3\n72\n0\n73\n0\n74\n4\n42\n0\n43\n0\n44\n0\n\
11\n0\n21\n100\n31\n0\n11\n30\n21\n130\n31\n0\n11\n60\n21\n90\n31\n0\n11\n100\n21\n120\n31\n0\n\
0\nENDSEC\n0\nEOF\n";

fn only_spline(d: &Drawing) -> &Spline {
    let mut splines = d.model.iter().filter_map(|e| match &e.kind {
        EntityKind::Spline(s) => Some(s),
        _ => None,
    });
    let s = splines.next().expect("a spline");
    assert!(splines.next().is_none(), "one spline");
    s
}

fn drawn() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "spline", None);
    s.execute("spline", &json!({ "fit": FIT })).unwrap();
    s
}

fn assert_as_autocad(got: &Spline, what: &str) {
    assert_eq!(got.degree, 3, "{what}: degree");
    assert_eq!(got.control.len(), 6, "{what}: control points {:?}", got.control);
    assert_eq!(got.knots.len(), KNOTS.len(), "{what}: knots {:?}", got.knots);
    for (k, want) in got.knots.iter().zip(KNOTS) {
        assert!((k * CHORD - want).abs() < 1e-2, "{what}: knots {:?} × {CHORD}, wanted {KNOTS:?}", got.knots);
    }
    assert_eq!(got.fit_opts.knots, KnotParam::Chord, "{what}: knot parametrisation");
    let fit: Vec<Vec2> = FIT.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    assert_eq!(got.fit, fit, "{what}: fit points");
    assert!(!got.closed, "{what}: open");
}

fn assert_same_curve(got: &Spline, drawn: &Spline, what: &str) {
    for (a, b) in got.control.iter().zip(&drawn.control) {
        assert!(a.near(*b, 1e-6), "{what}: control {:?}, drawn {:?}", got.control, drawn.control);
    }
    for i in 0..=40 {
        let t = i as f64 / 40.0;
        assert!(got.eval(t).near(drawn.eval(t), 1e-6), "{what}: differs at {t}");
    }
}

/// The SPLINE command itself builds AutoCAD's net: the net the DWG read must reproduce.
#[test]
fn spline_command_builds_autocads_net() {
    let s = drawn();
    let sp = only_spline(s.doc().unwrap());
    assert_as_autocad(sp, "SPLINE");
    assert!(sp.fit_opts.start_tangent.is_none() && sp.fit_opts.end_tangent.is_none(), "free ends");
}

/// Saved as DWG (which keeps the fit points only) and read back, the spline has the control net
/// the SPLINE command built, not another one, and still says its knots are chord-spaced.
#[test]
fn fit_point_spline_survives_dwg_as_drawn() {
    let s = drawn();
    let drawn = only_spline(s.doc().unwrap()).clone();
    let dwg = cadcraft_io::write(s.doc().unwrap(), "spline.dwg").unwrap();
    let back = cadcraft_io::read(&dwg, "spline.dwg").unwrap();
    let got = only_spline(&back);
    assert_as_autocad(got, "DWG");
    assert_same_curve(got, &drawn, "DWG");
}

/// The DXF the DWG bridge makes of a fit-point DWG spline (fit points, no control net) is
/// rebuilt as the SPLINE command builds it.
#[test]
fn fit_points_only_are_rebuilt_as_the_spline_command_builds_them() {
    let s = drawn();
    let drawn = only_spline(s.doc().unwrap()).clone();
    let back = cadcraft_io::read_dxf(FIT_ONLY_DXF.as_bytes()).unwrap();
    let got = only_spline(&back);
    assert_as_autocad(got, "fit-only DXF");
    assert_same_curve(got, &drawn, "fit-only DXF");
}
