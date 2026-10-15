//! The sheet stamp's settings and the Sheet Stamp window: the text (with `{drawing}`, `{sheet}`,
//! `{n}`, `{date}` and `{time}`), the corner and the height on paper, with a sample of how it reads.

use egui::{Align, Align2, Layout, RichText};
use serde_json::{Value, json};

use super::*;
use crate::theme::Tokens;

/// Text in a corner of every sheet, with `{drawing}`, `{sheet}`, `{n}`, `{date}` and `{time}`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StampSettings {
    pub(super) text: String,
    pub(super) corner: usize,
    pub(super) height: f64,
}

/// The stamp's corners as the engine names them, and as the window shows them.
pub(super) const CORNERS: [(&str, &str); 4] =
    [("bottom-right", "Bottom right"), ("bottom-left", "Bottom left"), ("top-right", "Top right"), ("top-left", "Top left")];

impl Default for StampSettings {
    fn default() -> Self {
        StampSettings { text: "{drawing} · {sheet} · {date} {time}".into(), corner: 0, height: 2.5 }
    }
}

impl StampSettings {
    pub(super) fn json(&self) -> Value {
        json!({ "text": self.text, "corner": CORNERS.get(self.corner).map_or("bottom-right", |c| c.0), "height": self.height })
    }
}

/// The Sheet Stamp window: the stamp's text, corner and height, with a sample of how it reads.
pub(super) fn stamp_window(ctx: &egui::Context, st: &mut PublishState) {
    let t = Tokens::get();
    let mut open = true;
    let mut done = false;
    egui::Window::new(crate::tl!("Sheet Stamp"))
        .id(egui::Id::new("cc_publish_stamp"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.label(RichText::new(crate::tl!("Text")).color(t.text_dim));
            ui.add(egui::TextEdit::singleline(&mut st.stamp.text).desired_width(f32::INFINITY).char_limit(200));
            ui.label(
                RichText::new(crate::tl!("Use {drawing}, {sheet}, {n}, {date} and {time}. The date and time are UTC."))
                    .font(crate::theme::small())
                    .color(t.text_faint),
            );
            ui.add_space(6.0);
            egui::Grid::new("cc_publish_stamp_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new(crate::tl!("Corner")).color(t.text_dim));
                let shown = CORNERS.get(st.stamp.corner).map_or("", |c| c.1);
                egui::ComboBox::from_id_salt("cc_publish_stamp_corner").selected_text(crate::i18n::t(shown)).show_ui(ui, |ui| {
                    for (i, (_, label)) in CORNERS.iter().enumerate() {
                        ui.selectable_value(&mut st.stamp.corner, i, crate::i18n::t(label));
                    }
                });
                ui.end_row();
                ui.label(RichText::new(crate::tl!("Height on paper")).color(t.text_dim));
                ui.add(egui::DragValue::new(&mut st.stamp.height).range(0.5..=50.0).speed(0.1).suffix(" mm"));
                ui.end_row();
            });
            ui.add_space(6.0);
            // The engine's own reading of the text, so the example and the error are what a run gives.
            let sample = cadcraft_engine::cmd::fill_tokens(
                "the stamp",
                &st.stamp.text,
                &[("drawing", "Drawing1"), ("sheet", "Layout1"), ("n", "1"), ("date", "2026-10-15"), ("time", "09:30 UTC")],
            );
            ui.label(RichText::new(crate::tl!("Example")).color(t.text_dim));
            match sample {
                Ok(text) => ui.label(RichText::new(text).font(crate::theme::mono()).color(t.text)),
                Err(e) => ui.label(RichText::new(e).color(t.warn)),
            };
            ui.separator();
            // A row, so the buttons take their own height rather than the window's.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(crate::tl!("Done")).clicked() {
                        done = true;
                    }
                    if ui.button(crate::tl!("Reset")).clicked() {
                        st.stamp = StampSettings::default();
                    }
                });
            });
        });
    if !open || done {
        st.editing_stamp = false;
        if st.stamp.text.trim().is_empty() {
            st.stamp = StampSettings::default();
        }
    }
}
