//! Drawing history (#OMNI): the OMNI toolkit ported from AutoLISP to native commands —
//! snapshots in `_OMNI_HISTORY` next to the drawing (DXF and DWG alike), the listing and opening
//! of a revision, the overlay diff laid under the drawing (recoloured, toggleable, detachable),
//! the native change report, and spatial notes.

use std::path::{Path, PathBuf};

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::doc::color::Color;
use cadcraft_engine::doc::geom::Vec3;
use cadcraft_engine::doc::{Drawing, EntityKind, Layer, Line, XrefStatus};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
}

/// A fresh folder for one test.
fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-omni-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::write(path, cadcraft_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

/// A drawing with a wall line on a red layer.
fn plan() -> Drawing {
    let mut d = Drawing::new_metric();
    d.layers.push(Layer { name: "Walls".into(), color: Color::Index(1), ..Layer::default() });
    let h = d.new_handle();
    let mut e = cadcraft_engine::doc::Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) }));
    e.common.layer = "Walls".into();
    d.model.push(e);
    d
}

fn open(s: &mut Session, path: &Path) {
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
}

fn history(dir: &Path) -> PathBuf {
    dir.join("_OMNI_HISTORY")
}

fn snapshots(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> =
        std::fs::read_dir(history(dir)).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// The name a snapshot of `plan.dxf` gets: `<date>.<time>_<ms>_<user>_<description>_plan.dxf`.
fn named(name: &str, description: &str) -> bool {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let f: Vec<&str> = stem.split('_').collect();
    let time = f.first().copied().unwrap_or_default();
    let (d, t) = time.split_once('.').unwrap_or(("", ""));
    f.get(3) == Some(&description)
        && f.get(4) == Some(&"plan")
        && ext == "dxf"
        && d.len() == 8
        && d.bytes().all(|b| b.is_ascii_digit())
        && t.len() == 6
        && t.bytes().all(|b| b.is_ascii_digit())
        && f.get(1).is_some_and(|ms| ms.len() == 3 && ms.bytes().all(|b| b.is_ascii_digit()))
        && !f.get(2).unwrap_or(&"").is_empty()
}

#[test]
fn a_snapshot_is_the_drawing_as_it_is_now_not_the_file_on_disk() {
    hooks();
    let dir = folder("snap");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();
    open(&mut s, &plan_path);

    // A change the file on disk doesn't have yet.
    s.execute("circle", &json!({ "center": [50, 50], "radius": 5 })).unwrap();
    let r = s.execute("omni-snap", &json!({ "description": "walls done" })).unwrap();
    assert_eq!(r["description"], "walls-done", "the reply says what the file name says, spaces folded into dashes");
    assert_eq!(r["snapshots"], 1);
    let name = r["file"].as_str().unwrap().to_string();
    assert!(name.ends_with("_walls-done_plan.dxf"), "{name}: a description's spaces become dashes");
    assert!(named(&name, "walls-done"), "{name}: the name carries date, time, milliseconds, user and description");

    // The snapshot holds the unsaved circle; the drawing's own file still doesn't.
    let snap = cadcraft_io::read(&std::fs::read(r["path"].as_str().unwrap()).unwrap(), &name).unwrap();
    assert_eq!(snap.model.len(), 2, "the snapshot is the drawing, not the file");
    let disk = cadcraft_io::read(&std::fs::read(&plan_path).unwrap(), "plan.dxf").unwrap();
    assert_eq!(disk.model.len(), 1, "taking a snapshot does not touch the drawing's file");
    assert!(std::path::Path::new(r["folder"].as_str().unwrap()).ends_with("_OMNI_HISTORY"));
    assert!(snapshots(&dir).len() == 1);
}

#[test]
fn snapshots_are_dxf_or_dwg_after_the_drawing_they_came_from() {
    hooks();
    let dir = folder("formats");
    for ext in ["dxf", "dwg"] {
        let path = dir.join(format!("plan.{ext}"));
        save(&plan(), &path);
        let mut s = Session::new();
        open(&mut s, &path);
        s.execute("line", &json!({ "points": [[0, 5], [10, 5]] })).unwrap();
        let r = s.execute("omni-snap", &json!({ "description": "two walls" })).unwrap();
        let file = r["file"].as_str().unwrap();
        assert!(file.ends_with(&format!("_two-walls_plan.{ext}")), "{file}: a DWG history stays DWG");
        let bytes = std::fs::read(r["path"].as_str().unwrap()).unwrap();
        let back = cadcraft_io::read(&bytes, file).unwrap();
        assert_eq!(back.model.len(), 2, "{ext}: the snapshot reads back");
    }
}

#[test]
fn log_lists_newest_first_and_opens_a_revision() {
    hooks();
    let dir = folder("log");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();
    open(&mut s, &plan_path);
    s.execute("omni-snap", &json!({ "description": "first" })).unwrap();
    s.execute("circle", &json!({ "center": [1, 1], "radius": 2 })).unwrap();
    s.execute("omni-snap", &json!({ "description": "second" })).unwrap();

    let r = s.execute("omni-log", &json!({})).unwrap();
    assert_eq!(r["count"], 2);
    assert_eq!(r["capped"], json!(false));
    assert_eq!(r["snapshots"][0]["description"], "second", "newest first");
    assert_eq!(r["snapshots"][1]["description"], "first");
    assert_eq!(r["snapshots"][0]["n"], 1);
    assert!(r["snapshots"][0]["time"].as_str().unwrap().ends_with('Z'), "an ISO-8601 UTC stamp");
    assert_eq!(r["snapshots"][1]["drawing"], "plan.dxf");
    assert!(!r["snapshots"][0]["user"].as_str().unwrap().is_empty());

    // Opening a revision brings it in as a drawing of its own, with what it held. A session
    // starts with one untitled drawing, so opening two files leaves three.
    let plan = plan_path.to_string_lossy().into_owned();
    let r = s.execute("omni-log", &json!({ "open": 2 })).unwrap();
    assert_eq!(r["opened"], r["snapshots"][1]["file"]);
    assert_eq!(s.docs.len(), 3);
    assert_eq!(s.doc().unwrap().model.len(), 1, "the older revision has one object");

    // The revision is the open drawing now, so a call without `path` asks about *its* history:
    // a snapshot in `_OMNI_HISTORY` has its own, in `_OMNI_HISTORY/_OMNI_HISTORY`.
    let r = s.execute("omni-log", &json!({})).unwrap();
    assert_eq!(r["count"], 0);
    assert!(r["folder"].as_str().unwrap().ends_with("_OMNI_HISTORY"), "{}", r["folder"]);

    // Naming the drawing puts the calls back on its history, by file name and by path alike.
    let r = s.execute("omni-log", &json!({ "path": plan })).unwrap();
    let file = r["snapshots"][0]["file"].as_str().unwrap().to_string();
    let r = s.execute("omni-log", &json!({ "path": plan, "open": file })).unwrap();
    assert_eq!(r["opened"], json!(file));
    assert_eq!(s.docs.len(), 4);
    assert_eq!(s.doc().unwrap().model.len(), 2);
    let path = r["snapshots"][0]["path"].as_str().unwrap().to_string();
    s.execute("omni-log", &json!({ "path": plan, "open": path })).unwrap();
    assert_eq!(s.docs.len(), 5);
}

#[test]
fn diff_lays_the_snapshot_under_the_drawing_recoloured_and_takes_it_away_again() {
    hooks();
    let dir = folder("diff");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    // A drawing of the user's own, which OMNI-CLEAR must leave alone.
    let mut site = Drawing::new_metric();
    let h = site.new_handle();
    site.model.push(cadcraft_engine::doc::Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(0.0, 10.0, 0.0) })));
    save(&site, &dir.join("site.dxf"));
    let mut s = Session::new();
    open(&mut s, &plan_path);
    s.execute("omni-snap", &json!({ "description": "revision" })).unwrap();
    s.execute("xattach", &json!({ "path": "site.dxf" })).unwrap();
    s.execute("circle", &json!({ "center": [20, 20], "radius": 3 })).unwrap();

    let r = s.execute("omni-diff", &json!({})).unwrap();
    let overlay = r["overlay"].as_str().unwrap().to_string();
    assert!(overlay.starts_with("OMNI_plan_"), "{overlay}");
    assert_eq!(r["mode"], "overlay");
    assert!(r["objects"].as_u64().unwrap() >= 1);
    assert_eq!(r["layers"].as_u64(), Some(2), "the snapshot's Walls layer and the Defpoints every drawing carries");
    let d = s.doc().unwrap();
    let b = d.block(&overlay).unwrap();
    assert_eq!(b.xref_status, XrefStatus::Loaded);
    assert!(b.xref_overlay, "an overlay, as the AutoLISP original attached it");
    let path = b.xref_path.clone().unwrap();
    assert!(path.starts_with("_OMNI_HISTORY/"), "{path}: relative with a forward slash, so the drawing resolves it on every platform");
    assert!(path.ends_with("plan.dxf"), "{path}");
    assert_eq!(d.layer(&format!("{overlay}|Walls")).map(|l| l.color), Some(Color::Index(1)), "the underlay is red");
    assert!(b.entities.iter().all(|e| e.common.color == Color::Index(1)), "the underlay's objects are red, not only its layers");
    assert!(d.block("site").is_some(), "the user's own xref is untouched");

    // A colour of the caller's choosing.
    s.execute("omni-diff", &json!({ "color": 5 })).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.layer(&format!("{overlay}|Walls")).map(|l| l.color), Some(Color::Index(5)));
    assert_eq!(
        d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(&overlay))).count(),
        1,
        "one reference, not a second one"
    );

    // Toggle: the underlay's objects leave the drawing and come back on reload, with the red
    // their layers were given, not the snapshot's own colours.
    let r = s.execute("omni-toggle", &json!({})).unwrap();
    assert_eq!(r["shown"], json!(false));
    assert_eq!(r["objects"], 0);
    assert_eq!(s.doc().unwrap().block(&overlay).map(|b| b.xref_status), Some(XrefStatus::Unloaded));
    assert!(s.doc().unwrap().block(&overlay).is_some(), "the reference stays attached, it is only unloaded");
    let r = s.execute("omni-toggle", &json!({ "show": true })).unwrap();
    assert_eq!(r["shown"], json!(true));
    assert!(r["objects"].as_u64().unwrap() >= 1);
    let d = s.doc().unwrap();
    assert_eq!(s.doc().unwrap().block(&overlay).map(|b| b.xref_status), Some(XrefStatus::Loaded));
    assert_eq!(d.layer(&format!("{overlay}|Walls")).map(|l| l.color), Some(Color::Index(5)), "the colour survives a reload");
    assert!(d.block(&overlay).is_some_and(|b| b.entities.iter().all(|e| e.common.color == Color::Index(5))), "and so does the underlay's own red");

    // Clear: the overlay goes, the user's xref and the drawing's own objects stay.
    let r = s.execute("omni-clear", &json!({})).unwrap();
    assert_eq!(r["cleared"], json!([overlay.clone()]));
    assert_eq!(r["erased"], 1, "its reference is erased with it");
    let d = s.doc().unwrap();
    assert!(d.block(&overlay).is_none());
    assert!(d.layers.iter().all(|l| !l.name.contains('|') || l.name.starts_with("site|")), "its layers go, the user's stay");
    assert!(d.block("site").is_some());
    assert_eq!(d.model.len(), 3, "the drawing's own line and circle, and the user's site reference");
    // Nothing left to clear, and nothing left to toggle.
    assert_eq!(s.execute("omni-clear", &json!({})).unwrap()["cleared"], json!([]));
    assert!(s.execute("omni-toggle", &json!({})).is_err());
    assert!(s.execute("omni-clear", &json!({})).is_ok(), "clearing twice is not an error");
}

