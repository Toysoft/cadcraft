//! Publish lists: opening a `.ccpub` file (its drawings, ticks, order, page setup changes and output
//! settings) and saving the list as it is shown, through the engine's `publishlist`.

use serde_json::{Value, json};

use super::*;
use crate::CadApp;

/// Open a list: its drawings (open ones as they are, others from their files), its ticks, order
/// and page setup changes, and its output settings.
pub(super) fn open_list(app: &mut CadApp, st: &mut PublishState, path: &str) -> Result<(), String> {
    let r = app.run("publishlist", json!({ "action": "read", "path": path }))?;
    let entries: Vec<&Value> = r["sheets"].as_array().into_iter().flatten().collect();
    let mut next = PublishState { tab: st.tab, list: Some(path.to_string()), ..PublishState::default() };
    // The drawings in the list's order, with the sheets it names.
    let mut drawings: Vec<(String, bool, Vec<&Value>)> = Vec::new();
    for e in &entries {
        let d = e["drawing"].as_str().unwrap_or_default().to_string();
        match drawings.iter_mut().find(|(p, _, _)| *p == d) {
            Some((_, _, v)) => v.push(e),
            None => drawings.push((d, e["found"] == true, vec![e])),
        }
    }
    let open: Vec<(u64, Option<String>)> = app.session.docs.iter().map(|d| (d.uid, d.path.clone())).collect();
    for (uid, p) in &open {
        if !p.as_ref().is_some_and(|p| drawings.iter().any(|(d, _, _)| d == p)) {
            next.removed.insert(*uid);
        }
    }
    for (d, found, named) in &drawings {
        let uid = match open.iter().find(|(_, p)| p.as_deref() == Some(d.as_str())) {
            Some((uid, _)) => *uid,
            None => {
                let file =
                    if *found { app.run("publishlist", json!({ "action": "drawing", "path": d })).ok().map(|r| file_drawing(d, &r)) } else { None };
                let file = file.unwrap_or_else(|| FileDrawing {
                    path: d.clone(),
                    uid: file_uid(d),
                    title: std::path::Path::new(d).file_name().map_or_else(|| d.clone(), |n| n.to_string_lossy().to_string()),
                    sheets: named.iter().filter_map(|e| e["layout"].as_str()).map(named_sheet).collect(),
                    found: false,
                });
                let uid = file.uid;
                next.files.push(file);
                uid
            }
        };
        next.order.insert(uid, named.iter().filter_map(|e| e["layout"].as_str().map(str::to_string)).collect());
    }
    // A sheet is ticked when the list ticks it; sheets the list doesn't name are left out. A page
    // setup change in the list is applied to the sheet's own page setup.
    for g in gather(app, &next) {
        let named = drawings.iter().find(|(d, _, _)| match &g.source {
            Source::Open(i) => app.session.docs.get(*i).and_then(|ds| ds.path.as_deref()) == Some(d.as_str()),
            Source::File(p) => p == d,
        });
        for s in &g.sheets {
            let entry = named.and_then(|(_, _, es)| es.iter().find(|e| e["layout"].as_str() == Some(s.name.as_str())));
            if !entry.is_some_and(|e| e["include"] != false) {
                next.skipped.insert((g.uid, s.name.clone()));
            }
            if let Some(change) = entry.and_then(|e| e["pageSetup"].as_object()) {
                next.changes.insert((g.uid, s.name.clone()), SetupChange { from: None, page: apply_setup(s.page.clone(), change) });
            }
        }
    }
    let out = &r["output"];
    next.to = match out["to"].as_str() {
        Some("svg") => To::Svg,
        Some("png") => To::Png,
        _ => To::Pdf,
    };
    next.combine = out["combine"] == true;
    next.drop_layers = out["layers"] == false;
    next.folder = out["folder"].as_str().map(str::to_string);
    next.name = out["name"].as_str().map(str::to_string);
    if let Some(stamp) = out["stamp"].as_object() {
        next.stamp_on = true;
        let d = StampSettings::default();
        next.stamp = StampSettings {
            text: stamp.get("text").and_then(Value::as_str).map_or(d.text, str::to_string),
            corner: stamp.get("corner").and_then(Value::as_str).and_then(|c| CORNERS.iter().position(|k| k.0 == c)).unwrap_or(0),
            height: stamp.get("height").and_then(Value::as_f64).unwrap_or(d.height),
        };
    } else if out["stamp"] == true {
        next.stamp_on = true;
    }
    next.foreground = st.foreground;
    next.open_result = st.open_result;
    *st = next;
    Ok(())
}

