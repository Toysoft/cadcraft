//! The native palette: everything the web build's note stands in for.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Context, Ui};

use super::{engine, hub, models, tasks};
use crate::theme::Tokens;

/// Lock the palette's state. A poisoned lock (a worker thread that died mid-answer) still hands
/// back its data: the state is UI state, and a worker that panicked is a lost answer, not a
/// broken app — the same recovery the shipped palettes use.
fn lock(state: &Arc<Mutex<State>>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which tab the palette shows.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Assistant,
    Models,
    About,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Assistant => "Assistant",
            Self::Models => "Models",
            Self::About => "About",
        }
    }
}

/// What the one background job is doing right now (a second click waits for it). Pulls and
/// saves carry their model's id, so a progress line shows under its own model's card only.
enum Work {
    None,
    /// Looking for runtimes and reading the machine's figures.
    Detecting,
    /// Asking the model a question.
    Asking,
    /// The runtime is pulling a model; the last progress line, and its bytes when it streams.
    Pulling {
        id: &'static str,
        note: String,
        done: u64,
        total: u64,
    },
    /// The shared folder is being filled; how many bytes of the file are done.
    Saving {
        id: &'static str,
        done: u64,
        total: u64,
    },
}

/// The palette's state, shared with its worker threads.
struct State {
    tab: Tab,
    /// First open: what runtimes, models and machine figures were found.
    detected: bool,
    runtimes: Vec<engine::Runtime>,
    runtime: usize,
    installed: Vec<String>,
    model: usize,
    device: Option<models::Device>,
    saved: Vec<String>,
    task: usize,
    input: String,
    /// The conversation so far, oldest first.
    history: Vec<engine::ChatMessage>,
    error: Option<String>,
    work: Work,
    started: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: Tab::Assistant,
            detected: false,
            runtimes: Vec::new(),
            runtime: 0,
            installed: Vec::new(),
            model: 0,
            device: None,
            saved: Vec::new(),
            task: 0,
            input: String::new(),
            history: Vec::new(),
            error: None,
            work: Work::None,
            started: None,
        }
    }
}

impl State {
    fn busy(&self) -> bool {
        !matches!(self.work, Work::None)
    }
}

/// The palette's state, created on first draw and kept in the egui data.
fn state(ctx: &Context) -> Arc<Mutex<State>> {
    let id = egui::Id::new("craft-ai-state");
    if let Some(s) = ctx.data(|d| d.get_temp::<Arc<Mutex<State>>>(id)) {
        return s;
    }
    let s = Arc::new(Mutex::new(State::default()));
    ctx.data_mut(|d| d.insert_temp(id, s.clone()));
    s
}

/// The palette's native body.
pub(super) fn show(_app: &mut crate::CadApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let state = state(&ctx);
    {
        let s = &mut *lock(&state);
        if !s.detected {
            s.detected = true;
            s.work = Work::Detecting;
            spawn_detect(&ctx, &state);
        }
        // A busy timer should tick even when the pointer is still.
        if s.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }
    let t = Tokens::get();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for tab in [Tab::Assistant, Tab::Models, Tab::About] {
            let selected = lock(&state).tab == tab;
            if ui.add(egui::Button::selectable(selected, crate::i18n::t(tab.label()))).clicked() {
                lock(&state).tab = tab;
            }
        }
    });
    ui.add_space(4.0);
    match lock(&state).tab {
        Tab::Assistant => assistant(ui, &t, &ctx, &state),
        Tab::Models => model_cards(ui, &t, &ctx, &state),
        Tab::About => about(ui, &t),
    }
}