#[test]
fn diff_can_report_what_changed_instead_of_only_showing_it() {
    hooks();
    let dir = folder("report");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();
    open(&mut s, &plan_path);
    s.execute("omni-snap", &json!({ "description": "before" })).unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 1 })).unwrap();

    let r = s.execute("omni-diff", &json!({ "mode": "report" })).unwrap();
    assert_eq!(r["counts"]["added"], 1, "the circle is only in this drawing");
    assert_eq!(r["counts"]["removed"], 0);
    assert_eq!(r["counts"]["common"], 1, "the wall line is in both");
    assert_eq!(r["regions"].as_array().map(Vec::len), Some(1));
    assert!(r["overlay"].is_null(), "report mode attaches nothing");
    assert!(s.doc().unwrap().blocks.values().all(|b| b.xref_path.is_none()));

    // Both: the overlay and the numbers in one call.
    let r = s.execute("omni-diff", &json!({ "mode": "both", "snapshot": 1 })).unwrap();
    assert!(r["overlay"].as_str().is_some_and(|n| n.starts_with("OMNI_")));
    assert_eq!(r["counts"]["added"], 1);
    // And no differences when the snapshot is the drawing as it stands.
    s.execute("omni-snap", &json!({ "description": "now" })).unwrap();
    let r = s.execute("omni-diff", &json!({ "mode": "report", "snapshot": 1 })).unwrap();
    assert_eq!(r["counts"]["added"], 0);
    assert_eq!(r["counts"]["changed"], 0);
    assert_eq!(r["counts"]["common"], 2);
}

