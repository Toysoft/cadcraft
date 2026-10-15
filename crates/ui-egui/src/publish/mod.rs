//! Publish Sheets: choose sheets from the open drawings, inspect them, choose where the PDFs go,
//! and publish them. A typed or menu-invoked PUBLISH opens this window; JSON calls (scripts, the
//! control channel, MCP) never get here and run the engine's `publish` directly.
//!
//! Publishing is one call to the engine's `publish` with the ticked sheets, so the window and
//! scripts produce the same files and names. The Status column and the file names come from
//! `publish {"action": "status"}`, refreshed a moment after the list or the options change.
//! Add Drawings... lists a drawing file's sheets without opening it, and a list can be saved to and
//! opened from a `.ccpub` file (the engine's `publishlist`). Options the engine can't honour yet
//! are shown disabled with "Not available yet".
//!
//! This file holds the window's state and its frame; the parts are [`sheets`] (what is listed),
//! [`sheet_list`] (the list), [`inspector`] (the Sheet and Output tabs and the footer),
//! [`status`] (requests and the status check), [`lists`] (`.ccpub` files) and [`stamp`].

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use egui::{Align, Align2, Layout, Ui, vec2};
use serde_json::json;

use crate::CadApp;

mod inspector;
mod lists;
mod sheet_list;
mod sheets;
mod stamp;
mod status;

use inspector::*;
use lists::*;
use sheet_list::*;
use sheets::*;
use stamp::*;
use status::*;

const INSPECTOR: f32 = 340.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Sheet,
    Output,
}

/// What the files are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum To {
    #[default]
    Pdf,
    Svg,
    Png,
}

impl To {
    fn id(self) -> &'static str {
        match self {
            To::Pdf => "pdf",
            To::Svg => "svg",
            To::Png => "png",
        }
    }
}

/// A sheet's page setup changed for one run (the drawing keeps its own).
#[derive(Clone, Debug, PartialEq)]
struct SetupChange {
    /// The layout whose page setup was copied; `None` when picked or edited here.
    from: Option<String>,
    /// The page setup the run plots with (`paper` may be a short name such as `A3`).
    page: cadcraft_doc::PageSetup,
}

/// What `publish {"action": "status"}` said, for the request it was asked about.
#[derive(Clone, Debug, Default)]
struct Checked {
    /// The request's fingerprint (see [`fingerprint`]).
    key: u64,
    /// Every listed sheet: (status, reason).
    sheets: BTreeMap<(u64, String), (String, String)>,
    /// The ticked sheets' file names, in order (one name when combining).
    files: Vec<String>,
}

/// The window's state between frames (egui temp data, not saved with the workspace).
#[derive(Clone, Debug, Default)]
pub struct PublishState {
    /// Sheets left out, by (drawing uid, sheet name).
    skipped: BTreeSet<(u64, String)>,
    /// Drawings taken out of the list (they stay open).
    removed: BTreeSet<u64>,
    /// Sheet order per drawing when the user moved sheets.
    order: BTreeMap<u64, Vec<String>>,
    inspected: Option<(u64, String)>,
    tab: Tab,
    only_current: bool,
    combine: bool,
    drop_layers: bool,
    ask_name: bool,
    folder: Option<String>,
    report: Option<String>,
    to: To,
    /// Page setups changed for this run, by (drawing uid, sheet name).
    changes: BTreeMap<(u64, String), SetupChange>,
    checked: Option<Checked>,
    /// A change waiting to be checked: (fingerprint, when it was seen).
    pending: Option<(u64, f64)>,
    /// Drawings listed from files (Add Drawings..., or a list), in the order they were added.
    files: Vec<FileDrawing>,
    /// The list file this list was opened from or saved to.
    list: Option<String>,
    /// A file name pattern from a list (the window has no field for it yet).
    name: Option<String>,
    /// Publish now, in the window, instead of on a worker thread.
    foreground: bool,
    /// Open the PDF (or the folder) when the run ends.
    open_result: bool,
    /// The sheet stamp: whether it's added, and its settings.
    stamp_on: bool,
    stamp: StampSettings,
    /// The Sheet Stamp window is open.
    editing_stamp: bool,
}

