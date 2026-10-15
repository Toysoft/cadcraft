//! Publish Sheets: PUBLISH typed or chosen from the menu opens the window; its list, tabs and
//! summary follow the open drawings, the Status column and file names come from the engine's
//! `publish {"action": "status"}`, and Publish is one `publish` call.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io, set_plot_book};
use cadcraft_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use serde_json::{Value, json};

/// A plotter that writes what it was asked to plot.
fn echo_plot(_: &cadcraft_doc::Drawing, space: &cadcraft_doc::Space, opts: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 {space:?} {opts}").into_bytes())
}

fn echo_book(parts: &[(&cadcraft_doc::Drawing, cadcraft_doc::Space, Value)], _: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 book {}", parts.len()).into_bytes())
}

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("cadcraft-publish-dialog-{}-{name}", std::process::id()))
}

/// A drawing with something in model space and on both layouts, so its three sheets publish.
fn app(save_to: Option<std::path::PathBuf>) -> CadApp {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Err("no writer in this test".into()), plot: Some(echo_plot) });
    set_plot_book(echo_book);
    let services = Services {
        pick_save: save_to.map(|dir| {
            let pick: Box<dyn Fn(&str) -> Option<String>> = Box::new(move |name: &str| Some(dir.join(name).to_string_lossy().to_string()));
            pick
        }),
        ..Services::default()
    };
    let mut app = CadApp::new(Session::default(), services);
    for layout in ["Layout1", "Layout2"] {
        app.run("layout.set", json!({ "name": layout })).unwrap();
        app.run("line", json!({ "points": [[0, 0], [10, 10]] })).unwrap();
    }
    app.run("layout.set", json!({ "name": "Model" })).unwrap();
    app.run("line", json!({ "points": [[0, 0], [100, 0]] })).unwrap();
    app.ui.start_tab = false;
    app.ui.in_window_menu = false;
    app
}

fn harness_for(mut app: CadApp, width: f32, height: f32, scale: f32) -> Harness<'static, CadApp> {
    app.cmdline("PUBLISH");
    assert_eq!(app.ui.dialog.as_deref(), Some("publish"));
    let mut h = Harness::builder().with_size(vec2(width, height)).with_pixels_per_point(scale).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn harness(width: f32, height: f32, scale: f32) -> Harness<'static, CadApp> {
    harness_for(app(None), width, height, scale)
}

fn save(h: &mut Harness<'static, CadApp>, name: &str) {
    if let Ok(directory) = std::env::var("CRAFT_UI_VISUAL_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(format!("{directory}/{name}.png")).unwrap();
    }
}

/// Let a change be checked (the Status column waits a moment after the last change).
fn settle(h: &mut Harness<'static, CadApp>) {
    h.run_steps(4);
}

