#[path = "../../io/tests/support/pdf.rs"]
mod fixture;
use cadcraft_engine::{
    Session,
    cmd::{file, pdfimport},
};
use cadcraft_ui_egui::{CadApp, Services, dialogs, menus};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
fn install() {
    pdfimport::set_previewer(|bytes, outlines| {
        let previews = cadcraft_io::pdf_read::page_previews(bytes, outlines).map_err(|e| e.to_string())?;
        let pages = previews.len();
        let previews: Vec<_> = previews
            .into_iter()
            .map(|mut p| {
                if let Some(png) = p.png {
                    p.info["thumbnail"] = json!(file::base64_encode(&png));
                }
                p.info
            })
            .collect();
        Ok(json!({"pages":pages,"previews":previews}))
    });
    pdfimport::set_reader(|bytes, page, thumbnail, outlines| {
        let r = cadcraft_io::pdf_read::read_with(bytes, page, outlines).map_err(|e| e.to_string())?;
        let mut info = r.info();
        if thumbnail {
            let png = cadcraft_io::png(&r.drawing, &cadcraft_doc::Space::Model, 640, 420).map_err(|e| e.to_string())?;
            info["thumbnail"] = json!(file::base64_encode(&png));
        }
        Ok((r.drawing, info))
    });
}
fn harness(app: CadApp) -> egui_kittest::Harness<'static, CadApp> {
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1400.0, 1000.0))
        .build_ui_state(|ui, app: &mut CadApp| dialogs::show(app, ui.ctx()), app);
    h.run_steps(5);
    wait_for_pages(&mut h);
    h
}
fn wait_for_pages(h: &mut egui_kittest::Harness<'static, CadApp>) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while h.query_by_label("Analyzing all pages...").is_some() {
        assert!(std::time::Instant::now() < until, "page analysis timed out");
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.run_steps(2);
    }
}
#[test]
fn ordinary_open_import_drop_preview_and_real_import_button() {
    install();
    let bytes = fixture::sample();
    let path = std::env::temp_dir().join(format!("cadcraft-pdf-ui-{}.PDF", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    for entry in ["open", "import", "imp", "drop_path", "drop_bytes", "empty"] {
        let picked = path.to_string_lossy().to_string();
        let services = Services { pick_open: Some(Box::new(move || Some(picked.clone()))), ..Default::default() };
        let session = if entry == "empty" { Session::empty() } else { Session::new() };
        let mut app = CadApp::new(session, services);
        match entry {
            "open" | "import" => menus::activate(&mut app, entry),
            "imp" => {
                app.cmdline("IMP");
            }
            "drop_path" => app.open_path(&path.to_string_lossy()),
            _ => app.open_bytes("drawing.PDF", &bytes),
        };
        assert_eq!(app.ui.dialog.as_deref(), Some("pdfimport"));
        let before = app.session.state().unwrap().doc.clone();
        let mut h = harness(app);
        assert!(std::sync::Arc::ptr_eq(&before, &h.state().session.state().unwrap().doc));
        assert!(!h.get_by_label("Import Vectors").accesskit_node().is_disabled());
        h.get_by_label("Import Vectors").click();
        h.run_steps(5);
        assert!(h.state().ui.dialog.is_none());
        assert_eq!(h.state().session.doc().unwrap().model.len(), 5);
        h.state_mut().session.undo().unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &h.state().session.state().unwrap().doc));
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn menu_alias_and_json_routing_broken_pdf_keeps_import_disabled() {
    install();
    let mut app = CadApp::new(Session::new(), Services::default());
    for id in ["importpdf", "IMPORTPDF", "ImportPdf", "-importpdf", "pdfimport", "PDFIMPORT", "-pdfimport"] {
        app.ui.dialog = None;
        assert!(app.cmdline(id).is_none(), "{id}");
        assert_eq!(app.ui.dialog.as_deref(), Some("pdfimport"));
    }
    app.ui.dialog = None;
    app.run("importpdf", json!({"data":file::base64_encode(&fixture::sample()),"preview":true})).unwrap();
    assert!(app.ui.dialog.is_none());
    app.open_bytes("bad.pdf", b"bad PDF");
    let h = harness(app);
    assert!(h.get_by_label("Import Vectors").accesskit_node().is_disabled());
    assert!(h.state().session.doc().unwrap().model.is_empty());
}

#[test]
fn importpdf_from_enter_autocomplete_and_menu_opens_the_same_dialog() {
    let mut app = CadApp::new(Session::empty(), Services::default());
    app.cmd.buffer = "IMPORTPDF".into();
    cadcraft_ui_egui::cmdline::submit(&mut app);
    assert_eq!(app.ui.dialog.as_deref(), Some("pdfimport"));
    assert!(app.cmd.buffer.is_empty());
    assert!(!app.ui.start_tab);
    assert!(app.session.log.iter().all(|s| !s.contains("unknown command")));
    app.ui.dialog = None;
    app.start("importpdf");
    assert_eq!(app.ui.dialog.as_deref(), Some("pdfimport"));
    app.ui.dialog = None;
    menus::activate(&mut app, "importpdf");
    assert_eq!(app.ui.dialog.as_deref(), Some("pdfimport"));
}

#[test]
fn importpdf_opens_the_pdf_picker_immediately_and_cancel_keeps_the_drawing() {
    install();
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let asked = calls.clone();
    let services = Services {
        pick_pdf: Some(Box::new(move || {
            asked.set(asked.get() + 1);
            None
        })),
        ..Services::default()
    };
    let mut app = CadApp::new(Session::empty(), services);
    app.cmd.buffer = "IMPORTPDF".into();
    cadcraft_ui_egui::cmdline::submit(&mut app);
    assert_eq!(calls.get(), 1);
    assert!(app.ui.dialog.is_none());
    assert!(app.session.state().is_err());
    app.session = Session::new();
    app.run("importpdf", json!({"data":file::base64_encode(&fixture::sample()),"preview":true,"pages":true})).unwrap();
    assert_eq!(calls.get(), 1, "JSON does not ask for a file");
    assert!(app.ui.dialog.is_none());

    let path = std::env::temp_dir().join(format!("cadcraft-pdf-picked-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture::sample()).unwrap();
    let picked = path.to_string_lossy().to_string();
    app.services.pick_pdf = Some(Box::new(move || Some(picked.clone())));
    menus::activate(&mut app, "importpdf");
    let h = harness(app);
    h.get_by_label("Pages: 2");
    h.get_by_label("Page 2");
    assert!(!h.get_by_label("Import Vectors").accesskit_node().is_disabled());
    assert!(h.state().session.doc().unwrap().model.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn choose_second_pdf_page_before_importing() {
    install();
    let mut app = CadApp::new(Session::new(), Services::default());
    app.open_bytes("two-pages.pdf", &fixture::sample());
    let mut h = harness(app);
    h.get_by_label("Pages: 2");
    h.get_by_label("Page 2").click();
    h.run_steps(3);
    assert!(h.query_by_label("Analyzing all pages...").is_none());
    assert!(!h.get_by_label("Import Vectors").accesskit_node().is_disabled());
    h.get_by_label("Import Vectors").click();
    h.run_steps(3);
    assert_eq!(h.state().session.doc().unwrap().model.len(), 1);
    assert!(h.state().session.doc().unwrap().model.iter().all(|e| e.common.layer == "PDF_PAGE_2"));
}
#[test]
fn pdf_dialog_and_import_visual_evidence() {
    let Some(dir) = std::env::var_os("CADCRAFT_PDF_SHOTS").map(std::path::PathBuf::from) else { return };
    install();
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = CadApp::new(Session::new(), Services::default());
    if let Ok(path) = std::env::var("CADCRAFT_PDF_FILE") {
        app.open_path(&path);
    } else {
        app.open_bytes("synthetic-plan.pdf", &fixture::sample());
        std::fs::write(dir.join("synthetic-plan.pdf"), fixture::sample()).unwrap();
    }
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1100.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    wait_for_pages(&mut h);
    h.render().unwrap().save(dir.join("pdf-preview.png")).unwrap();
    h.get_by_label("Import Vectors").click();
    h.run_steps(6);
    assert!(h.state().ui.dialog.is_none());
    h.render().unwrap().save(dir.join("pdf-imported.png")).unwrap();
    let dxf = cadcraft_io::write_dxf(h.state().session.doc().unwrap());
    std::fs::write(dir.join("pdf-converted.dxf"), dxf).unwrap();
}
