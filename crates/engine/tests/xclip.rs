//! XCLIP changes one INSERT and clips its displayed geometry without editing the block.

use std::sync::Arc;

use cadcraft_engine::Session;
use cadcraft_engine::doc::geom::{Vec2, Vec3};
use cadcraft_engine::doc::{Block, Common, Entity, EntityKind, Handle, Insert, Line, Space};
use serde_json::json;

fn drawing() -> (Session, Handle, Handle) {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    let mut block = Block::new("Plan");
    block.entities.push(Entity::new(Handle(0x90), EntityKind::Line(Line { a: Vec3::new(-10.0, 0.0, 0.0), b: Vec3::new(10.0, 0.0, 0.0) })));
    d.blocks.insert(block.name.clone(), Arc::new(block));
    let insert = |x| {
        EntityKind::Insert(Insert {
            block: "Plan".into(),
            insert: Vec3::new(x, 0.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        })
    };
    let a = d.add(&Space::Model, Common::default(), insert(0.0)).unwrap();
    let b = d.add(&Space::Model, Common::default(), insert(30.0)).unwrap();
    (s, a, b)
}

fn line_xs(s: &Session, h: Handle) -> Vec<f64> {
    let list = cadcraft_engine::render::build(s.doc().unwrap(), &Space::Model, &Default::default());
    list.prims.iter().filter(|p| p.handle == h).flat_map(|p| list.points(p).iter().map(|v| v.x)).collect()
}

#[test]
fn rectangle_clips_only_the_selected_reference() {
    let (mut s, a, b) = drawing();
    s.execute("xclip", &json!({ "handle": a.hex(), "rectangle": [[-2, -1], [2, 1]] })).unwrap();
    assert_eq!(line_xs(&s, a), [-2.0, 2.0]);
    assert_eq!(line_xs(&s, b), [20.0, 40.0]);
    let d = s.doc().unwrap();
    let bounds = cadcraft_engine::doc::entity_bounds(d, d.entity(a).unwrap(), 0);
    assert_eq!((bounds.min.x, bounds.max.x), (-2.0, 2.0));
    assert_eq!(s.doc().unwrap().block("Plan").unwrap().entities.len(), 1);
    assert_eq!(s.execute("xclip", &json!({ "handle": a.hex(), "option": "query" })).unwrap()["enabled"], true);
}

#[test]
fn on_off_delete_and_bad_boundary() {
    let (mut s, a, _) = drawing();
    assert!(s.execute("xclip", &json!({ "handle": a.hex(), "points": [[0, 0], [1, 0]] })).is_err());
    assert_eq!(line_xs(&s, a), [-10.0, 10.0]);
    s.execute("xclip", &json!({ "handle": a.hex(), "points": [[-3, -1], [3, -1], [3, 1], [-3, 1]] })).unwrap();
    assert_eq!(line_xs(&s, a), [-3.0, 3.0]);
    s.execute("xclip", &json!({ "handle": a.hex(), "option": "off" })).unwrap();
    assert_eq!(line_xs(&s, a), [-10.0, 10.0]);
    s.execute("xclip", &json!({ "handle": a.hex(), "option": "on" })).unwrap();
    assert_eq!(line_xs(&s, a), [-3.0, 3.0]);
    s.execute("xclip", &json!({ "handle": a.hex(), "option": "delete" })).unwrap();
    assert_eq!(line_xs(&s, a), [-10.0, 10.0]);
    assert!(s.execute("xclip", &json!({ "handle": a.hex(), "option": "on" })).is_err());
}

#[test]
fn rotated_reference_uses_a_world_aligned_rectangle() {
    let (mut s, a, _) = drawing();
    s.doc_mut()
        .unwrap()
        .modify_entity(a, |e| {
            if let EntityKind::Insert(i) = &mut e.kind {
                i.rotation = std::f64::consts::FRAC_PI_2;
            }
        })
        .unwrap();
    s.execute("xclip", &json!({ "handle": a.hex(), "rectangle": [[-1, -2], [1, 2]] })).unwrap();
    let list = cadcraft_engine::render::build(s.doc().unwrap(), &Space::Model, &Default::default());
    let ys: Vec<f64> = list.prims.iter().filter(|p| p.handle == a).flat_map(|p| list.points(p).iter().map(|v| v.y)).collect();
    assert_eq!(ys.len(), 2);
    assert!((ys[0] + 2.0).abs() < 1e-9 && (ys[1] - 2.0).abs() < 1e-9, "{ys:?}");
    assert!(Vec2::new(0.0, ys[0]).is_finite());
}

#[test]
fn command_boundary_survives_dxf_and_dwg_reopen() {
    let (mut s, a, _) = drawing();
    s.execute("xclip", &json!({ "handle": a.hex(), "rectangle": [[-2, -1], [2, 1]] })).unwrap();
    for name in ["x.dxf", "x.dwg"] {
        let bytes = cadcraft_io::write(s.doc().unwrap(), name).unwrap();
        let reopened = cadcraft_io::read(&bytes, name).unwrap();
        let list = cadcraft_engine::render::build(&reopened, &Space::Model, &Default::default());
        let xs: Vec<f64> = list.prims.iter().filter(|p| p.handle == a).flat_map(|p| list.points(p).iter().map(|v| v.x)).collect();
        assert_eq!(xs, [-2.0, 2.0], "{name}");
    }
}
