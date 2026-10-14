//! Synthetic modeler data, made through the unmodified codec's public API.
#![cfg(not(target_arch = "wasm32"))]

use std::io::Cursor;

use acadrust::entities::acis::primitives::build_planar_body;
use acadrust::entities::{EntityType, Region};
use acadrust::{CadDocument, DwgReader, DxfVersion, DxfWriter};

fn region_dxf(sat: &str) -> Vec<u8> {
    let mut doc = CadDocument::new();
    doc.version = DxfVersion::AC1024;
    doc.add_entity(EntityType::Region(Region::from_sat(sat))).unwrap();
    DxfWriter::new(&doc).write_to_vec().unwrap()
}

#[test]
fn inline_region_geometry_survives_legacy_and_modern_dwg_saves() {
    let sheet =
        build_planar_body(&[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0], [0.0, 3.0, 0.0]], &[vec![0, 1, 2, 3], vec![3, 2, 1, 0]]).unwrap();
    let sheet = acadrust::entities::acis::SatDocument::parse(&sheet.to_sat_string()).unwrap();
    let dxf = region_dxf(&sheet.to_sat_string());
    for version in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let mut source = dxf.clone();
        for _ in 0..2 {
            let dwg = cadcraft_dwg::dxf_to_dwg_as(&source, version).unwrap();
            let doc = DwgReader::from_stream(Cursor::new(&dwg)).read().unwrap();
            let regions: Vec<_> = doc.entities().filter_map(|e| if let EntityType::Region(r) = e { Some(r) } else { None }).collect();
            assert_eq!(regions.len(), 1, "{version}");
            let geometry = regions[0].acis_data.parse().unwrap();
            let geometry = acadrust::entities::acis::SatDocument::parse(&geometry.to_sat_string()).unwrap();
            assert_eq!(geometry.faces().len(), sheet.faces().len(), "{version}");
            assert_eq!(geometry.edges().len(), sheet.edges().len(), "{version}");
            let points = |d: &acadrust::entities::acis::SatDocument| {
                d.records.iter().filter(|r| r.entity_type == "point").map(|r| r.tokens.clone()).collect::<Vec<_>>()
            };
            assert_eq!(points(&geometry), points(&sheet), "{version}");
            assert!(geometry.validate().is_empty(), "{version}");
            source = cadcraft_dwg::dwg_to_dxf(&dwg).unwrap();
        }
    }
}

#[test]
fn unconvertible_region_geometry_is_refused_instead_of_written_empty() {
    let dxf = region_dxf("opaque SAT that this codec cannot convert\n");
    for version in ["AC1027", "AC1032"] {
        let error = cadcraft_dwg::dxf_to_dwg_as(&dxf, version).unwrap_err();
        assert!(error.contains("REGION") && error.contains("DXF"), "{error}");
    }
}

#[test]
fn custom_entities_and_objects_are_not_silently_removed() {
    for (section, kind) in [("ENTITIES", "CUSTOM_REGION"), ("OBJECTS", "CUSTOM_GEOMETRY")] {
        let dxf = format!(
            "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n0\nENDSEC\n0\nSECTION\n2\n{section}\n0\n{kind}\n5\n2A\n100\nAcDbEntity\n8\n0\n1\nopaque data\n0\nENDSEC\n0\nEOF\n"
        );
        assert!(cadcraft_dwg::dxf_to_dwg_as(dxf.as_bytes(), "AC1032").is_err(), "{kind} must not disappear from a successful save");
    }
}
