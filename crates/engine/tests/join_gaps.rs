//! JOIN across gaps: collinear lines become one line and arcs of one circle one arc (#1174).

use std::f64::consts::{FRAC_PI_2, PI};

use cadcraft_engine::Session;
use cadcraft_engine::doc::{Arc as DocArc, EntityKind, Handle, Line as DocLine};
use cadcraft_engine::geom::{Vec2, Vec3};
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

fn kind(s: &Session, h: &str) -> EntityKind {
    s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind.clone()
}

fn exists(s: &Session, h: &str) -> bool {
    s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).is_some()
}

fn count(s: &Session) -> usize {
    s.doc().unwrap().model.iter().count()
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    h(&s.execute("line", &json!({ "points": [a, b] })).unwrap())
}

/// An arc of the circle at (50,50), radius 30, from `start` to `end` degrees counter-clockwise.
fn arc(s: &mut Session, start: f64, end: f64) -> String {
    h(&s.execute("arc", &json!({ "center": [50, 50], "radius": 30, "start": start, "end": end })).unwrap())
}

fn line_ends(s: &Session, h: &str) -> (Vec2, Vec2) {
    match kind(s, h) {
        EntityKind::Line(l) => (l.a.xy(), l.b.xy()),
        k => panic!("not a line: {k:?}"),
    }
}

/// (center, radius, start, end) of an arc, angles in radians.
fn arc_of(s: &Session, h: &str) -> (Vec2, f64, f64, f64) {
    match kind(s, h) {
        EntityKind::Arc(a) => (a.center.xy(), a.radius, a.start, a.end),
        k => panic!("not an arc: {k:?}"),
    }
}

fn circle_of(s: &Session, h: &str) -> (Vec2, f64) {
    match kind(s, h) {
        EntityKind::Circle(c) => (c.center.xy(), c.radius),
        k => panic!("not a circle: {k:?}"),
    }
}

fn near_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d < 1e-9 || d > std::f64::consts::TAU - 1e-9
}

fn assert_arc(s: &Session, h: &str, start: f64, end: f64) {
    let (c, r, a, b) = arc_of(s, h);
    assert!(c.near(Vec2::new(50.0, 50.0), 1e-9), "{c:?}");
    assert!((r - 30.0).abs() < 1e-9, "{r}");
    assert!(near_angle(a, start) && near_angle(b, end), "arc from {a} to {b}, wanted {start} to {end}");
}

// ---------------- #1174: lines ----------------

#[test]
fn join_collinear_lines_across_a_gap_makes_one_line() {
    // LINE 0,0 50,0 and LINE 120,0 150,0: one LINE from 0,0 to 150,0.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [50.0, 0.0]);
    let b = line(&mut s, [120.0, 0.0], [150.0, 0.0]);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(h(&r), a, "the source line keeps its handle");
    let (p, q) = line_ends(&s, &a);
    assert!(p.near(Vec2::new(0.0, 0.0), 1e-9), "{p:?}");
    assert!(q.near(Vec2::new(150.0, 0.0), 1e-9), "{q:?}");
    assert_eq!(count(&s), 1);
    assert!(!exists(&s, &b));
}

#[test]
fn join_collinear_lines_in_any_order_overlapping_and_sloped() {
    // Three lines on y = x, picked out of order, two of them overlapping, the source pointing
    // back: one line over the whole extent.
    let mut s = Session::new();
    let a = line(&mut s, [30.0, 30.0], [10.0, 10.0]);
    let b = line(&mut s, [-20.0, -20.0], [-5.0, -5.0]);
    let c = line(&mut s, [20.0, 20.0], [60.0, 60.0]);
    let r = s.execute("join", &json!({ "handles": [a, b, c] })).unwrap();
    assert_eq!(h(&r), a);
    let (p, q) = line_ends(&s, &a);
    let (lo, hi) = (Vec2::new(-20.0, -20.0), Vec2::new(60.0, 60.0));
    assert!((p.near(lo, 1e-9) && q.near(hi, 1e-9)) || (p.near(hi, 1e-9) && q.near(lo, 1e-9)), "{p:?} {q:?}");
    assert_eq!(count(&s), 1);
}

