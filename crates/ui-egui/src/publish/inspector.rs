//! The right-hand side and the footer: the Sheet tab (preview, a page setup changed for the run,
//! details), the Output tab (format, files, folder, names), the run options, and the footer's
//! summary and Publish button.

use egui::{Align, Align2, FontId, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::*;
use crate::theme::Tokens;

/// Papers offered for a one-run page setup change.
pub(super) const PAPERS: &[&str] = &["A4", "A3", "A2", "A1", "A0", "Letter", "Legal", "Tabloid", "ANSI C", "ANSI D", "ANSI E"];

/// A row of equal segments; returns the clicked index. `enabled[i] == false` greys a segment out.
pub(super) fn segmented(ui: &mut Ui, id: &str, labels: &[&str], selected: usize, enabled: &[bool]) -> Option<usize> {
    let t = Tokens::get();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    ui.painter().rect(rect, 7.0, t.chrome_dark, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let n = labels.len().max(1) as f32;
    let inner = rect.shrink(2.0);
    let cw = (inner.width() - 2.0 * (n - 1.0)) / n;
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(inner.left() + i as f32 * (cw + 2.0), inner.top()), vec2(cw, inner.height()));
        let en = enabled.get(i).copied().unwrap_or(true);
        let resp = ui.interact(r, ui.id().with((id, i)), if en { Sense::click() } else { Sense::hover() });
        let on = i == selected;
        crate::icons::describe_control(ui, &resp, label, Some(on), true);
        let p = ui.painter();
        if on {
            p.rect_filled(r, 5.0, t.control);
        } else if resp.hovered() && en {
            p.rect_filled(r, 5.0, t.control_hover.gamma_multiply(0.5));
        }
        let color = if !en {
            t.text_faint.gamma_multiply(0.6)
        } else if on {
            t.text
        } else {
            t.text_dim
        };
        p.text(r.center(), Align2::CENTER_CENTER, crate::i18n::t(label), crate::theme::small(), color);
        if !en {
            resp.on_hover_text(crate::tl!("Not available yet"));
        } else if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

pub(super) fn section(ui: &mut Ui, label: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(crate::i18n::t(label).to_uppercase()).font(FontId::proportional(11.0)).color(Tokens::get().text_faint).strong());
}

pub(super) fn inspector(ui: &mut Ui, session: &cadcraft_engine::Session, groups: &[Group], st: &mut PublishState, act: &mut Actions, height: f32) {
    let t = Tokens::get();
    let tab = if st.tab == Tab::Sheet { 0 } else { 1 };
    if let Some(i) = segmented(ui, "cc_publish_tab", &["Sheet", "Output"], tab, &[true, true]) {
        st.tab = if i == 0 { Tab::Sheet } else { Tab::Output };
    }
    let body_h = (height - 28.0 - 104.0).max(120.0);
    egui::ScrollArea::vertical().id_salt("cc_publish_inspector").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| match st.tab {
        Tab::Sheet => sheet_tab(ui, session, groups, st, act),
        Tab::Output => output_tab(ui, groups, st, act),
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.checkbox(&mut st.stamp_on, crate::tl!("Add a sheet stamp")).on_hover_text(crate::tl!("A line of text in a corner of every sheet"));
        if ui.add_enabled(st.stamp_on, egui::Button::new(crate::tl!("Edit stamp...")).small()).clicked() {
            st.editing_stamp = true;
        }
    });
    let mut background = !st.foreground;
    let threads = !cfg!(target_arch = "wasm32");
    let resp = ui.add_enabled(threads, egui::Checkbox::new(&mut background, crate::tl!("Run in the background")));
    if threads {
        resp.on_hover_text(crate::tl!("Keep working while the sheets are published; a report opens when they're done"));
    } else {
        resp.on_disabled_hover_text(crate::tl!("The web version publishes while you wait."));
    }
    st.foreground = !background;
    ui.add_enabled(act.can_open, egui::Checkbox::new(&mut st.open_result, crate::tl!("Open the result when done")))
        .on_disabled_hover_text(crate::tl!("Not available yet"));
    let _ = t;
}

