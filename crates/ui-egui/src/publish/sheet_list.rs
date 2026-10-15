//! The sheet list: a header, a row per drawing with a tri-state tick, a row per sheet with its
//! page setup, paper and status, and the controls above and below it.

use egui::{Align, Align2, Color32, FontId, Layout, Painter, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::*;
use crate::theme::Tokens;

pub(super) const ROW: f32 = 22.0;
pub(super) const CHECK: f32 = 13.0;
pub(super) const INDENT: f32 = 29.0;
pub(super) const PAPER_COL: f32 = 96.0;
pub(super) const STATUS_COL: f32 = 110.0;
pub(super) const GAP: f32 = 8.0;

pub(super) fn list_column(ui: &mut Ui, groups: &[Group], st: &mut PublishState, act: &mut Actions, height: f32) {
    let t = Tokens::get();
    let total: usize = groups.iter().map(|g| g.sheets.len()).sum();
    let n = picked(groups, st).len();
    ui.horizontal(|ui| {
        let list_name = st.list.as_deref().and_then(|l| std::path::Path::new(l).file_name()).map(|n| n.to_string_lossy().to_string());
        let title = list_name.unwrap_or_else(|| crate::tl!("Unsaved list").to_string());
        ui.menu_button(format!("{title} ⏷"), |ui| {
            if ui.button(crate::tl!("New List")).on_hover_text(crate::tl!("Every open drawing's sheets")).clicked() {
                *st = PublishState { folder: st.folder.clone(), tab: st.tab, ..PublishState::default() };
                ui.close();
            }
            if ui.button(crate::tl!("Open List...")).clicked() {
                act.open_list = true;
                ui.close();
            }
            if ui.button(crate::tl!("Save List As...")).clicked() {
                act.save_list = true;
                ui.close();
            }
        })
        .response
        .on_hover_text(st.list.clone().unwrap_or_default());
        ui.label(RichText::new(crate::tf!("{n} of {total} sheets", n = n, total = total)).color(t.text_faint));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let inspected = st.inspected.clone();
            if ui.add_enabled(inspected.is_some(), egui::Button::new("↓")).on_hover_text(crate::tl!("Move down")).clicked() {
                move_sheet(groups, st, 1);
            }
            if ui.add_enabled(inspected.is_some(), egui::Button::new("↑")).on_hover_text(crate::tl!("Move up")).clicked() {
                move_sheet(groups, st, -1);
            }
            if ui.button(crate::tl!("Add Drawings...")).on_hover_text(crate::tl!("Add a drawing's sheets without opening it")).clicked() {
                act.add = true;
            }
            ui.separator();
            if ui.small_button(crate::tl!("None")).clicked() {
                for g in groups {
                    for s in &g.sheets {
                        st.skipped.insert((g.uid, s.name.clone()));
                    }
                }
            }
            if ui.small_button(crate::tl!("All")).clicked() {
                st.skipped.clear();
            }
        });
    });
    ui.add_space(4.0);
    let table_h = (height - 70.0).max(120.0);
    let frame = egui::Frame::NONE.fill(t.list_bg).stroke(Stroke::new(1.0, t.border)).corner_radius(4.0);
    frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), table_h));
        ui.spacing_mut().item_spacing.y = 0.0;
        header_row(ui);
        egui::ScrollArea::vertical().id_salt("cc_publish_list").max_height(table_h - ROW).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if groups.is_empty() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(crate::tl!("No drawings are open. Use Add Drawings... to open one.")).color(t.text_faint))
                });
            }
            for g in groups {
                group_row(ui, g, st);
                for s in &g.sheets {
                    sheet_row(ui, g, s, st);
                }
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let mut every = !st.only_current;
        if ui.checkbox(&mut every, crate::tl!("Include every open drawing")).changed() {
            st.only_current = !every;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(crate::tl!("Tick to publish · click a row to inspect it")).color(t.text_faint).font(crate::theme::small()));
        });
    });
}

pub(super) fn move_sheet(groups: &[Group], st: &mut PublishState, dir: isize) {
    let Some((uid, name)) = st.inspected.clone() else { return };
    let Some(g) = groups.iter().find(|g| g.uid == uid) else { return };
    let mut names: Vec<String> = g.sheets.iter().map(|s| s.name.clone()).collect();
    let Some(i) = names.iter().position(|n| *n == name) else { return };
    let Some(j) = i.checked_add_signed(dir).filter(|j| *j < names.len()) else { return };
    names.swap(i, j);
    st.order.insert(uid, names);
}

