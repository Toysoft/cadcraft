//! PUBLISH with a `sheets` list: sheets from open drawings and from drawing files (read for the
//! run only), a page setup override per sheet that leaves the drawing as it was, file names from a
//! pattern, one result per sheet, and a run that keeps going when one sheet fails. Combining puts
//! the sheets of any drawings in one PDF; SVG and PNG give a file per sheet. `action: "status"`
//! says what each sheet will do and writes nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, base64_decode, set_io, set_plot_book};
use cadcraft_engine::cmd::plotters::{PlotterStore, set_plotter_store};
use serde_json::{Value, json};

/// How often this test's reader decoded each file (tests run in parallel, so by file).
static READS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

fn reads(name: &str) -> usize {
    READS.lock().map(|r| r.get(name).copied().unwrap_or(0)).unwrap_or(0)
}

/// A plotter that echoes what it was asked to plot.
fn echo_plot(_: &cadcraft_doc::Drawing, space: &cadcraft_doc::Space, opts: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 {space:?} {opts}").into_bytes())
}

/// A combined-PDF plotter that lists its pages: `%PDF book <n>: <sheet>, <sheet>…`.
fn echo_book(parts: &[(&cadcraft_doc::Drawing, cadcraft_doc::Space, Value)], opts: &Value) -> Result<Vec<u8>, String> {
    let pages: Vec<String> = parts.iter().map(|(_, space, o)| format!("{} {}", space.tab_name(), o["title"].as_str().unwrap_or(""))).collect();
    Ok(format!("%PDF book {}: {} | {opts}", parts.len(), pages.join(", ")).into_bytes())
}

/// A reader for `*.sheets` files (a metric drawing with Layout1 and Layout2; in a `*.gone.sheets`
/// file Layout1's page setup names a plotter that isn't set up); anything else is unreadable.
fn read(bytes: &[u8], name: &str) -> Result<cadcraft_doc::Drawing, String> {
    if name.ends_with(".gone.sheets") && bytes == b"sheets" {
        let mut d = sheets_drawing();
        if let Some(l) = d.layouts.iter_mut().find(|l| l.name == "Layout1") {
            l.page.device = "Gone Plotter".into();
        }
        Ok(d)
    } else if name.ends_with(".sheets") && bytes == b"sheets" {
        if let Ok(mut r) = READS.lock() {
            *r.entry(name.to_string()).or_default() += 1;
        }
        Ok(sheets_drawing())
    } else {
        Err("not a drawing".into())
    }
}

/// A metric drawing with a line on each layout (an empty layout has nothing to plot).
fn sheets_drawing() -> cadcraft_doc::Drawing {
    let mut d = cadcraft_doc::Drawing::new_metric();
    for layout in ["Layout1", "Layout2"] {
        let line = cadcraft_doc::EntityKind::Line(cadcraft_doc::Line { a: cadcraft_geom::Vec3::ZERO, b: cadcraft_geom::Vec3::new(10.0, 10.0, 0.0) });
        let _ = d.add(&cadcraft_doc::Space::Paper(layout.into()), cadcraft_doc::Common::default(), line);
    }
    d
}

fn session() -> Session {
    set_io(IoHooks { read, write: |_, _| Err("no writer in this test".into()), plot: Some(echo_plot) });
    set_plot_book(echo_book);
    set_plotter_store(PlotterStore::memory());
    let mut s = Session::new();
    for layout in ["Layout1", "Layout2"] {
        s.execute("layout.set", &json!({"name": layout})).unwrap();
        s.execute("line", &json!({"points": [[0, 0], [10, 10]]})).unwrap();
    }
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [100, 0]]})).unwrap();
    s
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-publish-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn plotted(sheet: &Value) -> String {
    String::from_utf8(base64_decode(sheet["data"].as_str().unwrap()).unwrap()).unwrap()
}

fn files(r: &Value) -> Vec<String> {
    r["sheets"].as_array().unwrap().iter().filter_map(|s| s["file"].as_str().map(str::to_string)).collect()
}