#[test]
fn notes_are_placed_on_their_own_red_layer() {
    hooks();
    let dir = folder("note");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();
    open(&mut s, &plan_path);

    let r = s.execute("omni-note", &json!({ "at": [10, 10], "text": "check this wall" })).unwrap();
    assert_eq!(r["layer"], "OMNI_NOTES");
    let d = s.doc().unwrap();
    assert_eq!(d.layer("OMNI_NOTES").map(|l| l.color), Some(Color::Index(1)), "the note layer is red");
    let text = d.entity(cadcraft_engine::doc::Handle::parse_hex(r["textHandle"].as_str().unwrap()).unwrap()).unwrap();
    assert!(matches!(&text.kind, EntityKind::Text(t) if t.value == "check this wall"));
    assert_eq!(text.common.layer, "OMNI_NOTES");
    let marker = d.entity(cadcraft_engine::doc::Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap()).unwrap();
    assert!(matches!(&marker.kind, EntityKind::Circle(c) if (c.radius - 50.0).abs() < 1e-9), "the default marker radius");

    // A layer of the caller's own, with its own settings, and the sizes asked for.
    let r = s.execute("omni-note", &json!({ "at": [1, 2], "text": "2", "layer": "REVIEW", "height": 3, "radius": 4 })).unwrap();
    assert_eq!(r["layer"], "REVIEW");
    assert_eq!(s.doc().unwrap().layer("REVIEW").map(|l| l.color), Some(Color::Index(1)));
    assert!(s.undo().unwrap().is_some(), "a note undoes like any other drawing change");
    assert_eq!(s.doc().unwrap().model.len(), 3, "the line, the first note's two objects minus the undone one");
}

