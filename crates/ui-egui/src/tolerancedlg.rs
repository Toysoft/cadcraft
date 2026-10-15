//! The Geometric Tolerance dialog (TOLERANCE typed or from the Dimension menu): the symbol, two
//! tolerances and three datum references of each of two frame lines, the projected tolerance
//! zone and the datum identifier, with the Symbol and Material Condition pickers. It never edits
//! the drawing itself: OK composes the frame and starts TOLERANCE at its location prompt.

use cadcraft_doc::{GdtDatum, GdtFrame, GdtLine, GdtPrim, GdtSymbol, GdtTol};
use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// The geometric characteristics the Symbol picker offers, in its order.
pub const CHARACTERISTICS: [GdtSymbol; 14] = [
    GdtSymbol::Position,
    GdtSymbol::Concentricity,
    GdtSymbol::Symmetry,
    GdtSymbol::Parallelism,
    GdtSymbol::Perpendicularity,
    GdtSymbol::Angularity,
    GdtSymbol::Cylindricity,
    GdtSymbol::Flatness,
    GdtSymbol::Circularity,
    GdtSymbol::Straightness,
    GdtSymbol::SurfaceProfile,
    GdtSymbol::LineProfile,
    GdtSymbol::CircularRunout,
    GdtSymbol::TotalRunout,
];

/// The material conditions the Material Condition picker offers.
pub const MATERIALS: [GdtSymbol; 3] = [GdtSymbol::Mmc, GdtSymbol::Lmc, GdtSymbol::Rfs];

/// A picker open over the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    /// The Symbol picker for frame line `0` or `1`.
    Symbol(usize),
    /// The Material Condition picker for a line's slot: 0 and 1 the tolerances, 2–4 the datums.
    Material(usize, usize),
}

/// The dialog while open.
#[derive(Clone, Debug, PartialEq)]
pub struct ToleranceDlgState {
    /// Always two lines, each with both tolerances and three datum references.
    pub frame: GdtFrame,
    pub picker: Option<Picker>,
}

impl Default for ToleranceDlgState {
    fn default() -> Self {
        let line = GdtLine { symbol: None, tol1: Some(GdtTol::default()), tol2: Some(GdtTol::default()), datums: vec![GdtDatum::default(); 3] };
        ToleranceDlgState { frame: GdtFrame { lines: vec![line.clone(), line], ..Default::default() }, picker: None }
    }
}

/// Open the dialog empty.
pub fn open(app: &mut CadApp) {
    app.tolerance_dlg = Some(ToleranceDlgState::default());
    app.ui.dialog = Some("tolerance".into());
}

/// OK: close the dialog and ask for the frame's location (nothing when the frame is empty).
pub fn ok(app: &mut CadApp) {
    let Some(st) = app.tolerance_dlg.take() else { return };
    if let Err(e) = app.session.start_tolerance_location(cadcraft_doc::compose_frame(&st.frame)) {
        app.session.echo(e.to_string());
    }
}

fn symbol_label(s: GdtSymbol) -> &'static str {
    match s {
        GdtSymbol::Position => crate::tl!("Position"),
        GdtSymbol::Concentricity => crate::tl!("Concentricity"),
        GdtSymbol::Symmetry => crate::tl!("Symmetry"),
        GdtSymbol::Parallelism => crate::tl!("Parallelism"),
        GdtSymbol::Perpendicularity => crate::tl!("Perpendicularity"),
        GdtSymbol::Angularity => crate::tl!("Angularity"),
        GdtSymbol::Cylindricity => crate::tl!("Cylindricity"),
        GdtSymbol::Flatness => crate::tl!("Flatness"),
        GdtSymbol::Circularity => crate::tl!("Circularity"),
        GdtSymbol::Straightness => crate::tl!("Straightness"),
        GdtSymbol::SurfaceProfile => crate::tl!("Profile of a surface"),
        GdtSymbol::LineProfile => crate::tl!("Profile of a line"),
        GdtSymbol::CircularRunout => crate::tl!("Circular runout"),
        GdtSymbol::TotalRunout => crate::tl!("Total runout"),
        GdtSymbol::Diameter => crate::tl!("Diameter"),
        GdtSymbol::Mmc => crate::tl!("Maximum material condition"),
        GdtSymbol::Lmc => crate::tl!("Least material condition"),
        GdtSymbol::Rfs => crate::tl!("Regardless of feature size"),
        GdtSymbol::Projected => crate::tl!("Projected Tolerance Zone"),
    }
}

