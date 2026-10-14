//! IMPORTPDF: choose a file, analyze every page, and select a cached preview.
//! Conversion stays in the engine; one desktop worker keeps the UI responsive.
use crate::CadApp;
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
static BUSY: AtomicBool = AtomicBool::new(false);
type Job = Arc<Mutex<Option<Result<Value, String>>>>;

pub fn is_pdf(name: &str) -> bool {
    std::path::Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
}
pub(crate) fn choose_file(app: &mut CadApp) {
    if let Some(picker) = app.services.pick_pdf.as_ref().or(app.services.pick_open.as_ref()) {
        if let Some(path) = picker() {
            open_import(app, json!({"path":path}));
        }
    } else {
        open_import(app, json!({}));
    }
}
pub(crate) fn open_import(app: &mut CadApp, input: Value) {
    if app.session.state().is_err() {
        let _ = app.run("new", json!({}));
    }
    app.pdf_import_input = Some(input);
    app.ui.dialog = Some("pdfimport".into());
    app.ui.start_tab = false;
}
#[derive(Clone)]
struct Page {
    info: Value,
    image: Option<egui::TextureHandle>,
}
#[derive(Clone)]
struct State {
    source: Value,
    path: String,
    page: u32,
    units: String,
    scale: f64,
    x: f64,
    y: f64,
    rotation: f64,
    outlines: bool,
    pages: Vec<Page>,
    pending: bool,
    job: Option<Job>,
    error: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            source: json!({}),
            path: String::new(),
            page: 1,
            units: "mm".into(),
            scale: 1.0,
            x: 0.0,
            y: 0.0,
            rotation: 0.0,
            outlines: true,
            pages: Vec::new(),
            pending: false,
            job: None,
            error: None,
        }
    }
}
impl State {
    fn params(&self, preview: bool) -> Value {
        let mut p = self.source.clone();
        if !self.path.is_empty() {
            p["path"] = json!(self.path);
            if let Some(o) = p.as_object_mut() {
                o.remove("data");
            }
        }
        p["page"] = json!(self.page);
        p["units"] = json!(self.units);
        p["scale"] = json!(self.scale);
        p["at"] = json!([self.x, self.y]);
        p["rotation"] = json!(self.rotation);
        p["text"] = json!(if self.outlines { "outlines" } else { "editable" });
        p["preview"] = json!(preview);
        p
    }
    fn begin(&mut self) {
        self.pending = !self.path.is_empty() || self.source.get("data").is_some();
        self.job = None;
        self.pages.clear();
        self.error = None;
    }
    fn accept(&mut self, result: Result<Value, String>, ctx: &egui::Context) {
        self.pending = false;
        self.job = None;
        match result {
            Err(e) => self.error = Some(e),
            Ok(r) => {
                let Some(pages) = r["previews"].as_array().filter(|p| p.len() <= 512) else {
                    self.error = Some("Invalid PDF page overview.".into());
                    return;
                };
                for info in pages {
                    let mut info = info.clone();
                    let image = info
                        .get("thumbnail")
                        .and_then(Value::as_str)
                        .filter(|b| b.len() <= 1_400_000)
                        .and_then(cadcraft_engine::cmd::file::base64_decode)
                        .and_then(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).ok())
                        .map(|im| {
                            let im = im.to_rgba8();
                            ctx.load_texture(
                                format!("pdf-page-{}", info["page"]),
                                egui::ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw()),
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    if let Some(o) = info.as_object_mut() {
                        o.remove("thumbnail");
                    }
                    self.pages.push(Page { info, image });
                }
                if !self.pages.iter().any(|p| p.info["page"].as_u64() == Some(u64::from(self.page))) {
                    self.page = 1;
                }
            }
        }
    }
    fn poll(&mut self, app: &mut CadApp, ctx: &egui::Context) {
        if !self.pending {
            return;
        }
        if let Some(job) = &self.job {
            let result = job.lock().ok().and_then(|mut r| r.take());
            if let Some(result) = result {
                self.accept(result, ctx);
            }
        } else {
            let mut p = self.params(true);
            p["pages"] = json!(true);
            #[cfg(target_arch = "wasm32")]
            self.accept(app.run("importpdf", p), ctx);
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = app;
                if BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    let job: Job = Arc::new(Mutex::new(None));
                    let output = Arc::clone(&job);
                    let repaint = ctx.clone();
                    match std::thread::Builder::new().name("pdf-page-overview".into()).spawn(move || {
                        struct Release;
                        impl Drop for Release {
                            fn drop(&mut self) {
                                BUSY.store(false, Ordering::Release);
                            }
                        }
                        let _release = Release;
                        let r = cadcraft_engine::Session::new().execute("importpdf", &p).map_err(|e| e.to_string());
                        if let Ok(mut slot) = output.lock() {
                            *slot = Some(r);
                        }
                        repaint.request_repaint();
                    }) {
                        Ok(_) => self.job = Some(job),
                        Err(e) => {
                            BUSY.store(false, Ordering::Release);
                            self.accept(Err(e.to_string()), ctx);
                        }
                    }
                }
            }
        }
        if self.pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    fn selected(&self) -> Option<&Page> {
        self.pages.iter().find(|p| p.info["page"].as_u64() == Some(u64::from(self.page)))
    }
}
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("pdf_import_state");
    let mut state = ctx.data_mut(|d| d.get_temp::<State>(id)).unwrap_or_default();
    if let Some(source) = app.pdf_import_input.take() {
        state = State { path: source["path"].as_str().unwrap_or_default().into(), source, ..State::default() };
        state.begin();
    }
    state.poll(app, ctx);
    let (mut reload, mut apply, mut cancel) = (false, false, false);
    egui::Window::new(crate::tl!("Import Vector PDF")).id(id).open(open).default_width(1050.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if state.source.get("data").is_some() && state.path.is_empty() {
                ui.label(state.source["name"].as_str().unwrap_or("Dropped PDF"));
            } else {
                let response = ui.add(egui::TextEdit::singleline(&mut state.path).hint_text(crate::tl!("PDF file path")).desired_width(850.0));
                reload |= response.lost_focus() && response.changed();
            }
            if ui.button(crate::tl!("Browse...")).clicked()
                && let Some(picker) = app.services.pick_pdf.as_ref().or(app.services.pick_open.as_ref())
                && let Some(path) = picker()
            {
                state.path = path;
                state.source = json!({});
                reload = true;
            }
        });
        if state.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::tl!("Analyzing all pages..."));
            });
        }
        if !state.pages.is_empty() {
            ui.label(format!("{}: {}", crate::tl!("Pages"), state.pages.len()));
        }
        ui.columns(2, |cols| {
            egui::ScrollArea::vertical().id_salt("pdf_page_gallery").max_height(510.0).show(&mut cols[0], |ui| {
                egui::Grid::new("pdf_page_cards").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                    for (i, p) in state.pages.iter().enumerate() {
                        let page = p.info["page"].as_u64().unwrap_or(1) as u32;
                        let label = format!("{} {page}", crate::tl!("Page"));
                        let button = match &p.image {
                            Some(image) => egui::Button::image_and_text(egui::Image::new((image.id(), egui::vec2(170.0, 113.0))), &label),
                            None => egui::Button::new(format!("{label}\n{}", crate::tl!("Preview unavailable"))),
                        }
                        .selected(state.page == page)
                        .min_size(egui::vec2(235.0, 125.0));
                        if ui.add(button).clicked() {
                            state.page = page;
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            });
            let ui = &mut cols[1];
            egui::Grid::new("pdf_options").num_columns(2).show(ui, |ui| {
                ui.label(crate::tl!("Drawing units"));
                egui::ComboBox::from_id_salt("pdf_units")
                    .selected_text(match state.units.as_str() {
                        "m" => crate::tl!("Metres"),
                        "points" => crate::tl!("PDF points"),
                        _ => crate::tl!("Millimetres"),
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [("mm", crate::tl!("Millimetres")), ("m", crate::tl!("Metres")), ("points", crate::tl!("PDF points"))] {
                            ui.selectable_value(&mut state.units, value.into(), label);
                        }
                    });
                ui.end_row();
                ui.label(crate::tl!("Paper scale factor"));
                ui.add(egui::DragValue::new(&mut state.scale).range(1e-9..=1e9).speed(1.0));
                ui.end_row();
                ui.label(crate::tl!("Insertion X, Y"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut state.x).speed(0.1));
                    ui.add(egui::DragValue::new(&mut state.y).speed(0.1));
                });
                ui.end_row();
                ui.label(crate::tl!("Rotation (degrees)"));
                ui.add(egui::DragValue::new(&mut state.rotation).speed(1.0));
                ui.end_row();
                ui.label(crate::tl!("PDF text"));
                reload |= ui.checkbox(&mut state.outlines, crate::tl!("Convert embedded text to vector outlines")).changed();
                ui.end_row();
            });
            ui.label(crate::tl!("For a 1:100 drawing in metres, choose Metres and set the paper scale factor to 100."));
            if let Some(p) = state.selected() {
                ui.separator();
                ui.label(format!("{} {} · {} objects", crate::tl!("Page"), state.page, p.info["objects"].as_u64().unwrap_or(0)));
                if let Some(image) = &p.image {
                    ui.image((image.id(), egui::vec2(450.0, 300.0)));
                }
                if let Some(error) = p.info["error"].as_str() {
                    ui.label(error);
                }
                egui::ScrollArea::vertical().id_salt("pdf_page_warnings").max_height(85.0).show(ui, |ui| {
                    for warning in p.info["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                        ui.label(egui::RichText::new(warning).small());
                    }
                });
            }
        });
        if let Some(error) = &state.error {
            ui.label(error);
        }
        ui.label(crate::tl!("Imported objects are editable. Verify the scale using a known dimension before aligning survey points."));
        ui.horizontal(|ui| {
            reload |= ui.add_enabled(!state.pending, egui::Button::new(crate::tl!("Reload PDF"))).clicked();
            let ready = !state.pending && state.selected().is_some_and(|p| p.info.get("error").is_none());
            apply = ui.add_enabled(ready, egui::Button::new(crate::tl!("Import Vectors"))).clicked();
            cancel = ui.button(crate::tl!("Cancel")).clicked();
        });
    });
    if reload {
        state.begin();
    }
    if apply {
        match app.run("importpdf", state.params(false)) {
            Ok(_) => {
                let _ = app.run("zoom.extents", json!({}));
                *open = false;
            }
            Err(e) => state.error = Some(e),
        }
    }
    if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id, state));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id));
    }
}