#[test]
fn the_commands_are_typed_like_the_autolisp_ones() {
    hooks();
    let dir = folder("cmdline");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();
    open(&mut s, &plan_path);

    // OMNI-SNAP asks for the description (Enter takes the default).
    s.script("OMNI-SNAP\nwalls done\n").unwrap();
    assert!(s.running.is_none(), "finished: {}", s.prompt_text());
    assert_eq!(snapshots(&dir).len(), 1);
    assert!(snapshots(&dir)[0].contains("walls-done"), "{:?}", snapshots(&dir));

    // OMNI-LOG lists, then opens the number asked for.
    s.script("OMNI-LOG\n1\n").unwrap();
    assert!(s.running.is_none(), "finished: {}", s.prompt_text());
    assert_eq!(s.docs.len(), 3, "the untitled drawing, the plan, and the revision opened");
    assert!(s.log.iter().any(|m| m.contains("Snapshot history")), "{:?}", s.log);

    // OMNI-NOTE: the point, then the comment, on the plan itself.
    s.execute("document.switch", &json!({ "index": 1 })).unwrap();
    s.script("OMNI-NOTE\n5,5\nпроём 900\n").unwrap();
    assert!(s.running.is_none(), "finished: {}", s.prompt_text());
    let d = s.doc().unwrap();
    assert!(d.layer("OMNI_NOTES").is_some(), "the note typed in Russian lands on its layer");
    assert!(d.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "проём 900")));

    // OMNI-DIFF lists, then lays the snapshot under the drawing.
    s.script("OMNI-DIFF\n1\n").unwrap();
    assert!(s.running.is_none(), "finished: {}", s.prompt_text());
    assert!(s.doc().unwrap().blocks.values().any(|b| b.name.starts_with("OMNI_")));
    s.script("OMNI-CLEAR\n").unwrap();
    assert!(s.doc().unwrap().blocks.values().all(|b| !b.name.starts_with("OMNI_")));
    // Aliases, as the short names of every other command.
    assert!(cadcraft_engine::command_specs().iter().any(|c| c.id == "omni-snap" && c.aliases.contains(&"omnisnap")));
    // Cancel leaves everything as it was: 0 at the number prompt ends the command.
    s.script("OMNI-DIFF\n0\n").unwrap();
    assert!(s.running.is_none(), "finished: {}", s.prompt_text());
    assert!(s.doc().unwrap().blocks.values().all(|b| !b.name.starts_with("OMNI_")));
}

