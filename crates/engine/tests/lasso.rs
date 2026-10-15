//! Lasso selection, as in AutoCAD (PICKAUTO bit 4): a press-and-drag that starts on empty space
//! draws a free-form lasso. Dragging to the right makes a window lasso, to the left a crossing
//! lasso; Spacebar while dragging cycles crossing, window and fence.

use cadcraft_engine::doc::{Common, EntityKind, Handle, Solid, Space, entity_bounds};
use cadcraft_engine::geom::Vec2;
use cadcraft_engine::select::{Lasso, LassoKind};
use cadcraft_engine::{Input, Session};
use serde_json::json;

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    let r = s.execute("line", &json!({ "points": [a, b] })).unwrap();
    Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap()
}

/// The triangle (-2,-3), (14,-3), (-2,13), around: `inside` entirely in it, `crossed` through its
/// sloping side, `left` through its left side, `corner` inside its bounding box but not in it.
struct Fixture {
    s: Session,
    inside: Handle,
    crossed: Handle,
    left: Handle,
    corner: Handle,
}

fn fixture() -> Fixture {
    let mut s = Session::new();
    let inside = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let crossed = line(&mut s, [2.0, 5.0], [30.0, 5.0]);
    let left = line(&mut s, [-5.0, 5.0], [-1.0, 5.0]);
    let corner = line(&mut s, [10.0, 8.0], [12.0, 8.0]);
    s.set_selection(Vec::new());
    Fixture { s, inside, crossed, left, corner }
}

fn lasso(pts: &[[f64; 2]]) -> Lasso {
    let mut l = Lasso::new(Vec2::new(pts[0][0], pts[0][1]));
    for p in &pts[1..] {
        l.push(Vec2::new(p[0], p[1]));
    }
    l
}

/// Starting at the bottom left and going right first.
const RIGHTWARD: &[[f64; 2]] = &[[-2.0, -3.0], [6.0, -3.0], [14.0, -3.0], [4.0, 7.0], [-2.0, 13.0]];
/// The same triangle, starting at the bottom right and going left first.
const LEFTWARD: &[[f64; 2]] = &[[14.0, -3.0], [6.0, -3.0], [-2.0, -3.0], [-2.0, 4.0], [-2.0, 13.0]];

fn sorted(mut v: Vec<Handle>) -> Vec<Handle> {
    v.sort();
    v
}

#[test]
fn dragging_right_is_a_window_lasso() {
    let mut f = fixture();
    let l = lasso(RIGHTWARD);
    assert_eq!(l.kind(), LassoKind::Window);
    f.s.end_lasso(&l, false).unwrap();
    // Not `corner`: the lasso is the drawn shape, not its bounding rectangle.
    assert_eq!(f.s.selection(), vec![f.inside]);
}

#[test]
fn dragging_left_is_a_crossing_lasso() {
    let mut f = fixture();
    let l = lasso(LEFTWARD);
    assert_eq!(l.kind(), LassoKind::Crossing);
    f.s.end_lasso(&l, false).unwrap();
    assert_eq!(sorted(f.s.selection()), sorted(vec![f.inside, f.crossed, f.left]));
    assert!(!f.s.selection().contains(&f.corner));
}

#[test]
fn spacebar_cycles_crossing_window_fence() {
    let mut l = lasso(LEFTWARD);
    let mut seen = vec![l.kind()];
    for _ in 0..3 {
        l.cycle();
        seen.push(l.kind());
    }
    assert_eq!(seen, vec![LassoKind::Crossing, LassoKind::Window, LassoKind::Fence, LassoKind::Crossing]);
    // A cycled mode stays whichever way the drag goes on.
    l.push(Vec2::new(-30.0, 0.0));
    assert_eq!(l.kind(), LassoKind::Crossing);
}

#[test]
fn a_fence_lasso_selects_what_its_path_crosses() {
    let mut f = fixture();
    let mut l = lasso(RIGHTWARD);
    l.cycle();
    assert_eq!(l.kind(), LassoKind::Fence);
    f.s.end_lasso(&l, false).unwrap();
    // The path is not closed: `left` crosses only the side a polygon would add.
    assert_eq!(f.s.selection(), vec![f.crossed]);
}

