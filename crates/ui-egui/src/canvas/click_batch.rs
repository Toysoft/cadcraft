//! Recover complete click batches, leaving cross-frame gestures and drags on their existing path.
use super::{CadApp, Input, Xf, click_grip, grip_at};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect};

pub(super) fn input(app: &mut CadApp, xf: &Xf, ui: &egui::Ui) -> bool {
    if !selecting(app) {
        return false;
    }
    let max_distance = ui.ctx().options(|o| o.input_options.max_click_dist);
    let Some(clicks) = ui.input(|i| complete_clicks(&i.events, xf.rect, max_distance)) else { return false };
    for (pos, modifiers) in clicks {
        // A grip or command may have changed the prompt; never replay remaining selection
        // clicks as drawing points or grip movements in that new interaction.
        if !selecting(app) {
            break;
        }
        let point = xf.to_world(pos);
        let result = if app.session.prompting() {
            app.session.input(Input::Point(point))
        } else if app.session.pending_window.is_some() {
            app.session.idle_click(point, modifiers.shift)
        } else if let Some(grip) = crate::dyngrips::at(app, xf, Some(pos)) {
            crate::dyngrips::click(app, grip);
            Ok(())
        } else if let Some(grip) = grip_at(app, xf, Some(pos)) {
            click_grip(app, grip, modifiers.shift);
            Ok(())
        } else {
            app.session.idle_click(point, modifiers.shift)
        };
        if let Err(error) = result {
            app.session.echo(error.to_string());
            break;
        }
    }
    true
}

fn selecting(app: &CadApp) -> bool {
    !app.session.prompting() || app.session.current_prompt().is_some_and(|p| p.accept.select)
}

/// Accept only two or more complete primary clicks wholly inside this canvas in this frame.
/// Any unmatched press/release, other button, cancellation or travel beyond egui's existing
/// click distance falls back to the existing gesture handler. Do not introduce a new threshold.
fn complete_clicks(events: &[Event], rect: Rect, max_distance: f32) -> Option<Vec<(Pos2, Modifiers)>> {
    let mut origin: Option<Pos2> = None;
    let mut clicks = Vec::new();
    for event in events {
        match *event {
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => {
                if origin.is_some() || !rect.contains(pos) {
                    return None;
                }
                origin = Some(pos);
            }
            Event::PointerMoved(pos) => {
                if let Some(start) = origin
                    && (!pos.is_finite() || start.distance(pos) > max_distance)
                {
                    return None;
                }
            }
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers } => {
                let start = origin.take()?;
                if !rect.contains(pos) || start.distance(pos) > max_distance {
                    return None;
                }
                clicks.push((pos, modifiers));
            }
            Event::PointerButton { .. }
            | Event::PointerGone
            | Event::WindowFocused(false)
            | Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                return None;
            }
            _ => {}
        }
    }
    (origin.is_none() && clicks.len() > 1).then_some(clicks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    #[test]
    fn batches_require_complete_canvas_owned_clicks() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let (a, b) = (pos2(20.0, 20.0), pos2(80.0, 80.0));
        let batch = vec![button(a, true), button(a, false), button(b, true), button(b, false)];
        assert_eq!(complete_clicks(&batch, rect, 6.0).unwrap().len(), 2);
        for events in [
            vec![button(a, true), button(a, false)],
            vec![button(a, false), button(b, true), button(b, false)],
            vec![button(a, true), button(a, false), button(b, true)],
            vec![button(pos2(-1.0, 20.0), true), button(a, false), button(b, true), button(b, false)],
        ] {
            assert!(complete_clicks(&events, rect, 6.0).is_none());
        }
        for cancel in [Event::PointerGone, Event::WindowFocused(false)] {
            let mut events = batch.clone();
            events.insert(1, cancel);
            assert!(complete_clicks(&events, rect, 6.0).is_none());
        }
    }

    #[test]
    fn a_drag_that_returns_to_its_origin_is_not_replayed_as_a_click() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let (a, b) = (pos2(20.0, 20.0), pos2(80.0, 80.0));
        let events = [button(a, true), Event::PointerMoved(a + vec2(7.0, 0.0)), button(a, false), button(b, true), button(b, false)];
        assert!(complete_clicks(&events, rect, 6.0).is_none());
        assert_eq!(complete_clicks(&events, rect, 8.0).unwrap().len(), 2, "use egui's configured click distance");
    }
}
