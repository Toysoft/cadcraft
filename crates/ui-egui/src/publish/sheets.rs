//! The sheets the window lists: those of the open drawings and of drawing files added without
//! opening them (Add Drawings...), in the order the user chose, and which of them are ticked.

use serde_json::{Value, json};

use super::*;
use crate::CadApp;

pub(super) fn ratio(k: f64) -> String {
    if !k.is_finite() || k <= 0.0 {
        return "—".into();
    }
    let num = |v: f64| if (v - v.round()).abs() < 1e-6 { format!("{}", v.round()) } else { format!("{v:.2}") };
    if k < 1.0 { format!("1:{}", num(1.0 / k)) } else { format!("{}:1", num(k)) }
}

/// Model space, plotted with `page` (fitted to its paper unless the run changes that).
pub(super) fn model_sheet(page: cadcraft_doc::PageSetup) -> Sheet {
    Sheet {
        name: "Model".into(),
        model: true,
        page,
        setup: "Default".into(),
        device: "PDF".into(),
        size: String::new(),
        scale: "Fit".into(),
        landscape: true,
        default_setup: true,
    }
}

pub(super) fn layout_sheet(name: &str, p: &cadcraft_doc::PageSetup) -> Sheet {
    let default_setup = *p == cadcraft_doc::PageSetup::default();
    let (w, h) = if p.landscape {
        (p.height_mm.max(p.width_mm), p.height_mm.min(p.width_mm))
    } else {
        (p.width_mm.min(p.height_mm), p.width_mm.max(p.height_mm))
    };
    let device = if p.device.is_empty() || p.device.eq_ignore_ascii_case("none") { "PDF".to_string() } else { p.device.clone() };
    Sheet {
        name: name.to_string(),
        model: false,
        page: p.clone(),
        setup: if default_setup {
            "Default".into()
        } else if device == "PDF" {
            p.paper.clone()
        } else {
            device.clone()
        },
        device,
        size: if w.is_finite() && h.is_finite() && w > 0.0 { format!("{w:.0} × {h:.0} mm") } else { String::new() },
        scale: if p.scale_to_fit { "Fit".into() } else { ratio(p.scale) },
        landscape: p.landscape,
        default_setup,
    }
}

/// A sheet a list names in a drawing that is gone: only its name is known.
pub(super) fn named_sheet(name: &str) -> Sheet {
    if name.eq_ignore_ascii_case("model") {
        return model_sheet(cadcraft_doc::PageSetup::default());
    }
    Sheet {
        name: name.to_string(),
        model: false,
        page: cadcraft_doc::PageSetup::default(),
        setup: "—".into(),
        device: String::new(),
        size: String::new(),
        scale: String::new(),
        landscape: true,
        default_setup: false,
    }
}

pub(super) fn sheets_of(d: &cadcraft_doc::Drawing) -> Vec<Sheet> {
    let mut out = Vec::new();
    if !d.model.is_empty() {
        out.push(model_sheet(crate::managers::sheet_page(d, "Model")));
    }
    let mut layouts: Vec<_> = d.layouts.iter().collect();
    layouts.sort_by_key(|l| l.tab_order);
    out.extend(layouts.into_iter().map(|l| Sheet { page: crate::managers::sheet_page(d, &l.name), ..layout_sheet(&l.name, &l.page) }));
    out
}

/// A drawing file's sheets as the engine's `publishlist {"action": "drawing"}` lists them.
pub(super) fn file_drawing(path: &str, r: &Value) -> FileDrawing {
    let sheets = r["sheets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let name = x["layout"].as_str()?;
            if x["model"] == true {
                // The file isn't open, so model space starts from the default page setup.
                return Some(model_sheet(cadcraft_doc::PageSetup::default()));
            }
            let page: cadcraft_doc::PageSetup = serde_json::from_value(x["page"].clone()).unwrap_or_default();
            Some(layout_sheet(name, &page))
        })
        .collect();
    let title = r["title"].as_str().map_or_else(|| path.to_string(), str::to_string);
    FileDrawing { path: path.to_string(), uid: file_uid(path), title, sheets, found: true }
}

pub(super) fn stem_of(name: &str) -> String {
    std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Drawing".into())
}

pub(super) fn gather(app: &CadApp, st: &PublishState) -> Vec<Group> {
    let active = app.session.active;
    let ordered = |uid: u64, mut sheets: Vec<Sheet>| {
        if let Some(order) = st.order.get(&uid) {
            sheets.sort_by_key(|s| order.iter().position(|n| *n == s.name).unwrap_or(usize::MAX));
        }
        sheets
    };
    let mut groups = Vec::new();
    for (index, ds) in app.session.docs.iter().enumerate() {
        if st.removed.contains(&ds.uid) || (st.only_current && index != active) {
            continue;
        }
        let folder = ds.path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        groups.push(Group {
            source: Source::Open(index),
            uid: ds.uid,
            found: true,
            revision: ds.revision,
            title: ds.title.clone(),
            stem: stem_of(&ds.title),
            folder,
            current: index == active,
            sheets: ordered(ds.uid, sheets_of(&ds.doc)),
        });
    }
    for f in &st.files {
        let folder = std::path::Path::new(&f.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        groups.push(Group {
            source: Source::File(f.path.clone()),
            uid: f.uid,
            found: f.found,
            revision: 0,
            title: f.title.clone(),
            stem: stem_of(&f.title),
            folder,
            current: false,
            sheets: ordered(f.uid, f.sheets.clone()),
        });
    }
    groups
}

/// Add a drawing file's sheets to the list without opening it. A drawing that is already open
/// comes back into the list as the open drawing.
pub(super) fn add_drawing(app: &mut CadApp, st: &mut PublishState, path: &str) -> Result<(), String> {
    if let Some(ds) = app.session.docs.iter().find(|d| d.path.as_deref() == Some(path)) {
        st.removed.remove(&ds.uid);
        return Ok(());
    }
    if st.files.iter().any(|f| f.path == path && f.found) {
        return Ok(());
    }
    let r = app.run("publishlist", json!({ "action": "drawing", "path": path }))?;
    st.files.retain(|f| f.path != path);
    st.files.push(file_drawing(path, &r));
    Ok(())
}

/// The (group, sheet) pairs that will be published, in order.
pub(super) fn picked<'a>(groups: &'a [Group], st: &PublishState) -> Vec<(&'a Group, &'a Sheet)> {
    groups.iter().flat_map(|g| g.sheets.iter().filter(|s| !st.skipped.contains(&(g.uid, s.name.clone()))).map(move |s| (g, s))).collect()
}

pub(super) fn default_folder(groups: &[Group], st: &PublishState) -> String {
    st.folder.clone().filter(|f| !f.trim().is_empty()).or_else(|| groups.iter().map(|g| g.folder.clone()).find(|f| !f.is_empty())).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_read_like_drawing_scales() {
        assert_eq!(ratio(0.02), "1:50");
        assert_eq!(ratio(2.0), "2:1");
        assert_eq!(ratio(1.0), "1:1");
        assert_eq!(ratio(0.0), "—");
        assert_eq!(ratio(f64::NAN), "—");
    }
}
