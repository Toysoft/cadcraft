#[path = "support/pdf.rs"]
mod fixture;
use cadcraft_doc::{EntityKind, Space};
use cadcraft_geom::Vec2;
use cadcraft_io::pdf_read;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} != {b}");
}
fn xy(a: Vec2, b: Vec2) {
    close(a.x, b.x);
    close(a.y, b.y);
}
const K: f64 = 25.4 / 72.0;

#[test]
fn automatic_overview_has_all_pages_and_keeps_unsupported_pages_in_place() {
    let previews = pdf_read::page_previews(&fixture::sample(), true).unwrap();
    assert_eq!(previews.len(), 2);
    assert_eq!(previews[0].info["page"], 1);
    assert_eq!(previews[1].info["page"], 2);
    assert!(previews.iter().all(|p| p.png.as_ref().is_some_and(|p| p.starts_with(b"\x89PNG"))));
    let mixed = fixture::file(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 100 100] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_vec(),
        fixture::stream(b"0 0 m 100 100 l S", ""),
    ]);
    let previews = pdf_read::page_previews(&mixed, true).unwrap();
    assert_eq!(previews.len(), 2);
    assert!(previews[0].info["error"].as_str().is_some());
    assert!(previews[0].png.is_none());
    assert_eq!(previews[1].info["page"], 2);
    assert_eq!(previews[1].info["objects"], 1);
    assert!(previews[1].png.is_some());
    assert!(pdf_read::page_previews(b"invalid", true).is_err());
}

#[test]
fn vector_paths_forms_text_and_rotated_inherited_page_boxes() {
    let pdf = fixture::sample();
    let r = pdf_read::read(&pdf, 1).unwrap();
    assert_eq!(r.pages, 2);
    close(r.width_mm, 360.0 * K);
    close(r.height_mm, 240.0 * K);
    assert!(r.drawing.model.iter().all(|e| e.common.layer == "PDF_PAGE_1"));
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind,EntityKind::Text(t) if t.value=="SYNTHETIC PLAN")));
    let spline = r.drawing.model.iter().find_map(|e| if let EntityKind::Spline(s) = &e.kind { Some(s) } else { None }).unwrap();
    xy(spline.control[0], Vec2::new(30.0, 35.0) * K);
    xy(spline.control[3], Vec2::new(120.0, 35.0) * K);
    let form = r
        .drawing
        .model
        .iter()
        .find_map(|e| {
            if let EntityKind::LwPolyline(p) = &e.kind {
                (p.vertices.first().is_some_and(|p| (p.p - Vec2::new(70.0, 70.0) * K).len() < 1e-6)).then_some(p)
            } else {
                None
            }
        })
        .unwrap();
    xy(form.vertices[0].p, Vec2::new(70.0, 70.0) * K);
    xy(form.vertices[1].p, Vec2::new(100.0, 100.0) * K);
    let dxf = cadcraft_io::write_dxf(&r.drawing);
    let reopened = cadcraft_io::read_dxf(dxf.as_bytes()).unwrap();
    assert_eq!(reopened.model.len(), r.drawing.model.len());
    let dwg = cadcraft_io::write(&r.drawing, "converted.dwg").unwrap();
    let reopened = cadcraft_io::read(&dwg, "converted.dwg").unwrap();
    assert_eq!(reopened.model.len(), r.drawing.model.len());
    let page2 = pdf_read::read(&pdf, 2).unwrap();
    close(page2.width_mm, 200.0 * K);
    close(page2.height_mm, 100.0 * K);
    let p = page2.drawing.model.iter().find_map(|e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p) } else { None }).unwrap();
    xy(p.vertices[0].p, Vec2::new(0.0, 100.0) * K);
    xy(p.vertices[1].p, Vec2::ZERO);
    xy(p.vertices[2].p, Vec2::new(200.0, 0.0) * K);
    assert!(pdf_read::read(&pdf, 0).is_err());
    assert!(pdf_read::read(&pdf, 3).is_err());
    assert_eq!(cadcraft_io::read(&pdf, "plan.PDF").unwrap().model.len(), r.drawing.model.len());
}
#[test]
fn clipping_exact_lines_and_clipped_curves_do_not_escape() {
    let pdf = fixture::page(b"q 20 20 100 100 re W n\n0 70 m 200 70 l S\n0 40 m 70 10 100 150 180 40 c S Q", "", "");
    let r = pdf_read::read(&pdf, 1).unwrap();
    assert!(r.drawing.model.len() > 2);
    for e in r.drawing.model.iter() {
        let bounds = cadcraft_doc::entity_bounds(&r.drawing, e, 0);
        assert!(bounds.min.x >= 20.0 * K - 1e-6 && bounds.max.x <= 120.0 * K + 1e-6);
        assert!(bounds.min.y >= 20.0 * K - 1e-6 && bounds.max.y <= 120.0 * K + 1e-6);
    }
    let pdf = fixture::page(b"0 0 m 100 0 l 0 100 l h W n\n-10 50 m 110 50 l S", "", "");
    let r = pdf_read::read(&pdf, 1).unwrap();
    let l = r.drawing.model.iter().find_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l) } else { None }).unwrap();
    xy(l.a.xy(), Vec2::new(0.0, 50.0) * K);
    xy(l.b.xy(), Vec2::new(50.0, 50.0) * K);
}
#[test]
fn text_spacing_and_fills_are_explicit_editable_objects() {
    let pdf = fixture::page(b"BT /F1 10 Tf 20 20 Td (AB) Tj (CD) Tj ET\n1 0 0 rg 30 60 10 10 re f", "/Font << /F1 5 0 R >>", "");
    let r = pdf_read::read(&pdf, 1).unwrap();
    let texts: Vec<_> = r.drawing.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t) } else { None }).collect();
    assert_eq!(texts.len(), 2);
    xy(texts[0].insert.xy(), Vec2::new(20.0, 20.0) * K);
    xy(texts[1].insert.xy(), Vec2::new(32.0, 20.0) * K);
    assert!(r.warnings.iter().any(|s| s.contains("Filled areas")));
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind,EntityKind::LwPolyline(p) if p.closed)));
    assert!(!cadcraft_io::png(&r.drawing, &Space::Model, 640, 420).unwrap().is_empty());
}
#[test]
fn compressed_and_modern_object_stream_pdfs_are_supported() {
    let mut doc = lopdf::Document::load_mem(&fixture::sample()).unwrap();
    doc.compress();
    let mut compressed = Vec::new();
    doc.save_modern(&mut compressed).unwrap();
    let r = pdf_read::read(&compressed, 1).unwrap();
    assert_eq!(r.drawing.model.len(), 5);
}
#[test]
fn malformed_cyclic_and_raster_only_pdfs_return_errors() {
    for content in [b"1 2 l S".as_slice(), b"q Q Q", b"1 0 cm", b"1e100 0 m 1 1 l S"] {
        let pdf = fixture::page(content, "", "");
        assert!(pdf_read::read(&pdf, 1).is_err(), "malformed content: {:?}", String::from_utf8_lossy(content));
    }
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Loop 5 0 R >> >> >>".to_vec(),
        fixture::stream(b"/Loop Do", ""),
        fixture::stream(b"/Loop Do", "/Subtype /Form /BBox [0 0 100 100]"),
    ];
    assert!(pdf_read::read(&fixture::file(&objects), 1).is_err());
    objects[4] = fixture::stream(b"123", "/Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8");
    assert!(pdf_read::read(&fixture::file(&objects), 1).err().unwrap().to_string().contains("scanned image"));
    assert!(pdf_read::read(b"not a PDF", 1).is_err());
}
#[test]
fn decompression_bombs_are_bounded() {
    let content = vec![b' '; 17 << 20];
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&content, 9);
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        fixture::stream(&packed, "/Filter /FlateDecode"),
    ];
    assert!(pdf_read::read(&fixture::file(&objects), 1).is_err());
    objects.clear();
}

