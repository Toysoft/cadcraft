//! Publish lists (`.ccpub`): a round trip keeps every sheet, tick and page setup change with
//! drawing paths relative to the list; a drawing that is gone is reported as not found; hostile
//! files are refused with a reason; drawing files are listed without opening them, and read once
//! while they don't change.

use std::collections::BTreeMap;
use std::sync::Mutex;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};

/// How often this test's reader decoded each file.
static READS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

fn reads(name: &str) -> usize {
    READS.lock().map(|r| r.get(name).copied().unwrap_or(0)).unwrap_or(0)
}

/// `*.sheets` files are a metric drawing with something in model space and on Layout1.
fn read(bytes: &[u8], name: &str) -> Result<cadcraft_doc::Drawing, String> {
    if !(name.ends_with(".sheets") && bytes.starts_with(b"sheets")) {
        return Err("not a drawing".into());
    }
    if let Ok(mut r) = READS.lock() {
        *r.entry(name.to_string()).or_default() += 1;
    }
    let mut d = cadcraft_doc::Drawing::new_metric();
    for space in [cadcraft_doc::Space::Model, cadcraft_doc::Space::Paper("Layout1".into())] {
        let line = cadcraft_doc::EntityKind::Line(cadcraft_doc::Line { a: cadcraft_geom::Vec3::ZERO, b: cadcraft_geom::Vec3::new(10.0, 10.0, 0.0) });
        let _ = d.add(&space, cadcraft_doc::Common::default(), line);
    }
    Ok(d)
}

fn plot(_: &cadcraft_doc::Drawing, space: &cadcraft_doc::Space, _: &Value) -> Result<Vec<u8>, String> {
    Ok(format!("%PDF-1.4 {space:?}").into_bytes())
}

fn session() -> Session {
    set_io(IoHooks { read, write: |_, _| Err("no writer in this test".into()), plot: Some(plot) });
    Session::new()
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-publish-list-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn drawing(dir: &std::path::Path, name: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, b"sheets").unwrap();
    p.to_string_lossy().to_string()
}

#[test]
fn a_list_round_trips_with_relative_paths() {
    let mut s = session();
    let root = temp("roundtrip");
    let lists = root.join("lists");
    let a = drawing(&lists.join("plans"), "ground floor.sheets");
    let b = drawing(&root.join("site"), "site.sheets");
    let list = lists.join("set.ccpub").to_string_lossy().to_string();
    let sheets = json!([
        {"drawing": a, "layout": "Model"},
        {"drawing": a, "layout": "Layout1", "include": false, "pageSetup": {"paper": "A3", "landscape": true, "fit": false}},
        {"drawing": b, "layout": "Layout1"},
    ]);
    let output = json!({"to": "PDF", "combine": true, "folder": root.join("out").to_string_lossy(), "name": "{drawing} set", "layers": false, "stamp": {"text": "{sheet} {date}", "corner": "top-left"}});
    let r = s.execute("publishlist", &json!({"action": "write", "path": list, "sheets": sheets, "output": output})).unwrap();
    assert_eq!(r["sheets"], 3);
    // On disk: our own format, paths relative to the list's folder.
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
    assert_eq!((file["cadcraft"].as_str(), file["version"].as_u64()), (Some("publish-list"), Some(1)));
    let written: Vec<&str> = file["sheets"].as_array().unwrap().iter().map(|x| x["drawing"].as_str().unwrap()).collect();
    assert_eq!(written, ["plans/ground floor.sheets", "plans/ground floor.sheets", "../site/site.sheets"]);
    assert_eq!(file["output"]["folder"], "../out");
    // Read back: absolute paths again, everything kept, and the drawings found.
    let r = s.execute("publishlist", &json!({"action": "read", "path": list})).unwrap();
    let back = r["sheets"].as_array().unwrap();
    assert_eq!(back[0]["drawing"].as_str(), Some(a.as_str()));
    assert_eq!(back[2]["drawing"].as_str(), Some(b.as_str()));
    assert!(back.iter().all(|x| x["found"] == true));
    assert_eq!(back[1]["include"], false);
    assert_eq!(back[1]["pageSetup"], json!({"paper": "A3", "landscape": true, "fit": false}));
    assert_eq!(r["output"]["to"], "pdf");
    assert_eq!(r["output"]["folder"].as_str().map(std::path::PathBuf::from), Some(root.join("out")));
    assert_eq!((r["output"]["combine"].as_bool(), r["output"]["layers"].as_bool()), (Some(true), Some(false)));
    assert_eq!(r["output"]["stamp"], json!({"text": "{sheet} {date}", "corner": "top-left"}));
    // The folder of drawings and its list can move together.
    let moved = temp("moved");
    std::fs::rename(&root, moved.join("project")).unwrap();
    let r = s.execute("publishlist", &json!({"action": "read", "path": moved.join("project/lists/set.ccpub").to_string_lossy()})).unwrap();
    assert!(r["sheets"].as_array().unwrap().iter().all(|x| x["found"] == true), "{r}");
    let _ = std::fs::remove_dir_all(moved);
}

