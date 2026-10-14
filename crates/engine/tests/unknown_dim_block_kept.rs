//! A dimension CADCraft doesn't model (an ARC_DIMENSION, kept raw) names its anonymous `*D` block
//! in group 2, and AutoCAD refuses a file whose dimension names a block it doesn't define
//! (#1161). Save counts that block as used; PURGE and the copies between drawings (clipboard,
//! IMPORT) must too, or the block is lost and the reference stays.

use std::path::PathBuf;
use std::sync::Arc;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::doc::{Block, Common, Drawing, Entity, EntityKind, Handle, Line, RawTag, Space, Unknown};
use cadcraft_engine::geom::Vec3;
use serde_json::json;

fn tag(code: i32, value: &str) -> RawTag {
    RawTag { code, value: value.into() }
}

fn arc_dim(block: &str) -> EntityKind {
    EntityKind::Unknown(Unknown {
        dxf_type: "ARC_DIMENSION".into(),
        tags: vec![
            tag(100, "AcDbEntity"),
            tag(8, "0"),
            tag(100, "AcDbDimension"),
            tag(2, block),
            tag(10, "333.8"),
            tag(20, "169.5"),
            tag(100, "AcDbArcDimension"),
        ],
    })
}

/// A SHAPE kept raw: its group 2 is the shape's name (from a shape file), not a block.
fn shape(name: &str) -> EntityKind {
    EntityKind::Unknown(Unknown {
        dxf_type: "SHAPE".into(),
        tags: vec![tag(100, "AcDbEntity"), tag(8, "0"), tag(100, "AcDbShape"), tag(10, "0"), tag(20, "0"), tag(30, "0"), tag(40, "1"), tag(2, name)],
    })
}

/// The group 2 of every unknown entity of model space.
fn group2(d: &Drawing) -> Vec<String> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Unknown(u) => u.tags.iter().find(|t| t.code == 2).map(|t| t.value.clone()),
            _ => None,
        })
        .collect()
}

fn line_kind() -> EntityKind {
    EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) })
}

/// A block holding one line (a dimension's drawn geometry) with the given first x.
fn block(name: &str, x: f64, entities: Vec<EntityKind>) -> Arc<Block> {
    let mut b = Block::new(name);
    b.anonymous = name.starts_with('*');
    b.entities.push(Entity::new(Handle(0x8000), EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x + 1.0, 1.0, 0.0) })));
    for (i, k) in entities.into_iter().enumerate() {
        b.entities.push(Entity::new(Handle(0x8001 + i as u64), k));
    }
    Arc::new(b)
}

/// The block names the unknown dimensions of model space name, in order.
fn named(d: &Drawing) -> Vec<String> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Unknown(u) => u.block_names().next().map(str::to_string),
            _ => None,
        })
        .collect()
}

