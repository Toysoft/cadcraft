//! TRIM and EXTEND keep the object (#1164): the piece that stays is the same object, with the
//! same handle, so its associative dimensions follow the new length as they do after LENGTHEN,
//! and its xdata and hyperlinks stay on it. A trim that leaves two pieces keeps the handle on
//! the first (the one at the object's start); the other piece is new.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{Dimension, EntityKind, Handle};
use cadcraft_engine::geom::Vec2;
use serde_json::{Value, json};

fn handle(v: &Value) -> Handle {
    let s = v.get("handle").unwrap_or(&v["handles"][0]).as_str().unwrap();
    Handle::parse_hex(s).unwrap()
}

fn handles(v: &Value) -> Vec<Handle> {
    v["handles"].as_array().unwrap().iter().map(|h| Handle::parse_hex(h.as_str().unwrap()).unwrap()).collect()
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    handle(&s.execute("line", &json!({ "points": [a, b] })).unwrap())
}

/// The ends of the line `h` (`None` when it is gone or not a line).
fn ends(s: &Session, h: Handle) -> Option<(Vec2, Vec2)> {
    match &s.doc().unwrap().entity(h)?.kind {
        EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
        _ => None,
    }
}

fn dim(s: &Session, h: Handle) -> Dimension {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Dimension(d) => d.clone(),
        k => panic!("not a dimension: {k:?}"),
    }
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.dist(b) < 1e-9
}

/// The text a dimension shows.
fn measured(s: &Session, h: Handle) -> String {
    cadcraft_engine::render::dimension_in(s.doc().unwrap(), &dim(s, h)).value
}

/// A horizontal line from 0,0 to `len`,0 with a linear dimension of it (both origins attached).
fn dimensioned_line(s: &mut Session, len: f64) -> (Handle, Handle) {
    let l = line(s, [0.0, 0.0], [len, 0.0]);
    let d = handle(&s.execute("dimlinear", &json!({ "object": l.hex(), "at": [50, -20] })).unwrap());
    let dm = dim(s, d);
    assert_eq!(dm.assoc.len(), 2, "{:?}", dm.assoc);
    assert!(dm.assoc.iter().all(|a| a.handle == l));
    (l, d)
}

#[test]
fn trim_keeps_the_line_so_its_dimension_follows() {
    let mut s = Session::new();
    let (l, d) = dimensioned_line(&mut s, 100.0);
    let cutter = line(&mut s, [60.0, -10.0], [60.0, 10.0]);
    let r = s.execute("trim", &json!({ "handle": l.hex(), "pick": [90, 0], "edges": [cutter.hex()] })).unwrap();
    assert_eq!(handles(&r), vec![l], "the piece that stays is the same object: {r}");
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(60.0, 0.0))));
    let dm = dim(&s, d);
    assert_eq!(dm.assoc.len(), 2, "the dimension stays attached: {:?}", dm.assoc);
    assert!(dm.assoc.iter().all(|a| a.handle == l));
    assert!(near(dm.p14.xy(), Vec2::new(60.0, 0.0)), "p14 follows the trimmed end: {:?}", dm.p14);
    assert!(near(dm.p13.xy(), Vec2::ZERO));
    assert_eq!(measured(&s, d), "60.0000");
    // Undo restores the line and the dimension in one step.
    s.undo().unwrap();
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(100.0, 0.0))));
    assert!(near(dim(&s, d).p14.xy(), Vec2::new(100.0, 0.0)));
}

#[test]
fn a_trim_in_the_middle_keeps_the_handle_on_the_first_piece() {
    let mut s = Session::new();
    let (l, d) = dimensioned_line(&mut s, 100.0);
    let c1 = line(&mut s, [40.0, -10.0], [40.0, 10.0]);
    let c2 = line(&mut s, [60.0, -10.0], [60.0, 10.0]);
    let r = s.execute("trim", &json!({ "handle": l.hex(), "pick": [50, 0], "edges": [c1.hex(), c2.hex()] })).unwrap();
    let hs = handles(&r);
    assert_eq!(hs.len(), 2, "{r}");
    assert_eq!(hs[0], l, "the first piece keeps the handle: {r}");
    assert_ne!(hs[1], l, "the second piece is a new object: {r}");
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(40.0, 0.0))));
    assert_eq!(ends(&s, hs[1]), Some((Vec2::new(60.0, 0.0), Vec2::new(100.0, 0.0))));
    let dm = dim(&s, d);
    assert_eq!(dm.assoc.len(), 2, "{:?}", dm.assoc);
    assert!(near(dm.p14.xy(), Vec2::new(40.0, 0.0)), "p14 follows the first piece's end: {:?}", dm.p14);
    assert_eq!(measured(&s, d), "40.0000");
}

