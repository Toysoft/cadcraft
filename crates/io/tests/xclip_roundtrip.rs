//! Standard SPATIAL_FILTER data must survive save, reopen, and removal.

use std::sync::Arc;

use cadcraft_doc::{Block, ClipBoundary, Common, Drawing, Entity, EntityKind, Handle, Insert, Line, Space};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_io::{read, read_dxf, write, write_dxf};

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let mut block = Block::new("Plan");
    block.entities.push(Entity::new(d.new_handle(), EntityKind::Line(Line { a: Vec3::new(-10.0, 0.0, 0.0), b: Vec3::new(10.0, 0.0, 0.0) })));
    d.blocks.insert(block.name.clone(), Arc::new(block));
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Insert(Insert {
            block: "Plan".into(),
            insert: Vec3::new(10.0, 20.0, 0.0),
            scale: Vec3::new(2.0, 1.0, 1.0),
            rotation: 0.3,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: Some(ClipBoundary { points: vec![Vec2::new(-2.0, -1.0), Vec2::new(2.0, 1.0)], enabled: true, inverted: false, deleted: false }),
        }),
    )
    .unwrap();
    d
}

fn clip(d: &Drawing) -> Option<&ClipBoundary> {
    match &d.model.iter().find(|e| matches!(e.kind, EntityKind::Insert(_)))?.kind {
        EntityKind::Insert(i) => i.clip.as_ref(),
        _ => None,
    }
}

#[test]
fn spatial_filter_round_trips_without_duplication() {
    let d = drawing();
    let text = write_dxf(&d);
    assert!(text.contains("ACAD_FILTER"), "missing extension dictionary");
    assert!(text.contains("SPATIAL_FILTER"), "missing spatial filter object");
    let back = read_dxf(text.as_bytes()).unwrap();
    assert_eq!(clip(&back), clip(&d));
    let again = write_dxf(&back);
    assert_eq!(again.matches("SPATIAL_FILTER").count(), 1, "duplicate filter");
    let twice = read_dxf(again.as_bytes()).unwrap();
    assert_eq!(clip(&twice), clip(&d));
}

#[test]
fn off_and_delete_survive_reopen() {
    let mut d = read_dxf(write_dxf(&drawing()).as_bytes()).unwrap();
    let h = d.model.iter().find(|e| matches!(e.kind, EntityKind::Insert(_))).unwrap().handle;
    d.modify_entity(h, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.clip.as_mut().unwrap().enabled = false;
        }
    })
    .unwrap();
    let off = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    assert_eq!(clip(&off).map(|c| c.enabled), Some(false));

    d.modify_entity(h, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.clip = Some(ClipBoundary { points: vec![], enabled: false, inverted: false, deleted: true });
        }
    })
    .unwrap();
    let deleted = write_dxf(&d);
    assert!(!deleted.contains("SPATIAL_FILTER"), "old filter remained in DXF");
    assert!(clip(&read_dxf(deleted.as_bytes()).unwrap()).is_none());
}

#[test]
fn spatial_filter_survives_dwg_bridge() {
    let d = drawing();
    let bytes = write(&d, "x.dwg").unwrap();
    let back = read(&bytes, "x.dwg").unwrap();
    assert_eq!(clip(&back), clip(&d));
}

fn dictionary_entry(d: &Drawing, dictionary: Handle, name: &str) -> Handle {
    let tags = &d.foreign.objects.iter().find(|o| o.handle == dictionary).unwrap().tags;
    let pair = tags.windows(2).find(|pair| pair[0].code == 3 && pair[0].value == name && matches!(pair[1].code, 350 | 360)).unwrap();
    Handle::parse_hex(&pair[1].value).unwrap()
}

fn spatial_filter(d: &Drawing, insert: Handle) -> (&cadcraft_doc::foreign::ForeignObject, Handle) {
    let extension = d.foreign.entities.get(&insert).unwrap().xdict.unwrap();
    let owner = dictionary_entry(d, extension, "ACAD_FILTER");
    let filter = dictionary_entry(d, owner, "SPATIAL");
    (d.foreign.objects.iter().find(|o| o.handle == filter).unwrap(), owner)
}

#[test]
fn exported_filters_use_spatial_owner_and_preserve_transforms() {
    let mut d = drawing();
    let template = match &d.model.iter().next().unwrap().kind {
        EntityKind::Insert(i) => i.clone(),
        _ => unreachable!(),
    };
    let cases = [
        (150.0, vec![Vec2::new(20.0, 20.0), Vec2::new(80.0, 20.0), Vec2::new(80.0, 80.0), Vec2::new(20.0, 80.0)]),
        (300.0, vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(50.0, 100.0)]),
    ];
    let mut handles = Vec::new();
    for (x, points) in &cases {
        let mut insert = template.clone();
        insert.insert = Vec3::new(*x, 0.0, 0.0);
        insert.scale = Vec3::new(1.0, 1.0, 1.0);
        insert.rotation = 0.0;
        insert.clip = Some(ClipBoundary { points: points.clone(), enabled: true, inverted: false, deleted: false });
        handles.push(d.add(&Space::Model, Common::default(), EntityKind::Insert(insert)).unwrap());
    }

    let dxf = write_dxf(&d);
    let dwg = write(&d, "x.dwg").unwrap();
    for (is_dxf, reopened) in [(true, read_dxf(dxf.as_bytes()).unwrap()), (false, read(&dwg, "x.dwg").unwrap())] {
        for ((x, points), handle) in cases.iter().zip(&handles) {
            let (filter, owner) = spatial_filter(&reopened, *handle);
            assert_eq!(filter.kind, "SPATIAL_FILTER");
            if is_dxf {
                let reactors = filter.tags.windows(3).find(|tags| tags[0].code == 102 && tags[0].value == "{ACAD_REACTORS").unwrap();
                assert_eq!((reactors[1].code, reactors[1].value.as_str(), reactors[2].code), (330, owner.hex().as_str(), 102));
            } else {
                assert_eq!(filter.tags.iter().find(|tag| tag.code == 330).unwrap().value, owner.hex());
            }
            assert_eq!(filter.tags.iter().find(|tag| tag.code == 70).unwrap().value.parse::<usize>().unwrap(), points.len());
            let vertices: Vec<_> = filter
                .tags
                .windows(2)
                .filter(|tags| tags[0].code == 10 && tags[1].code == 20)
                .map(|tags| Vec2::new(tags[0].value.parse().unwrap(), tags[1].value.parse().unwrap()))
                .collect();
            let world: Vec<_> = points.iter().map(|p| Vec2::new(p.x + *x, p.y)).collect();
            assert_eq!(vertices, world);
            let matrices: Vec<f64> = filter.tags.iter().filter(|tag| tag.code == 40).map(|tag| tag.value.parse().unwrap()).collect();
            assert_eq!(
                matrices,
                vec![1.0, 0.0, 0.0, -*x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]
            );
        }
    }
}