#[test]
fn join_touching_collinear_lines_make_one_line_not_a_polyline() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [10.0, 0.0], [20.0, 0.0]);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    let (p, q) = line_ends(&s, &a);
    assert!(p.near(Vec2::new(0.0, 0.0), 1e-9) && q.near(Vec2::new(20.0, 0.0), 1e-9), "{p:?} {q:?}");
    assert_eq!(count(&s), 1);
}

#[test]
fn join_lines_off_the_source_line_are_refused_across_a_gap() {
    // Parallel but not collinear, with a gap: nothing joins and both stay.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [50.0, 0.0]);
    let b = line(&mut s, [120.0, 1.0], [150.0, 1.0]);
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
    assert_eq!(count(&s), 2);
    assert!(exists(&s, &a) && exists(&s, &b));
    // A line crossing the source's line without touching it: the same.
    let c = line(&mut s, [70.0, -10.0], [70.0, 10.0]);
    assert!(s.execute("join", &json!({ "handles": [a, c] })).is_err());
    assert_eq!(count(&s), 3);
}

#[test]
fn join_lines_meeting_at_a_corner_still_make_a_polyline() {
    // Lines that are not collinear keep chaining end to end into a polyline.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [5.0, 0.0]);
    let b = line(&mut s, [5.0, 0.0], [5.0, 5.0]);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert!(matches!(kind(&s, &h(&r)), EntityKind::LwPolyline(p) if p.vertices.len() == 3));
    assert_eq!(count(&s), 1);
}

// ---------------- #1174: arcs ----------------

#[test]
fn join_arcs_of_one_circle_across_a_gap_makes_one_arc() {
    // ARC C 50,50 80,50 50,80 (0° to 90°) and ARC C 50,50 20,50 50,20 (180° to 270°): one arc
    // counter-clockwise from the source's start, 0 to 4.71239 rad.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 180.0, 270.0);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(h(&r), a, "the source arc keeps its handle");
    assert_arc(&s, &a, 0.0, 1.5 * PI);
    assert_eq!(count(&s), 1);
    assert!(!exists(&s, &b));
    // Picked the other way round the second arc is the source: from 180° counter-clockwise
    // round to 90°.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 180.0, 270.0);
    let r = s.execute("join", &json!({ "handles": [b, a] })).unwrap();
    assert_eq!(h(&r), b);
    assert_arc(&s, &b, PI, FRAC_PI_2);
    assert_eq!(count(&s), 1);
}

#[test]
fn join_arcs_touching_overlapping_or_running_back_over_the_source_make_one_arc() {
    // Touching.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 90.0, 180.0);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_arc(&s, &a, 0.0, PI);
    assert_eq!(count(&s), 1);
    // Overlapping, three arcs out of order.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 120.0);
    let b = arc(&mut s, 200.0, 260.0);
    let c = arc(&mut s, 90.0, 210.0);
    s.execute("join", &json!({ "handles": [a, b, c] })).unwrap();
    assert_arc(&s, &a, 0.0, 260.0_f64.to_radians());
    assert_eq!(count(&s), 1);
    // An arc running back across the source's start: the result starts where that arc starts
    // and holds every piece.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 300.0, 30.0);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_arc(&s, &a, 300.0_f64.to_radians(), FRAC_PI_2);
    assert_eq!(count(&s), 1);
}

#[test]
fn join_arcs_covering_the_whole_circle_make_a_circle() {
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 180.0);
    let b = arc(&mut s, 180.0, 0.0);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(h(&r), a);
    let (c, radius) = circle_of(&s, &a);
    assert!(c.near(Vec2::new(50.0, 50.0), 1e-9) && (radius - 30.0).abs() < 1e-9, "{c:?} {radius}");
    assert_eq!(count(&s), 1);
    // Three arcs round the circle, the last one running back over the first's start.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 100.0);
    let b = arc(&mut s, 100.0, 250.0);
    let c = arc(&mut s, 250.0, 10.0);
    s.execute("join", &json!({ "handles": [a, b, c] })).unwrap();
    circle_of(&s, &a);
    assert_eq!(count(&s), 1);
}

