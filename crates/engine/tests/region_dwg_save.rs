//! QSAVE with real file-format hooks: preserve imported REGIONs or leave the original alone.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::doc::foreign::Foreign;
use cadcraft_engine::doc::{Common, Drawing, EntityKind, RawTag, Space, Unknown};
use serde_json::json;

// Opaque synthetic modeler bytes, as in the IO data-store preservation tests.
const SAB_HEX: &str = "414349532042696E61727946696C65000102030405060708090A0B0C0D0E0F456E642D6F662D414349532D64617461";

fn tag(code: i32, value: impl Into<String>) -> RawTag {
    RawTag { code, value: value.into() }
}

fn install() {
    set_io(IoHooks {
        read: |bytes, name| cadcraft_io::read(bytes, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
}

#[test]
fn qsave_keeps_all_72_regions_and_their_binary_geometry() {
    install();
    let mut d = Drawing::new_imperial();
    let mut data_store = vec![tag(70, "2"), tag(71, "2")];
    for _ in 0..72 {
        let kind = EntityKind::Unknown(Unknown {
            dxf_type: "REGION".into(),
            tags: vec![tag(100, "AcDbModelerGeometry"), tag(290, "0"), tag(2, "{00000000-0000-0000-0000-000000000000}")],
        });
        let handle = d.add(&Space::Model, Common::default(), kind).unwrap();
        data_store.extend([
            tag(0, "ACDSRECORD"),
            tag(90, "1"),
            tag(2, "AcDbDs::ID"),
            tag(280, "10"),
            tag(320, handle.hex()),
            tag(2, "ASM_Data"),
            tag(280, "15"),
            tag(94, (SAB_HEX.len() / 2).to_string()),
            tag(310, SAB_HEX),
        ]);
    }
    d.foreign = Arc::new(Foreign { data_store, ..Foreign::default() });
    let path = std::env::temp_dir().join(format!("cadcraft-72-regions-{}.dwg", std::process::id()));
    std::fs::write(&path, cadcraft_io::write(&d, "regions.dwg").unwrap()).unwrap();
    let mut s = Session::empty();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    assert!(s.state().unwrap().is_dirty());
    let result = s.execute("qsave", &json!({})).unwrap();
    assert!(result.get("notSaved").is_none());
    assert!(!s.state().unwrap().is_dirty());
    let bytes = std::fs::read(&path).unwrap();
    let reopened = cadcraft_io::read(&bytes, "regions.dwg").unwrap();
    assert_eq!(reopened.model.iter().filter(|e| matches!(&e.kind, EntityKind::Unknown(u) if u.dxf_type == "REGION")).count(), 72);
    let payloads: Vec<_> = reopened.foreign.data_store.iter().filter(|t| t.code == 310).map(|t| t.value.as_str()).collect();
    assert_eq!(payloads, vec![SAB_HEX; 72]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn failed_modeler_conversion_keeps_the_original_and_offers_a_dxf_backup() {
    install();
    let mut d = Drawing::new_imperial();
    let payload = "opaque geometry the codec cannot convert";
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Unknown(Unknown { dxf_type: "REGION".into(), tags: vec![tag(100, "AcDbModelerGeometry"), tag(70, "1"), tag(1, payload)] }),
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!("cadcraft-unconvertible-region-{}.dwg", std::process::id()));
    std::fs::write(&path, b"original drawing").unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, "original.dwg", Some(path.to_string_lossy().into_owned()));
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let st = s.state().unwrap();
    let (doc, saved) = (st.doc.clone(), st.saved.clone());
    let error = s.execute("qsave", &json!({})).unwrap_err().to_string();
    assert!(error.contains("REGION") && error.contains("SAVEAS") && error.contains("DXF"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"original drawing");
    let st = s.state().unwrap();
    assert!(st.is_dirty() && Arc::ptr_eq(&doc, &st.doc) && Arc::ptr_eq(&saved, &st.saved));
    let backup = path.with_extension("dxf");
    s.execute("saveas", &json!({ "path": backup.to_string_lossy() })).unwrap();
    let d = cadcraft_io::read(&std::fs::read(&backup).unwrap(), "backup.dxf").unwrap();
    assert!(d.model.iter().any(|e| matches!(&e.kind, EntityKind::Unknown(u) if u.tags.iter().any(|t| t.code == 1 && t.value == payload))));
    assert_eq!(std::fs::read(&path).unwrap(), b"original drawing");
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(backup);
}