const BOX: f32 = 26.0;

/// Paint `s` in `r` from the same unit-box geometry the drawing uses.
fn paint_symbol(p: &egui::Painter, r: Rect, s: GdtSymbol, color: Color32) {
    let size = r.width().min(r.height()) - 8.0;
    let origin = pos2(r.center().x - size / 2.0, r.center().y + size / 2.0);
    let at = |q: cadcraft_geom::Vec2| Pos2::new(origin.x + q.x as f32 * size, origin.y - q.y as f32 * size);
    let stroke = Stroke::new(1.3, color);
    let arc = |c: cadcraft_geom::Vec2, radius: f64, start: f64, end: f64| {
        let sweep = if end > start { end - start } else { end - start + std::f64::consts::TAU };
        (0..=24).map(|i| at(c + cadcraft_geom::Vec2::from_angle(start + sweep * f64::from(i) / 24.0) * radius)).collect::<Vec<_>>()
    };
    for prim in cadcraft_doc::symbol_prims(s) {
        match prim {
            GdtPrim::Line(pts) => {
                p.add(egui::Shape::line(pts.into_iter().map(at).collect(), stroke));
            }
            GdtPrim::Circle { center, radius } => {
                p.circle_stroke(at(center), radius as f32 * size, stroke);
            }
            GdtPrim::Arc { center, radius, start, end } => {
                p.add(egui::Shape::line(arc(center, radius, start, end), stroke));
            }
            GdtPrim::Fill(t) => {
                p.add(egui::Shape::convex_polygon(t.iter().map(|q| at(*q)).collect(), color, Stroke::NONE));
            }
        }
    }
}

/// A clickable box showing `s` (empty when `None`), as the dialog's symbol and modifier boxes.
fn symbol_box(ui: &mut egui::Ui, s: Option<GdtSymbol>, tip: &str) -> egui::Response {
    let t = Tokens::get();
    let (r, resp) = ui.allocate_exact_size(vec2(BOX, BOX), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
    if ui.is_rect_visible(r) {
        let p = ui.painter();
        let border = if resp.hovered() { t.accent } else { t.border };
        p.rect_filled(r, 2.0, t.control);
        p.rect_stroke(r, 2.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        if let Some(s) = s {
            paint_symbol(p, r, s, t.text);
        }
    }
    resp.on_hover_text(tip)
}

fn material(m: &Option<String>) -> Option<GdtSymbol> {
    m.as_deref().and_then(GdtSymbol::parse).filter(|s| s.is_material())
}

fn tolerance_cell(ui: &mut egui::Ui, t: &mut GdtTol, pick: impl FnOnce()) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if symbol_box(ui, t.diameter.then_some(GdtSymbol::Diameter), crate::tl!("Diameter")).clicked() {
            t.diameter = !t.diameter;
        }
        ui.add(egui::TextEdit::singleline(&mut t.value).desired_width(56.0));
        if symbol_box(ui, material(&t.material), crate::tl!("Material Condition")).clicked() {
            pick();
        }
    });
}

fn datum_cell(ui: &mut egui::Ui, d: &mut GdtDatum, pick: impl FnOnce()) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.add(egui::TextEdit::singleline(&mut d.letter).desired_width(36.0));
        if symbol_box(ui, material(&d.material), crate::tl!("Material Condition")).clicked() {
            pick();
        }
    });
}