#[test]
fn sheets_are_named_from_the_pattern_and_written_to_the_folder() {
    let mut s = session();
    let dir = temp("named");
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [{"layout": "layout1"}, {"layout": "MODEL"}], "folder": dir.to_string_lossy(), "name": "{n} {drawing} {sheet}"}),
        )
        .unwrap();
    assert_eq!(files(&r), ["1 Drawing1 Layout1.pdf", "2 Drawing1 Model.pdf"]);
    assert_eq!((r["published"].as_u64(), r["failed"].as_u64()), (Some(2), Some(0)));
    for sheet in r["sheets"].as_array().unwrap() {
        let path = sheet["path"].as_str().unwrap();
        assert!(std::fs::read_to_string(path).unwrap().starts_with("%PDF"), "{path} was written");
    }
    // SVG and PNG take their own extension; without a folder each sheet comes back as data.
    let r = s.execute("publish", &json!({"sheets": [{"layout": "Layout2"}], "to": "SVG"})).unwrap();
    assert_eq!(files(&r), ["Drawing1-Layout2.svg"]);
    assert!(plotted(&r["sheets"][0]).contains(r#""output":"svg""#));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failing_sheet_is_reported_and_the_rest_still_publish() {
    let mut s = session();
    let bad_file = temp("unreadable").join("broken.dxf");
    std::fs::write(&bad_file, b"not a drawing").unwrap();
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [
                {"layout": "Layout1"},
                {"layout": "Nope"},
                {"drawing": 7, "layout": "Layout1"},
                {"drawing": bad_file.to_string_lossy(), "layout": "Layout1"},
                {"drawing": "/no/such/folder/plan.sheets", "layout": "Layout1"},
                {"layout": "Model"},
            ]}),
        )
        .unwrap();
    let ok: Vec<bool> = r["sheets"].as_array().unwrap().iter().map(|s| s["ok"].as_bool().unwrap()).collect();
    assert_eq!(ok, [true, false, false, false, false, true]);
    assert_eq!((r["published"].as_u64(), r["failed"].as_u64()), (Some(2), Some(4)));
    assert!(r["sheets"][1]["error"].as_str().unwrap().contains("no layout `Nope`"));
    assert!(r["sheets"][2]["error"].as_str().unwrap().contains("no open drawing 7"));
    assert!(r["sheets"][3]["error"].as_str().unwrap().contains("not a drawing"));
    assert!(r["message"].as_str().unwrap().contains("4 failed"));
    // When no sheet publishes, the command fails and says why.
    let e = s.execute("publish", &json!({"sheets": [{"layout": "Nope"}, {"layout": "Gone"}]})).unwrap_err().to_string();
    assert!(e.contains("no sheet was published") && e.contains("Nope") && e.contains("Gone"), "{e}");
    let _ = std::fs::remove_file(bad_file);
}

#[test]
fn a_page_setup_override_is_for_this_run_only() {
    let mut s = session();
    let before = s.doc().unwrap().layout("Layout1").unwrap().page.clone();
    let (revision, undo) = (s.state().unwrap().revision, s.state().unwrap().undo.len());
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [{"layout": "Layout1", "pageSetup": {"paper": "A3", "landscape": true, "scale": "1:50", "margins": [5, 5, 5, 5]}}]}),
        )
        .unwrap();
    let opts = plotted(&r["sheets"][0]);
    for part in [r#""paper":"A3""#, r#""landscape":true"#, r#""scale":0.02"#, r#""margins":[5.0,5.0,5.0,5.0]"#] {
        assert!(opts.contains(part), "the plotter gets {part}: {opts}");
    }
    assert_eq!(s.doc().unwrap().layout("Layout1").unwrap().page, before, "the layout's page setup is unchanged");
    assert_eq!(s.state().unwrap().revision, revision, "the drawing isn't edited");
    assert_eq!(s.state().unwrap().undo.len(), undo, "no undo step is added");
}

#[test]
fn sheets_come_from_open_drawings_and_files_without_opening_them() {
    let mut s = session();
    s.execute("new", &json!({})).unwrap();
    assert_eq!((s.docs.len(), s.active), (2, 1));
    let file = temp("files").join("site plan.sheets");
    std::fs::write(&file, b"sheets").unwrap();
    let path = file.to_string_lossy().to_string();
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [
                {"drawing": 0, "layout": "Model"},
                {"drawing": path, "layout": "Layout1"},
                {"drawing": path, "layout": "Layout2"},
            ]}),
        )
        .unwrap();
    assert_eq!(files(&r), ["Drawing1-Model.pdf", "site plan-Layout1.pdf", "site plan-Layout2.pdf"]);
    assert_eq!(r["sheets"][1]["drawing"], path);
    assert_eq!(reads(&path), 1, "a file is read once per run");
    assert_eq!((s.docs.len(), s.active), (2, 1), "no drawing is opened and the current one stays current");
    let _ = std::fs::remove_file(file);
}