#[test]
fn join_arcs_of_different_circles_are_refused_across_a_gap() {
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = h(&s.execute("arc", &json!({ "center": [50, 50], "radius": 20, "start": 180, "end": 270 })).unwrap());
    let c = h(&s.execute("arc", &json!({ "center": [51, 50], "radius": 30, "start": 180, "end": 270 })).unwrap());
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
    assert!(s.execute("join", &json!({ "handles": [a, c] })).is_err());
    assert_eq!(count(&s), 3);
    assert_arc(&s, &a, 0.0, FRAC_PI_2);
    // Arcs of different circles that touch end to end still chain into a polyline.
    let mut s = Session::new();
    let a = h(&s.execute("arc", &json!({ "center": [0, 0], "radius": 10, "start": 0, "end": 90 })).unwrap());
    let b = h(&s.execute("arc", &json!({ "center": [0, 20], "radius": 10, "start": 270, "end": 0 })).unwrap());
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert!(matches!(kind(&s, &h(&r)), EntityKind::LwPolyline(_)));
    assert_eq!(count(&s), 1);
}

#[test]
fn join_at_the_command_line_names_the_line_or_arc() {
    let mut s = Session::new();
    line(&mut s, [0.0, 0.0], [50.0, 0.0]);
    line(&mut s, [120.0, 0.0], [150.0, 0.0]);
    s.script("JOIN ALL\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(s.log.iter().any(|l| l.contains("2 objects joined into 1 line")), "{:?}", s.log);
    assert_eq!(count(&s), 1);
    let mut s = Session::new();
    arc(&mut s, 0.0, 90.0);
    arc(&mut s, 180.0, 270.0);
    s.script("JOIN ALL\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(s.log.iter().any(|l| l.contains("2 objects joined into 1 arc")), "{:?}", s.log);
    assert_eq!(count(&s), 1);
}

// ---------------- review follow-ups ----------------

#[test]
fn arcs_touching_the_start_of_the_source_do_not_close_a_circle() {
    // An arc ending exactly where the source starts moves the start back, it does not wrap the circle.
    let mut s = Session::new();
    let a = arc(&mut s, 90.0, 180.0);
    let b = arc(&mut s, 0.0, 90.0);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(h(&r), a);
    assert_eq!(count(&s), 1);
    assert_arc(&s, &a, 0.0, PI);
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 270.0, 360.0);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(count(&s), 1);
    assert_arc(&s, &a, 3.0 * FRAC_PI_2, 2.5 * PI);
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let b = arc(&mut s, 180.0, 270.0);
    let c = arc(&mut s, 270.0, 360.0);
    s.execute("join", &json!({ "handles": [a, b, c] })).unwrap();
    assert_eq!(count(&s), 1);
    assert_arc(&s, &a, PI, 2.5 * PI);
    // Four quadrants that really cover the circle still make one.
    let mut s = Session::new();
    let a = arc(&mut s, 0.0, 90.0);
    let rest = [arc(&mut s, 90.0, 180.0), arc(&mut s, 180.0, 270.0), arc(&mut s, 270.0, 360.0)];
    s.execute("join", &json!({ "handles": [a.clone(), rest[0].clone(), rest[1].clone(), rest[2].clone()] })).unwrap();
    assert_eq!(count(&s), 1);
    assert_eq!(circle_of(&s, &a), (Vec2::new(50.0, 50.0), 30.0));
}

#[test]
fn lines_not_collinear_in_3d_do_not_join() {
    let mut s = Session::new();
    let a = s.add_entity(EntityKind::Line(DocLine { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(10.0, 0.0, 10.0) })).unwrap().hex();
    let b = s.add_entity(EntityKind::Line(DocLine { a: Vec3::new(20.0, 0.0, 0.0), b: Vec3::new(30.0, 0.0, 0.0) })).unwrap().hex();
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
    assert_eq!(count(&s), 2);
    match kind(&s, &a) {
        EntityKind::Line(l) => assert_eq!(l.b, Vec3::new(10.0, 0.0, 10.0)),
        k => panic!("not a line: {k:?}"),
    }
}

#[test]
fn joined_lines_keep_their_elevation() {
    let mut s = Session::new();
    let a = s.add_entity(EntityKind::Line(DocLine { a: Vec3::new(0.0, 0.0, 5.0), b: Vec3::new(10.0, 0.0, 5.0) })).unwrap().hex();
    let b = s.add_entity(EntityKind::Line(DocLine { a: Vec3::new(20.0, 0.0, 5.0), b: Vec3::new(30.0, 0.0, 5.0) })).unwrap().hex();
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(count(&s), 1);
    match kind(&s, &a) {
        EntityKind::Line(l) => assert_eq!((l.a, l.b), (Vec3::new(0.0, 0.0, 5.0), Vec3::new(30.0, 0.0, 5.0))),
        k => panic!("not a line: {k:?}"),
    }
}

#[test]
fn arcs_with_non_finite_angles_are_not_absorbed() {
    // Such an arc is no part of the source's circle: it never becomes a circle or an arc of the
    // source, it is left to the polyline chain as before (or refused).
    for (start, end) in [(f64::NAN, f64::NAN), (f64::INFINITY, 1.0), (0.5, f64::NEG_INFINITY)] {
        let mut s = Session::new();
        let a = arc(&mut s, 0.0, 90.0);
        let b = s.add_entity(EntityKind::Arc(DocArc { center: Vec3::new(50.0, 50.0, 0.0), radius: 30.0, start, end })).unwrap().hex();
        match s.execute("join", &json!({ "handles": [a, b] })) {
            Ok(r) => assert!(matches!(kind(&s, &h(&r)), EntityKind::LwPolyline(_)), "angles {start}, {end}: {:?}", kind(&s, &h(&r))),
            Err(_) => assert_eq!(count(&s), 2),
        }
    }
}

// ---------------- review follow-up 2: whole degrees, elevation, extrusion ----------------

fn deg(a: f64) -> f64 {
    a.rem_euclid(360.0)
}

#[test]
fn arcs_touching_or_overlapping_in_whole_degrees_stay_one_arc() {
    // Touching: 114-124 with 104-114; 29-149 with -1-29. Overlapping: 114-144 with 104-134; 29-59 with -11-39.
    // They leave a gap, so the result is one arc holding both, never the circle.
    for (src, other, from, to) in [
        ((114.0, 124.0), (104.0, 114.0), 104.0, 124.0),
        ((29.0, 149.0), (-1.0, 29.0), -1.0, 149.0),
        ((114.0, 144.0), (104.0, 134.0), 104.0, 144.0),
        ((29.0, 59.0), (-11.0, 39.0), -11.0, 59.0),
    ] {
        let mut s = Session::new();
        let a = arc(&mut s, deg(src.0), deg(src.1));
        let b = arc(&mut s, deg(other.0), deg(other.1));
        let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
        assert_eq!(h(&r), a);
        assert_eq!(count(&s), 1);
        assert_arc(&s, &a, f64::to_radians(from), f64::to_radians(to));
    }
}

/// Whether the integer-degree arcs (start, sweep) together cover the whole circle.
fn covers_circle(arcs: &[(i32, i32)]) -> bool {
    let mut cov = [false; 360];
    for (st, sw) in arcs {
        for k in 0..*sw {
            cov[(st + k).rem_euclid(360) as usize] = true;
        }
    }
    cov.iter().all(|c| *c)
}

/// Joins arcs (start, sweep in whole degrees; the first is the source) and checks the outcome.
fn check_join_in_degrees(arcs: &[(i32, i32)]) -> Result<(), String> {
    let mut s = Session::new();
    let hs: Vec<String> = arcs.iter().map(|(st, sw)| arc(&mut s, deg(*st as f64), deg((*st + *sw) as f64))).collect();
    let r = s.execute("join", &json!({ "handles": hs })).map_err(|e| format!("{arcs:?}: refused {e}"))?;
    if h(&r) != hs[0] {
        return Err(format!("{arcs:?}: source handle lost"));
    }
    if count(&s) != 1 {
        return Err(format!("{arcs:?}: {} entities left", count(&s)));
    }
    let covered = covers_circle(arcs);
    match kind(&s, &hs[0]) {
        EntityKind::Circle(_) if covered => Ok(()),
        EntityKind::Circle(_) => Err(format!("{arcs:?}: CIRCLE but the inputs leave a gap")),
        EntityKind::Arc(_) if covered => Err(format!("{arcs:?}: ARC but the inputs cover the circle")),
        EntityKind::Arc(res) => {
            let ext = (res.end - res.start).rem_euclid(std::f64::consts::TAU);
            for (st, sw) in arcs {
                let off = (deg(*st as f64).to_radians() - res.start).rem_euclid(std::f64::consts::TAU);
                // An offset a hair under a full turn is a hair behind the start.
                let off = if off > std::f64::consts::TAU - 1e-9 { 0.0 } else { off };
                if off + (*sw as f64).to_radians() > ext + 1e-9 {
                    return Err(format!("{arcs:?}: piece {st}+{sw} not inside the result {} to {}", res.start, res.end));
                }
            }
            Ok(())
        }
        k => Err(format!("{arcs:?}: {k:?}")),
    }
}

#[test]
fn join_of_whole_degree_arcs_holds_every_piece_and_closes_a_circle_only_when_covered() {
    let mut bad = Vec::new();
    let mut n = 0;
    for start in (0..360).step_by(5) {
        for sw in [10, 30, 90, 180, 270] {
            for off in [-30, -20, -10, 0, 10, sw - 10, sw, sw + 10, sw + 30] {
                for osw in [10, 20, 90, 200] {
                    // The other arc starts `off` after the source's start: touching either end
                    // (off = sw, off = -osw) or overlapping it, or clear of it.
                    for o in [off, -osw] {
                        n += 1;
                        if let Err(e) = check_join_in_degrees(&[(start, sw), (start + o, osw)]) {
                            bad.push(e);
                        }
                    }
                }
            }
        }
    }
    // Three arcs: the source, one touching its end and one touching its start.
    for start in (0..360).step_by(15) {
        for (sw, a, b) in [(30, 20, 40), (90, 100, 120), (60, 150, 140)] {
            n += 1;
            if let Err(e) = check_join_in_degrees(&[(start, sw), (start + sw, a), (start - b, b)]) {
                bad.push(e);
            }
        }
    }
    assert!(n > 5000);
    assert!(bad.is_empty(), "{} of {n} wrong, first: {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

fn arc_at(s: &mut Session, z: f64, start: f64, end: f64) -> String {
    s.add_entity(EntityKind::Arc(DocArc { center: Vec3::new(50.0, 50.0, z), radius: 30.0, start: start.to_radians(), end: end.to_radians() }))
        .unwrap()
        .hex()
}

#[test]
fn arcs_at_another_elevation_are_not_absorbed_across_a_gap() {
    for z in [10.0, f64::NAN, f64::INFINITY] {
        let mut s = Session::new();
        let a = arc_at(&mut s, 0.0, 0.0, 90.0);
        let b = arc_at(&mut s, z, 180.0, 270.0);
        assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err(), "z = {z}");
        assert_eq!(count(&s), 2);
        assert!(exists(&s, &a) && exists(&s, &b));
        assert_eq!(arc_of(&s, &a).2, 0.0);
    }
}

#[test]
fn arcs_with_another_extrusion_are_not_absorbed_across_a_gap() {
    // A flipped extrusion.
    let mut s = Session::new();
    let a = arc_at(&mut s, 0.0, 0.0, 90.0);
    let b = arc_at(&mut s, 0.0, 180.0, 270.0);
    let hb = Handle::parse_hex(&b).unwrap();
    s.doc_mut().unwrap().modify_entity(hb, |e| e.common.extrusion = Vec3::new(0.0, 0.0, -1.0)).unwrap();
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
    assert_eq!(count(&s), 2);
    // A non-finite extrusion too.
    let mut s = Session::new();
    let a = arc_at(&mut s, 0.0, 0.0, 90.0);
    let b = arc_at(&mut s, 0.0, 180.0, 270.0);
    let hb = Handle::parse_hex(&b).unwrap();
    s.doc_mut().unwrap().modify_entity(hb, |e| e.common.extrusion = Vec3::new(f64::NAN, 0.0, 1.0)).unwrap();
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
    assert_eq!(count(&s), 2);
}

#[test]
fn joined_arcs_keep_their_elevation() {
    let mut s = Session::new();
    let a = arc_at(&mut s, 5.0, 0.0, 90.0);
    let b = arc_at(&mut s, 5.0, 180.0, 270.0);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(count(&s), 1);
    match kind(&s, &a) {
        EntityKind::Arc(r) => assert_eq!(r.center, Vec3::new(50.0, 50.0, 5.0)),
        k => panic!("not an arc: {k:?}"),
    }
}
