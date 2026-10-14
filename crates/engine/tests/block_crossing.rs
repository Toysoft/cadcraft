//! Crossing windows must touch visible block geometry, not the insert's enclosing rectangle.
use cadcraft_engine::Session;
use cadcraft_engine::doc::{Common, EntityKind, Handle, Point, Space};
use cadcraft_engine::geom::Vec3;
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, params: Value) -> String {
    let r = s.execute(id, &params).unwrap();
    r.get("handle").unwrap_or(&r["handles"][0]).as_str().unwrap().into()
}

fn make_block(s: &mut Session, name: &str, handles: &[String]) {
    s.execute("block", &json!({"name":name,"base":[0,0],"handles":handles,"keep":"delete"})).unwrap();
}

fn pad(s: &mut Session, indexed: bool) {
    if indexed {
        for i in 0..cadcraft_engine::spatial::MIN_INDEXED {
            s.doc_mut()
                .unwrap()
                .add(&Space::Model, Common::default(), EntityKind::Point(Point { p: Vec3::new(1000.0 + i as f64, 1000.0, 0.0), angle: 0.0 }))
                .unwrap();
        }
    }
}

fn select(s: &mut Session, window: [[f64; 2]; 2], crossing: bool) -> Vec<String> {
    let r = s.execute("select", &json!({"window":window,"crossing":crossing})).unwrap();
    serde_json::from_value(r["handles"].clone()).unwrap()
}

fn circle_block(s: &mut Session) {
    let c = run(s, "circle", json!({"center":[0,0],"radius":10}));
    make_block(s, "RING", &[c]);
}

#[test]
fn crossing_rejects_hollow_interior_and_empty_bbox_corner() {
    for indexed in [false, true] {
        let mut s = Session::new();
        circle_block(&mut s);
        let h = run(&mut s, "insert", json!({"name":"RING","at":[0,0]}));
        pad(&mut s, indexed);
        assert!(select(&mut s, [[-1.0, -1.0], [1.0, 1.0]], true).is_empty());
        assert!(select(&mut s, [[9.0, 9.0], [11.0, 11.0]], true).is_empty());
        assert_eq!(select(&mut s, [[9.9, -0.1], [10.1, 0.1]], true), vec![h.clone()]);
        assert!(select(&mut s, [[9.9, -0.1], [10.1, 0.1]], false).is_empty());
        assert_eq!(select(&mut s, [[-11.0, -11.0], [11.0, 11.0]], false), vec![h]);
    }
}

#[test]
fn crossing_tests_nested_transformed_and_arrayed_geometry() {
    for indexed in [false, true] {
        let mut s = Session::new();
        circle_block(&mut s);
        let child = run(&mut s, "insert", json!({"name":"RING","at":[0,0]}));
        make_block(&mut s, "NEST", &[child]);
        let h = run(&mut s, "insert", json!({"name":"NEST","at":[100,50],"scale":[-2,1],"rotation":90}));
        s.doc_mut()
            .unwrap()
            .modify_entity(Handle::parse_hex(&h).unwrap(), |e| {
                if let EntityKind::Insert(i) = &mut e.kind {
                    i.cols = 2;
                    i.col_spacing = 60.0;
                }
            })
            .unwrap();
        pad(&mut s, indexed);
        assert!(select(&mut s, [[99.0, 49.0], [101.0, 51.0]], true).is_empty());
        // The column offset rotates too: there is empty space between y=70 and y=90.
        assert!(select(&mut s, [[99.0, 79.0], [101.0, 81.0]], true).is_empty());
        assert_eq!(select(&mut s, [[109.9, 49.9], [110.1, 50.1]], true), vec![h.clone()]);
        assert_eq!(select(&mut s, [[109.9, 109.9], [110.1, 110.1]], true), vec![h]);
    }
}

#[test]
fn crossing_respects_filled_triangles_and_the_gaps_between_them() {
    for indexed in [false, true] {
        let mut s = Session::new();
        let mut handles = Vec::new();
        for o in [0.0, 10.0] {
            handles.push(
                s.doc_mut()
                    .unwrap()
                    .add(
                        &Space::Model,
                        Common::default(),
                        EntityKind::Solid(cadcraft_engine::doc::Solid {
                            corners: [Vec3::new(o, o, 0.0), Vec3::new(o + 4.0, o, 0.0), Vec3::new(o, o + 4.0, 0.0), Vec3::new(o, o + 4.0, 0.0)],
                        }),
                    )
                    .unwrap()
                    .hex(),
            );
        }
        make_block(&mut s, "FILLS", &handles);
        let h = run(&mut s, "insert", json!({"name":"FILLS","at":[0,0]}));
        pad(&mut s, indexed);
        assert!(select(&mut s, [[6.9, 6.9], [7.1, 7.1]], true).is_empty());
        assert!(select(&mut s, [[2.9, 2.9], [3.1, 3.1]], true).is_empty());
        assert_eq!(select(&mut s, [[0.9, 0.9], [1.1, 1.1]], true), vec![h.clone()]);
        assert_eq!(select(&mut s, [[10.9, 10.9], [11.1, 11.1]], true), vec![h]);
    }
}

