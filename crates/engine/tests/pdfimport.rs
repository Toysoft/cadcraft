#[path = "../../io/tests/support/pdf.rs"]
mod fixture;
use cadcraft_engine::{
    Session,
    cmd::{file, pdfimport},
};
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
#[test]
fn preview_placement_metre_conversion_rotation_and_single_undo() {
    install();
    let mut s = Session::new();
    s.execute("point", &json!({"at":[100,200]})).unwrap();
    let before = s.state().unwrap().doc.clone();
    let data = file::base64_encode(&fixture::sample());
    let preview = s.execute("importpdf", &json!({"data":data,"page":1,"preview":true,"thumbnail":true})).unwrap();
    assert_eq!(preview["pages"], 2);
    assert_eq!(preview["objects"], 5);
    assert!(preview["thumbnail"].as_str().is_some());
    assert!(std::sync::Arc::ptr_eq(&before, &s.state().unwrap().doc));
    let r = s.execute("importpdf", &json!({"data":data,"page":1,"units":"m","scale":100,"rotation":90,"at":[5,7]})).unwrap();
    assert_eq!(r["imported"], 5);
    assert_eq!(s.doc().unwrap().model.len(), 6);
    let handle = cadcraft_doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();
    let entity = s.doc().unwrap().entity(handle).unwrap();
    let cadcraft_doc::EntityKind::LwPolyline(p) = &entity.kind else { panic!("expected editable path") };
    let k = 25.4 / 72.0 * 0.1;
    assert!((p.vertices[0].p.x - (5.0 - 20.0 * k)).abs() < 1e-6);
    assert!((p.vertices[0].p.y - (7.0 + 20.0 * k)).abs() < 1e-6);
    s.undo().unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &s.state().unwrap().doc));
}
#[test]
fn invalid_inputs_never_change_the_existing_drawing() {
    install();
    let mut s = Session::new();
    let before = s.state().unwrap().doc.clone();
    let data = file::base64_encode(&fixture::sample());
    for p in [
        json!({"data":data,"page":0}),
        json!({"data":data,"page":999}),
        json!({"data":data,"page":1.5}),
        json!({"data":data,"scale":0}),
        json!({"data":data,"scale":-1}),
        json!({"data":data,"units":"feet"}),
        json!({"data":data,"at":"wrong"}),
        json!({"data":data,"rotation":true}),
        json!({"data":data,"preview":"yes"}),
        json!({"data":"wrong"}),
    ] {
        assert!(s.execute("importpdf", &p).is_err(), "{p}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.state().unwrap().doc));
    }
}

#[test]
fn importpdf_is_registered_and_legacy_script_aliases_use_the_same_reader() {
    install();
    let spec = cadcraft_engine::cmd::find_command("importpdf").unwrap();
    assert_eq!(spec.id, "importpdf");
    let data = file::base64_encode(&fixture::sample());
    for name in ["IMPORTPDF", "PDFIMPORT", "-importpdf", "-pdfimport"] {
        assert_eq!(cadcraft_engine::cmd::resolve_alias(name), "importpdf");
        let mut s = Session::new();
        s.cmdline(&format!("{name} {}", json!({"data":data,"preview":true}))).unwrap();
        assert!(s.doc().unwrap().model.is_empty());
        let info: serde_json::Value = serde_json::from_str(s.log.last().unwrap()).unwrap();
        assert_eq!(info["pages"], 2);
        assert_eq!(info["objects"], 5);
    }
}

#[test]
fn all_page_previews_are_read_only_and_require_preview_mode() {
    install();
    let mut s = Session::new();
    let before = s.state().unwrap().doc.clone();
    let data = file::base64_encode(&fixture::sample());
    let r = s.execute("importpdf", &json!({"data":data,"preview":true,"pages":true})).unwrap();
    assert_eq!(r["pages"], 2);
    assert_eq!(r["previews"].as_array().unwrap().len(), 2);
    assert!(r["previews"][1]["thumbnail"].as_str().is_some());
    for p in [json!({"data":data,"pages":true}), json!({"data":data,"preview":true,"pages":3})] {
        assert!(s.execute("importpdf", &p).is_err());
    }
    assert!(std::sync::Arc::ptr_eq(&before, &s.state().unwrap().doc));
}