#[test]
fn trim_at_the_command_line_keeps_the_object_too() {
    let mut s = Session::new();
    let (l, d) = dimensioned_line(&mut s, 100.0);
    line(&mut s, [60.0, -10.0], [60.0, 10.0]);
    // Quick mode (the default): every object is a cutting edge; pick the part to remove.
    s.script("TRIM 90,0\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(60.0, 0.0))));
    let dm = dim(&s, d);
    assert_eq!(dm.assoc.len(), 2, "{:?}", dm.assoc);
    assert!(near(dm.p14.xy(), Vec2::new(60.0, 0.0)), "{:?}", dm.p14);
}

/// A LINE from 0,0 to 100,0 with a hyperlink and another application's data, read from DXF.
const XDATA_DXF: &str = "0\nSECTION\n2\nENTITIES\n\
0\nLINE\n5\n2A\n8\n0\n10\n0\n20\n0\n30\n0\n11\n100\n21\n0\n31\n0\n\
1001\nPE_URL\n1000\nhttps://example.com/door-7\n1002\n{\n1000\nDoor 7\n1002\n}\n\
1001\nACME_TAGS\n1000\nD-07\n1070\n3\n\
0\nENDSEC\n0\nEOF\n";

#[test]
fn the_kept_piece_keeps_its_xdata_and_hyperlink() {
    let mut s = Session::empty();
    s.open_drawing(cadcraft_io::read_dxf(XDATA_DXF.as_bytes()).unwrap(), "xdata", None);
    let l = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle).unwrap();
    let before = s.doc().unwrap().entity(l).unwrap().common.clone();
    assert!(before.xdata.iter().any(|t| t.value.trim() == "D-07"), "{before:?}");
    let cutter = line(&mut s, [60.0, -10.0], [60.0, 10.0]);
    let r = s.execute("trim", &json!({ "handle": l.hex(), "pick": [90, 0], "edges": [cutter.hex()] })).unwrap();
    assert_eq!(handles(&r), vec![l], "{r}");
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(60.0, 0.0))));
    assert_eq!(s.doc().unwrap().entity(l).unwrap().common, before);
}

#[test]
fn extend_keeps_the_line_so_its_dimension_follows() {
    let mut s = Session::new();
    let (l, d) = dimensioned_line(&mut s, 60.0);
    let boundary = line(&mut s, [100.0, -10.0], [100.0, 10.0]);
    s.execute("extend", &json!({ "handle": l.hex(), "pick": [55, 0], "edges": [boundary.hex()] })).unwrap();
    assert_eq!(ends(&s, l), Some((Vec2::ZERO, Vec2::new(100.0, 0.0))));
    let dm = dim(&s, d);
    assert_eq!(dm.assoc.len(), 2, "{:?}", dm.assoc);
    assert!(near(dm.p14.xy(), Vec2::new(100.0, 0.0)), "p14 follows the extended end: {:?}", dm.p14);
    assert_eq!(measured(&s, d), "100.0000");
}

