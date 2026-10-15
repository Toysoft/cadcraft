//! What the window asks the engine: the `publish` request for the ticked sheets, and the status
//! checks (`publish {"action": "status"}`) behind the Status column and the file names, run a
//! moment after the last change.

use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::*;
use crate::CadApp;

/// Most sheets one status check asks about (the engine's limit for one run).
const STATUS_BATCH: usize = 256;

/// Seconds after the last change before the Status column is checked again.
pub(super) const STATUS_DELAY: f64 = 0.25;

/// A `sheets` entry for the engine: the drawing by its index, the sheet, and a page setup
/// changed for this run.
pub(super) fn sheet_json(g: &Group, s: &Sheet, st: &PublishState) -> Value {
    let mut v = match &g.source {
        Source::Open(i) => json!({ "drawing": i, "layout": s.name }),
        Source::File(path) => json!({ "drawing": path, "layout": s.name }),
    };
    if let Some(c) = st.changes.get(&(g.uid, s.name.clone())) {
        v["pageSetup"] = setup_json(&s.page, &c.page);
    }
    v
}

/// The engine's `pageSetup` for a run that plots with `page` instead of `own`: paper (by name,
/// else its size), orientation and fit always; the rest only where they differ.
pub(super) fn setup_json(own: &cadcraft_doc::PageSetup, page: &cadcraft_doc::PageSetup) -> Value {
    let mut v = json!({ "landscape": page.landscape, "fit": page.scale_to_fit });
    if cadcraft_render::paper_size(&page.paper).is_some() {
        v["paper"] = json!(page.paper);
    } else {
        v["width"] = json!(page.width_mm);
        v["height"] = json!(page.height_mm);
    }
    if !page.scale_to_fit {
        v["scale"] = json!(page.scale);
    }
    if page.margins_mm != own.margins_mm {
        v["margins"] = json!(page.margins_mm);
    }
    if page.lineweights != own.lineweights {
        v["lineweights"] = json!(page.lineweights);
    }
    if page.plot_transparency != own.plot_transparency {
        v["plotTransparency"] = json!(page.plot_transparency);
    }
    if page.plot_style_table != own.plot_style_table {
        v["plotStyleTable"] = json!(if cadcraft_doc::no_plot_style_table(&page.plot_style_table) { "None" } else { page.plot_style_table.as_str() });
    }
    v
}

/// Combining is for PDFs.
pub(super) fn combining(st: &PublishState) -> bool {
    st.combine && st.to == To::Pdf
}

/// The publish request for the ticked sheets (without a folder: the caller adds it).
pub(super) fn publish_request(groups: &[Group], st: &PublishState) -> Value {
    let sheets: Vec<Value> = picked(groups, st).into_iter().map(|(g, s)| sheet_json(g, s, st)).collect();
    let mut r = json!({ "sheets": sheets, "to": st.to.id(), "combine": combining(st) });
    if let Some(name) = &st.name {
        r["name"] = json!(name);
    }
    if st.stamp_on {
        r["stamp"] = st.stamp.json();
    }
    if st.to == To::Pdf {
        r["pdf"] = json!({ "layers": !st.drop_layers });
    }
    r
}

/// The two status requests: every listed sheet (for the Status column), and the ticked ones as
/// they will be published (for the file names).
pub(super) fn status_requests(groups: &[Group], st: &PublishState) -> (Value, Value) {
    let all: Vec<Value> = groups.iter().flat_map(|g| g.sheets.iter().map(move |s| (g, s))).map(|(g, s)| sheet_json(g, s, st)).collect();
    let mut names = publish_request(groups, st);
    names["action"] = json!("status");
    (json!({ "action": "status", "sheets": all }), names)
}

/// A fingerprint of what the status depends on: the requests and the drawings' revisions.
pub(super) fn fingerprint(groups: &[Group], requests: &(Value, Value)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    requests.0.to_string().hash(&mut h);
    requests.1.to_string().hash(&mut h);
    for g in groups {
        (g.uid, g.revision).hash(&mut h);
    }
    h.finish()
}

/// Ask the engine about the listed sheets: a moment after the last change, so typing a folder or
/// ticking several sheets checks once. The first check runs at once.
pub(super) fn refresh_status(app: &mut CadApp, ctx: &egui::Context, groups: &[Group], st: &mut PublishState) {
    let requests = status_requests(groups, st);
    let key = fingerprint(groups, &requests);
    if st.checked.as_ref().is_some_and(|c| c.key == key) {
        st.pending = None;
        return;
    }
    let now = ctx.input(|i| i.time);
    let since = match st.pending {
        Some((k, t)) if k == key => now - t,
        _ => {
            st.pending = Some((key, now));
            0.0
        }
    };
    if st.checked.is_some() && since < STATUS_DELAY {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((STATUS_DELAY - since).max(0.01)));
        return;
    }
    let mut checked = Checked { key, ..Checked::default() };
    // Every listed sheet, in batches the engine takes in one call. These are questions, not
    // commands: they go to the engine directly, so a refusal isn't echoed to the command line;
    // a batch that can't be checked marks its sheets with the reason.
    let listed: Vec<(u64, String)> = groups.iter().flat_map(|g| g.sheets.iter().map(move |s| (g.uid, s.name.clone()))).collect();
    let entries = requests.0["sheets"].as_array().cloned().unwrap_or_default();
    for (keys, batch) in listed.chunks(STATUS_BATCH).zip(entries.chunks(STATUS_BATCH)) {
        match app.session.execute("publish", &json!({ "action": "status", "sheets": batch })) {
            Ok(r) => {
                for (key, x) in keys.iter().zip(r["sheets"].as_array().into_iter().flatten()) {
                    let status = x["status"].as_str().unwrap_or("error").to_string();
                    checked.sheets.insert(key.clone(), (status, x["reason"].as_str().unwrap_or_default().to_string()));
                }
            }
            Err(e) => {
                for key in keys {
                    checked.sheets.insert(key.clone(), ("error".into(), e.to_string()));
                }
            }
        }
    }
    if !picked(groups, st).is_empty()
        && let Ok(r) = app.session.execute("publish", &requests.1)
    {
        checked.files = match r["combined"]["file"].as_str() {
            Some(f) => vec![f.to_string()],
            None => r["sheets"].as_array().into_iter().flatten().filter_map(|x| x["file"].as_str().map(str::to_string)).collect(),
        };
    }
    st.checked = Some(checked);
    st.pending = None;
}

/// How many ticked sheets the last check says will fail.
pub(super) fn will_fail(groups: &[Group], st: &PublishState) -> usize {
    let Some(c) = &st.checked else { return 0 };
    picked(groups, st).iter().filter(|(g, s)| c.sheets.get(&(g.uid, s.name.clone())).is_some_and(|(k, _)| k == "error")).count()
}
