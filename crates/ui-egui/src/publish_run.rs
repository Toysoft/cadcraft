//! Publishing without freezing the window: a [`PublishJob`] (the request checked, the drawings
//! taken as a snapshot) runs on a worker thread while the status bar shows its progress and a
//! small card offers Cancel. When it ends, the Publish Report lists every sheet (published or
//! why not) with Reveal in Finder / Show in Explorer and Publish Again, and "Open the result when
//! done" opens the file. The web build has no threads and publishes in the foreground.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use cadcraft_engine::cmd::PublishJob;
use egui::{Align, Align2, Color32, Layout, RichText, Stroke, vec2};
use serde_json::Value;

use crate::CadApp;
use crate::theme::Tokens;

enum Msg {
    Progress(usize, usize),
    Done(Result<Value, String>),
}

struct Running {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    done: usize,
    total: usize,
    request: Value,
    open_result: bool,
}

/// The last run's request and what it gave, for the Publish Report.
pub struct Report {
    request: Value,
    result: Result<Value, String>,
    open_result: bool,
}

/// The run in progress, if any, and the last run's report.
#[derive(Default)]
pub struct Publishing {
    running: Option<Running>,
    report: Option<Report>,
}

impl Publishing {
    /// Whether a run is in progress.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }
}

/// Publish `request` (the engine's `publish` parameters). With `background`, the plotting runs on
/// a worker thread and this returns at once; otherwise it runs now. Bad parameters are refused
/// here, before anything runs.
pub fn start(app: &mut CadApp, request: Value, background: bool, open_result: bool) -> Result<(), String> {
    if app.publishing.busy() {
        return Err(crate::tl!("Publishing is already running.").to_string());
    }
    let job = PublishJob::new(&app.session, &request).map_err(|e| e.to_string())?;
    if background && !cfg!(target_arch = "wasm32") {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let total = job.len();
        let worker = std::thread::Builder::new().name("cadcraft-publish".into()).spawn(move || {
            let progress = tx.clone();
            // A panic in a plotter must not take the app down: it ends the run with an error.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job.run(&mut |done, total| {
                    let _ = progress.send(Msg::Progress(done, total));
                    !stop.load(Ordering::Relaxed)
                })
            }));
            let result = match result {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("publish: publishing stopped unexpectedly".to_string()),
            };
            let _ = tx.send(Msg::Done(result));
        });
        if worker.is_ok() {
            app.publishing.running = Some(Running { rx, cancel, done: 0, total, request, open_result });
            app.set_status(crate::tf!("Publishing {done} of {total} sheets...", done = 0, total = total));
            return Ok(());
        }
        // No thread to publish on: publish now instead.
        let job = PublishJob::new(&app.session, &request).map_err(|e| e.to_string())?;
        let result = job.run(&mut |_, _| true).map_err(|e| e.to_string());
        finish(app, request, result, open_result);
        return Ok(());
    }
    let result = job.run(&mut |_, _| true).map_err(|e| e.to_string());
    finish(app, request, result, open_result);
    Ok(())
}

/// The report line for a result.
fn summary(result: &Result<Value, String>) -> String {
    match result {
        Ok(r) => {
            let ok = r["published"].as_u64().unwrap_or(0);
            let total = r["sheets"].as_array().map_or(0, Vec::len);
            let mut line = crate::tf!("Published {ok} of {total} sheets", ok = ok, total = total);
            if r["cancelled"] == true {
                line = format!("{line} · {}", crate::tl!("cancelled"));
            }
            line
        }
        Err(e) => e.clone(),
    }
}

/// The file the result is best shown by: the combined PDF, the only file, or the folder.
fn result_target(r: &Value) -> Option<(String, bool)> {
    if let Some(p) = r["combined"]["path"].as_str() {
        return Some((p.to_string(), true));
    }
    let paths: Vec<&str> = r["sheets"].as_array().into_iter().flatten().filter_map(|x| x["path"].as_str()).collect();
    match paths.as_slice() {
        [one] => Some((one.to_string(), true)),
        [] => None,
        _ => r["folder"].as_str().map(|f| (f.to_string(), false)),
    }
}