#[test]
fn cid_unicode_text_and_widths_survive_import() {
    let pdf=fixture::file(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 360 240] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        fixture::stream(b"BT /F1 10 Tf 10 20 Td <00010002> Tj <0001> Tj ET", ""),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /SyntheticFont /Encoding /Identity-H /ToUnicode 6 0 R /DescendantFonts [7 0 R] >>".to_vec(),
        fixture::stream(b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /SyntheticMap def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfchar <0001> <010C> <0002> <0159> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end", ""),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /SyntheticFont /CIDSystemInfo << /Registry (Synthetic) /Ordering (Identity) /Supplement 0 >> /DW 1000 /W [1 [600 700]] >>".to_vec(),
    ]);
    let r = pdf_read::read(&pdf, 1).unwrap();
    let texts: Vec<_> = r.drawing.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t) } else { None }).collect();
    assert_eq!(texts[0].value, "Čř");
    assert_eq!(texts[1].value, "Č");
    xy(texts[1].insert.xy(), Vec2::new(23.0, 20.0) * K);
}

/// Opt-in local checks. No user's drawing or path is recorded in the repository.
#[test]
fn local_pdf_validation() {
    let Some(path) = std::env::var_os("CADCRAFT_PDF_FILE") else { return };
    let bytes = std::fs::read(path).unwrap();
    let first = pdf_read::read(&bytes, 1).unwrap();
    let output = std::env::var_os("CADCRAFT_PDF_SHOTS").map(std::path::PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    for page in 1..=first.pages as u32 {
        let r = pdf_read::read(&bytes, page).unwrap();
        let mut info = r.info();
        let dxf = cadcraft_io::write_dxf(&r.drawing);
        let reopened = cadcraft_io::read_dxf(dxf.as_bytes()).unwrap();
        assert_eq!(r.drawing.model.len(), reopened.model.len());
        let text_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
        info["texts"] = serde_json::json!(text_count);
        if let Some(dir) = &output {
            std::fs::write(dir.join(format!("real-page-{page}.dxf")), dxf).unwrap();
            let window = cadcraft_geom::Bounds2 { min: Vec2::ZERO, max: Vec2::new(r.width_mm, r.height_mm) };
            std::fs::write(
                dir.join(format!("real-page-{page}.png")),
                cadcraft_io::png_framed(&r.drawing, &Space::Model, 1684, 1190, Some(window)).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join(format!("real-page-{page}-info.json")), serde_json::to_vec_pretty(&info).unwrap()).unwrap();
        }
        println!("PDF local page: {info}");
    }
}
