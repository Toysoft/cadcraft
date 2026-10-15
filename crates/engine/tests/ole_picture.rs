//! Embedded OLE objects (#956): their pictures are read when the drawing opens; an object
//! without one, or one whose reader fails, stays a crossed-out frame and the drawing still opens.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, base64_encode, set_io, set_ole_decoder};
use cadcraft_engine::doc::{Common, Drawing, EntityKind, Handle, Raster, RawTag, Space, Unknown, ole_raster_key};
use serde_json::json;

/// The reader stands in for the real one: the data's third byte says what the object holds.
fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
    set_ole_decoder(|data| match data.get(2) {
        Some(1) => Raster::new(2, 1, vec![9; 8]).ok_or_else(|| "empty".to_string()),
        Some(2) => Err("OLE object: its picture is a drawing".into()),
        _ => panic!("reader bug"),
    });
}

/// An OLE object whose data is `data` (hexadecimal).
fn object(data: &str) -> EntityKind {
    let tags = [
        (70, "2"),
        (10, "0"),
        (20, "10"),
        (30, "0"),
        (11, "10"),
        (21, "0"),
        (31, "0"),
        (71, "2"),
        (72, "0"),
        (73, "2"),
        (90, "3"),
        (310, data),
        (1, "OLE"),
    ];
    EntityKind::Unknown(Unknown {
        dxf_type: "OLE2FRAME".into(),
        tags: tags.iter().map(|(code, value)| RawTag { code: *code, value: (*value).into() }).collect(),
    })
}

/// The OLE objects of the open drawing's model space that have data, with its third byte.
fn objects(s: &Session) -> Vec<(Handle, u8)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Unknown(u) => Some((e.handle, *u.ole_data(16)?.get(2)?)),
            _ => None,
        })
        .collect()
}

#[test]
fn pictures_are_read_on_open_and_failures_leave_frames() {
    hooks();
    let mut d = Drawing::new_metric();
    for data in ["805501", "805502", "805503", "805501"] {
        d.add(&Space::Model, Common::default(), object(data)).unwrap();
    }
    // One with data that is not hexadecimal, and one with none.
    d.add(&Space::Model, Common::default(), object("80550z")).unwrap();
    let EntityKind::Unknown(mut empty) = object("") else { unreachable!() };
    empty.tags.retain(|t| t.code != 310);
    d.add(&Space::Model, Common::default(), EntityKind::Unknown(empty)).unwrap();
    let file = cadcraft_io::write(&d, "ole.dxf").unwrap();

    let mut s = Session::new();
    s.execute("open", &json!({ "data": base64_encode(&file), "name": "ole.dxf" })).unwrap();
    let all = objects(&s);
    assert_eq!(all.iter().map(|(_, kind)| *kind).collect::<Vec<_>>(), [1, 2, 3, 1], "the objects are kept as they were");
    let d = s.doc().unwrap();
    for (handle, kind) in &all {
        let picture = d.rasters.get(&ole_raster_key(*handle));
        assert_eq!(picture.map(|r| (r.width(), r.height())), (*kind == 1).then_some((2, 1)), "{kind}");
    }
    assert_eq!(d.rasters.len(), 2);
    assert!(!s.state().unwrap().is_dirty(), "reading pictures is not a change");
    assert!(s.log.iter().any(|l| l.contains("4 embedded OLE object(s)") && l.contains("its picture is a drawing")), "{:?}", s.log);

    // The pictures are drawn; the others are frames.
    let list = cadcraft_engine::render::build(s.doc().unwrap(), &Space::Model, &cadcraft_engine::render::Options::default());
    assert_eq!(list.images.len(), 2);

    // Still unchanged data after an edit and an undo: the pictures stay.
    s.execute("line", &json!({ "points": [[0, 0], [5, 5]] })).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().rasters.len(), 2);
}

#[test]
fn a_drawing_without_ole_objects_says_nothing() {
    hooks();
    let file = cadcraft_io::write(&Drawing::new_metric(), "plain.dxf").unwrap();
    let mut s = Session::new();
    s.execute("open", &json!({ "data": base64_encode(&file), "name": "plain.dxf" })).unwrap();
    assert!(s.doc().unwrap().rasters.is_empty());
    assert!(!s.log.iter().any(|l| l.contains("OLE")), "{:?}", s.log);
}
