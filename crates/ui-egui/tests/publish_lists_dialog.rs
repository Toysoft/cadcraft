//! Publish Sheets with drawing files and list files: Add Drawings... lists a file's sheets without
//! opening it, a list saved with Save List As... opens again with its drawings and output settings
//! (the engine's tests cover ticks and page setup changes), and a drawing that is gone shows "File
//! not found".

use std::cell::RefCell;
use std::rc::Rc;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io, set_plot_book};
use cadcraft_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use serde_json::{Value, json};

/// `*.sheets` files are a metric drawing with something in model space and on Layout1.
fn read(bytes: &[u8], name: &str) -> Result<cadcraft_doc::Drawing, String> {
    if !(name.ends_with(".sheets") && bytes.starts_with(b"sheets")) {
        return Err("not a drawing".into());
    }
    let mut d = cadcraft_doc::Drawing::new_metric();
    for space in [cadcraft_doc::Space::Model, cadcraft_doc::Space::Paper("Layout1".into())] {
        let line = cadcraft_doc::EntityKind::Line(cadcraft_doc::Line { a: cadcraft_geom::Vec3::ZERO, b: cadcraft_geom::Vec3::new(10.0, 10.0, 0.0) });
        let _ = d.add(&space, cadcraft_doc::Common::default(), line);
    }
    Ok(d)
}

fn echo_plot(_: &cadcraft_doc::Drawing, space: &cadcraft_doc::Space, opts: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 {space:?} {opts}").into_bytes())
}