/// The Geometric Tolerance window (`app.ui.dialog == "tolerance"`).
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    // Opened by name (control channel) with no state, or with no drawing to put a frame in.
    if app.tolerance_dlg.is_none() {
        self::open(app);
    }
    let Some(mut st) = app.tolerance_dlg.take().filter(|_| app.session.state().is_ok()) else {
        *open = false;
        return;
    };
    let (mut ok_clicked, mut cancel) = (false, false);
    let ToleranceDlgState { frame, picker } = &mut st;
    egui::Window::new(crate::tl!("Geometric Tolerance")).id(egui::Id::new("tolerance_dialog")).open(open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            egui::Grid::new("gdt_frame").num_columns(6).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                for h in [
                    crate::tl!("Sym"),
                    crate::tl!("Tolerance 1"),
                    crate::tl!("Tolerance 2"),
                    crate::tl!("Datum 1"),
                    crate::tl!("Datum 2"),
                    crate::tl!("Datum 3"),
                ] {
                    ui.label(h);
                }
                ui.end_row();
                for (k, line) in frame.lines.iter_mut().enumerate().take(2) {
                    let tip = line.symbol.map(symbol_label).unwrap_or(crate::tl!("Symbol"));
                    if symbol_box(ui, line.symbol, tip).clicked() {
                        *picker = Some(Picker::Symbol(k));
                    }
                    for (slot, tol) in [&mut line.tol1, &mut line.tol2].into_iter().enumerate() {
                        tolerance_cell(ui, tol.get_or_insert_with(GdtTol::default), || *picker = Some(Picker::Material(k, slot)));
                    }
                    if line.datums.len() < 3 {
                        line.datums.resize(3, GdtDatum::default());
                    }
                    for (n, d) in line.datums.iter_mut().enumerate().take(3) {
                        datum_cell(ui, d, || *picker = Some(Picker::Material(k, n + 2)));
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            egui::Grid::new("gdt_zone").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                ui.label(crate::tl!("Height"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut frame.height).desired_width(80.0));
                    ui.add_space(12.0);
                    ui.label(crate::tl!("Projected Tolerance Zone"));
                    if symbol_box(ui, frame.projected.then_some(GdtSymbol::Projected), crate::tl!("Projected Tolerance Zone")).clicked() {
                        frame.projected = !frame.projected;
                    }
                });
                ui.end_row();
                ui.label(crate::tl!("Datum Identifier"));
                ui.add(egui::TextEdit::singleline(&mut frame.datum_id).desired_width(80.0));
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                ok_clicked = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        },
    );
    if let Some(p) = *picker {
        let (title, choices): (&str, &[GdtSymbol]) = match p {
            Picker::Symbol(_) => (crate::tl!("Symbol"), &CHARACTERISTICS),
            Picker::Material(..) => (crate::tl!("Material Condition"), &MATERIALS),
        };
        let mut chosen = None;
        let mut picker_open = true;
        egui::Window::new(title).id(egui::Id::new("tolerance_picker")).open(&mut picker_open).collapsible(false).resizable(false).show(ctx, |ui| {
            egui::Grid::new("gdt_picker").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
                for (i, s) in choices.iter().map(|s| Some(*s)).chain([None]).enumerate() {
                    if symbol_box(ui, s, s.map(symbol_label).unwrap_or(crate::tl!("None"))).clicked() {
                        chosen = Some(s);
                    }
                    if i % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
        });
        if let Some(s) = chosen {
            set_pick(frame, p, s);
        }
        if chosen.is_some() || !picker_open {
            *picker = None;
        }
    }
    if ok_clicked {
        app.tolerance_dlg = Some(st);
        ok(app);
        *open = false;
    } else if !cancel && *open {
        app.tolerance_dlg = Some(st);
    } else {
        *open = false;
    }
}

/// Put the symbol `s` picked with `p` in the frame (`None` clears it).
pub fn set_pick(frame: &mut GdtFrame, p: Picker, s: Option<GdtSymbol>) {
    let (Picker::Symbol(k) | Picker::Material(k, _)) = p;
    let Some(line) = frame.lines.get_mut(k) else { return };
    let code = s.map(|s| s.code().to_ascii_uppercase().to_string());
    match p {
        Picker::Symbol(_) => line.symbol = s.filter(|s| s.is_characteristic()),
        Picker::Material(_, slot @ 0..=1) => {
            let t = if slot == 0 { &mut line.tol1 } else { &mut line.tol2 };
            t.get_or_insert_with(GdtTol::default).material = code;
        }
        Picker::Material(_, slot) => {
            if let Some(d) = slot.checked_sub(2).and_then(|n| line.datums.get_mut(n)) {
                d.material = code;
            }
        }
    }
}