/// A drawing listed from its file: read for publishing, never opened as a tab.
#[derive(Clone, Debug)]
struct FileDrawing {
    path: String,
    uid: u64,
    title: String,
    sheets: Vec<Sheet>,
    /// False when the file is gone; its sheets are then the ones the list named.
    found: bool,
}

/// Where a group's drawing comes from.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// An open drawing, by its index in the session.
    Open(usize),
    /// A drawing file, by its path.
    File(String),
}

/// A listed file's id: its path's hash with the top bit set, so it never meets an open
/// drawing's uid (those count up from 1).
fn file_uid(path: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish() | (1 << 63)
}

/// One sheet as the list shows it.
#[derive(Clone, Debug)]
struct Sheet {
    name: String,
    model: bool,
    /// The page setup the sheet plots with unless the run changes it.
    page: cadcraft_doc::PageSetup,
    setup: String,
    device: String,
    size: String,
    scale: String,
    landscape: bool,
    default_setup: bool,
}

/// One open drawing and its sheets, in publishing order.
#[derive(Clone, Debug)]
struct Group {
    source: Source,
    uid: u64,
    /// False for a listed file that is gone.
    found: bool,
    /// The drawing's revision: the Status column is checked again when it changes.
    revision: u64,
    title: String,
    stem: String,
    folder: String,
    current: bool,
    sheets: Vec<Sheet>,
}

#[derive(Default)]
struct Actions {
    /// In: a run is in progress; the host can open files.
    busy: bool,
    can_open: bool,
    /// Keep a sheet's run page setup in its layout: (drawing index, uid, layout, page).
    save: Option<(usize, u64, String, cadcraft_doc::PageSetup)>,
    publish: bool,
    add: bool,
    open_list: bool,
    save_list: bool,
    choose: bool,
    help: bool,
    close: bool,
}

