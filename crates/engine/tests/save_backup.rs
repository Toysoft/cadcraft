//! Saving over a drawing keeps the previous file as `<name>.bak` (ISAVEBAK, #540), and the
//! save itself replaces the file atomically (craft-atomic): no staging files are left behind.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, backup_path, looks_like_drawing, set_io};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadcraft_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadcraft_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
    });
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadcraft-bak-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn circles(path: &std::path::Path) -> usize {
    let d = cadcraft_io::read(&std::fs::read(path).unwrap(), &path.to_string_lossy()).unwrap();
    d.model.iter().filter(|e| matches!(e.kind, cadcraft_engine::doc::EntityKind::Circle(_))).count()
}

/// Only the drawing and its backup are in the folder: atomic saves leave no staging files.
fn files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

fn add_circle(s: &mut Session, x: f64) {
    s.execute("circle", &json!({ "center": [x, 0], "radius": 1 })).unwrap();
}

#[test]
fn each_save_keeps_the_previous_file_as_bak() {
    hooks();
    let dir = temp_dir("rotate");
    let path = dir.join("Plan.dxf");
    let p = path.to_string_lossy().to_string();
    let mut s = Session::new();
    add_circle(&mut s, 0.0);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    assert_eq!(files(&dir), ["Plan.dxf"], "a first save has nothing to back up");

    add_circle(&mut s, 5.0);
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!(files(&dir), ["Plan.bak", "Plan.dxf"]);
    assert_eq!((circles(&path), circles(&backup_path(&path))), (2, 1), "the .bak is the previous save");

    add_circle(&mut s, 10.0);
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!((circles(&path), circles(&backup_path(&path))), (3, 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_file_never_replaces_a_good_backup_and_isavebak_0_keeps_none() {
    hooks();
    let dir = temp_dir("damaged");
    let path = dir.join("Plan.dxf");
    let p = path.to_string_lossy().to_string();
    let mut s = Session::new();
    add_circle(&mut s, 0.0);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    add_circle(&mut s, 5.0);
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!(circles(&backup_path(&path)), 1);

    // Another program truncates the drawing; the next save keeps the good backup.
    std::fs::write(&path, b"").unwrap();
    add_circle(&mut s, 10.0);
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!((circles(&path), circles(&backup_path(&path))), (3, 1));

    // ISAVEBAK 0: the backup is left as it was.
    s.execute("setvar", &json!({ "name": "ISAVEBAK", "value": 0 })).unwrap();
    add_circle(&mut s, 15.0);
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!((circles(&path), circles(&backup_path(&path))), (4, 1));
    s.execute("setvar", &json!({ "name": "ISAVEBAK", "value": 1 })).unwrap();
    assert_eq!(files(&dir), ["Plan.bak", "Plan.dxf"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drawings_are_recognised_by_their_start() {
    assert!(looks_like_drawing(b"AC1032\0\0\0\0\0"));
    assert!(looks_like_drawing(b"  0\r\nSECTION\r\n  2\r\nHEADER\r\n"));
    assert!(looks_like_drawing(b"999\ndxfrw\n0\nSECTION\n"));
    assert!(looks_like_drawing(b"AutoCAD Binary DXF\r\n\x1a\0"));
    for junk in [&b""[..], b"\0\0\0\0", b"0\nEOF\n", b"<svg>", b"hello"] {
        assert!(!looks_like_drawing(junk), "{junk:?}");
    }
}