/// The Assistant tab: runtime and model pickers, the task, the question, the answer.
fn assistant(ui: &mut Ui, t: &Tokens, ctx: &Context, state: &Arc<Mutex<State>>) {
    let mut detect = false;
    let mut ask: Option<(engine::Runtime, String, String)> = None;
    {
        let s = &mut *lock(state);
        if s.runtimes.is_empty() {
            if matches!(s.work, Work::Detecting) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dim(ui, t, crate::tl!("Looking for a local model runtime…").to_string());
                });
            } else {
                ui.label(egui::RichText::new(crate::tl!("No local model runtime found.")).color(t.text));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(crate::tl!(
                        "Craft AI talks to a model runtime on this machine — Ollama, LM Studio or llama.cpp's server — and never to a cloud service. Start one, then Refresh."
                    ))
                    .font(crate::theme::small())
                    .weak(),
                );
                if ui.button(crate::tl!("Refresh")).clicked() {
                    detect = true;
                }
            }
        } else {
            let rt = s.runtimes[s.runtime.min(s.runtimes.len() - 1)];
            let names: Vec<String> = s.runtimes.iter().map(|r| r.to_string()).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            label_row(ui, t, crate::tl!("Runtime"), |ui| {
                if let Some(i) = dropdown(ui, "craft-ai-runtime", &names[s.runtime.min(names.len() - 1)], &refs) {
                    s.runtime = i.min(s.runtimes.len() - 1);
                    detect = true;
                }
            });
            if s.installed.is_empty() {
                if !matches!(s.work, Work::Detecting) {
                    dim(ui, t, crate::tl!("No model installed yet — see the Models tab.").to_string());
                }
            } else {
                // A Gemma model, if any, is the obvious default; an index that outlived its
                // list lands back on one.
                if s.model >= s.installed.len() {
                    s.model = s.installed.iter().position(|m| m.contains("gemma")).unwrap_or(0);
                }
                let model_refs: Vec<&str> = s.installed.iter().map(String::as_str).collect();
                label_row(ui, t, crate::tl!("Model"), |ui| {
                    if let Some(i) = dropdown(ui, "craft-ai-model", &s.installed[s.model.min(s.installed.len() - 1)], &model_refs) {
                        s.model = i.min(s.installed.len() - 1);
                    }
                });
                let labels: Vec<&str> = tasks::TASKS.iter().map(|task| task.label).collect();
                label_row(ui, t, crate::tl!("Task"), |ui| {
                    if let Some(i) = dropdown(ui, "craft-ai-task", labels[s.task.min(labels.len() - 1)], &labels) {
                        s.task = i;
                    }
                });
                ui.add_space(4.0);
                let hint = crate::i18n::t(tasks::TASKS[s.task.min(tasks::TASKS.len() - 1)].hint);
                ui.add(
                    egui::TextEdit::multiline(&mut s.input)
                        .hint_text(hint)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .font(crate::theme::small()),
                );
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let busy = s.busy();
                    let ready = !busy && !s.input.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new(crate::tl!("Ask"))).clicked() {
                        let model = s.installed[s.model.min(s.installed.len() - 1)].clone();
                        ask = Some((rt, model, tasks::user_prompt(s.task, &s.input)));
                    }
                    if !s.history.is_empty() && !busy && ui.small_button(crate::tl!("Clear")).clicked() {
                        s.history.clear();
                    }
                    if busy {
                        ui.spinner();
                        let secs = s.started.map(|at| at.elapsed().as_secs()).unwrap_or(0);
                        dim(ui, t, crate::tf!("Thinking… {s}s", s = secs));
                    }
                });
            }
            if let Some(e) = &s.error {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(e).font(crate::theme::small()).color(t.text).weak());
            }
            if !s.history.is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    let rtl = crate::i18n::is_rtl();
                    for m in &s.history {
                        if m.role == "user" {
                            ui.label(egui::RichText::new(format!("› {}", m.content)).font(crate::theme::small()).color(t.text_dim));
                        } else {
                            // What the model says may be Arabic the user wrote in: it keeps its
                            // logical order (clipboard order) on arrival and is drawn visual,
                            // like every Arabic the interface shows.
                            let text = if rtl { crate::i18n::bidi::visual_rtl(&m.content) } else { m.content.clone() };
                            ui.add(egui::Label::new(egui::RichText::new(text).font(crate::theme::small())).wrap());
                            ui.add_space(2.0);
                            if ui.small_button(crate::tl!("Copy")).clicked() {
                                ctx.copy_text(m.content.clone());
                            }
                        }
                    }
                });
            }
        }
    }
    if let Some((rt, model, prompt)) = ask {
        let s = &mut *lock(state);
        s.work = Work::Asking;
        s.started = Some(Instant::now());
        s.error = None;
        let question = s.input.trim().to_string();
        let history = s.history.clone();
        spawn_ask(ctx, state, rt, &model, &history, &question, &prompt);
    }
    if detect {
        let s = &mut *lock(state);
        s.work = Work::Detecting;
        spawn_detect(ctx, state);
    }
}