pub(super) fn sheet_tab(ui: &mut Ui, session: &cadcraft_engine::Session, groups: &[Group], st: &mut PublishState, act: &mut Actions) {
    let t = Tokens::get();
    let inspected = st.inspected.clone();
    let found = inspected
        .as_ref()
        .and_then(|(uid, name)| groups.iter().find(|g| g.uid == *uid).and_then(|g| g.sheets.iter().find(|s| s.name == *name).map(|s| (g, s))));
    let Some((g, s)) = found else {
        ui.add_space(12.0);
        ui.label(RichText::new(crate::tl!("Click a sheet to see it here.")).color(t.text_faint));
        return;
    };
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, 4.0, t.canvas, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let landscape = st.changes.get(&(g.uid, s.name.clone())).map_or(s.landscape || s.model, |c| c.page.landscape);
    let paper = if landscape { vec2(176.0, 124.0) } else { vec2(88.0, 124.0) };
    let sheet = Rect::from_center_size(rect.center(), paper);
    p.rect_filled(sheet.translate(vec2(0.0, 2.0)), 1.0, t.chrome_dark);
    p.rect_filled(sheet, 1.0, t.paper);
    let ink = Stroke::new(1.5, t.canvas_ink.gamma_multiply(0.6));
    let frame = Rect::from_min_size(sheet.min + vec2(sheet.width() * 0.12, sheet.height() * 0.16), vec2(sheet.width() * 0.54, sheet.height() * 0.6));
    p.rect_stroke(frame, 0.0, ink, StrokeKind::Inside);
    if !s.model {
        let tb = Rect::from_min_size(
            pos2(sheet.right() - sheet.width() * 0.3, sheet.bottom() - sheet.height() * 0.22),
            vec2(sheet.width() * 0.24, sheet.height() * 0.14),
        );
        p.rect_stroke(tb, 0.0, Stroke::new(1.0, ink.color), StrokeKind::Inside);
    }
    p.text(rect.right_bottom() - vec2(8.0, 6.0), Align2::RIGHT_BOTTOM, crate::tl!("Preview"), crate::theme::small(), t.text_faint);

    section(ui, "Page setup");
    let key = (g.uid, s.name.clone());
    let mut change = st.changes.get(&key).cloned();
    let saved = crate::tf!("As saved ({setup})", setup = crate::i18n::t(&s.setup));
    let shown = match &change {
        None => saved.clone(),
        Some(SetupChange { from: Some(n), .. }) => crate::tf!("From {layout}", layout = n),
        Some(SetupChange { from: None, page }) => page.paper.clone(),
    };
    // The drawing's other layouts, whose page setups this sheet can borrow for the run.
    let others: Vec<(&str, &cadcraft_doc::PageSetup)> =
        g.sheets.iter().filter(|o| !o.model && o.name != s.name).map(|o| (o.name.as_str(), &o.page)).collect();
    egui::ComboBox::from_id_salt("cc_publish_setup")
        .width(ui.available_width())
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if ui.selectable_label(change.is_none(), &saved).clicked() {
                change = None;
            }
            for (name, page) in &others {
                let on = change.as_ref().is_some_and(|c| c.from.as_deref() == Some(*name));
                if ui.selectable_label(on, crate::tf!("From {layout}", layout = name)).clicked() {
                    let mut page = (*page).clone();
                    if s.model {
                        // Model space fits its extents on the borrowed paper.
                        page.plot_area = "extents".into();
                        page.scale_to_fit = true;
                    }
                    change = Some(SetupChange { from: Some((*name).to_string()), page });
                }
            }
            if !others.is_empty() {
                ui.separator();
            }
            for paper in PAPERS {
                let on = change.as_ref().is_some_and(|c| c.from.is_none() && c.page.paper == *paper);
                if ui.selectable_label(on, *paper).clicked()
                    && let Some(size) = cadcraft_render::paper_size(paper)
                {
                    let mut page = change.as_ref().map_or_else(|| s.page.clone(), |c| c.page.clone());
                    page.paper = (*paper).to_string();
                    page.width_mm = size.width_mm;
                    page.height_mm = size.height_mm;
                    if change.is_none() {
                        page.landscape = true;
                        page.scale_to_fit = s.model || s.scale == "Fit";
                    }
                    change = Some(SetupChange { from: None, page });
                }
            }
        })
        .response
        .on_hover_text(crate::tl!("Change the paper for this run only. The drawing keeps its page setup."));
    // Every setting the run can change, for an open drawing (a file listed without opening it has
    // only the paper choices); editing one turns the sheet into a change for this run.
    let open_index = match &g.source {
        Source::Open(i) => Some(*i),
        Source::File(_) => None,
    };
    if let Some(d) = open_index.and_then(|i| session.docs.get(i)).map(|ds| &ds.doc) {
        let mut page = change.as_ref().map_or_else(|| s.page.clone(), |c| c.page.clone());
        if crate::managers::page_editor(ui, egui::Id::new(("cc_publish_page", g.uid, s.name.as_str())), d, &mut page) {
            change = Some(SetupChange { from: None, page });
        }
    }
    if let Some(c) = &change {
        ui.label(RichText::new(crate::tl!("For this run only. The drawing keeps its page setup.")).font(crate::theme::small()).color(t.text_faint));
        let mut reset = false;
        ui.horizontal(|ui| {
            reset = ui.button(crate::tl!("Reset")).on_hover_text(crate::tl!("Use the sheet's own page setup again")).clicked();
            if let Some(i) = open_index
                && !s.model
                && ui.button(crate::tl!("Save to Layout")).on_hover_text(crate::tl!("Keep this page setup in the layout (can be undone)")).clicked()
            {
                act.save = Some((i, g.uid, s.name.clone(), c.page.clone()));
            }
        });
        if reset {
            change = None;
        }
    }
    match change {
        Some(c) => st.changes.insert(key, c),
        None => st.changes.remove(&key),
    };

    section(ui, "Details");
    egui::Grid::new("cc_publish_details").num_columns(2).spacing([10.0, 5.0]).show(ui, |ui| {
        let kind = if s.model { crate::tl!("Model space") } else { crate::tl!("Layout") };
        let folder = if g.folder.is_empty() { crate::tl!("Not saved yet").to_string() } else { g.folder.clone() };
        let change = st.changes.get(&(g.uid, s.name.clone()));
        let paper = match change {
            Some(c) => format!("{}, {}", c.page.paper, if c.page.landscape { crate::tl!("landscape") } else { crate::tl!("portrait") }),
            None if s.model => crate::tl!("Fit to paper").to_string(),
            None => format!("{}, {}", s.size, if s.landscape { crate::tl!("landscape") } else { crate::tl!("portrait") }),
        };
        let setup = match st.changes.get(&(g.uid, s.name.clone())) {
            Some(c) => crate::tf!("{paper}, for this run", paper = c.page.paper),
            None if s.default_setup => crate::tl!("Drawing default").to_string(),
            None => s.setup.clone(),
        };
        let rows: [(&str, String); 7] = [
            ("Drawing", g.title.clone()),
            ("Folder", folder),
            ("Sheet", format!("{} ({kind})", s.name)),
            ("Device", s.device.clone()),
            ("Paper", paper),
            ("Scale", s.scale.clone()),
            ("Setup", setup),
        ];
        for (k, v) in rows {
            ui.label(RichText::new(crate::i18n::t(k)).color(t.text_faint));
            ui.add(egui::Label::new(v).truncate());
            ui.end_row();
        }
    });
}