#[test]
fn crossing_finds_text_and_visible_attributes_but_not_hidden_contents() {
    for indexed in [false, true] {
        let mut s = Session::new();
        let text = run(&mut s, "text", json!({"at":[0,0],"text":"ABC","height":1}));
        let att = run(&mut s, "attdef", json!({"tag":"TAG","default":"ABC","at":[10,0],"height":1}));
        let line = run(&mut s, "line", json!({"points":[[-10,10],[20,10]]}));
        make_block(&mut s, "LABEL", &[text, att, line]);
        let h = run(&mut s, "insert", json!({"name":"LABEL","at":[0,0]}));
        pad(&mut s, indexed);
        assert_eq!(select(&mut s, [[-0.1, -0.1], [3.0, 1.1]], true), vec![h.clone()]);
        assert_eq!(select(&mut s, [[9.9, -0.1], [13.0, 1.1]], true), vec![h]);
        s.execute("setvar", &json!({"name":"ATTMODE","value":0})).unwrap();
        assert!(select(&mut s, [[9.9, -0.1], [13.0, 1.1]], true).is_empty());
        assert!(select(&mut s, [[4.0, 4.0], [5.0, 5.0]], true).is_empty());
    }
    // A block containing only hidden attributes draws nothing even when the window encloses
    // its entire approximate bounds; the broad-phase containment shortcut must not select it.
    let mut s = Session::new();
    let att = run(&mut s, "attdef", json!({"tag":"TAG","default":"ABC","at":[0,0],"height":1}));
    make_block(&mut s, "ATTR_ONLY", &[att]);
    run(&mut s, "insert", json!({"name":"ATTR_ONLY","at":[0,0]}));
    s.execute("setvar", &json!({"name":"ATTMODE","value":0})).unwrap();
    assert!(select(&mut s, [[-10.0, -10.0], [10.0, 10.0]], true).is_empty());
}

#[test]
fn crossing_ignores_block_geometry_on_hidden_layers() {
    let mut s = Session::new();
    s.doc_mut().unwrap().ensure_layer("HIDDEN");
    // A visible line maintains broad-phase bounds over the location of the hidden circle.
    let line = run(&mut s, "line", json!({"points":[[-20,-20],[20,20]]}));
    let circle = run(&mut s, "circle", json!({"center":[0,0],"radius":10}));
    s.doc_mut().unwrap().modify_entity(Handle::parse_hex(&circle).unwrap(), |e| e.common.layer = "HIDDEN".into()).unwrap();
    make_block(&mut s, "VISIBILITY", &[line, circle]);
    let h = run(&mut s, "insert", json!({"name":"VISIBILITY","at":[0,0]}));
    assert_eq!(select(&mut s, [[9.9, -0.1], [10.1, 0.1]], true), vec![h]);
    s.doc_mut().unwrap().layer_mut("HIDDEN").unwrap().on = false;
    assert!(select(&mut s, [[9.9, -0.1], [10.1, 0.1]], true).is_empty());
}

#[test]
fn cyclic_block_expansion_stays_bounded_and_does_not_fill_holes() {
    let mut s = Session::new();
    circle_block(&mut s);
    let h = run(&mut s, "insert", json!({"name":"RING","at":[0,0]}));
    // Simulate a malformed imported block; BLOCK itself correctly rejects this cycle.
    let d = s.doc_mut().unwrap();
    let reference = (**d.entity(Handle::parse_hex(&h).unwrap()).unwrap()).clone();
    std::sync::Arc::make_mut(d.blocks.get_mut("RING").unwrap()).entities.push(reference);
    assert!(select(&mut s, [[-1.0, -1.0], [1.0, 1.0]], true).is_empty());
    assert_eq!(select(&mut s, [[9.9, -0.1], [10.1, 0.1]], true), vec![h]);
}
