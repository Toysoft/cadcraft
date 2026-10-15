//! A slow frame must not collapse two complete selection clicks into the last corner.
use cadcraft_engine::{Input, Session};
use cadcraft_geom::Vec2;
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness(prompting: bool, scale: f32) -> Harness<'static, CadApp> {
    let mut session = Session::new();
    session.new_drawing(false);
    session.execute("line", &json!({"points":[[40,40],[60,60]]})).unwrap();
    session.execute("view.set", &json!({"center":[50,50],"height":100})).unwrap();
    session.settings.osmode = 0;
    session.settings.snapmode = false;
    let mut app = CadApp::new(session, Services::default());
    app.canvas.zoom_pending = false;
    if prompting {
        app.start("select");
    }
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(scale).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn button(h: &mut Harness<'_, CadApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
}

fn corners(h: &mut Harness<'_, CadApp>, crossing: bool, modifiers: Modifiers) {
    let points = if crossing { [Vec2::new(70.0, 70.0), Vec2::new(30.0, 30.0)] } else { [Vec2::new(30.0, 30.0), Vec2::new(70.0, 70.0)] };
    for world in points {
        let pos = h.state().canvas.xf.unwrap().to_screen(world);
        button(h, pos, true, modifiers);
        button(h, pos, false, modifiers);
    }
}

#[test]
fn batched_window_and_crossing_keep_both_corners_idle_and_during_select() {
    for prompting in [false, true] {
        for crossing in [false, true] {
            for scale in [1.0, 2.0] {
                let mut h = harness(prompting, scale);
                corners(&mut h, crossing, Modifiers::NONE);
                h.run_steps(1);
                assert!(h.state().session.pending_window.is_none(), "both corners must be consumed in order");
                if prompting {
                    h.state_mut().session.input(Input::Enter).unwrap();
                }
                assert_eq!(h.state().session.selection().len(), 1);
                assert_eq!(h.state().session.doc().unwrap().model.len(), 1);
                if !prompting
                    && !crossing
                    && scale == 1.0
                    && let Ok(directory) = std::env::var("CRAFT_BATCH_VISUAL_DIR")
                {
                    std::fs::create_dir_all(&directory).unwrap();
                    h.render().unwrap().save(format!("{directory}/batched-selection.png")).unwrap();
                }
            }
        }
    }
}

#[test]
fn batched_selection_uses_each_release_modifier_and_position() {
    let mut h = harness(false, 1.0);
    corners(&mut h, false, Modifiers::NONE);
    // A second pair removes the same entity, even though the frame's final modifier is false.
    for world in [Vec2::new(80.0, 80.0), Vec2::new(20.0, 20.0)] {
        let pos = h.state().canvas.xf.unwrap().to_screen(world);
        button(&mut h, pos, true, Modifiers { shift: true, ..Modifiers::NONE });
        button(&mut h, pos, false, Modifiers { shift: true, ..Modifiers::NONE });
    }
    let final_hover = h.state().canvas.xf.unwrap().to_screen(Vec2::new(15.0, 15.0));
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.input_mut().events.push(Event::PointerMoved(final_hover));
    h.run_steps(1);
    assert!(h.state().session.pending_window.is_none());
    assert!(h.state().session.selection().is_empty());
}

#[test]
fn clicks_consumed_by_a_finishing_drawing_prompt_are_not_replayed_as_selection() {
    let mut h = harness(false, 1.0);
    h.state_mut().start("circle");
    h.state_mut().session.input(Input::Point(Vec2::new(50.0, 50.0))).unwrap();
    h.run_steps(2);
    for world in [Vec2::new(60.0, 50.0), Vec2::new(80.0, 80.0)] {
        let pos = h.state().canvas.xf.unwrap().to_screen(world);
        button(&mut h, pos, true, Modifiers::NONE);
        button(&mut h, pos, false, Modifiers::NONE);
    }
    h.run_steps(1);
    assert_eq!(h.state().session.doc().unwrap().model.len(), 2);
    assert!(h.state().session.selection().is_empty(), "the radius click must not also select the circle it created");
    assert!(h.state().session.pending_window.unwrap().corner.near(Vec2::new(80.0, 80.0), 1e-4));
}