fn echo_book(parts: &[(&cadcraft_doc::Drawing, cadcraft_doc::Space, Value)], _: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 book {}", parts.len()).into_bytes())
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-publish-lists-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// What the file pickers answer next.
#[derive(Clone, Default)]
struct Picks {
    open: Rc<RefCell<Option<String>>>,
    list: Rc<RefCell<Option<String>>>,
    save: Rc<RefCell<Option<String>>>,
}

fn harness(picks: &Picks) -> Harness<'static, CadApp> {
    set_io(IoHooks { read, write: |_, _| Err("no writer in this test".into()), plot: Some(echo_plot) });
    set_plot_book(echo_book);
    let (o, l, s) = (picks.open.clone(), picks.list.clone(), picks.save.clone());
    let services = Services {
        pick_open: Some(Box::new(move || o.borrow().clone())),
        pick_list: Some(Box::new(move || l.borrow().clone())),
        pick_save: Some(Box::new(move |_: &str| s.borrow().clone())),
        open_path: None,
    };
    let mut app = CadApp::new(Session::default(), services);
    app.run("line", json!({ "points": [[0, 0], [100, 0]] })).unwrap();
    app.ui.start_tab = false;
    app.ui.in_window_menu = false;
    app.cmdline("PUBLISH");
    let mut h = Harness::builder().with_size(vec2(1280.0, 820.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn save(h: &mut Harness<'static, CadApp>, name: &str) {
    if let Ok(directory) = std::env::var("CRAFT_UI_VISUAL_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(format!("{directory}/{name}.png")).unwrap();
    }
}

fn menu(h: &mut Harness<'static, CadApp>, item: &str) {
    h.get(By::new().label_contains("⏷")).click();
    h.run_steps(3);
    h.get_by_label(item).click();
    h.run_steps(6);
}

/// The status of a row in the list (its accessible value).
fn row_status(h: &Harness<'static, CadApp>, sheet: &str) -> Vec<String> {
    h.get_all(By::new().label(sheet).predicate(|n| n.value().is_some())).map(|n| n.accesskit_node().value().unwrap_or_default()).collect()
}

#[test]
fn add_drawings_lists_a_file_without_opening_it() {
    let dir = temp("add");
    let file = dir.join("site plan.sheets");
    std::fs::write(&file, b"sheets").unwrap();
    let picks = Picks::default();
    *picks.open.borrow_mut() = Some(file.to_string_lossy().to_string());
    let mut h = harness(&picks);
    h.get_by_label("Add Drawings...").click();
    h.run_steps(8);
    save(&mut h, "publish-add-drawing");
    h.get_by_label("site plan.sheets");
    assert_eq!(h.state().session.docs.len(), 1, "no tab was opened");
    // Drawing1's Model, Layout1 (empty), Layout2 (empty), then the file's three sheets.
    assert_eq!(row_status(&h, "Model").len(), 2);
    h.get_by_label("6 files · 3 will fail");
    // Publishing reads the file for the run.
    h.get_by_label("Publish 6 Sheets").click();
    for _ in 0..500 {
        h.run_steps(1);
        if !h.state().publishing.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // The file's folder is the default output folder (Drawing1 has none).
    assert!(dir.join("site plan-Layout1.pdf").exists());
    // Adding it again doesn't list it twice.
    h.state_mut().cmdline("PUBLISH");
    h.run_steps(4);
    h.get_by_label("Add Drawings...").click();
    h.run_steps(6);
    assert_eq!(h.get_all_by_label("site plan.sheets").count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_saved_list_opens_again_with_its_drawings_and_output() {
    let dir = temp("roundtrip");
    let file = dir.join("plans").join("tower.sheets");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"sheets").unwrap();
    let picks = Picks::default();
    *picks.open.borrow_mut() = Some(file.to_string_lossy().to_string());
    let mut h = harness(&picks);
    h.get_by_label("Add Drawings...").click();
    h.run_steps(6);
    // Drawing1 was never saved, so a list can't point to it: it's left out and the report says so.
    h.get_by_label("Output").click();
    h.run_steps(3);
    h.get_by_label("SVG").click();
    h.run_steps(3);
    let list = dir.join("set.ccpub").to_string_lossy().to_string();
    *picks.save.borrow_mut() = Some(dir.join("set").to_string_lossy().to_string());
    menu(&mut h, "Save List As...");
    let saved = std::fs::read_to_string(&list).expect("`.ccpub` is added to the name");
    let v: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["output"]["to"], "svg");
    let drawings: Vec<&str> = v["sheets"].as_array().unwrap().iter().map(|x| x["drawing"].as_str().unwrap()).collect();
    assert!(drawings.iter().all(|d| *d == "plans/tower.sheets"), "relative to the list, and the unsaved drawing left out: {drawings:?}");
    h.get_by_label_contains("Not saved yet, so left out: Drawing1");
    h.get(By::new().label_contains("set.ccpub ⏷"));
    // A new list is every open drawing again; opening the saved one brings the file back.
    menu(&mut h, "New List");
    assert_eq!(h.query_all_by_label("tower.sheets").count(), 0);
    *picks.list.borrow_mut() = Some(list.clone());
    menu(&mut h, "Open List...");
    save(&mut h, "publish-opened-list");
    h.get_by_label("tower.sheets");
    assert_eq!(h.query_all(By::new().label("Drawing1").predicate(|n| n.value().is_some())).count(), 0, "drawings the list doesn't name are left out");
    h.get(By::new().label_contains("set.ccpub ⏷"));
    h.get_by_label("Output").click();
    h.run_steps(3);
    h.get_by_label("Vector SVG, one file per sheet.");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_drawing_that_is_gone_shows_file_not_found() {
    let dir = temp("gone");
    let file = dir.join("old.sheets");
    std::fs::write(&file, b"sheets").unwrap();
    let list = dir.join("l.ccpub");
    let text = json!({"cadcraft": "publish-list", "version": 1, "sheets": [
        {"drawing": "old.sheets", "layout": "Layout1"},
        {"drawing": "deleted.sheets", "layout": "Layout1"},
        {"drawing": "deleted.sheets", "layout": "Model", "include": false},
    ]});
    std::fs::write(&list, text.to_string()).unwrap();
    let picks = Picks::default();
    *picks.list.borrow_mut() = Some(list.to_string_lossy().to_string());
    let mut h = harness(&picks);
    menu(&mut h, "Open List...");
    h.run_steps(6);
    save(&mut h, "publish-file-not-found");
    let group = h.get_by_label("deleted.sheets");
    assert_eq!(group.accesskit_node().value().as_deref(), Some("File not found"), "the group says so");
    let gone = h.get_all(By::new().value("File not found")).count();
    assert_eq!(gone, 3, "the group and both of its sheets");
    // Only the ticked ones count: old.sheets' Layout1 and the missing Layout1.
    h.get_by_label("2 files · 1 will fail");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_list_is_reported_and_the_list_is_kept() {
    let dir = temp("bad");
    let list = dir.join("bad.ccpub");
    std::fs::write(&list, b"{\"cadcraft\": \"publish-list\", \"version\": 99, \"sheets\": []}").unwrap();
    let picks = Picks::default();
    *picks.list.borrow_mut() = Some(list.to_string_lossy().to_string());
    let mut h = harness(&picks);
    menu(&mut h, "Open List...");
    h.get_by_label_contains("made by a newer CADCraft");
    // The list is as it was: Drawing1's three sheets.
    h.get_by_label("Publish 3 Sheets");
    let _ = std::fs::remove_dir_all(dir);
}