/// Hand the files a run returned as data (no folder: the web build) to the host, which starts a
/// download for each. Returns why one couldn't be delivered.
fn deliver_data(r: &Value) -> Option<String> {
    use cadcraft_engine::cmd::file::{base64_decode, deliver};
    let files: Vec<&Value> = match r.get("combined") {
        Some(c) => vec![c],
        None => r["sheets"].as_array().into_iter().flatten().collect(),
    };
    let files: Vec<(&str, &str)> = files.iter().filter_map(|x| Some((x["file"].as_str()?, x["data"].as_str()?))).collect();
    if files.is_empty() {
        return None;
    }
    let Some(deliver) = deliver() else { return Some(crate::tl!("Choose a folder to publish to.").to_string()) };
    for (name, data) in files {
        let mime = match name.rsplit('.').next() {
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            _ => "application/pdf",
        };
        let Some(bytes) = base64_decode(data) else { return Some(format!("{name}: the file could not be read back")) };
        if let Err(e) = deliver(name, mime, &bytes) {
            return Some(format!("{name}: {e}"));
        }
    }
    None
}

fn finish(app: &mut CadApp, request: Value, result: Result<Value, String>, open_result: bool) {
    app.set_status(summary(&result));
    if let Ok(r) = &result
        && let Some(e) = deliver_data(r)
    {
        app.set_status(e);
    }
    if open_result
        && let Ok(r) = &result
        && let Some((target, _)) = result_target(r)
        && let Some(open) = app.services.open_path.as_ref()
        && let Err(e) = open(&target, false)
    {
        app.set_status(e);
    }
    app.publishing.report = Some(Report { request, result, open_result });
    // Show the report unless another window took the place of Publish Sheets meanwhile.
    if app.ui.dialog.as_deref().is_none_or(|d| d == "publish") {
        app.ui.dialog = Some("publish_report".into());
    }
}

/// Take the worker's news; show the progress card while it runs. Called every frame.
pub fn poll(app: &mut CadApp, ctx: &egui::Context) {
    let Some(run) = app.publishing.running.as_mut() else { return };
    let mut done = None;
    loop {
        match run.rx.try_recv() {
            Ok(Msg::Progress(d, t)) => {
                run.done = d;
                run.total = t;
            }
            Ok(Msg::Done(r)) => {
                done = Some(r);
                break;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                done = Some(Err("publish: publishing stopped unexpectedly".to_string()));
                break;
            }
        }
    }
    if let Some(result) = done {
        if let Some(run) = app.publishing.running.take() {
            finish(app, run.request, result, run.open_result);
        }
        return;
    }
    let (d, t) = (run.done, run.total);
    let text = if run.cancel.load(Ordering::Relaxed) {
        crate::tl!("Cancelling...").to_string()
    } else {
        crate::tf!("Publishing {done} of {total} sheets...", done = d, total = t)
    };
    app.set_status(text);
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// The progress card with Cancel, drawn with the windows while a run is in progress.
pub fn card(app: &mut CadApp, ctx: &egui::Context) {
    let Some(run) = app.publishing.running.as_ref() else { return };
    let (d, t) = (run.done, run.total);
    let cancelling = run.cancel.load(Ordering::Relaxed);
    let text =
        if cancelling { crate::tl!("Cancelling...").to_string() } else { crate::tf!("Publishing {done} of {total} sheets...", done = d, total = t) };
    let t_ = Tokens::get();
    let mut cancel = false;
    // Above windows, so a window fading out or opened meanwhile never covers Cancel. A fixed
    // width keeps the card (and its button) in place from its first frame.
    // Pinned to the window's corner above the status bar, not to the room the panels leave
    // (which isn't known yet when this runs, so the card would move and a click miss it).
    let corner = ctx.content_rect().right_bottom() - vec2(16.0, 44.0);
    egui::Area::new(egui::Id::new("cc_publish_progress")).order(egui::Order::Foreground).pivot(Align2::RIGHT_BOTTOM).fixed_pos(corner).show(
        ctx,
        |ui| {
            egui::Frame::NONE
                .fill(t_.panel)
                .stroke(Stroke::new(1.0, t_.accent))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.label(RichText::new(&text).color(t_.text));
                    ui.horizontal(|ui| {
                        let fraction = if t == 0 { 0.0 } else { d as f32 / t as f32 };
                        ui.add(egui::ProgressBar::new(fraction).desired_width(190.0).desired_height(6.0));
                        if ui.add_enabled(!cancelling, egui::Button::new(crate::tl!("Cancel"))).clicked() {
                            cancel = true;
                        }
                    });
                });
        },
    );
    if cancel && let Some(run) = app.publishing.running.as_ref() {
        run.cancel.store(true, Ordering::Relaxed);
    }
}

