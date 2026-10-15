//! Publishing in the background: the window closes, the status bar and a card show progress with
//! Cancel, and the Publish Report lists every sheet with Reveal and Publish Again. Publishing in
//! the foreground, "Open the result when done", and the sheet stamp with its Sheet Stamp window.

use std::cell::RefCell;
use std::rc::Rc;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_deliver, set_io, set_plot_book};
use cadcraft_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use serde_json::{Value, json};

/// A slow plotter (so a run can be watched and cancelled) that writes the texts it plots.
fn slow_plot(d: &cadcraft_doc::Drawing, space: &cadcraft_doc::Space, _: &Value) -> Result<Vec<u8>, String> {
    std::thread::sleep(std::time::Duration::from_millis(80));
    let texts: Vec<String> = d
        .space(space)
        .into_iter()
        .flat_map(|s| s.iter())
        .filter_map(|e| if let cadcraft_doc::EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None })
        .collect();
    Ok(json!({ "texts": texts }).to_string().into_bytes())
}

fn book(parts: &[(&cadcraft_doc::Drawing, cadcraft_doc::Space, Value)], _: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 book {}", parts.len()).into_bytes())
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-publish-run-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

type Opened = Rc<RefCell<Vec<(String, bool)>>>;

fn harness(dir: &std::path::Path) -> (Harness<'static, CadApp>, Opened) {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Err("no writer in this test".into()), plot: Some(slow_plot) });
    set_plot_book(book);
    let opened: Opened = Rc::default();
    let log = opened.clone();
    let out = dir.to_path_buf();
    let services = Services {
        pick_save: Some(Box::new(move |name: &str| Some(out.join(name).to_string_lossy().to_string()))),
        open_path: Some(Box::new(move |path: &str, reveal: bool| {
            log.borrow_mut().push((path.to_string(), reveal));
            Ok(())
        })),
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
    app.cmdline("PUBLISH");
    let mut h = Harness::builder().with_size(vec2(1280.0, 820.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    (h, opened)
}

fn save(h: &mut Harness<'static, CadApp>, name: &str) {
    if let Ok(directory) = std::env::var("CRAFT_UI_VISUAL_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(format!("{directory}/{name}.png")).unwrap();
    }
}

fn wait_for_publish(h: &mut Harness<'static, CadApp>) {
    for _ in 0..500 {
        h.run_steps(1);
        if !h.state().publishing.busy() {
            h.run_steps(3);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("publishing didn't finish");
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Show in Folder"
    }
}

#[test]
fn a_background_run_shows_progress_then_a_report() {
    let dir = temp("background");
    let (mut h, opened) = harness(&dir);
    h.get_by_label("Publish 3 Sheets").click();
    h.run_steps(2);
    // The window closes and the drawing stays usable; the card shows how far it is.
    assert!(h.state().publishing.busy());
    assert!(h.query_by_label("Publish Sheets").is_none());
    h.get_by_label_contains("of 3 sheets...");
    save(&mut h, "publish-progress");
    let status = h.state().status.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(status.starts_with("Publishing "), "the status bar says so: {status}");
    wait_for_publish(&mut h);
    save(&mut h, "publish-report");
    h.get_by_label("Publish Report");
    h.get_by_label("Published 3 of 3 sheets");
    for f in ["Drawing1-Model.pdf", "Drawing1-Layout1.pdf", "Drawing1-Layout2.pdf"] {
        h.get_by_label(f);
        assert!(dir.join(f).exists(), "{f}");
    }
    // Reveal shows the folder (three files); Publish Again runs the same request.
    h.get_by_label(reveal_label()).click();
    h.run_steps(2);
    assert_eq!(opened.borrow().as_slice(), [(dir.to_string_lossy().to_string(), false)]);
    std::fs::remove_file(dir.join("Drawing1-Model.pdf")).unwrap();
    h.get_by_label("Publish Again").click();
    wait_for_publish(&mut h);
    assert!(dir.join("Drawing1-Model.pdf").exists(), "published again");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cancel_stops_a_background_run() {
    let dir = temp("cancel");
    let (mut h, _) = harness(&dir);
    h.get_by_label("Publish 3 Sheets").click();
    // Once the window has gone, the only Cancel is the progress card's.
    for _ in 0..20 {
        h.run_steps(1);
        if h.query_by_label("Publish Sheets").is_none() && h.query_all_by_label("Cancel").count() == 1 {
            break;
        }
    }
    assert!(h.state().publishing.busy(), "still running");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    h.get_by_label("Cancelling...");
    wait_for_publish(&mut h);
    // The report: the summary says so, and so does each sheet not reached.
    h.get_by_label_contains("sheets · cancelled");
    assert!(h.get_all_by_label("cancelled").count() >= 1, "the sheets not reached say so");
    assert!(!dir.join("Drawing1-Layout2.pdf").exists(), "the last sheet wasn't written");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn in_the_foreground_it_publishes_now_and_can_open_the_result() {
    let dir = temp("foreground");
    let (mut h, opened) = harness(&dir);
    h.get_by_label("Run in the background").click();
    h.get_by_label("Open the result when done").click();
    h.get_by_label("Output").click();
    h.run_steps(3);
    h.get_by_label("Combine into one").click();
    h.run_steps(4);
    h.get_by_label("Publish 3 Sheets").click();
    h.run_steps(3);
    assert!(!h.state().publishing.busy());
    let pdf = dir.join("Drawing1.pdf");
    assert!(pdf.exists(), "written before the click returned");
    assert_eq!(opened.borrow().as_slice(), [(pdf.to_string_lossy().to_string(), false)], "the combined PDF is opened");
    h.get_by_label("Publish Report");
    h.get_by_label("Drawing1.pdf · page 3");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_stamp_is_edited_in_its_window_and_printed_on_every_sheet() {
    let dir = temp("stamp");
    let (mut h, _) = harness(&dir);
    assert!(h.get_by_label("Edit stamp...").accesskit_node().is_disabled(), "until the stamp is on");
    h.get_by_label("Add a sheet stamp").click();
    h.run_steps(2);
    h.get_by_label("Edit stamp...").click();
    h.run_steps(3);
    h.get_by_label("Sheet Stamp");
    let field =
        || By::new().role(egui::accesskit::Role::TextInput).predicate(|n| n.is_focused() || n.value().is_some_and(|v| v.starts_with("{drawing}")));
    h.get(field()).focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get(field()).type_text("CHECKED {sheet} of {drawing}");
    h.run_steps(3);
    save(&mut h, "publish-stamp-window");
    h.get_by_label("CHECKED Layout1 of Drawing1");
    h.get_by_label("Done").click();
    h.run_steps(2);
    assert!(h.query_by_label("Sheet Stamp").is_none());
    h.get_by_label("Run in the background").click();
    h.get_by_label("Publish 3 Sheets").click();
    h.run_steps(3);
    let plotted = std::fs::read_to_string(dir.join("Drawing1-Layout2.pdf")).unwrap();
    assert!(plotted.contains("CHECKED Layout2 of Drawing1"), "{plotted}");
    let _ = std::fs::remove_dir_all(dir);
}

/// What the browser download hook was given: (file name, MIME type, size).
static DELIVERED: std::sync::Mutex<Vec<(String, String, usize)>> = std::sync::Mutex::new(Vec::new());

fn download(name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    if let Ok(mut d) = DELIVERED.lock() {
        d.push((name.to_string(), mime.to_string(), bytes.len()));
    }
    Ok(())
}

#[test]
fn without_a_folder_the_files_are_handed_to_the_browser() {
    // The web build: its save picker gives a bare name (no folder), and files are downloads.
    set_deliver(download);
    let (mut h, _) = harness(std::path::Path::new(""));
    h.get_by_label("Run in the background").click();
    h.get_by_label("Publish 3 Sheets").click();
    h.run_steps(3);
    h.get_by_label("Published 3 of 3 sheets");
    let got = DELIVERED.lock().map(|d| d.clone()).unwrap_or_default();
    let names: Vec<&str> = got.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, ["Drawing1-Model.pdf", "Drawing1-Layout1.pdf", "Drawing1-Layout2.pdf"]);
    assert!(got.iter().all(|(_, mime, size)| mime == "application/pdf" && *size > 0));
}

#[test]
fn the_stamp_window_shows_what_is_wrong_with_the_text() {
    let dir = temp("stamp-error");
    let (mut h, _) = harness(&dir);
    h.get_by_label("Add a sheet stamp").click();
    h.run_steps(2);
    h.get_by_label("Edit stamp...").click();
    h.run_steps(3);
    let field =
        || By::new().role(egui::accesskit::Role::TextInput).predicate(|n| n.is_focused() || n.value().is_some_and(|v| v.starts_with("{drawing}")));
    h.get(field()).focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get(field()).type_text("{sheet");
    h.run_steps(3);
    h.get_by_label_contains("without its }");
}