/// Column rectangles for a row: (name, setup, paper, status).
pub(super) fn columns(row: Rect) -> [Rect; 4] {
    let x0 = row.left() + INDENT + CHECK + GAP;
    let right = row.right() - GAP;
    let flex = (right - x0 - PAPER_COL - STATUS_COL - 3.0 * GAP).max(80.0);
    let name_w = flex * 1.3 / 2.3;
    let setup_w = flex - name_w;
    let mut x = x0;
    let mut out = [Rect::NOTHING; 4];
    for (i, w) in [name_w, setup_w, PAPER_COL, STATUS_COL].into_iter().enumerate() {
        if let Some(r) = out.get_mut(i) {
            *r = Rect::from_min_max(pos2(x, row.top()), pos2(x + w, row.bottom()));
        }
        x += w + GAP;
    }
    out
}

pub(super) fn cell_text(p: &Painter, r: Rect, text: &str, color: Color32, font: FontId) {
    p.with_clip_rect(r).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, font, color);
}

pub(super) fn header_row(ui: &mut Ui) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let p = ui.painter();
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border));
    let font = crate::theme::small();
    let [a, b, c, d] = columns(rect);
    let a = Rect::from_min_max(pos2(rect.left() + GAP + CHECK + GAP, a.top()), a.max);
    for (r, label) in [(a, "Sheet"), (b, "Page setup"), (c, "Paper"), (d, "Status")] {
        cell_text(p, r, crate::i18n::t(label), t.text_faint, font.clone());
    }
}

pub(super) fn paint_check(p: &Painter, r: Rect, state: Option<bool>) {
    let t = Tokens::get();
    let on = state != Some(false);
    p.rect(r, 2.0, if on { t.accent } else { t.field }, Stroke::new(1.0, if on { t.accent } else { t.text_faint }), StrokeKind::Inside);
    let w = Stroke::new(1.6, t.paper);
    match state {
        Some(true) => {
            let (x, y, s) = (r.left(), r.top(), r.width());
            p.line_segment([pos2(x + s * 0.22, y + s * 0.52), pos2(x + s * 0.42, y + s * 0.72)], w);
            p.line_segment([pos2(x + s * 0.42, y + s * 0.72), pos2(x + s * 0.78, y + s * 0.3)], w);
        }
        None => {
            p.line_segment([pos2(r.left() + 3.0, r.center().y), pos2(r.right() - 3.0, r.center().y)], w);
        }
        Some(false) => {}
    }
}

pub(super) fn group_row(ui: &mut Ui, g: &Group, st: &mut PublishState) {
    let t = Tokens::get();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let on = g.sheets.iter().filter(|s| !st.skipped.contains(&(g.uid, s.name.clone()))).count();
    let state = if on == 0 {
        Some(false)
    } else if on == g.sheets.len() {
        Some(true)
    } else {
        None
    };
    let check = Rect::from_center_size(pos2(rect.left() + GAP + CHECK / 2.0, rect.center().y), vec2(CHECK, CHECK));
    let remove = Rect::from_center_size(pos2(rect.right() - GAP - 7.0, rect.center().y), vec2(18.0, 18.0));
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        if remove.expand(2.0).contains(at) {
            match &g.source {
                Source::Open(_) => {
                    st.removed.insert(g.uid);
                }
                Source::File(_) => st.files.retain(|f| f.uid != g.uid),
            }
        } else {
            let all_on = state == Some(true);
            for s in &g.sheets {
                let key = (g.uid, s.name.clone());
                if all_on {
                    st.skipped.insert(key);
                } else {
                    st.skipped.remove(&key);
                }
            }
        }
    }
    crate::icons::describe_control(ui, &resp, &g.title, state, false);
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.chrome_dark);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border));
    paint_check(p, check, state);
    let font = crate::theme::body();
    let title = p.text(pos2(check.right() + GAP, rect.center().y), Align2::LEFT_CENTER, &g.title, FontId::proportional(font.size), t.text);
    let (note, color) = match &g.source {
        Source::Open(_) if g.current => (crate::tl!("current"), t.text_faint),
        Source::Open(_) => (crate::tl!("open"), t.text_faint),
        Source::File(_) if g.found => (crate::tl!("not open"), t.text_faint),
        Source::File(_) => (crate::tl!("File not found"), t.warn),
    };
    p.text(pos2(title.right() + GAP, rect.center().y), Align2::LEFT_CENTER, note, crate::theme::small(), color);
    ui.ctx().accesskit_node_builder(resp.id, |node| node.set_value(note.to_string()));
    let count = crate::tf!("{n} of {total}", n = on, total = g.sheets.len());
    p.text(pos2(remove.left() - GAP, rect.center().y), Align2::RIGHT_CENTER, count, crate::theme::small(), t.text_dim);
    let hot = resp.hovered() && resp.hover_pos().is_some_and(|h| remove.expand(2.0).contains(h));
    let c = if hot { t.text } else { t.text_faint };
    let k = remove.shrink(5.0);
    p.line_segment([k.left_top(), k.right_bottom()], Stroke::new(1.3, c));
    p.line_segment([k.right_top(), k.left_bottom()], Stroke::new(1.3, c));
    if hot {
        resp.on_hover_text(crate::tl!("Remove from the list"));
    }
}

