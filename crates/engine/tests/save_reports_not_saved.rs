//! An incomplete DWG save is refused before writing or marking the drawing saved. DXF keeps
//! foreign entities, and supported modeler geometry is not mistaken for an unsupported type.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::doc::{Block, Common, Drawing, EntityKind, RawTag, Space, Unknown};
use serde_json::json;

fn unknown(kind: &str) -> EntityKind {
    EntityKind::Unknown(Unknown { dxf_type: kind.into(), tags: vec![RawTag { code: 8, value: "0".into() }] })
}

#[test]
fn an_incomplete_save_keeps_the_original_file_and_unsaved_changes() {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Ok(b"0\nEOF\n".to_vec()), plot: None });
    let mut d = Drawing::new_imperial();
    for k in ["MLINE", "3DSOLID", "MLINE"] {
        d.add(&Space::Model, Common::default(), unknown(k)).unwrap();
    }
    let mut b = Block::new("Part");
    let h = d.new_handle();
    b.entities.push(cadcraft_engine::doc::Entity { handle: h, common: Common::default(), kind: unknown("REGION") });
    d.blocks.insert("Part".into(), std::sync::Arc::new(b));
    let mut s = Session::empty();
    let path = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.dwg", std::process::id()));
    std::fs::write(&path, b"original drawing").unwrap();
    s.open_drawing(d, "foreign.dwg", Some(path.to_string_lossy().into_owned()));
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let st = s.state().unwrap();
    let (doc, saved, undo, revision) = (st.doc.clone(), st.saved.clone(), st.undo.len(), st.revision);
    let template = path.with_extension("dwt");
    for (command, params) in [
        ("qsave", json!({})),
        ("save", json!({})),
        ("saveas", json!({ "path": path.to_string_lossy() })),
        ("saveas", json!({ "path": template.to_string_lossy() })),
        ("document.bytes", json!({ "format": "dwg" })),
        ("document.bytes", json!({ "format": "dwt" })),
    ] {
        let result = if command == "save" { s.cmdline("SAVE {}").map(|()| json!(null)) } else { s.execute(command, &params) };
        let error = result.unwrap_err().to_string();
        assert!(error.contains("1 3DSOLID, 2 MLINE, 1 REGION") && error.contains("SAVEAS") && error.contains("DXF"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"original drawing");
        assert!(!template.exists() && !std::path::Path::new(&format!("{}.cadcraft-tmp", path.display())).exists());
        let st = s.state().unwrap();
        assert!(st.is_dirty() && std::sync::Arc::ptr_eq(&doc, &st.doc) && std::sync::Arc::ptr_eq(&saved, &st.saved));
        assert_eq!((st.undo.len(), st.revision), (undo, revision));
        assert_eq!(st.path.as_deref(), path.to_str());
        assert_eq!(st.title, "foreign.dwg");
    }
    let _ = std::fs::remove_file(&path);

    // DXF keeps them.
    let dxf = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.dxf", std::process::id()));
    let n = s.log.len();
    let r = s.execute("saveas", &json!({ "path": dxf.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&dxf);
    assert!(r.get("notSaved").is_none() && s.log.len() == n);

    // Exporting an image isn't a save of the drawing: no report.
    let png = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.png", std::process::id()));
    let r = s.execute("saveas", &json!({ "path": png.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&png);
    assert!(r.get("notSaved").is_none());

    // A drawing the file keeps completely: nothing to report.
    s.open_drawing(Drawing::new_imperial(), "plain.dxf", None);
    let n = s.log.len();
    let r = s.execute("document.bytes", &json!({})).unwrap();
    assert!(r.get("notSaved").is_none() && s.log.len() == n);
}

#[test]
fn inline_sat_regions_are_not_reported_as_lost() {
    use cadcraft_engine::cmd::file::{Version, not_saved_as};
    let mut d = Drawing::new_imperial();
    for _ in 0..72 {
        let mut kind = unknown("REGION");
        let EntityKind::Unknown(u) = &mut kind else { unreachable!() };
        // SAT stays opaque here; the DWG bridge validates conversion to modern binary data.
        u.tags.extend([RawTag { code: 70, value: "1".into() }, RawTag { code: 1, value: "encoded SAT geometry".into() }]);
        d.add(&Space::Model, Common::default(), kind).unwrap();
    }
    for version in Version::ALL {
        assert!(not_saved_as(&d, version).is_empty(), "{version:?}");
    }
}

#[test]
fn preflight_checks_the_drawing_outside_the_block_editor() {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Ok(b"0\nEOF\n".to_vec()), plot: None });
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), unknown("REGION")).unwrap();
    d.blocks.insert("Empty".into(), std::sync::Arc::new(Block::new("Empty")));
    let mut s = Session::empty();
    s.open_drawing(d, "original.dwg", None);
    s.execute("bedit", &json!({ "name": "Empty" })).unwrap();
    assert!(s.doc().unwrap().model.is_empty());
    let error = s.execute("document.bytes", &json!({ "format": "dwg" })).unwrap_err().to_string();
    assert!(error.contains("1 REGION"), "{error}");
}

#[test]
fn templates_report_what_dwg_does_not_keep() {
    use cadcraft_engine::cmd::file::drops_unknown;
    assert!(drops_unknown("a.dwg") && drops_unknown("B.DWT"));
    assert!(!drops_unknown("a.dxf"));
}

#[test]
fn solids_whose_geometry_the_data_store_keeps_are_not_reported_from_2013() {
    use cadcraft_engine::cmd::file::{Version, not_saved_as};
    use cadcraft_engine::doc::Handle;
    use cadcraft_engine::doc::foreign::Foreign;
    let mut d = Drawing::new_imperial();
    let kept = d.add(&Space::Model, Common::default(), unknown("3DSOLID")).unwrap();
    d.add(&Space::Model, Common::default(), unknown("3DSOLID")).unwrap();
    d.add(&Space::Model, Common::default(), unknown("REGION")).unwrap();
    let record = |h: Handle| {
        vec![
            RawTag { code: 0, value: "ACDSRECORD".into() },
            RawTag { code: 320, value: h.hex() },
            RawTag { code: 2, value: "ASM_Data".into() },
            RawTag { code: 310, value: "00".into() },
        ]
    };
    d.foreign = std::sync::Arc::new(Foreign { data_store: record(kept), ..Foreign::default() });
    // The solid with a data store record keeps its geometry from 2013 on; the others don't.
    assert_eq!(not_saved_as(&d, Version::R2018), [("3DSOLID".to_string(), 1), ("REGION".to_string(), 1)].into_iter().collect());
    // Before 2013 a file has no data store: every solid loses its geometry.
    assert_eq!(not_saved_as(&d, Version::R2010), [("3DSOLID".to_string(), 2), ("REGION".to_string(), 1)].into_iter().collect());
}