/// Wait for a background run to end (the worker thread writes the files).
fn wait_for_publish(h: &mut Harness<'static, CadApp>) {
    for _ in 0..500 {
        h.run_steps(1);
        if !h.state().publishing.busy() {
            h.run_steps(2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("publishing didn't finish");
}

fn window_width(h: &Harness<'static, CadApp>) -> f32 {
    h.ctx.memory(|m| m.area_rect(egui::Id::new("cc_publish"))).map_or(0.0, |r| r.width())
}

#[test]
fn publish_lists_every_sheet_of_the_open_drawing() {
    let mut h = harness(1280.0, 820.0, 1.0);
    save(&mut h, "publish-sheet-tab");
    // The status bar's layout tabs carry the same names, so each appears at least twice.
    for sheet in ["Model", "Layout1", "Layout2"] {
        assert!(h.get_all_by_label(sheet).count() >= 2, "{sheet} is listed");
    }
    // The engine's status: model space has no page setup, the layouts use the default one.
    assert_eq!(h.get_all(By::new().value("Uses default")).count(), 3);
    h.get_by_label("3 files");
    h.get_by_label("Publish 3 Sheets");
}

#[test]
fn none_clears_the_selection_and_disables_publish() {
    let mut h = harness(1280.0, 820.0, 1.0);
    h.get_by_label("None").click();
    h.run_steps(3);
    h.get_by_label("Nothing selected");
    assert!(h.get_by_label("Publish").accesskit_node().is_disabled());
    h.get_by_label("All").click();
    h.run_steps(3);
    h.get_by_label("Publish 3 Sheets");
}

#[test]
fn an_empty_layout_will_fail_and_the_footer_says_so() {
    let mut app = app(None);
    // A new drawing's layouts have nothing on them.
    app.run("new", json!({})).unwrap();
    app.run("line", json!({ "points": [[0, 0], [5, 5]] })).unwrap();
    let mut h = harness_for(app, 1280.0, 820.0, 1.0);
    settle(&mut h);
    save(&mut h, "publish-will-fail");
    let failing: Vec<String> = h.get_all(By::new().value("Will fail")).map(|n| n.accesskit_node().label().unwrap_or_default()).collect();
    assert_eq!(failing, ["Layout1", "Layout2"], "Drawing2's empty layouts");
    let reason = h.get_all(By::new().value("Will fail")).next().and_then(|n| n.accesskit_node().description());
    assert!(reason.is_some_and(|r| r.contains("is empty")), "the reason is the row's description");
    h.get_by_label("6 files · 2 will fail");
}

#[test]
fn combining_works_across_drawings_and_names_come_from_the_engine() {
    let mut app = app(None);
    app.run("new", json!({})).unwrap();
    app.run("line", json!({ "points": [[0, 0], [5, 5]] })).unwrap();
    let mut h = harness_for(app, 1280.0, 820.0, 1.0);
    h.get_by_label("Output").click();
    h.run_steps(3);
    h.get_by_label("Combine into one").click();
    settle(&mut h);
    save(&mut h, "publish-output-tab");
    // Six sheets from two drawings; Drawing2's two layouts are empty, so the PDF has four pages.
    h.get_by_label("1 PDF with 4 pages · 2 will fail");
    h.get_by_label("Drawing1.pdf (4 pages)");
}

#[test]
fn svg_and_png_are_files_per_sheet() {
    let mut h = harness(1280.0, 820.0, 1.0);
    h.get_by_label("Output").click();
    h.run_steps(3);
    h.get_by_label("SVG").click();
    settle(&mut h);
    h.get_by_label("Vector SVG, one file per sheet.");
    h.get_by_label("Only PDFs can be combined.");
    h.get_by_label("Drawing1-Model.svg");
    h.get_by_label("PNG").click();
    settle(&mut h);
    h.get_by_label("Drawing1-Layout2.png");
}

#[test]
fn publish_is_one_engine_call_and_writes_every_sheet() {
    let dir = temp("run");
    let _ = std::fs::remove_dir_all(&dir);
    let mut h = harness_for(app(Some(dir.clone())), 1280.0, 820.0, 1.0);
    let previous = h.state().session.active;
    h.get_by_label("Publish 3 Sheets").click();
    wait_for_publish(&mut h);
    // The unsaved drawing has no folder: the save picker gave one, and the engine made it.
    let mut files: Vec<String> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    files.sort();
    assert_eq!(files, ["Drawing1-Layout1.pdf", "Drawing1-Layout2.pdf", "Drawing1-Model.pdf"]);
    assert_eq!(h.state().session.active, previous, "the current drawing stays current");
    // The report lists every sheet.
    h.get_by_label("Publish Report");
    h.get_by_label("Published 3 of 3 sheets");
    assert_eq!(h.get_all_by_label("Published").count(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_page_setup_change_is_for_this_run_only() {
    let dir = temp("change");
    let _ = std::fs::remove_dir_all(&dir);
    let mut h = harness_for(app(Some(dir.clone())), 1280.0, 820.0, 1.0);
    let before = h.state().session.doc().unwrap().layout("Layout1").unwrap().page.clone();
    // Inspect Layout1 and pick A3 for this run.
    // The list's row, not the status bar's layout tab of the same name.
    h.get(By::new().label("Layout1").predicate(|n| n.value().is_some())).click();
    h.run_steps(3);
    h.get(By::new().value("As saved (Default)")).click();
    h.run_steps(3);
    h.get_by_label("A3").click();
    settle(&mut h);
    save(&mut h, "publish-change-for-run");
    h.get(By::new().value("Changed for this run"));
    h.get_by_label("A3, landscape");
    h.get_by_label("Publish 3 Sheets").click();
    wait_for_publish(&mut h);
    let plotted = std::fs::read_to_string(dir.join("Drawing1-Layout1.pdf")).unwrap();
    assert!(plotted.contains(r#""paper":"A3""#), "the plotter got A3: {plotted}");
    assert_eq!(h.state().session.doc().unwrap().layout("Layout1").unwrap().page, before, "the drawing keeps its page setup");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_window_keeps_its_width() {
    let mut h = harness(1280.0, 820.0, 1.0);
    let width = window_width(&h);
    assert!(width > 0.0);
    for tab in ["Output", "Sheet", "Output", "Sheet"] {
        h.get_by_label(tab).click();
        h.run_steps(3);
        assert!((window_width(&h) - width).abs() < 1.0, "{tab}: {} then {width}", window_width(&h));
    }
}

#[test]
fn the_window_fits_a_compact_screen_at_2x() {
    let mut h = harness(900.0, 600.0, 2.0);
    save(&mut h, "publish-compact-2x");
    h.get_by_label("Publish Sheets");
}

/// Click a sheet's row in the list (the status bar's layout tabs carry the same names).
fn pick_sheet(h: &mut Harness<'static, CadApp>, name: &str) {
    h.get(By::new().label(name).predicate(|n| n.value().is_some())).click();
    h.run_steps(3);
}

#[test]
fn every_page_setting_can_change_for_one_run() {
    let dir = temp("rows");
    let _ = std::fs::remove_dir_all(&dir);
    let mut h = harness_for(app(Some(dir.clone())), 1280.0, 900.0, 1.0);
    pick_sheet(&mut h, "Layout1");
    // The inspector's page setup rows: portrait, and object lineweights off.
    h.get_by_label("Portrait").click();
    h.run_steps(3);
    h.get_by_label("Plot object lineweights").click();
    settle(&mut h);
    save(&mut h, "publish-page-setup-rows");
    h.get(By::new().value("Changed for this run"));
    assert!(h.state().session.doc().unwrap().layout("Layout1").unwrap().page.landscape, "the drawing keeps its page setup");
    h.get_by_label("Publish 3 Sheets").click();
    wait_for_publish(&mut h);
    let plotted = std::fs::read_to_string(dir.join("Drawing1-Layout1.pdf")).unwrap();
    assert!(plotted.contains(r#""landscape":false"#), "portrait for this run: {plotted}");
    assert!(plotted.contains(r#""lineweights":false"#), "no lineweights for this run: {plotted}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reset_and_save_to_layout() {
    let mut h = harness(1280.0, 900.0, 1.0);
    pick_sheet(&mut h, "Layout1");
    h.get_by_label("Portrait").click();
    settle(&mut h);
    h.get_by_label("Reset").click();
    settle(&mut h);
    assert_eq!(h.query_all(By::new().value("Changed for this run")).count(), 0, "Reset drops the change");
    h.get_by_label("Portrait").click();
    settle(&mut h);
    h.get_by_label("Save to Layout").click();
    settle(&mut h);
    assert!(!h.state().session.doc().unwrap().layout("Layout1").unwrap().page.landscape, "the layout keeps it now");
    assert_eq!(h.query_all(By::new().value("Changed for this run")).count(), 0, "and the run uses the layout's own again");
}

#[test]
fn a_sheet_can_borrow_another_layouts_page_setup() {
    let mut h = harness(1280.0, 900.0, 1.0);
    h.run_steps(1);
    h.state_mut().run("pagesetup", json!({ "layout": "Layout2", "paper": "A3", "landscape": false })).unwrap();
    settle(&mut h);
    pick_sheet(&mut h, "Layout1");
    h.get(By::new().value("As saved (Default)")).click();
    h.run_steps(3);
    h.get_by_label("From Layout2").click();
    settle(&mut h);
    h.get(By::new().value("From Layout2"));
    h.get(By::new().value("Changed for this run"));
}

#[test]
fn a_list_longer_than_one_run_is_checked_in_batches_without_noise() {
    let mut app = app(None);
    // 300 layouts, each with something on it: more sheets than one publish run takes.
    for i in 0..300 {
        let name = format!("S{i:03}");
        app.run("layout.new", json!({ "name": name })).unwrap();
    }
    let before = app.session.log.len();
    let mut h = harness_for(app, 1280.0, 820.0, 1.0);
    settle(&mut h);
    let unchecked = h.query_all(By::new().value("Checking...")).count();
    assert_eq!(unchecked, 0, "every row has a status");
    let log: Vec<String> = h.state().session.log.iter().skip(before).cloned().collect();
    assert!(!log.iter().any(|l| l.contains("at most")), "nothing echoed to the command line: {log:?}");
}