#[test]
fn shift_lasso_removes_from_the_selection() {
    let mut f = fixture();
    f.s.set_selection(vec![f.inside, f.corner]);
    f.s.end_lasso(&lasso(RIGHTWARD), true).unwrap();
    assert_eq!(f.s.selection(), vec![f.corner]);
}

#[test]
fn a_lasso_feeds_a_command_selecting_objects() {
    let mut f = fixture();
    f.s.cmdline("erase").unwrap();
    assert!(f.s.lasso_at(Vec2::new(-2.0, -3.0)));
    f.s.end_lasso(&lasso(RIGHTWARD), false).unwrap();
    f.s.input(Input::Enter).unwrap();
    let d = f.s.doc().unwrap();
    assert!(d.entity(f.inside).is_none());
    assert!(d.entity(f.crossed).is_some() && d.entity(f.corner).is_some());
}

#[test]
fn where_a_lasso_starts() {
    let mut f = fixture();
    assert_eq!(cadcraft_engine::sysvars::get(&f.s, "PICKAUTO"), Some(json!(5)));
    // Empty space starts one; an object under the press does not (a window starts there instead).
    assert!(f.s.lasso_at(Vec2::new(-2.0, -3.0)));
    assert!(!f.s.lasso_at(Vec2::new(5.0, 0.0)));
    // A drawing command never draws one, and neither does a hostile point.
    f.s.cmdline("line").unwrap();
    assert!(!f.s.lasso_at(Vec2::new(-2.0, -3.0)));
    f.s.cancel();
    assert!(!f.s.lasso_at(Vec2::new(f64::NAN, 0.0)));
    // PICKAUTO without bit 4 turns the lasso off: press-and-drag opens a window.
    f.s.execute("setvar", &json!({"name": "PICKAUTO", "value": 1})).unwrap();
    assert!(!f.s.lasso_at(Vec2::new(-2.0, -3.0)));
    assert!(f.s.execute("setvar", &json!({"name": "PICKAUTO", "value": 8})).is_err());
}

/// A small diamond around `c`, drawn leftwards (a crossing lasso).
fn small_crossing_lasso(c: Vec2, r: f64) -> Lasso {
    let pts = [[c.x + r, c.y], [c.x, c.y + r], [c.x - r, c.y], [c.x, c.y - r]];
    lasso(&pts)
}

#[test]
fn a_crossing_lasso_inside_text_or_a_solid_selects_it() {
    // As a crossing window wholly inside them does: text is hit anywhere in its box, a SOLID in its fill.
    let mut s = Session::new();
    let r = s.execute("mtext", &json!({"at": [0, 40], "text": "A LONG LINE OF TEXT", "height": 5})).unwrap();
    let text = Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
    let corners = [[0.0, 0.0], [30.0, 0.0], [0.0, 30.0], [30.0, 30.0]].map(|[x, y]| cadcraft_engine::geom::Vec3::new(x, y, 0.0));
    let solid = s.doc_mut().unwrap().add(&Space::Model, Common::default(), EntityKind::Solid(Solid { corners })).unwrap();
    s.set_selection(Vec::new());
    let d = s.doc().unwrap();
    let tb = entity_bounds(d, d.entity(text).unwrap(), 0);
    for (h, c, r) in [(text, tb.center(), tb.height().min(tb.width()) / 4.0), (solid, Vec2::new(15.0, 15.0), 2.0)] {
        let l = small_crossing_lasso(c, r);
        assert_eq!(l.kind(), LassoKind::Crossing);
        assert_eq!(cadcraft_engine::select::select_polygon(s.doc().unwrap(), &Space::Model, &l.pts, true), vec![h]);
        // A window lasso there selects nothing.
        assert!(cadcraft_engine::select::select_polygon(s.doc().unwrap(), &Space::Model, &l.pts, false).is_empty());
        s.set_selection(Vec::new());
        s.end_lasso(&l, false).unwrap();
        assert_eq!(s.selection(), vec![h]);
    }
}