#[test]
fn a_drawing_that_is_gone_is_reported_not_found() {
    let mut s = session();
    let dir = temp("missing");
    let a = drawing(&dir, "here.sheets");
    let list = dir.join("l.ccpub").to_string_lossy().to_string();
    let gone = dir.join("gone.sheets").to_string_lossy().to_string();
    s.execute(
        "publishlist",
        &json!({"action": "write", "path": list, "sheets": [{"drawing": a, "layout": "Model"}, {"drawing": gone, "layout": "Layout1"}]}),
    )
    .unwrap();
    let r = s.execute("publishlist", &json!({"action": "read", "path": list})).unwrap();
    let found: Vec<bool> = r["sheets"].as_array().unwrap().iter().map(|x| x["found"].as_bool().unwrap()).collect();
    assert_eq!(found, [true, false]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hostile_lists_are_refused() {
    let mut s = session();
    let dir = temp("hostile");
    let ok_sheet = json!({"drawing": "a.sheets", "layout": "Model"});
    let many: Vec<Value> = (0..1025).map(|_| ok_sheet.clone()).collect();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("not json", b"<?xml version='1.0'?><list/>".to_vec()),
        ("array", b"[1,2,3]".to_vec()),
        ("wrong kind", json!({"cadcraft": "drawing", "version": 1, "sheets": []}).to_string().into_bytes()),
        ("no version", json!({"cadcraft": "publish-list", "sheets": []}).to_string().into_bytes()),
        ("newer", json!({"cadcraft": "publish-list", "version": 9, "sheets": []}).to_string().into_bytes()),
        ("unknown key", json!({"cadcraft": "publish-list", "version": 1, "sheets": [], "macro": "rm -rf"}).to_string().into_bytes()),
        ("sheets not a list", json!({"cadcraft": "publish-list", "version": 1, "sheets": "all"}).to_string().into_bytes()),
        ("too many sheets", json!({"cadcraft": "publish-list", "version": 1, "sheets": many}).to_string().into_bytes()),
        ("sheet not an object", json!({"cadcraft": "publish-list", "version": 1, "sheets": [3]}).to_string().into_bytes()),
        ("empty drawing", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "", "layout": "Model"}]}).to_string().into_bytes()),
        ("control char path", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a\u{0}b", "layout": "Model"}]}).to_string().into_bytes()),
        ("huge path", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a".repeat(5000), "layout": "Model"}]}).to_string().into_bytes()),
        ("no layout", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a.sheets"}]}).to_string().into_bytes()),
        ("long layout", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a.sheets", "layout": "L".repeat(300)}]}).to_string().into_bytes()),
        ("include text", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a.sheets", "layout": "Model", "include": "yes"}]}).to_string().into_bytes()),
        ("bad paper", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a.sheets", "layout": "Model", "pageSetup": {"paper": "Z99"}}]}).to_string().into_bytes()),
        ("huge paper", json!({"cadcraft": "publish-list", "version": 1, "sheets": [{"drawing": "a.sheets", "layout": "Model", "pageSetup": {"width": 1e300, "height": 1}}]}).to_string().into_bytes()),
        ("bad output", json!({"cadcraft": "publish-list", "version": 1, "sheets": [], "output": {"to": "dwg"}}).to_string().into_bytes()),
        ("escaping name", json!({"cadcraft": "publish-list", "version": 1, "sheets": [], "output": {"name": "../../{sheet}"}}).to_string().into_bytes()),
        ("bad stamp", json!({"cadcraft": "publish-list", "version": 1, "sheets": [], "output": {"stamp": {"text": "{rm}"}}}).to_string().into_bytes()),
        ("unknown output", json!({"cadcraft": "publish-list", "version": 1, "sheets": [], "output": {"printer": "x"}}).to_string().into_bytes()),
        ("deep nesting", format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)).into_bytes()),
        ("too big", vec![b' '; (1 << 20) + 1]),
        ("random bytes", (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()),
    ];
    for (name, bytes) in cases {
        let path = dir.join(format!("{}.ccpub", name.replace(' ', "-")));
        std::fs::write(&path, &bytes).unwrap();
        let r = s.execute("publishlist", &json!({"action": "read", "path": path.to_string_lossy()}));
        assert!(r.is_err(), "{name}: refused, got {r:?}");
    }
    // Writing checks the same fields.
    let list = dir.join("w.ccpub").to_string_lossy().to_string();
    for sheets in [json!("all"), json!([{"drawing": "", "layout": "Model"}]), json!([{"drawing": "a", "layout": "Model", "pageSetup": {"scale": 0}}])]
    {
        assert!(s.execute("publishlist", &json!({"action": "write", "path": list, "sheets": sheets})).is_err(), "{sheets}");
    }
    assert!(!std::path::Path::new(&list).exists(), "nothing written");
    for p in [json!({"action": "read"}), json!({"action": "nope", "path": "x"}), json!({"action": "read", "path": "x", "extra": 1}), json!("read")] {
        assert!(s.execute("publishlist", &p).is_err(), "{p}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_drawing_file_is_listed_without_opening_it_and_read_once() {
    let mut s = session();
    let dir = temp("drawing");
    let a = drawing(&dir, "plan.sheets");
    let r = s.execute("publishlist", &json!({"action": "drawing", "path": a})).unwrap();
    assert_eq!(r["title"], "plan.sheets");
    let names: Vec<&str> = r["sheets"].as_array().unwrap().iter().map(|x| x["layout"].as_str().unwrap()).collect();
    assert_eq!(names, ["Model", "Layout1", "Layout2"]);
    assert!(r["sheets"][1]["page"]["paper"].is_string(), "each layout's page setup comes along");
    assert_eq!(s.docs.len(), 1, "no drawing was opened");
    // Listing it, checking its status and publishing it read the file once while it's unchanged.
    for _ in 0..3 {
        s.execute("publishlist", &json!({"action": "drawing", "path": a})).unwrap();
        s.execute("publish", &json!({"action": "status", "sheets": [{"drawing": a, "layout": "Layout1"}]})).unwrap();
    }
    s.execute("publish", &json!({"sheets": [{"drawing": a, "layout": "Model"}]})).unwrap();
    assert_eq!(reads(&a), 1);
    // A changed file is read again.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&a, b"sheets, changed").unwrap();
    s.execute("publishlist", &json!({"action": "drawing", "path": a})).unwrap();
    assert_eq!(reads(&a), 2);
    // Not a drawing, not there, or a folder: an error, not a crash.
    for bad in [dir.join("nope.sheets").to_string_lossy().to_string(), dir.to_string_lossy().to_string()] {
        assert!(s.execute("publishlist", &json!({"action": "drawing", "path": bad})).is_err());
    }
    let junk = dir.join("junk.dxf");
    std::fs::write(&junk, b"\0\0\0").unwrap();
    assert!(s.execute("publishlist", &json!({"action": "drawing", "path": junk.to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// `target` relative to `base` (both absolute), the way a user would type it.
fn relative_to(base: &std::path::Path, target: &std::path::Path) -> String {
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), b.len() - common).collect();
    parts.extend(t.iter().skip(common).map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

#[test]
fn a_relative_drawing_path_is_relative_to_the_working_folder() {
    let mut s = session();
    let dir = temp("cwd");
    let a = drawing(&dir.join("plans"), "a.sheets");
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let typed = relative_to(&cwd, &std::path::Path::new(&a).canonicalize().unwrap());
    let list = dir.join("lists").join("l.ccpub");
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let list = list.to_string_lossy().to_string();
    s.execute("publishlist", &json!({"action": "write", "path": list, "sheets": [{"drawing": typed, "layout": "Model"}]})).unwrap();
    let r = s.execute("publishlist", &json!({"action": "read", "path": list})).unwrap();
    assert_eq!(r["sheets"][0]["found"], true, "the same file read back: {r}");
    let back = std::path::Path::new(r["sheets"][0]["drawing"].as_str().unwrap()).canonicalize().unwrap();
    assert_eq!(back, std::path::Path::new(&a).canonicalize().unwrap());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn five_drawing_files_are_each_read_once_across_status_checks() {
    let mut s = session();
    let dir = temp("five");
    let files: Vec<String> = (0..5).map(|i| drawing(&dir, &format!("d{i}.sheets"))).collect();
    let sheets: Vec<Value> = files.iter().map(|f| json!({"drawing": f, "layout": "Layout1"})).collect();
    for _ in 0..3 {
        s.execute("publish", &json!({"action": "status", "sheets": sheets})).unwrap();
    }
    for f in &files {
        assert_eq!(reads(f), 1, "{f} read once");
    }
    let _ = std::fs::remove_dir_all(dir);
}