pub(super) fn sheet_row(ui: &mut Ui, g: &Group, s: &Sheet, st: &mut PublishState) {
    let t = Tokens::get();
    let key = (g.uid, s.name.clone());
    let on = !st.skipped.contains(&key);
    let inspected = st.inspected.as_ref() == Some(&key);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let check = Rect::from_center_size(pos2(rect.left() + INDENT + CHECK / 2.0, rect.center().y), vec2(CHECK, CHECK));
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        if check.expand(3.0).contains(at) {
            if on {
                st.skipped.insert(key.clone());
            } else {
                st.skipped.remove(&key);
            }
        } else {
            st.inspected = Some(key.clone());
        }
    }
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Space)) {
        if on {
            st.skipped.insert(key.clone());
        } else {
            st.skipped.remove(&key);
        }
    }
    crate::icons::describe_control(ui, &resp, &s.name, Some(on), false);
    // The status is painted, so screen readers get it as the row's value and the reason as its
    // description.
    let (label, color, reason) = sheet_status(g, s, st);
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_value(label.clone());
        if !reason.is_empty() {
            node.set_description(reason.clone());
        }
    });
    let p = ui.painter();
    if inspected {
        p.rect(rect.shrink2(vec2(1.0, 0.5)), 0.0, t.tab_active, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, t.list_row);
    }
    paint_check(p, check, Some(on));
    let fg = if on { t.text } else { t.text_faint };
    let font = crate::theme::body();
    let [name, setup, paper, status] = columns(rect);
    let glyph = Rect::from_center_size(pos2(name.left() + 6.0, rect.center().y), vec2(11.0, 11.0));
    let gs = Stroke::new(1.2, t.text_faint);
    p.rect_stroke(glyph, 1.0, gs, StrokeKind::Inside);
    if s.model {
        p.line_segment([pos2(glyph.left() + 1.5, glyph.bottom() - 3.0), pos2(glyph.center().x, glyph.center().y - 1.0)], gs);
        p.line_segment([pos2(glyph.center().x, glyph.center().y - 1.0), pos2(glyph.right() - 1.5, glyph.bottom() - 4.0)], gs);
    } else {
        let tb = Rect::from_min_max(pos2(glyph.center().x + 0.5, glyph.bottom() - 4.0), pos2(glyph.right() - 1.5, glyph.bottom() - 1.5));
        p.rect_stroke(tb, 0.0, gs, StrokeKind::Inside);
    }
    let name_text = Rect::from_min_max(pos2(glyph.right() + 6.0, name.top()), name.max);
    cell_text(p, name_text, &s.name, fg, font.clone());
    cell_text(p, setup, crate::i18n::t(&s.setup), if on { t.text_dim } else { t.text_faint }, font.clone());
    cell_text(p, paper, &paper_text(s), if on { t.text_dim } else { t.text_faint }, font.clone());
    cell_text(p, status, &label, if on { color } else { t.text_faint }, font);
    if !reason.is_empty() && resp.hovered() {
        resp.on_hover_text(reason);
    }
}

/// The Status column: the last check's answer, or "Checking..." until it comes.
pub(super) fn sheet_status(g: &Group, s: &Sheet, st: &PublishState) -> (String, Color32, String) {
    let t = Tokens::get();
    let key = (g.uid, s.name.clone());
    if !g.found {
        let path = match &g.source {
            Source::File(p) => p.clone(),
            Source::Open(_) => g.title.clone(),
        };
        return (crate::tl!("File not found").to_string(), t.warn, crate::tf!("{path} isn't there any more", path = path));
    }
    let Some((status, reason)) = st.checked.as_ref().and_then(|c| c.sheets.get(&key)) else {
        return (crate::tl!("Checking...").to_string(), t.text_faint, String::new());
    };
    let changed = st.changes.contains_key(&key);
    match status.as_str() {
        "error" => (crate::tl!("Will fail").to_string(), t.warn, reason.clone()),
        "default" => (crate::tl!("Uses default").to_string(), t.text_faint, reason.clone()),
        _ if changed => (crate::tl!("Changed for this run").to_string(), t.text_dim, reason.clone()),
        _ => (crate::tl!("Ready").to_string(), t.text_dim, reason.clone()),
    }
}

pub(super) fn paper_text(s: &Sheet) -> String {
    if s.model {
        return crate::tl!("Fit to paper").to_string();
    }
    if s.size.is_empty() {
        return s.scale.clone();
    }
    format!("{} · {}", s.size.trim_end_matches(" mm").replace(" × ", "×"), s.scale)
}