pub(super) fn output_tab(ui: &mut Ui, groups: &[Group], st: &mut PublishState, act: &mut Actions) {
    let t = Tokens::get();
    let n = picked(groups, st).len();
    section(ui, "Send to");
    let selected = match st.to {
        To::Pdf => 1,
        To::Svg => 2,
        To::Png => 3,
    };
    // Sending to the page setup's plotter isn't available yet; files are.
    if let Some(i) = segmented(ui, "cc_publish_to", &["Page setup", "PDF", "SVG", "PNG"], selected, &[false, true, true, true]) {
        st.to = match i {
            2 => To::Svg,
            3 => To::Png,
            _ => To::Pdf,
        };
    }
    let about = match st.to {
        To::Pdf => crate::tl!("Vector PDF. Text stays selectable."),
        To::Svg => crate::tl!("Vector SVG, one file per sheet."),
        To::Png => crate::tl!("PNG image, one file per sheet."),
    };
    ui.label(RichText::new(about).font(crate::theme::small()).color(t.text_faint));

    ui.add_space(6.0);
    egui::Grid::new("cc_publish_pdf").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        if st.to == To::Png {
            ui.label(RichText::new(crate::tl!("Quality")).color(t.text_dim));
            ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt("cc_publish_quality").width(200.0).selected_text(crate::tl!("Standard — 300 dpi")).show_ui(ui, |_| {})
            })
            .response
            .on_disabled_hover_text(crate::tl!("Not available yet"));
            ui.end_row();
        }
        ui.label(RichText::new(crate::tl!("Overlaps")).color(t.text_dim));
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("cc_publish_overlap").width(200.0).selected_text(crate::tl!("Last drawn on top")).show_ui(ui, |_| {})
        })
        .response
        .on_disabled_hover_text(crate::tl!("Not available yet"));
        ui.end_row();
    });
    if st.to == To::Pdf {
        let mut layers = !st.drop_layers;
        if ui.checkbox(&mut layers, crate::tl!("Keep layers (viewers can switch them on and off)")).changed() {
            st.drop_layers = !layers;
        }
    }

    section(ui, "Files");
    let pdf = st.to == To::Pdf;
    if let Some(i) = segmented(ui, "cc_publish_files", &["One per sheet", "Combine into one"], usize::from(combining(st)), &[true, pdf]) {
        st.combine = i == 1;
    }
    if !pdf {
        ui.label(RichText::new(crate::tl!("Only PDFs can be combined.")).font(crate::theme::small()).color(t.text_faint));
    }

    section(ui, "Save to");
    let mut folder = default_folder(groups, st);
    ui.horizontal(|ui| {
        let w = ui.available_width() - 80.0;
        if ui
            .add(egui::TextEdit::singleline(&mut folder).font(crate::theme::mono()).desired_width(w).hint_text(crate::tl!("Choose a folder")))
            .changed()
        {
            st.folder = Some(folder.clone());
        }
        if ui.button(crate::tl!("Choose...")).clicked() {
            act.choose = true;
        }
    });

    section(ui, "Names");
    ui.checkbox(&mut st.ask_name, crate::tl!("Ask for a name when publishing"));
    // The engine's names (from the last check), so the list shows exactly the files a run writes.
    let files: Vec<String> = match &st.checked {
        _ if n == 0 => Vec::new(),
        Some(c) if combining(st) => {
            let pages = n.saturating_sub(will_fail(groups, st));
            c.files.iter().map(|f| crate::tf!("{name} ({n} pages)", name = f, n = pages)).collect()
        }
        Some(c) => c.files.clone(),
        None => Vec::new(),
    };
    egui::Frame::NONE.fill(t.list_bg).stroke(Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 6)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("cc_publish_names").max_height(96.0).show(ui, |ui| {
                if n == 0 {
                    ui.label(RichText::new(crate::tl!("Select at least one sheet.")).color(t.text_faint));
                } else if files.is_empty() {
                    ui.label(RichText::new(crate::tl!("Checking...")).color(t.text_faint));
                }
                for f in files {
                    ui.add(egui::Label::new(RichText::new(f).font(crate::theme::mono()).color(t.text_dim)).truncate());
                }
            });
        },
    );
}