/// The file manager's name on this system.
fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        crate::tl!("Reveal in Finder")
    } else if cfg!(target_os = "windows") {
        crate::tl!("Show in Explorer")
    } else {
        crate::tl!("Show in Folder")
    }
}

/// The Publish Report (dialog name `"publish_report"`).
pub fn report_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(report) = app.publishing.report.as_ref() else {
        *open = false;
        return;
    };
    let t = Tokens::get();
    let line = summary(&report.result);
    let rows: Vec<(bool, String, String, String)> = match &report.result {
        Ok(r) => r["sheets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                let ok = x["ok"] == true;
                let what = if ok {
                    match x["page"].as_u64() {
                        Some(p) => format!("{} · {}", x["file"].as_str().unwrap_or(""), crate::tf!("page {n}", n = p)),
                        None => x["file"].as_str().unwrap_or("").to_string(),
                    }
                } else {
                    x["error"].as_str().unwrap_or("").to_string()
                };
                let drawing = std::path::Path::new(x["drawing"].as_str().unwrap_or(""))
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                (ok, drawing, x["layout"].as_str().unwrap_or("").to_string(), what)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let target = report.result.as_ref().ok().and_then(result_target);
    let (request, open_result) = (report.request.clone(), report.open_result);
    let busy = app.publishing.busy();
    let can_reveal = target.is_some() && app.services.open_path.is_some();
    let (mut reveal, mut again, mut close) = (false, false, false);
    egui::Window::new(crate::tl!("Publish Report"))
        .id(egui::Id::new("cc_publish_report"))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(640.0, 360.0))
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let failed = rows.iter().filter(|r| !r.0).count();
            ui.label(RichText::new(&line).strong().color(if failed > 0 || report.result.is_err() { t.warn } else { t.text }));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("cc_publish_report_rows").num_columns(4).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (ok, drawing, sheet, what) in &rows {
                        let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                        mark(ui.painter(), rect, *ok);
                        crate::icons::describe_control(ui, &resp, if *ok { "Published" } else { "Failed" }, None, false);
                        ui.label(RichText::new(drawing).color(t.text_dim));
                        ui.label(sheet);
                        ui.add(egui::Label::new(RichText::new(what).color(if *ok { t.text_dim } else { t.warn })).truncate());
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(can_reveal, egui::Button::new(reveal_label())).clicked() {
                    reveal = true;
                }
                let again_button = ui.add_enabled(!busy, egui::Button::new(crate::tl!("Publish Again")));
                if again_button.clicked() {
                    again = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(crate::tl!("Close")).clicked() {
                        close = true;
                    }
                });
            });
        });
    if reveal
        && let Some((path, file)) = target
        && let Some(open_path) = app.services.open_path.as_ref()
        && let Err(e) = open_path(&path, file)
    {
        app.set_status(e);
    }
    if again && let Err(e) = start(app, request, true, open_result) {
        app.set_status(e);
    }
    if close {
        *open = false;
    }
}

/// A tick or a cross, in the theme's colours.
fn mark(p: &egui::Painter, r: egui::Rect, ok: bool) {
    let t = Tokens::get();
    let c: Color32 = if ok { t.accent } else { t.warn };
    let s = Stroke::new(1.8, c);
    let (x, y, w) = (r.left(), r.top(), r.width());
    if ok {
        p.line_segment([egui::pos2(x + w * 0.15, y + w * 0.55), egui::pos2(x + w * 0.4, y + w * 0.8)], s);
        p.line_segment([egui::pos2(x + w * 0.4, y + w * 0.8), egui::pos2(x + w * 0.88, y + w * 0.22)], s);
    } else {
        p.line_segment([egui::pos2(x + w * 0.2, y + w * 0.2), egui::pos2(x + w * 0.8, y + w * 0.8)], s);
        p.line_segment([egui::pos2(x + w * 0.8, y + w * 0.2), egui::pos2(x + w * 0.2, y + w * 0.8)], s);
    }
}
