//! Dragging an existing selection uses MOVE, with a preview and a single undo step.
use cadcraft_engine::{Session, doc::EntityKind, geom::Vec2};
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("line", json!({"points":[[0,0],[100,0]]})).unwrap();
    app.run("view.set", json!({"center":[50,20],"height":180})).unwrap();
    app.session.settings.osmode = 0;
    app.session.settings.snapmode = false;
    app.session.settings.orthomode = false;
    app.canvas.zoom_pending = false;
    let builder = Harness::builder().with_size(vec2(1280.0, 800.0));
    let builder = if std::env::var_os("CADCRAFT_DRAG_SHOTS").is_some() { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    let handles = h.state().session.doc().unwrap().model.iter().map(|e| e.handle).collect();
    h.state_mut().session.set_selection(handles);
    h.run_steps(2);
    h
}

fn at(h: &Harness<'_, CadApp>, x: f64, y: f64) -> Pos2 {
    h.state().canvas.xf.unwrap().to_screen(Vec2::new(x, y))
}

fn button(h: &mut Harness<'_, CadApp>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(2);
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn move_to(h: &mut Harness<'_, CadApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(2);
}

fn line_start(h: &Harness<'_, CadApp>) -> Vec2 {
    match &h.state().session.doc().unwrap().model.iter().next().unwrap().kind {
        EntityKind::Line(l) => l.a.xy(),
        _ => panic!("expected line"),
    }
}

#[test]
fn drag_previews_then_moves_and_undo_redo_are_single_steps() {
    let mut h = harness();
    let before = h.state().session.doc().unwrap().clone();
    let selection = h.state().session.selection();
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    assert_eq!(h.state().session.running.as_ref().unwrap().id, "move");
    assert_eq!(h.state().session.doc().unwrap(), &before, "preview does not edit geometry");
    assert!(h.state().session.pending_window.is_none());
    if let Some(path) = std::env::var_os("CADCRAFT_DRAG_SHOTS") {
        h.render().unwrap().save(std::path::PathBuf::from(path).join("drag-preview.png")).unwrap();
    }
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert!((line_start(&h) - Vec2::new(20.0, 10.0)).len() < 1e-4);
    assert_eq!(h.state().session.selection(), selection);
    if let Some(path) = std::env::var_os("CADCRAFT_DRAG_SHOTS") {
        h.render().unwrap().save(std::path::PathBuf::from(path).join("drag-result.png")).unwrap();
    }
    h.state_mut().session.undo().unwrap();
    assert_eq!(h.state().session.doc().unwrap(), &before);
    h.state_mut().session.redo().unwrap();
    assert!((line_start(&h) - Vec2::new(20.0, 10.0)).len() < 1e-4);
}

#[test]
fn click_without_drag_does_not_move_or_start_command() {
    let mut h = harness();
    let before = h.state().session.doc().unwrap().clone();
    let from = at(&h, 25.0, 0.0);
    button(&mut h, from, true);
    button(&mut h, from, false);
    assert!(h.state().session.running.is_none());
    assert_eq!(h.state().session.doc().unwrap(), &before);
}

#[test]
fn escape_cancels_and_release_does_not_commit_or_open_window() {
    let mut h = harness();
    let before = h.state().session.doc().unwrap().clone();
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    move_to(&mut h, to + vec2(30.0, 30.0));
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert!(h.state().session.pending_window.is_none());
    assert_eq!(h.state().session.doc().unwrap(), &before);
}

#[test]
fn empty_space_drag_keeps_window_selection() {
    let mut h = harness();
    h.state_mut().session.set_selection(Vec::new());
    let before = h.state().session.doc().unwrap().clone();
    let from = at(&h, -10.0, 20.0);
    let to = at(&h, 110.0, -20.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert_eq!(h.state().session.selection().len(), 1);
    assert!(h.state().session.running.is_none());
    assert_eq!(h.state().session.doc().unwrap(), &before);
}

#[test]
fn dragging_with_pickadd_off_preserves_and_moves_whole_selection() {
    let mut h = harness();
    h.state_mut().run("circle", json!({"center":[60,40],"radius":10})).unwrap();
    let handles = h.state().session.doc().unwrap().model.iter().map(|e| e.handle).collect();
    h.state_mut().session.set_selection(handles);
    h.state_mut().session.settings.pickadd = false;
    h.run_steps(2);
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert_eq!(h.state().session.selection().len(), 2);
    for e in h.state().session.doc().unwrap().model.iter() {
        if let EntityKind::Circle(c) = &e.kind {
            assert!((c.center.xy() - Vec2::new(80.0, 50.0)).len() < 1e-4);
        }
    }
}

#[test]
fn grips_keep_their_editing_command() {
    let mut h = harness();
    let from = at(&h, 0.0, 0.0);
    let to = at(&h, 20.0, 10.0);
    button(&mut h, from, true);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert!(h.state().session.running.is_some(), "grip click: {:?}", h.state().session.log);
    move_to(&mut h, to);
    assert_eq!(h.state().session.running.as_ref().unwrap().id, "grip");
    assert_eq!(line_start(&h), Vec2::ZERO);
}

#[test]
fn locked_layer_geometry_does_not_move() {
    let mut h = harness();
    h.state_mut().run("layer.set", json!({"name":"0","locked":true})).unwrap();
    let before = h.state().session.doc().unwrap().clone();
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert_eq!(h.state().session.doc().unwrap(), &before);
}

#[test]
fn drag_uses_ortho_relative_to_press_origin() {
    let mut h = harness();
    h.state_mut().session.settings.orthomode = true;
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert!((line_start(&h) - Vec2::new(20.0, 0.0)).len() < 1e-4);
}

#[test]
fn selection_drag_also_works_with_pickfirst_off() {
    let mut h = harness();
    h.state_mut().session.settings.pickfirst = false;
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert!((line_start(&h) - Vec2::new(20.0, 10.0)).len() < 1e-4);
}

#[test]
fn losing_window_focus_cancels_drag() {
    let mut h = harness();
    let before = h.state().session.doc().unwrap().clone();
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 45.0, 10.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    h.event(Event::WindowFocused(false));
    h.input_mut().focused = false;
    h.run_steps(2);
    h.event(Event::WindowFocused(true));
    h.input_mut().focused = true;
    h.run_steps(2);
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert_eq!(h.state().session.doc().unwrap(), &before);
}

#[test]
fn release_outside_canvas_finishes_captured_drag() {
    let mut h = harness();
    let xf = h.state().canvas.xf.unwrap();
    let from = at(&h, 25.0, 0.0);
    let to = Pos2::new(xf.rect.right() + 40.0, from.y - 30.0);
    let delta = xf.to_world(to) - xf.to_world(from);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert!(h.state().session.running.is_none());
    assert!((line_start(&h) - delta).len() < 1e-4);
}

#[test]
fn drag_destination_uses_endpoint_snap() {
    let mut h = harness();
    let selection = h.state().session.selection();
    h.state_mut().run("line", json!({"points":[[80,30],[100,30]]})).unwrap();
    h.state_mut().session.set_selection(selection);
    h.state_mut().session.settings.osmode = cadcraft_engine::snap::mode::END;
    h.run_steps(2);
    let from = at(&h, 25.0, 0.0);
    let to = at(&h, 80.0, 30.0) + vec2(2.0, 2.0);
    button(&mut h, from, true);
    move_to(&mut h, to);
    button(&mut h, to, false);
    assert!((line_start(&h) - Vec2::new(55.0, 30.0)).len() < 1e-4);
}