pub(super) fn footer_row(ui: &mut Ui, groups: &[Group], st: &PublishState, act: &mut Actions) {
    let t = Tokens::get();
    let n = picked(groups, st).len();
    let mut summary = if n == 0 {
        crate::tl!("Nothing selected").to_string()
    } else if combining(st) {
        // Sheets that will fail aren't pages.
        crate::tf!("1 PDF with {n} pages", n = n.saturating_sub(will_fail(groups, st)))
    } else {
        crate::i18n::count(n as u64, "{n} file", "{n} files")
    };
    let failing = will_fail(groups, st);
    ui.horizontal(|ui| {
        if ui.button(" ? ").on_hover_text(crate::tl!("Help")).clicked() {
            act.help = true;
        }
        if failing > 0 {
            summary = format!("{summary} · {}", crate::tf!("{n} will fail", n = failing));
        }
        ui.label(RichText::new(summary).color(if failing > 0 { t.warn } else { t.text_dim }));
        if let Some(r) = &st.report {
            ui.add(egui::Label::new(RichText::new(r).font(crate::theme::small()).color(t.text_faint)).truncate());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label =
                if n == 0 { crate::tl!("Publish").to_string() } else { crate::i18n::count(n as u64, "Publish {n} Sheet", "Publish {n} Sheets") };
            let button = egui::Button::new(RichText::new(label).color(if n == 0 { t.text_faint } else { t.paper }).strong())
                .fill(if n == 0 { t.control } else { t.accent })
                .min_size(vec2(120.0, 26.0));
            let resp = ui.add_enabled(n > 0 && !act.busy, button);
            if act.busy {
                resp.on_disabled_hover_text(crate::tl!("Publishing is already running."));
            } else if resp.clicked() {
                act.publish = true;
            }
            if ui.add(egui::Button::new(crate::tl!("Cancel")).min_size(vec2(80.0, 26.0))).clicked() {
                act.close = true;
            }
        });
    });
}