#[test]
fn hostile_input_never_panics_and_never_guesses() {
    hooks();
    let dir = folder("hostile");
    let plan_path = dir.join("plan.dxf");
    save(&plan(), &plan_path);
    let mut s = Session::new();

    // A note needs no file, so it works on the untitled drawing a session starts with; the
    // history commands need to know which file the history belongs to.
    assert!(s.execute("omni-snap", &json!({})).is_err());
    assert!(s.execute("omni-log", &json!({})).is_err());
    assert!(s.execute("omni-diff", &json!({})).is_err());
    assert!(s.execute("omni-note", &json!({ "at": [0, 0], "text": "x" })).is_ok());

    open(&mut s, &plan_path);
    // A drawing with no history yet.
    assert!(s.execute("omni-diff", &json!({})).is_err());
    assert!(s.execute("omni-toggle", &json!({})).is_err());
    let r = s.execute("omni-log", &json!({})).unwrap();
    assert_eq!(r["count"], 0);
    assert!(s.log.iter().any(|m| m.contains("No snapshots")), "{:?}", s.log);

    for p in [json!({ "path": "" }), json!({ "path": "a" }), json!({ "path": "plan.txt" }), json!({ "path": "\u{0}" })] {
        assert!(s.execute("omni-snap", &p).is_err(), "{p}");
    }
    // A parameter of the wrong type is ignored, as everywhere else, not guessed at.
    assert!(s.execute("omni-snap", &json!({ "description": 5 })).is_ok());
    assert!(s.execute("omni-snap", &json!({ "path": 7 })).is_ok());
    // A description long enough to flood a file name is cut, not written as is.
    let r = s.execute("omni-snap", &json!({ "description": "очень длинное описание ".repeat(20) })).unwrap();
    assert!(r["file"].as_str().unwrap().chars().count() < 130, "{}", r["file"]);
    assert!(named_or_lenient(r["file"].as_str().unwrap()));

    for p in [
        json!({ "snapshot": 0 }),
        json!({ "snapshot": 99 }),
        json!({ "snapshot": -1 }),
        json!({ "snapshot": "no/such.dxf" }),
        json!({ "snapshot": true }),
        json!({ "mode": "sideways" }),
        json!({ "color": 0 }),
        json!({ "color": 300 }),
        json!({ "color": -3 }),
        json!({ "color": "red" }),
        json!({ "snapshot": [], "mode": null }),
    ] {
        assert!(s.execute("omni-diff", &p).is_err(), "{p}");
    }
    for p in [json!({ "show": 1 }), json!({ "show": "yes" })] {
        assert!(s.execute("omni-toggle", &p).is_err(), "{p}");
    }
    for p in [
        json!({}),
        json!({ "at": [0, 0] }),
        json!({ "at": [0, 0], "text": "" }),
        json!({ "at": [0, 0], "text": "   " }),
        json!({ "at": "nope", "text": "x" }),
        json!({ "at": [0, 0], "text": "x", "height": 0 }),
        json!({ "at": [0, 0], "text": "x", "radius": -1 }),
        json!({ "at": [0, 0], "text": "x", "layer": "bad|name" }),
        json!({ "at": [f64::INFINITY, 0.0], "text": "x" }),
        json!({ "at": [0, 0], "text": 5 }),
    ] {
        assert!(s.execute("omni-note", &p).is_err(), "{p}");
    }
    assert_eq!(s.doc().unwrap().model.len(), 1, "nothing hostile touched the drawing");
}

/// A snapshot of a long description still has a parseable name (its parts are cut, not merged).
fn named_or_lenient(name: &str) -> bool {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let f: Vec<&str> = stem.split('_').collect();
    f.len() >= 5 && ext == "dxf" && f[0].len() == 15 && f[1].len() == 3 && !f[2].is_empty() && !f[3].is_empty()
}

#[test]
fn a_history_survives_on_a_drawing_without_a_file_name_yet() {
    hooks();
    let dir = folder("untitled");
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    // An untitled drawing needs to be told which file its history belongs to.
    assert!(s.execute("omni-snap", &json!({ "description": "wip" })).is_err());
    let r = s.execute("omni-snap", &json!({ "description": "wip", "path": dir.join("wip.dxf").to_string_lossy() })).unwrap();
    assert_eq!(r["snapshots"], 1);
    assert!(r["file"].as_str().unwrap().ends_with("_wip_wip.dxf"), "{}", r["file"]);
    assert!(history(&dir).join(r["file"].as_str().unwrap()).is_file());
    // The list can be read back by the file name alone.
    let r = s.execute("omni-log", &json!({ "path": dir.join("wip.dxf").to_string_lossy() })).unwrap();
    assert_eq!(r["count"], 1);
    assert_eq!(r["snapshots"][0]["drawing"], "wip.dxf");
}
