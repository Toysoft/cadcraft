//! TOLERANCE typed or from the Dimension menu opens the Geometric Tolerance dialog, as in AutoCAD;
//! OK asks for the frame's location. -TOLERANCE, scripts and JSON calls keep the command line.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, GdtSymbol, Tolerance};
use cadcraft_ui_egui::tolerancedlg::{Picker, set_pick};
use cadcraft_ui_egui::{CadApp, Services};
use egui_kittest::kittest::Queryable;
use serde_json::json;

#[test]
fn typed_and_menu_tolerance_open_the_dialog_but_dash_and_json_do_not() {
    let mut app = CadApp::new(Session::new(), Services::default());
    for typed in ["tolerance", "TOL"] {
        app.ui.dialog = None;
        app.cmdline(typed);
        assert_eq!(app.ui.dialog.as_deref(), Some("tolerance"), "{typed}");
        assert!(app.session.running.is_none(), "{typed}: {}", app.session.prompt_text());
    }
    app.ui.dialog = None;
    app.start("tolerance");
    assert_eq!(app.ui.dialog.as_deref(), Some("tolerance"), "menu");
    // -TOLERANCE: the command-line prompts.
    app.ui.dialog = None;
    app.cmdline("-tolerance");
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.prompt_text().contains("geometric characteristic"), "{}", app.session.prompt_text());
    app.session.cancel();
    // JSON: runs, never opens.
    app.run("tolerance", json!({ "at": [0, 0], "symbol": "flatness", "tol1": "0.1" })).unwrap();
    app.cmdline("tolerance {\"at\": [5, 5], \"symbol\": \"flatness\", \"tol1\": \"0.1\"}");
    assert_eq!(app.ui.dialog, None);
}

fn harness(app: CadApp) -> egui_kittest::Harness<'static, CadApp> {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn tolerances(app: &CadApp) -> Vec<Tolerance> {
    let d = app.session.doc().unwrap();
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Tolerance(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn ok_places_the_frame_the_dialog_shows() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("tolerance");
    let mut h = harness(app);
    {
        let st = h.state_mut().tolerance_dlg.as_mut().expect("the dialog's state");
        let f = &mut st.frame;
        set_pick(f, Picker::Symbol(0), Some(GdtSymbol::Position));
        let l = &mut f.lines[0];
        l.tol1.as_mut().unwrap().diameter = true;
        l.tol1.as_mut().unwrap().value = "0.1".into();
        l.datums[0].letter = "A".into();
        l.datums[1].letter = "B".into();
        set_pick(f, Picker::Material(0, 0), Some(GdtSymbol::Mmc));
        set_pick(f, Picker::Material(0, 3), Some(GdtSymbol::Lmc));
        set_pick(f, Picker::Symbol(1), Some(GdtSymbol::Flatness));
        f.lines[1].tol1.as_mut().unwrap().value = "0.02".into();
        f.height = "2.5".into();
        f.projected = true;
        f.datum_id = "D".into();
    }
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let app = h.state_mut();
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.prompt_text().contains("Enter tolerance location"), "{}", app.session.prompt_text());
    app.cmdline("10,20");
    assert!(app.session.running.is_none(), "{}", app.session.prompt_text());
    // The same frame as the JSON form with the same fields.
    let mut json = CadApp::new(Session::new(), Services::default());
    let want = json
        .run(
            "tolerance",
            json!({ "at": [0, 0], "symbol": "position", "tol1": "%%c0.1(M)", "datums": ["A", "B(L)"],
                    "line2": { "symbol": "flatness", "tol1": "0.02" }, "height": "2.5", "projected": true, "datumId": "D" }),
        )
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    let t = tolerances(app);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].text, want);
    assert!((t[0].insert.x - 10.0).abs() < 1e-9 && (t[0].insert.y - 20.0).abs() < 1e-9);
}

#[test]
fn the_pickers_set_and_clear_symbols_and_material_conditions() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("tolerance");
    let mut h = harness(app);
    h.state_mut().tolerance_dlg.as_mut().unwrap().picker = Some(Picker::Symbol(0));
    h.run_steps(2);
    h.get_by_label("Flatness").click();
    h.run_steps(2);
    let st = h.state().tolerance_dlg.clone().unwrap();
    assert_eq!((st.frame.lines[0].symbol, st.picker), (Some(GdtSymbol::Flatness), None));
    h.state_mut().tolerance_dlg.as_mut().unwrap().picker = Some(Picker::Material(0, 2));
    h.run_steps(2);
    h.get_by_label("Maximum material condition").click();
    h.run_steps(2);
    assert_eq!(h.state().tolerance_dlg.clone().unwrap().frame.lines[0].datums[0].material.as_deref(), Some("M"));
    // The empty box clears.
    h.state_mut().tolerance_dlg.as_mut().unwrap().picker = Some(Picker::Symbol(0));
    h.run_steps(2);
    h.get_by_label("None").click();
    h.run_steps(2);
    assert_eq!(h.state().tolerance_dlg.clone().unwrap().frame.lines[0].symbol, None);
}

#[test]
fn cancel_and_an_empty_frame_add_nothing() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("tolerance");
    let mut h = harness(app);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!((h.state().ui.dialog.clone(), h.state().tolerance_dlg.clone()), (None, None));
    h.state_mut().cmdline("tolerance");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let app = h.state();
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.running.is_none(), "{}", app.session.prompt_text());
    assert!(tolerances(app).is_empty());
}