/// Save the list as it is shown: every sheet with its tick, order and page setup change, and the
/// output settings. Returns the report line. Drawings that were never saved have no file to
/// point to and are left out.
pub(super) fn save_list(app: &mut CadApp, groups: &[Group], st: &mut PublishState, path: &str) -> String {
    let mut sheets = Vec::new();
    let mut unsaved = Vec::new();
    for g in groups {
        let drawing = match &g.source {
            Source::Open(i) => app.session.docs.get(*i).and_then(|d| d.path.clone()),
            Source::File(p) => Some(p.clone()),
        };
        let Some(drawing) = drawing else {
            unsaved.push(g.title.clone());
            continue;
        };
        for s in &g.sheets {
            let mut e = json!({ "drawing": drawing, "layout": s.name, "include": !st.skipped.contains(&(g.uid, s.name.clone())) });
            if let Some(c) = st.changes.get(&(g.uid, s.name.clone())) {
                e["pageSetup"] = setup_json(&s.page, &c.page);
            }
            sheets.push(e);
        }
    }
    let mut output = json!({ "to": st.to.id(), "combine": st.combine, "layers": !st.drop_layers });
    if let Some(folder) = st.folder.as_ref().filter(|f| !f.trim().is_empty()) {
        output["folder"] = json!(folder);
    }
    if let Some(name) = &st.name {
        output["name"] = json!(name);
    }
    if st.stamp_on {
        output["stamp"] = st.stamp.json();
    }
    match app.run("publishlist", json!({ "action": "write", "path": path, "sheets": sheets, "output": output })) {
        Ok(_) => {
            st.list = Some(path.to_string());
            let saved = crate::tf!("Saved the list to {path}", path = path);
            if unsaved.is_empty() {
                saved
            } else {
                format!("{saved} · {}", crate::tf!("Not saved yet, so left out: {names}", names = unsaved.join(", ")))
            }
        }
        Err(e) => e,
    }
}

/// `page` with a list's page setup change (as the engine's `pageSetup`, already checked) applied.
fn apply_setup(mut page: cadcraft_doc::PageSetup, change: &serde_json::Map<String, Value>) -> cadcraft_doc::PageSetup {
    let num = |k: &str| change.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0);
    if let Some(paper) = change.get("paper").and_then(Value::as_str)
        && let Some(size) = cadcraft_render::paper_size(paper)
    {
        page.paper = paper.to_string();
        page.width_mm = size.width_mm;
        page.height_mm = size.height_mm;
    }
    if let (Some(w), Some(h)) = (num("width"), num("height")) {
        page.paper = format!("User ({w:.2} x {h:.2} MM)");
        page.width_mm = w;
        page.height_mm = h;
    }
    if let Some(l) = change.get("landscape").and_then(Value::as_bool) {
        page.landscape = l;
    }
    if let Some(f) = change.get("fit").and_then(Value::as_bool) {
        page.scale_to_fit = f;
    }
    if let Some(k) = num("scale") {
        page.scale = k;
    }
    if let Some(m) = change.get("margins").and_then(Value::as_array).filter(|m| m.len() == 4) {
        for (slot, v) in page.margins_mm.iter_mut().zip(m) {
            if let Some(v) = v.as_f64().filter(|v| v.is_finite() && *v >= 0.0) {
                *slot = v;
            }
        }
    }
    if let Some(b) = change.get("lineweights").and_then(Value::as_bool) {
        page.lineweights = b;
    }
    if let Some(b) = change.get("plotTransparency").and_then(Value::as_bool) {
        page.plot_transparency = b;
    }
    if let Some(t) = change.get("plotStyleTable").and_then(Value::as_str) {
        page.plot_style_table = if cadcraft_doc::no_plot_style_table(t) { String::new() } else { t.to_string() };
    }
    page
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_saved_change_comes_back_as_the_same_page_setup() {
        let own = cadcraft_doc::PageSetup::default();
        let mut page = own.clone();
        page.paper = "A3".into();
        page.width_mm = 297.0;
        page.height_mm = 420.0;
        page.landscape = false;
        page.scale_to_fit = false;
        page.scale = 0.02;
        page.margins_mm = [5.0, 5.0, 5.0, 5.0];
        page.lineweights = !own.lineweights;
        page.plot_transparency = !own.plot_transparency;
        page.plot_style_table = "monochrome.ctb".into();
        // Saved as the engine's pageSetup, read back onto the sheet's own page setup.
        let saved = setup_json(&own, &page);
        let back = apply_setup(own.clone(), saved.as_object().unwrap());
        assert_eq!(back, page);
        // A custom paper size goes by its size; bad numbers are ignored, not applied.
        let custom = json!({"width": 500.0, "height": 700.0, "scale": -3, "margins": [1, 2]});
        let back = apply_setup(own.clone(), custom.as_object().unwrap());
        assert_eq!((back.width_mm, back.height_mm, back.scale, back.margins_mm), (500.0, 700.0, own.scale, own.margins_mm));
    }
}
