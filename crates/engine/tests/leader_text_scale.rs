//! #1056: a leader's annotation is drawn at the dimension text size, DIMTXT × DIMSCALE, like the
//! dimensions beside it.

use cadcraft_engine::Session;
use cadcraft_engine::doc::EntityKind;
use serde_json::json;

#[test]
fn leader_text_takes_dimscale() {
    let mut s = Session::new();
    s.execute("dimstyle", &json!({"name": "S25", "current": true, "DIMSCALE": 25, "DIMTXT": 0.25, "DIMASZ": 0.2})).unwrap();
    s.execute("leader", &json!({"points": [[0, 0], [20, 20], [30, 20]], "text": "NOTE"})).unwrap();
    let h = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::MText(m) = &e.kind { Some(m.height) } else { None }).unwrap();
    assert!((h - 6.25).abs() < 1e-9, "{h}");
}