#[test]
fn trimming_an_arc_keeps_the_arc() {
    let mut s = Session::new();
    // A half circle from 10,0 over 0,10 to -10,0, cut by the vertical line at x = 0.
    let a = handle(&s.execute("arc", &json!({ "center": [0, 0], "radius": 10, "start": 0, "end": 180 })).unwrap());
    let cutter = line(&mut s, [0.0, -20.0], [0.0, 20.0]);
    let r = s.execute("trim", &json!({ "handle": a.hex(), "pick": [-7.0710678, 7.0710678], "edges": [cutter.hex()] })).unwrap();
    assert_eq!(handles(&r), vec![a], "{r}");
    match &s.doc().unwrap().entity(a).unwrap().kind {
        EntityKind::Arc(ar) => {
            assert!((ar.start - 0.0).abs() < 1e-9 && (ar.end - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{ar:?}");
        }
        k => panic!("not an arc: {k:?}"),
    }
}

#[test]
fn trimming_a_hatch_boundary_open_ends_the_hatch_link_and_keeps_the_hatch() {
    let mut s = Session::new();
    let c = handle(&s.execute("circle", &json!({ "center": [0, 0], "radius": 10 })).unwrap());
    let h = handle(&s.execute("hatch", &json!({ "points": [[0, 0]] })).unwrap());
    let cutter = line(&mut s, [0.0, -20.0], [0.0, 20.0]);
    let r = s.execute("trim", &json!({ "handle": c.hex(), "pick": [-10, 0], "edges": [cutter.hex()] })).unwrap();
    assert_eq!(handles(&r), vec![c], "the circle stays the same object, now an arc: {r}");
    assert!(matches!(s.doc().unwrap().entity(c).unwrap().kind, EntityKind::Arc(_)));
    let hh = match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Hatch(x) => x.clone(),
        k => panic!("not a hatch: {k:?}"),
    };
    assert!(!hh.associative, "an open boundary ends the link");
    assert_eq!(hh.loops.len(), 1, "the hatch keeps its shape");
    assert!(s.log.iter().any(|m| m.contains("no longer encloses an area")), "{:?}", s.log);
}

// A polyline keeps its handle when it is trimmed, but its vertices are renumbered, so a
// dimension attached to one of its vertices (`AssocSnap::Vertex`) would follow the wrong vertex.
// Those attachments are dropped and the dimension keeps its points and its measurement.

/// A polyline 0,0 - 50,0 - 100,0 - 100,50 and a linear dimension of its last segment.
fn polyline_with_dim(s: &mut Session, p1: [f64; 2], p2: [f64; 2], at: [f64; 2]) -> (Handle, Handle) {
    let pl = handle(&s.execute("pline", &json!({ "points": [[0, 0], [50, 0], [100, 0], [100, 50]] })).unwrap());
    let d = handle(&s.execute("dimlinear", &json!({ "p1": p1, "p2": p2, "at": at })).unwrap());
    (pl, d)
}

fn vertex_attachments(s: &Session, d: Handle, pl: Handle) -> Vec<usize> {
    let mut v: Vec<usize> = dim(s, d)
        .assoc
        .iter()
        .filter(|a| a.handle == pl)
        .filter_map(|a| match a.snap {
            cadcraft_engine::doc::AssocSnap::Vertex { index } => Some(index),
            _ => None,
        })
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn trimming_a_polyline_drops_the_vertex_attachments_of_its_dimension() {
    let mut s = Session::new();
    let (pl, d) = polyline_with_dim(&mut s, [100.0, 0.0], [100.0, 50.0], [120.0, 25.0]);
    assert_eq!(vertex_attachments(&s, d, pl), vec![2, 3], "{:?}", dim(&s, d).assoc);
    let before = dim(&s, d);
    let cutter = line(&mut s, [75.0, -10.0], [75.0, 10.0]);
    let r = s.execute("trim", &json!({ "handle": pl.hex(), "pick": [10, 0], "edges": [cutter.hex()] })).unwrap();
    assert_eq!(handles(&r), vec![pl], "{r}");
    let dm = dim(&s, d);
    assert!(dm.assoc.is_empty(), "no attachment is left: {:?}", dm.assoc);
    assert_eq!((dm.p13, dm.p14), (before.p13, before.p14), "the points stay");
    assert_eq!(measured(&s, d), "50.0000");
}

#[test]
fn a_cut_that_removes_the_attached_vertices_drops_them_too() {
    let mut s = Session::new();
    let (pl, d) = polyline_with_dim(&mut s, [0.0, 0.0], [50.0, 0.0], [25.0, -20.0]);
    assert_eq!(vertex_attachments(&s, d, pl), vec![0, 1], "{:?}", dim(&s, d).assoc);
    let before = dim(&s, d);
    let cutter = line(&mut s, [75.0, -10.0], [75.0, 10.0]);
    s.execute("trim", &json!({ "handle": pl.hex(), "pick": [10, 0], "edges": [cutter.hex()] })).unwrap();
    let dm = dim(&s, d);
    assert!(dm.assoc.is_empty(), "{:?}", dm.assoc);
    assert_eq!((dm.p13, dm.p14), (before.p13, before.p14));
    assert_eq!(measured(&s, d), "50.0000");
}

/// Safe and the same as before the object was kept: even a trim at the end, which leaves the
/// early vertices where they were, drops the vertex attachments.
#[test]
fn an_end_trim_of_a_polyline_drops_the_vertex_attachments_as_well() {
    let mut s = Session::new();
    let (pl, d) = polyline_with_dim(&mut s, [0.0, 0.0], [50.0, 0.0], [25.0, -20.0]);
    assert_eq!(vertex_attachments(&s, d, pl), vec![0, 1]);
    let cutter = line(&mut s, [90.0, 25.0], [110.0, 25.0]);
    s.execute("trim", &json!({ "handle": pl.hex(), "pick": [100, 45], "edges": [cutter.hex()] })).unwrap();
    assert!(dim(&s, d).assoc.is_empty(), "{:?}", dim(&s, d).assoc);
    assert_eq!(measured(&s, d), "50.0000");
}

#[test]
fn undoing_the_trim_restores_the_polyline_and_its_attachments() {
    let mut s = Session::new();
    let (pl, d) = polyline_with_dim(&mut s, [100.0, 0.0], [100.0, 50.0], [120.0, 25.0]);
    let cutter = line(&mut s, [75.0, -10.0], [75.0, 10.0]);
    s.execute("trim", &json!({ "handle": pl.hex(), "pick": [10, 0], "edges": [cutter.hex()] })).unwrap();
    assert!(dim(&s, d).assoc.is_empty());
    // `Session::undo` (the undo stack itself, as the line test above).
    s.undo().unwrap();
    match &s.doc().unwrap().entity(pl).unwrap().kind {
        EntityKind::LwPolyline(p) => assert_eq!(p.vertices.len(), 4),
        k => panic!("not a polyline: {k:?}"),
    }
    assert_eq!(vertex_attachments(&s, d, pl), vec![2, 3], "{:?}", dim(&s, d).assoc);
    assert_eq!(measured(&s, d), "50.0000");
}

/// A trimmed polyline with its dimension: the session, the polyline and the dimension.
fn trimmed_polyline() -> (Session, Handle, Handle) {
    let mut s = Session::new();
    let (pl, d) = polyline_with_dim(&mut s, [100.0, 0.0], [100.0, 50.0], [120.0, 25.0]);
    let cutter = line(&mut s, [75.0, -10.0], [75.0, 10.0]);
    s.execute("trim", &json!({ "handle": pl.hex(), "pick": [10, 0], "edges": [cutter.hex()] })).unwrap();
    assert!(dim(&s, d).assoc.is_empty());
    (s, pl, d)
}

/// The polyline is whole again, the dimension is attached to its vertices 2 and 3 and measures 50.
fn assert_restored(s: &Session, pl: Handle, d: Handle) {
    match &s.doc().unwrap().entity(pl).unwrap().kind {
        EntityKind::LwPolyline(p) => assert_eq!(p.vertices.len(), 4),
        k => panic!("not a polyline: {k:?}"),
    }
    assert_eq!(vertex_attachments(s, d, pl), vec![2, 3], "{:?}", dim(s, d).assoc);
    assert_eq!(measured(s, d), "50.0000");
}

/// The trim is applied again: the attachments to the polyline's vertices are gone, the dimension
/// keeps its points and its measurement.
fn assert_trimmed(s: &Session, pl: Handle, d: Handle) {
    assert!(vertex_attachments(s, d, pl).is_empty(), "{:?}", dim(s, d).assoc);
    assert_eq!(measured(s, d), "50.0000");
}

/// UNDO and REDO swap in a stored drawing, which is already consistent: the dimension pass must
/// not compare it with the drawing it replaced, where the polyline keeps its handle but its
/// vertex 3 is another point.
#[test]
fn undo_through_the_command_restores_the_attachments() {
    let (mut s, pl, d) = trimmed_polyline();
    s.execute("u", &json!({})).unwrap();
    assert_restored(&s, pl, d);
    s.execute("redo", &json!({})).unwrap();
    assert_trimmed(&s, pl, d);
}

#[test]
fn undo_at_the_command_line_restores_the_attachments() {
    let (mut s, pl, d) = trimmed_polyline();
    s.cmdline("u").unwrap();
    assert_restored(&s, pl, d);
    s.cmdline("redo").unwrap();
    assert_trimmed(&s, pl, d);
}

#[test]
fn undo_with_a_count_restores_the_attachments() {
    let (mut s, pl, d) = trimmed_polyline();
    s.execute("undo", &json!({ "count": 1 })).unwrap();
    assert_restored(&s, pl, d);
    s.execute("redo", &json!({ "count": 1 })).unwrap();
    assert_trimmed(&s, pl, d);
}

#[test]
fn undo_back_and_mredo_restore_the_attachments() {
    let (mut s, pl, d) = trimmed_polyline();
    s.execute("undo", &json!({ "mode": "back", "confirm": true })).unwrap();
    assert!(s.doc().unwrap().entity(pl).is_none(), "everything is undone");
    s.execute("mredo", &json!({ "all": true })).unwrap();
    assert_trimmed(&s, pl, d);
    s.execute("u", &json!({})).unwrap();
    assert_restored(&s, pl, d);
    s.execute("mredo", &json!({ "count": 1 })).unwrap();
    assert_trimmed(&s, pl, d);
}