/// Show the window (dialog name `"publish"`).
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("cc_publish_state");
    let mut st: PublishState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let groups = gather(app, &st);
    let valid = |key: &(u64, String)| groups.iter().any(|g| g.uid == key.0 && g.sheets.iter().any(|s| s.name == key.1));
    if !st.inspected.as_ref().is_some_and(valid) {
        st.inspected = picked(&groups, &st)
            .first()
            .copied()
            .or_else(|| groups.first().and_then(|g| g.sheets.first().map(|s| (g, s))))
            .map(|(g, s)| (g.uid, s.name.clone()));
    }
    refresh_status(app, ctx, &groups, &mut st);
    let mut act = Actions { busy: app.publishing.busy(), can_open: app.services.open_path.is_some(), ..Actions::default() };
    let screen = ctx.content_rect();
    let center = screen.center();
    // Fit the screen: the window's frame and title bar take about 60 points beyond its content.
    let room = (screen.size() - vec2(40.0, 90.0)).max(vec2(480.0, 360.0));
    let size = vec2(1080.0, 640.0).min(room);
    egui::Window::new(crate::tl!("Publish Sheets"))
        .id(egui::Id::new("cc_publish"))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size(size)
        .min_size(vec2(860.0, 500.0).min(size))
        .max_size(room)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(center)
        .show(ctx, |ui| body(ui, &app.session, &groups, &mut st, &mut act));
    if act.close {
        *open = false;
    }
    if act.help {
        app.ui.dialog = Some("commands".into());
    }
    if act.add
        && let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick())
        && let Err(e) = add_drawing(app, &mut st, &path)
    {
        st.report = Some(e);
    }
    if act.open_list
        && let Some(path) = app.services.pick_list.as_ref().or(app.services.pick_open.as_ref()).and_then(|pick| pick())
    {
        st.report = Some(match open_list(app, &mut st, &path) {
            Ok(()) => crate::tf!("Opened the list {path}", path = path),
            Err(e) => e,
        });
    }
    if act.save_list {
        let suggested = st
            .list
            .as_deref()
            .and_then(|l| std::path::Path::new(l).file_name())
            .map_or_else(|| format!("{}.ccpub", groups.first().map_or("Sheets", |g| g.stem.as_str())), |n| n.to_string_lossy().to_string());
        if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick(&suggested)) {
            let path = if path.to_ascii_lowercase().ends_with(".ccpub") { path } else { format!("{path}.ccpub") };
            let report = save_list(app, &groups, &mut st, &path);
            st.report = Some(report);
        }
    }
    if act.choose {
        let stem = groups.first().map_or_else(|| "Drawing".to_string(), |g| g.stem.clone());
        if let Some(pick) = app.services.pick_save.as_ref()
            && let Some(path) = pick(&format!("{stem}.{}", st.to.id()))
            && let Some(dir) = std::path::Path::new(&path).parent()
        {
            st.folder = Some(dir.to_string_lossy().to_string());
        }
    }
    if let Some((index, uid, layout, page)) = act.save.take() {
        let previous = app.session.active;
        app.session.active = index;
        let r = app.run("pagesetup", crate::managers::store_params(&layout, &page));
        app.session.active = previous.min(app.session.docs.len().saturating_sub(1));
        st.report = Some(match r {
            Ok(_) => {
                st.changes.remove(&(uid, layout.clone()));
                crate::tf!("Saved the page setup in {layout}", layout = layout)
            }
            Err(e) => e,
        });
    }
    if act.publish {
        match run(app, &groups, &st) {
            // In the background the window closes; the report opens when the run ends.
            Ok(true) => *open = false,
            Ok(false) => {}
            Err(report) => {
                app.set_status(report.clone());
                st.report = Some(report);
            }
        }
        // The files exist now; check again so the names and statuses are current.
        st.checked = None;
    }
    if st.editing_stamp {
        stamp_window(ctx, &mut st);
    }
    if !*open {
        cadcraft_engine::cmd::clear_publish_cache();
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

/// Publish the ticked sheets with one engine `publish` call: on a worker thread, or now when
/// "Run in the background" is off. `Ok(true)` when it runs in the background; `Err` says why it
/// didn't start.
fn run(app: &mut CadApp, groups: &[Group], st: &PublishState) -> Result<bool, String> {
    let sheets = picked(groups, st);
    let Some((first, first_sheet)) = sheets.first() else {
        return Err(crate::tl!("Nothing selected").to_string());
    };
    let mut request = publish_request(groups, st);
    let mut folder = default_folder(groups, st);
    if st.ask_name || folder.is_empty() {
        let suggested = st
            .checked
            .as_ref()
            .and_then(|c| c.files.first().cloned())
            .unwrap_or_else(|| format!("{}-{}.{}", first.stem, first_sheet.name, st.to.id()));
        let Some(pick) = app.services.pick_save.as_ref() else {
            return Err(crate::tl!("Choose a folder to publish to.").to_string());
        };
        let Some(path) = pick(&suggested) else { return Err(crate::tl!("Publishing cancelled.").to_string()) };
        let p = std::path::Path::new(&path);
        folder = p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        // A combined file takes the name given; braces in it stay braces.
        if combining(st)
            && let Some(stem) = p.file_stem().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty())
        {
            request["name"] = json!(stem.replace('{', "{{").replace('}', "}}"));
        }
    }
    // Without a folder (the web build's picker gives only a name), the engine returns each file
    // and the report hands it to the browser as a download.
    if !folder.trim().is_empty() {
        request["folder"] = json!(folder);
    }
    let background = !st.foreground && !cfg!(target_arch = "wasm32");
    crate::publish_run::start(app, request, background, st.open_result)?;
    Ok(background)
}

fn body(ui: &mut Ui, session: &cadcraft_engine::Session, groups: &[Group], st: &mut PublishState, act: &mut Actions) {
    let size = ui.available_size();
    let footer = 34.0;
    let main_h = (size.y - footer - 14.0).max(240.0);
    // The separator between the columns takes its own width plus the item spacing on each side;
    // counting it keeps the content inside the window, which would otherwise grow every frame.
    let gap = 2.0 * ui.spacing().item_spacing.x + 8.0;
    let list_w = (size.x - INSPECTOR - gap).max(320.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(list_w, main_h), Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(vec2(list_w, main_h));
            list_column(ui, groups, st, act, main_h);
        });
        ui.separator();
        ui.allocate_ui_with_layout(vec2(INSPECTOR, main_h), Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(vec2(INSPECTOR, main_h));
            inspector(ui, session, groups, st, act, main_h);
        });
    });
    ui.separator();
    footer_row(ui, groups, st, act);
}