/// The Models tab: the catalogue with each model's figures, and its download buttons.
fn model_cards(ui: &mut Ui, t: &Tokens, ctx: &Context, state: &Arc<Mutex<State>>) {
    let mut detect = false;
    let mut pull: Option<&'static models::ModelSpec> = None;
    let mut save: Option<&'static models::ModelSpec> = None;
    let mut remove: Option<&'static models::ModelSpec> = None;
    {
        let s = &mut *lock(state);
        if let Some(d) = &s.device {
            let ram = d.ram_gb.map(|g| g.to_string()).unwrap_or_else(|| "?".into());
            let disk = d.free_gb.map(|g| g.to_string()).unwrap_or_else(|| "?".into());
            dim(ui, t, crate::tf!("This machine: {ram} GB RAM, {disk} GB free", ram = ram, disk = disk));
        } else if matches!(s.work, Work::Detecting) {
            ui.horizontal(|ui| {
                ui.spinner();
                dim(ui, t, crate::tl!("Looking for a local model runtime…").to_string());
            });
        }
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(crate::tl!("One download serves every ArtCraft app: the model lands in a shared folder all of them read."))
                .font(crate::theme::small())
                .weak(),
        );
        ui.add_space(4.0);
        let ollama = s.runtimes.iter().copied().find(|r| r.kind == engine::RuntimeKind::Ollama);
        for m in models::MODELS {
            ui.add(egui::Label::new(egui::RichText::new(m.name).strong().color(t.accent)).wrap());
            let (ram_ok, disk_ok) = match &s.device {
                Some(d) => (d.ram_gb.is_none_or(|g| g >= m.ram_gb), d.free_gb.is_none_or(|g| g >= m.bytes / 1_000_000_000)),
                None => (true, true),
            };
            ui.label(egui::RichText::new(format!("{} · {} · {}", m.params, m.context, m.inputs)).font(crate::theme::small()).color(t.text_dim));
            ui.label(egui::RichText::new(crate::i18n::t(m.best)).font(crate::theme::small()).color(t.text_dim));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::tf!("Disk {size}", size = m.gb())).font(crate::theme::small()).color(t.text_dim));
                ui.separator();
                ui.label(egui::RichText::new(crate::tf!("RAM {gb} GB", gb = m.ram_gb)).font(crate::theme::small()).color(t.text_dim));
                if s.device.is_some() {
                    ui.separator();
                    if ram_ok && disk_ok {
                        ui.label(egui::RichText::new(crate::tl!("Fits this machine")).font(crate::theme::small()).color(t.accent));
                    } else if !ram_ok {
                        ui.label(
                            egui::RichText::new(crate::tf!("Needs {ram} GB of RAM", ram = m.ram_gb))
                                .font(crate::theme::small())
                                .color(t.text),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(crate::tf!("Needs {disk} GB of disk", disk = models::gb(m.bytes)))
                                .font(crate::theme::small())
                                .color(t.text),
                        );
                    }
                }
            });
            let saved = s.saved.iter().any(|id| id == m.id);
            ui.horizontal(|ui| {
                let busy = s.busy();
                if ollama.is_some() && !busy && ui.small_button(crate::tl!("Download with Ollama")).clicked() {
                    pull = Some(m);
                }
                if !busy && !saved && ui.small_button(crate::tl!("Save model file")).clicked() {
                    save = Some(m);
                }
                if !busy && saved && ui.small_button(crate::tl!("Delete file")).clicked() {
                    remove = Some(m);
                }
            });
            match &s.work {
                Work::Pulling { id, note, done, total } if *id == m.id => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let pct = ((*done as f64 / (*total as f64).max(1.0)) * 100.0).round() as u64;
                        dim(ui, t, crate::tf!("Downloading… {pct}% — {note}", pct = pct, note = note));
                    });
                }
                Work::Saving { id, done, total } if *id == m.id => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let pct = ((*done as f64 / (*total as f64).max(1.0)) * 100.0).round() as u64;
                        dim(ui, t, crate::tf!("Saving… {pct}%", pct = pct));
                    });
                }
                _ if saved => {
                    dim(ui, t, crate::tl!("Saved in the shared folder").to_string());
                }
                _ => {}
            }
            if let Some(e) = &s.error {
                ui.label(egui::RichText::new(e).font(crate::theme::small()).color(t.text).weak());
            }
            ui.add_space(6.0);
        }
        if !s.runtimes.is_empty() && !s.busy() && ui.small_button(crate::tl!("Refresh")).clicked() {
            detect = true;
        }
    }
    // The pull button only shows when an Ollama was found, but a detection refresh may have
    // emptied it between the click and this frame — a missing runtime is an error line, not a
    // panic.
    if let Some(m) = pull {
        let Some(rt) = lock(state).runtimes.iter().copied().find(|r| r.kind == engine::RuntimeKind::Ollama) else {
            lock(state).error = Some(crate::tl!("Ollama is no longer running — Refresh finds it again.").to_string());
            return;
        };
        let s = &mut *lock(state);
        s.work = Work::Pulling { id: m.id, note: String::new(), done: 0, total: 0 };
        s.error = None;
        spawn_pull(ctx, state, rt, m);
    }
    if let Some(m) = save {
        let s = &mut *lock(state);
        s.work = Work::Saving { id: m.id, done: 0, total: m.bytes };
        s.error = None;
        spawn_save(ctx, state, m);
    }
    if let Some(m) = remove {
        lock(state).error = hub::remove(m).err();
        detect = true;
    }
    if detect {
        let s = &mut *lock(state);
        s.work = Work::Detecting;
        spawn_detect(ctx, state);
    }
}