#[test]
fn two_sheets_with_one_name_do_not_overwrite_each_other() {
    let mut s = session();
    let dir = temp("same");
    let r = s.execute("publish", &json!({"sheets": [{"layout": "Layout1"}, {"layout": "Layout1"}], "folder": dir.to_string_lossy()})).unwrap();
    assert_eq!(files(&r), ["Drawing1-Layout1.pdf", "Drawing1-Layout1 (2).pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_older_path_and_layouts_form_still_works() {
    let mut s = session();
    let dir = temp("legacy");
    let base = dir.join("set{1}.pdf");
    let r = s.execute("publish", &json!({"path": base.to_string_lossy(), "layouts": ["layout2", "MODEL"]})).unwrap();
    assert_eq!(files(&r), ["set{1}-Layout2.pdf", "set{1}-Model.pdf"], "braces in the old NAME stay braces");
    assert!(r["sheets"][0]["path"].as_str().unwrap().ends_with("set{1}-Layout2.pdf"));
    // Unknown layouts are still refused before anything is written.
    assert!(s.execute("publish", &json!({"layouts": ["Nope"]})).is_err());
    // No parameters: Model (it has objects), then every layout.
    let r = s.execute("publish", &json!({})).unwrap();
    let layouts: Vec<&str> = r["sheets"].as_array().unwrap().iter().filter_map(|x| x["layout"].as_str()).collect();
    assert_eq!(layouts, ["Model", "Layout1", "Layout2"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bad_parameters_are_refused_before_anything_is_written() {
    let mut s = session();
    let many: Vec<Value> = (0..257).map(|_| json!({"layout": "Layout1"})).collect();
    let cases = [
        json!({"sheets": [{"layout": "Layout1"}], "widht": 5}),
        json!({"sheets": [{"layout": "Layout1"}], "path": "x.pdf"}),
        json!({"sheets": "Layout1"}),
        json!({"sheets": []}),
        json!({"sheets": many}),
        json!({"sheets": [{"layout": "Layout1", "colour": 1}]}),
        json!({"sheets": [{"drawing": -1, "layout": "Layout1"}]}),
        json!({"sheets": [{"drawing": 1.5, "layout": "Layout1"}]}),
        json!({"sheets": [{"drawing": ""}]}),
        json!({"sheets": [{}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"paper": "Z9"}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"width": 100}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"width": 1e308, "height": 5}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"margins": [1, 2, 3]}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"scale": -1}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"landscape": "yes"}}]}),
        json!({"sheets": [{"layout": "Layout1", "pageSetup": {"plotArea": "display"}}]}),
        json!({"sheets": [{"layout": "Layout1"}], "to": "dwg"}),
        json!({"sheets": [{"layout": "Layout1"}], "to": 3}),
        json!({"sheets": [{"layout": "Layout1"}], "name": "../{sheet}"}),
        json!({"sheets": [{"layout": "Layout1"}], "name": "{foo}"}),
        json!({"sheets": [{"layout": "Layout1"}], "name": "a}"}),
        json!({"sheets": [{"layout": "Layout1"}], "name": ".."}),
        json!({"sheets": [{"layout": "Layout1"}], "name": ""}),
        json!({"sheets": [{"layout": "Layout1"}], "folder": 4}),
        json!({"sheets": [{"layout": "Layout1"}], "pdf": {"layers": "on"}}),
        json!({"sheets": [{"layout": "Layout1"}], "pdf": {"text": "outline"}}),
        json!({"sheets": [{"layout": "Layout1"}], "pdf": {"dpi": 600}}),
        json!({"sheets": [{"layout": "Layout1"}], "copies": 0}),
        json!({"sheets": [{"layout": "Layout1"}], "copies": 101}),
        // More than one copy needs a printer, which publishing can't send to yet.
        json!({"sheets": [{"layout": "Layout1"}], "copies": 3}),
        // A `{` without its `}` (it used to be read as a token).
        json!({"sheets": [{"layout": "Layout1"}], "name": "{drawing"}),
        json!({"sheets": [{"layout": "Layout1"}], "stamp": {"text": "{sheet"}}),
        json!({"sheets": [{"layout": "Layout1"}], "stamp": "yes"}),
        json!({"sheets": [{"layout": "Layout1"}], "stamp": {"text": "{who}"}}),
        json!([1, 2]),
    ];
    for p in cases {
        assert!(s.execute("publish", &p).is_err(), "refused: {p}");
    }
    // Accepted: one copy, PDF settings and an escaped brace.
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [{"layout": "Layout1"}], "copies": 1, "pdf": {"layers": false, "text": "geometry"}, "name": "{{{sheet}}}"}),
        )
        .unwrap();
    assert_eq!(r["copies"], 1);
    assert_eq!(files(&r), ["{Layout1}.pdf"]);
    let opts = plotted(&r["sheets"][0]);
    assert!(opts.contains(r#""layers":false"#) && opts.contains(r#""text":"geometry""#), "{opts}");
}

#[test]
fn combine_puts_sheets_of_several_drawings_in_one_pdf() {
    let mut s = session();
    let dir = temp("combined");
    let file = dir.join("site.sheets");
    std::fs::write(&file, b"sheets").unwrap();
    let path = file.to_string_lossy().to_string();
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [
                {"layout": "Model"},
                {"drawing": path, "layout": "Layout2", "pageSetup": {"paper": "A3"}},
                {"layout": "Nope"},
                {"drawing": path, "layout": "Layout1"},
            ], "combine": true, "folder": dir.to_string_lossy(), "pdf": {"layers": false}}),
        )
        .unwrap();
    let combined = &r["combined"];
    assert_eq!((combined["file"].as_str(), combined["pages"].as_u64()), (Some("Drawing1.pdf"), Some(3)), "named after the first sheet's drawing");
    let written = std::fs::read_to_string(combined["path"].as_str().unwrap()).unwrap();
    assert!(written.starts_with("%PDF book 3: Model Drawing1 - Model, Layout2 site - Layout2, Layout1 site - Layout1"), "{written}");
    assert!(written.contains(r#""layers":false"#), "file-wide PDF settings reach the plotter: {written}");
    let pages: Vec<Option<u64>> = r["sheets"].as_array().unwrap().iter().map(|x| x["page"].as_u64()).collect();
    assert_eq!(pages, [Some(1), Some(2), None, Some(3)]);
    assert_eq!(r["sheets"][2]["ok"], false, "the missing layout is reported, the rest are combined");
    assert_eq!((r["published"].as_u64(), r["failed"].as_u64()), (Some(3), Some(1)));
    // A name pattern names the combined file too; without a folder it comes back as data.
    let r =
        s.execute("publish", &json!({"sheets": [{"layout": "Layout1"}, {"layout": "Layout2"}], "combine": true, "name": "{drawing} set"})).unwrap();
    assert_eq!(r["combined"]["file"], "Drawing1 set.pdf");
    assert!(plotted(&r["combined"]).starts_with("%PDF book 2"));
    // Only PDFs combine; the older form doesn't.
    for p in [
        json!({"sheets": [{"layout": "Layout1"}], "combine": true, "to": "svg"}),
        json!({"sheets": [{"layout": "Layout1"}], "combine": "yes"}),
        json!({"layouts": ["Layout1"], "combine": true}),
    ] {
        assert!(s.execute("publish", &p).is_err(), "refused: {p}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn svg_and_png_give_a_file_per_sheet() {
    let mut s = session();
    for to in ["svg", "png"] {
        let dir = temp(&format!("each-{to}"));
        let r = s
            .execute(
                "publish",
                &json!({"sheets": [{"layout": "Model"}, {"layout": "Layout1"}, {"layout": "Layout2"}], "to": to, "folder": dir.to_string_lossy()}),
            )
            .unwrap();
        let expected: Vec<String> = ["Model", "Layout1", "Layout2"].iter().map(|l| format!("Drawing1-{l}.{to}")).collect();
        assert_eq!(files(&r), expected);
        let mut on_disk: Vec<String> =
            std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        on_disk.sort();
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(on_disk, sorted, "{to}: three files in the folder");
        for sheet in r["sheets"].as_array().unwrap() {
            let text = std::fs::read_to_string(sheet["path"].as_str().unwrap()).unwrap();
            assert!(text.contains(&format!(r#""output":"{to}""#)), "{to}: the plotter's {to} output: {text}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn status_says_what_each_sheet_will_do_and_writes_nothing() {
    let mut s = session();
    let dir = temp("status");
    let unreadable = dir.join("broken.dxf");
    std::fs::write(&unreadable, b"not a drawing").unwrap();
    let gone = dir.join("office.gone.sheets");
    std::fs::write(&gone, b"sheets").unwrap();
    s.execute("pagesetup", &json!({"layout": "Layout2", "paper": "A3", "landscape": true})).unwrap();
    s.execute("new", &json!({})).unwrap();
    let before: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
    let r = s
        .execute(
            "publish",
            &json!({"action": "status", "folder": dir.to_string_lossy(), "sheets": [
                {"drawing": 0, "layout": "Layout2"},
                {"drawing": 0, "layout": "Layout1"},
                {"drawing": 0, "layout": "Model"},
                {"layout": "Model"},
                {"drawing": 0, "layout": "Nope"},
                {"drawing": unreadable.to_string_lossy(), "layout": "Layout1"},
                {"drawing": gone.to_string_lossy(), "layout": "Layout1"},
                {"drawing": 0, "layout": "Layout1", "pageSetup": {"paper": "A4"}},
            ]}),
        )
        .unwrap();
    let sheets = r["sheets"].as_array().unwrap();
    let status: Vec<&str> = sheets.iter().map(|x| x["status"].as_str().unwrap()).collect();
    assert_eq!(status, ["ready", "default", "default", "error", "error", "error", "default", "ready"]);
    let reason = |i: usize| sheets[i]["reason"].as_str().unwrap_or("").to_string();
    assert!(reason(1).contains("default page setup"), "{}", reason(1));
    assert!(reason(2).contains("Model space has no page setup"), "{}", reason(2));
    assert!(reason(3).contains("model space is empty"), "the new drawing has nothing in model space: {}", reason(3));
    assert!(reason(4).contains("no layout `Nope`"), "{}", reason(4));
    assert!(reason(5).contains("not a drawing"), "unreadable file: {}", reason(5));
    assert!(reason(6).contains("Gone Plotter") && reason(6).contains("isn't set up"), "device not found: {}", reason(6));
    assert!(reason(7).contains("changed for this run"), "{}", reason(7));
    assert!(sheets[0]["reason"].is_null(), "a ready sheet has no reason");
    assert_eq!((r["ready"].as_u64(), r["default"].as_u64(), r["errors"].as_u64()), (Some(2), Some(3), Some(3)));
    // The files it would write, without writing them.
    assert_eq!(sheets[0]["file"], "Drawing1-Layout2.pdf");
    assert_eq!(sheets[6]["file"], "office.gone-Layout1.pdf");
    assert_eq!(sheets[7]["file"], "Drawing1-Layout1 (2).pdf", "the same names a run would give");
    assert!(sheets[4]["file"].is_null());
    let after: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
    assert_eq!(before, after, "status writes no files");
    // Combining: every sheet that works is a page of one file.
    let r = s
        .execute("publish", &json!({"action": "status", "combine": true, "sheets": [{"drawing": 0, "layout": "Layout1"}, {"layout": "Nope"}, {"drawing": 0, "layout": "Layout2"}]}))
        .unwrap();
    assert_eq!(r["combined"], json!({"file": "Drawing1.pdf", "pages": 2}));
    let pages: Vec<Option<u64>> = r["sheets"].as_array().unwrap().iter().map(|x| x["page"].as_u64()).collect();
    assert_eq!(pages, [Some(1), None, Some(2)]);
    // Publishing an empty model space fails that sheet the way status said it would.
    let e = s.execute("publish", &json!({"sheets": [{"layout": "Model"}]})).unwrap_err().to_string();
    assert!(e.contains("model space is empty"), "{e}");
    // A bad action is refused.
    assert!(s.execute("publish", &json!({"action": "check", "sheets": [{"layout": "Layout1"}]})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_empty_layout_fails_and_a_missing_folder_is_made() {
    let mut s = session();
    s.execute("new", &json!({})).unwrap();
    // The new drawing's layouts have nothing on them: status says so and publishing fails them.
    let r = s.execute("publish", &json!({"action": "status", "sheets": [{"layout": "Layout1"}, {"drawing": 0, "layout": "Layout1"}]})).unwrap();
    let status: Vec<&str> = r["sheets"].as_array().unwrap().iter().map(|x| x["status"].as_str().unwrap()).collect();
    assert_eq!(status, ["error", "default"]);
    assert!(r["sheets"][0]["reason"].as_str().unwrap().contains("layout `Layout1` is empty"));
    let dir = temp("made").join("not").join("there yet");
    let _ = std::fs::remove_dir_all(&dir);
    let r = s
        .execute("publish", &json!({"folder": dir.to_string_lossy(), "sheets": [{"layout": "Layout1"}, {"drawing": 0, "layout": "Layout1"}]}))
        .unwrap();
    assert_eq!((r["published"].as_u64(), r["failed"].as_u64()), (Some(1), Some(1)));
    assert!(std::path::Path::new(r["sheets"][1]["path"].as_str().unwrap()).exists(), "the folder was made and the file written");
    let _ = std::fs::remove_dir_all(temp("made"));
}
