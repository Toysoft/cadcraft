//! The `entities` query (control channel `engine.execute`, MCP `query_entities`) rejects malformed
//! filters instead of ignoring them and returning everything.

use cadcraft_engine::Session;
use serde_json::json;

#[test]
fn malformed_filters_are_errors() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    s.execute("circle", &json!({"center": [100, 100], "radius": 1})).unwrap();
    for p in [
        json!({"window": [-5, -5, 5, 5]}),
        json!({"window": [[-5, -5]]}),
        json!({"window": [[-5, -5], [5, 5], [9, 9]]}),
        json!({"window": {"min": [-5, -5], "max": [5, 5]}}),
        json!({"window": "x"}),
        json!({"type": 7}),
        json!({"layer": ["0"]}),
        json!({"limit": -1}),
        json!({"offset": "1"}),
        json!({"crossing": "yes"}),
    ] {
        assert!(s.execute("entities", &p).is_err(), "{p} must be rejected");
    }
    let r = s.execute("entities", &json!({"window": [[-5, -5], [5, 5]]})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
}

/// Entities come in world coordinates with their bounding box; `extrusion` is only reported when it
/// is not +Z, so an agent can tell an object mirrored in its source file from a plain one.
#[test]
fn entities_report_bbox_and_only_unusual_extrusions() {
    // Two circles of radius 1: one plain at (0,0), one mirrored (extrusion 0,0,-1) at OCS (-10,5),
    // which is WCS (10,5).
    let dxf = "0\nSECTION\n2\nENTITIES\n\
               0\nCIRCLE\n5\n1A\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n\
               0\nCIRCLE\n5\n1B\n8\n0\n10\n-10\n20\n5\n30\n0\n40\n1\n210\n0\n220\n0\n230\n-1\n\
               0\nENDSEC\n0\nEOF\n";
    let d = cadcraft_io::read_dxf(dxf.as_bytes()).unwrap();
    let mut s = Session::new();
    s.open_drawing(d, "mirrored", None);
    let r = s.execute("entities", &json!({"type": "Circle"})).unwrap();
    let es = r["entities"].as_array().unwrap();
    assert_eq!(es.len(), 2, "{r}");
    let plain = es.iter().find(|e| e["geometry"]["center"]["x"] == 0.0).unwrap();
    let mirrored = es.iter().find(|e| e["geometry"]["center"]["x"] == 10.0).unwrap();
    assert!(plain.get("extrusion").is_none(), "{plain}");
    assert_eq!(plain["bbox"], json!([[-1.0, -1.0], [1.0, 1.0]]));
    assert_eq!(mirrored["extrusion"], json!([0.0, 0.0, -1.0]), "{mirrored}");
    assert_eq!(mirrored["bbox"], json!([[9.0, 4.0], [11.0, 6.0]]));
    // drawing.inspect serializes entities the same way and names the insertion units.
    let i = s.execute("drawing.inspect", &json!({})).unwrap();
    assert!(i["entities"].as_array().unwrap().iter().any(|e| e["extrusion"] == json!([0.0, 0.0, -1.0])), "{i}");
    assert!(i["insunits"]["code"].is_i64() && i["insunits"]["name"].is_string(), "{i}");
}

/// A direction `Mat4::ocs` treats as the default plane is not reported: a zero extrusion (as a
/// damaged file may hold) or a longer +Z.
#[test]
fn zero_or_scaled_plus_z_extrusion_is_not_reported() {
    let dxf = "0\nSECTION\n2\nENTITIES\n\
               0\nCIRCLE\n5\n1A\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n210\n0\n220\n0\n230\n0\n\
               0\nCIRCLE\n5\n1B\n8\n0\n10\n5\n20\n0\n30\n0\n40\n1\n210\n0\n220\n0\n230\n2\n\
               0\nENDSEC\n0\nEOF\n";
    let d = cadcraft_io::read_dxf(dxf.as_bytes()).unwrap();
    let mut s = Session::new();
    s.open_drawing(d, "zero", None);
    let r = s.execute("entities", &json!({})).unwrap();
    let es = r["entities"].as_array().unwrap();
    assert_eq!(es.len(), 2, "{r}");
    assert!(es.iter().all(|e| e.get("extrusion").is_none()), "{r}");
}

/// A block reference's `bbox` is where its contents are drawn: here a line from (0,0) to (10,0) in
/// the block, inserted at (100,50), spans (100,50)..(110,50).
#[test]
fn block_reference_bbox_follows_its_contents() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let line = s.execute("entities", &json!({"type": "Line"})).unwrap()["entities"][0]["handle"].clone();
    s.execute("block", &json!({"name": "L", "base": [0, 0], "handles": [line], "keep": "delete"})).unwrap();
    s.execute("insert", &json!({"name": "L", "at": [100, 50]})).unwrap();
    let r = s.execute("entities", &json!({"type": "Insert"})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    assert_eq!(r["entities"][0]["bbox"], json!([[100.0, 50.0], [110.0, 50.0]]), "{r}");
}