#[test]
fn purge_keeps_the_block_an_unknown_dimension_names() {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    // *D1: named from model space; *D2: from model space, named in lower case; *D5: from a paper-space layout; *D3: from a block that is inserted;
    // *D4: only from a block nothing inserts (so it goes, with OLD); SPARE: unused.
    d.blocks.insert("*D1".into(), block("*D1", 0.0, vec![]));
    d.blocks.insert("*D2".into(), block("*D2", 0.0, vec![]));
    d.blocks.insert("*D3".into(), block("*D3", 0.0, vec![]));
    d.blocks.insert("*D4".into(), block("*D4", 0.0, vec![]));
    d.blocks.insert("*D5".into(), block("*D5", 0.0, vec![]));
    d.blocks.insert("SPARE".into(), block("SPARE", 0.0, vec![]));
    d.blocks.insert("WALLS".into(), block("WALLS", 0.0, vec![arc_dim("*D3")]));
    d.blocks.insert("OLD".into(), block("OLD", 0.0, vec![arc_dim("*D4")]));
    d.add(&Space::Model, Common::default(), arc_dim("*D1")).unwrap();
    let ins = cadcraft_engine::doc::Insert {
        block: "WALLS".into(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
    // A name in another case still names the block.
    d.add(&Space::Model, Common::default(), arc_dim("*d2")).unwrap();
    // A dimension in a paper-space layout names its block too.
    let layout = d.layouts.first().map(|l| l.name.clone()).expect("a new drawing has layouts");
    d.add(&Space::Paper(layout), Common::default(), arc_dim("*D5")).unwrap();

    s.execute("purge", &json!({ "type": "blocks" })).unwrap();
    let d = s.doc().unwrap();
    for kept in ["*D1", "*D2", "*D3", "*D5", "WALLS"] {
        assert!(d.block(kept).is_some(), "PURGE removed {kept}, which a kept entity names or inserts");
    }
    for gone in ["*D4", "SPARE", "OLD"] {
        assert!(d.block(gone).is_none(), "PURGE kept {gone}, which nothing uses");
    }
}

#[test]
fn clipboard_brings_the_block_an_unknown_dimension_names() {
    let mut s = Session::empty();
    let mut a = Drawing::new_metric();
    a.blocks.insert("*D1".into(), block("*D1", 1.0, vec![]));
    a.add(&Space::Model, Common::default(), arc_dim("*D1")).unwrap();
    let mut b = Drawing::new_metric();
    // B already has a *D1 of its own: the copy gets a fresh name and the reference follows.
    b.blocks.insert("*D1".into(), block("*D1", 5.0, vec![]));
    let a = s.open_drawing(a, "A", None);
    let b = s.open_drawing(b, "B", None);
    s.active = a;
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("copyclip", &json!({ "handles": hs })).unwrap();
    s.active = b;
    s.execute("pasteclip", &json!({ "at": [0, 0] })).unwrap();

    let d = s.doc().unwrap();
    let names = named(d);
    assert_eq!(names.len(), 1);
    let pasted = d.block(&names[0]).unwrap_or_else(|| panic!("pasted dimension names {}, which B does not define", names[0]));
    assert_ne!(names[0], "*D1", "the pasted dimension points at B's own *D1");
    assert!(matches!(pasted.entities.iter().next().map(|e| &e.kind), Some(EntityKind::Line(l)) if l.a.x == 1.0), "A's geometry came along");
    assert!(
        matches!(d.block("*D1").and_then(|b| b.entities.iter().next().map(|e| e.kind.clone())), Some(EntityKind::Line(l)) if l.a.x == 5.0),
        "B's *D1 is unchanged"
    );
}

#[test]
fn purge_ignores_the_group_2_of_an_entity_that_is_not_a_dimension() {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    d.blocks.insert("BOX".into(), block("BOX", 0.0, vec![]));
    d.add(&Space::Model, Common::default(), shape("BOX")).unwrap();
    s.execute("purge", &json!({ "type": "blocks" })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.block("BOX").is_none(), "PURGE kept BOX, which only a SHAPE's name matches");
    assert_eq!(group2(d), ["BOX"], "the shape keeps its name");
}

#[test]
fn clipboard_does_not_bring_a_block_named_like_a_shape() {
    let mut s = Session::empty();
    let mut a = Drawing::new_metric();
    a.blocks.insert("BOX".into(), block("BOX", 1.0, vec![]));
    a.add(&Space::Model, Common::default(), shape("BOX")).unwrap();
    let a = s.open_drawing(a, "A", None);
    let b = s.open_drawing(Drawing::new_metric(), "B", None);
    s.active = a;
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("copyclip", &json!({ "handles": hs })).unwrap();
    s.active = b;
    s.execute("pasteclip", &json!({ "at": [0, 0] })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.block("BOX").is_none(), "the paste brought a block BOX, only a SHAPE's name matches it");
    assert_eq!(group2(d), ["BOX"], "the shape keeps its name");
}

/// The drawing every test file reads: an ARC_DIMENSION in model space naming its own `*D1`.
fn read(_bytes: &[u8], _name: &str) -> Result<Drawing, String> {
    let mut d = Drawing::new_metric();
    d.blocks.insert("*D1".into(), block("*D1", 1.0, vec![]));
    d.add(&Space::Model, Common::default(), arc_dim("*D1")).map_err(|e| e.to_string())?;
    d.model.push(Entity::new(Handle(0x70), line_kind()));
    Ok(d)
}

fn file() -> String {
    let dir: PathBuf = std::env::temp_dir().join(format!("cadcraft-unknown-dim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("arc.dxf");
    std::fs::write(&p, "x").unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn import_follows_the_block_rename_of_an_unknown_dimension() {
    set_io(IoHooks { read, write: |_, _| Err("no writer in this test".into()), plot: None });
    let mut s = Session::new();
    // This drawing already has a different *D1: the imported one is copied as *D2.
    s.doc_mut().unwrap().blocks.insert("*D1".into(), block("*D1", 5.0, vec![]));
    s.execute("import", &json!({ "path": file() })).unwrap();

    let d = s.doc().unwrap();
    let names = named(d);
    assert_eq!(names.len(), 1);
    let imported = d.block(&names[0]).unwrap_or_else(|| panic!("imported dimension names {}, which is not defined", names[0]));
    assert!(
        matches!(imported.entities.iter().next().map(|e| &e.kind), Some(EntityKind::Line(l)) if l.a.x == 1.0),
        "{} is not the imported *D1",
        names[0]
    );
}