/// The About tab: what runs where, and its honest limits.
fn about(ui: &mut Ui, t: &Tokens) {
    ui.label(egui::RichText::new(crate::tl!("Local and private")).strong().color(t.accent));
    ui.add_space(2.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(crate::tl!(
                "Craft AI runs a local Gemma model on this machine. Nothing you write here leaves it: no account, no cloud service, no telemetry — the palette talks to a model runtime on 127.0.0.1."
            ))
            .font(crate::theme::small()),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.label(egui::RichText::new(crate::tl!("Limits")).strong().color(t.accent));
    ui.add_space(2.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(crate::tl!(
                "A local model is not a frontier cloud model: it can be wrong, and a long answer takes its time on CPU. Check anything critical, and mind a model's size against this machine before downloading it."
            ))
            .font(crate::theme::small()),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    if let Some(dir) = hub::models_dir() {
        ui.label(egui::RichText::new(crate::tl!("The shared folder:")).font(crate::theme::small()).color(t.text_dim));
        ui.add(egui::Label::new(egui::RichText::new(dir.display().to_string()).font(crate::theme::small()).color(t.text_dim).monospace()).wrap());
    }
}

/// Find the runtimes, their models, the machine's figures and the shared folder — once, off the
/// UI thread.
fn spawn_detect(ctx: &Context, state: &Arc<Mutex<State>>) {
    let ctx = ctx.clone();
    let state = state.clone();
    let state_runtime = lock(&state).runtime;
    spawn("craft-ai-detect", move || {
        let runtimes = engine::detect();
        let installed = runtimes.get(state_runtime).copied().and_then(|rt| engine::installed(rt).ok()).unwrap_or_default();
        let device = models::device(hub::models_dir().as_deref());
        let saved = hub::saved_ids();
        let mut s = lock(&state);
        if s.runtime >= runtimes.len() {
            s.runtime = 0;
        }
        s.runtimes = runtimes;
        s.installed = installed;
        s.device = Some(device);
        s.saved = saved;
        s.work = Work::None;
        ctx.request_repaint();
    });
}

/// Ask the model the question, then bring the answer back.
fn spawn_ask(ctx: &Context, state: &Arc<Mutex<State>>, rt: engine::Runtime, model: &str, history: &[engine::ChatMessage], question: &str, prompt: &str) {
    let ctx = ctx.clone();
    let state = state.clone();
    let model = model.to_string();
    let history = history.to_vec();
    let question = question.to_string();
    let prompt = prompt.to_string();
    spawn("craft-ai-ask", move || {
        let reply = engine::chat(rt, &model, &tasks::system_prompt(), &history, &prompt);
        let mut s = lock(&state);
        match reply {
            Ok(answer) => {
                s.history.push(engine::ChatMessage { role: "user", content: question });
                s.history.push(engine::ChatMessage { role: "assistant", content: answer });
                // Keep the oldest turns out of the context when the conversation grows.
                if s.history.len() > 12 {
                    let cut = s.history.len() - 12;
                    s.history.drain(..cut);
                }
                s.input.clear();
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
        s.work = Work::None;
        s.started = None;
        ctx.request_repaint();
    });
}

/// Have Ollama pull the model (its own download manager, into its own store — shared by every
/// app on this machine in its own way).
fn spawn_pull(ctx: &Context, state: &Arc<Mutex<State>>, rt: engine::Runtime, m: &'static models::ModelSpec) {
    let ctx = ctx.clone();
    let state = state.clone();
    let tag = m.ollama.to_string();
    spawn("craft-ai-pull", move || {
        let reply = engine::pull(rt, &tag, &mut |note, progress| {
            let mut s = lock(&state);
            if let Work::Pulling { note: n, done, total, .. } = &mut s.work {
                *n = note.to_string();
                if let Some((d, t)) = progress {
                    *done = d;
                    *total = t;
                }
            }
            ctx.request_repaint();
        });
        let mut s = lock(&state);
        match reply {
            Ok(()) => s.error = None,
            Err(e) => s.error = Some(e),
        }
        s.work = Work::None;
        ctx.request_repaint();
    });
}

/// Download the model file into the shared folder every ArtCraft app reads.
fn spawn_save(ctx: &Context, state: &Arc<Mutex<State>>, m: &'static models::ModelSpec) {
    let ctx = ctx.clone();
    let state = state.clone();
    let spec = m;
    spawn("craft-ai-save", move || {
        let reply = hub::save(spec, tasks::APP, &mut |done, total| {
            let mut s = lock(&state);
            if let Work::Saving { done: d, total: t, .. } = &mut s.work {
                *d = done;
                *t = total;
            }
            ctx.request_repaint();
        });
        let mut s = lock(&state);
        match reply {
            Ok(()) => {
                s.saved = hub::saved_ids();
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
        s.work = Work::None;
        ctx.request_repaint();
    });
}

/// Run `body` on a named worker thread; if threads are unavailable the palette's error line says
/// so on the next draw.
fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_string()).spawn(body);
}

/// A quiet aside line, the style the app's other palettes use for theirs.
fn dim(ui: &mut Ui, t: &Tokens, text: String) {
    ui.label(egui::RichText::new(text).font(crate::theme::small()).color(t.text_dim));
}

/// A labelled row: the label at a fixed width, so the pickers under it align.
fn label_row(ui: &mut Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 20.0], egui::Label::new(egui::RichText::new(label).font(crate::theme::small()).color(t.text_dim)));
        add(ui);
    });
}

/// A picker of the options given, returning the index chosen this frame. The options are
/// catalog keys: they show translated, and the current one shows as the picker's own face.
fn dropdown(ui: &mut Ui, id: &str, selected: &str, options: &[&str]) -> Option<usize> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(crate::i18n::t(selected)).show_ui(ui, |ui| {
        for (i, opt) in options.iter().enumerate() {
            if ui.selectable_label(*opt == selected, crate::i18n::t(*opt)).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}
