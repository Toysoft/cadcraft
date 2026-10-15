//! Lasso selection on the canvas, as in AutoCAD: a press-and-drag on empty space draws a lasso
//! that follows the pointer; to the right it is a window lasso, to the left a crossing lasso, and
//! Spacebar while dragging cycles crossing, window and fence.
//!
//! `CADCRAFT_LASSO_SHOTS=<dir>` saves a picture of a lasso being dragged there.

use cadcraft_doc::Handle;
use cadcraft_engine::Session;
use cadcraft_geom::Vec2;
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    let r = s.execute("line", &json!({ "points": [a, b] })).unwrap();
    Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap()
}

/// `inside` lies in the triangle (-2,-3), (14,-3), (-2,13); `crossed` goes through its sloping
/// side; `corner` is inside its bounding box but not in it; `outside` is far away.
fn harness(render: bool) -> (Harness<'static, CadApp>, [Handle; 4]) {
    let mut session = Session::new();
    session.new_drawing(true);
    session.viewport_px = (1600.0, 1000.0);
    let inside = line(&mut session, [0.0, 0.0], [10.0, 0.0]);
    let crossed = line(&mut session, [2.0, 5.0], [30.0, 5.0]);
    let corner = line(&mut session, [10.0, 8.0], [12.0, 8.0]);
    let outside = line(&mut session, [40.0, 20.0], [50.0, 20.0]);
    session.set_selection(Vec::new());
    session.execute("view.set", &json!({"center": [10, 5], "height": 40})).unwrap();
    let mut app = CadApp::new(session, Services::default());
    app.canvas.zoom_pending = false;
    let builder = Harness::builder().with_size(vec2(1600.0, 1000.0));
    let builder = if render { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    (h, [inside, crossed, corner, outside])
}

fn screen(h: &Harness<'_, CadApp>, p: [f64; 2]) -> Pos2 {
    h.state().canvas.xf.unwrap().to_screen(Vec2::new(p[0], p[1]))
}

/// Press at the first point and move through the others in small steps, keeping the button down.
fn press_and_move(h: &mut Harness<'_, CadApp>, path: &[[f64; 2]]) {
    let start = screen(h, path[0]);
    h.event(Event::PointerMoved(start));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for w in path.windows(2) {
        let (a, b) = (screen(h, w[0]), screen(h, w[1]));
        for i in 1..=8 {
            h.event(Event::PointerMoved(a + (b - a) * (i as f32 / 8.0)));
            h.run_steps(1);
        }
    }
}

fn release(h: &mut Harness<'_, CadApp>, at: [f64; 2]) {
    let pos = screen(h, at);
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn space(h: &mut Harness<'_, CadApp>) {
    h.event(Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.event(Event::Text(" ".into()));
    h.event(Event::Key { key: Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

const RIGHTWARD: &[[f64; 2]] = &[[-2.0, -3.0], [14.0, -3.0], [-2.0, 13.0]];
const LEFTWARD: &[[f64; 2]] = &[[14.0, -3.0], [-2.0, -3.0], [-2.0, 13.0]];

#[test]
fn dragging_right_draws_a_window_lasso() {
    let shots = std::env::var_os("CADCRAFT_LASSO_SHOTS").map(std::path::PathBuf::from);
    let (mut h, [inside, ..]) = harness(shots.is_some());
    press_and_move(&mut h, RIGHTWARD);
    assert!(h.state().session.pending_window.is_none(), "a lasso, not a rectangular window");
    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).unwrap();
        h.render().unwrap().save(dir.join("cadcraft-lasso.png")).unwrap();
    }
    release(&mut h, RIGHTWARD[2]);
    assert_eq!(h.state().session.selection(), vec![inside]);
}

#[test]
fn dragging_left_draws_a_crossing_lasso() {
    let (mut h, [inside, crossed, ..]) = harness(false);
    press_and_move(&mut h, LEFTWARD);
    release(&mut h, LEFTWARD[2]);
    let mut sel = h.state().session.selection();
    sel.sort();
    let mut want = vec![inside, crossed];
    want.sort();
    // Not `corner`: the lasso is the drawn shape, not a rectangle around it.
    assert_eq!(sel, want);
}

#[test]
fn spacebar_mid_drag_makes_a_fence_and_is_not_enter() {
    let (mut h, [inside, crossed, corner, outside]) = harness(false);
    h.state_mut().session.cmdline("erase").unwrap();
    h.run_steps(2);
    press_and_move(&mut h, &RIGHTWARD[..2]);
    // Window, then fence: Spacebar must not end ERASE's "Select objects" either.
    space(&mut h);
    press_and_move_on(&mut h, RIGHTWARD[1], RIGHTWARD[2]);
    assert_eq!(h.state().session.running.as_ref().map(|r| r.id.as_str()), Some("erase"));
    release(&mut h, RIGHTWARD[2]);
    h.state_mut().session.cmdline("").unwrap();
    h.run_steps(2);
    let d = h.state().session.doc().unwrap();
    assert!(d.entity(crossed).is_none(), "the fence crosses it");
    assert!(d.entity(inside).is_some() && d.entity(corner).is_some() && d.entity(outside).is_some());
}

/// Keep dragging from `a` to `b` (the button is already down).
fn press_and_move_on(h: &mut Harness<'_, CadApp>, a: [f64; 2], b: [f64; 2]) {
    let (a, b) = (screen(h, a), screen(h, b));
    for i in 1..=8 {
        h.event(Event::PointerMoved(a + (b - a) * (i as f32 / 8.0)));
        h.run_steps(1);
    }
}
