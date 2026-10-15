//! Embedded OLE objects (OLE2FRAME): the picture read from the object is drawn over the
//! object's rectangle; without one the object is a crossed-out frame.

use std::sync::Arc;

use cadcraft_doc::{Common, Drawing, EntityKind, Handle, Raster, RawTag, Space, Unknown, ole_raster_key};
use cadcraft_geom::Vec2;
use cadcraft_render::raster::{RasterOptions, View, render};
use cadcraft_render::{Kind, Options, build, build_plot};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn tags(tags: &[(i32, &str)]) -> Vec<RawTag> {
    tags.iter().map(|(code, value)| RawTag { code: *code, value: (*value).into() }).collect()
}

/// An OLE object over the 10 x 10 square at the origin: upper-left corner first.
fn object() -> Unknown {
    Unknown {
        dxf_type: "OLE2FRAME".into(),
        tags: tags(&[(70, "2"), (10, "0"), (20, "10"), (30, "0"), (11, "10"), (21, "0"), (31, "0"), (310, "8055")]),
    }
}

/// A drawing with `u`; its picture is two by two pixels, red and green on top, blue and white
/// below.
fn drawing(u: Unknown, picture: bool) -> (Drawing, Handle) {
    let mut d = Drawing::new_imperial();
    let h = d.add(&Space::Model, Common::default(), EntityKind::Unknown(u)).unwrap();
    if picture {
        d.rasters.insert(&ole_raster_key(h), Arc::new(Raster::new(2, 2, [RED, GREEN, BLUE, WHITE].concat()).unwrap()));
    }
    (d, h)
}

fn polylines(list: &cadcraft_render::DisplayList) -> usize {
    list.prims.iter().filter(|p| p.kind == Kind::Polyline).count()
}

#[test]
fn the_picture_is_drawn_over_the_object_s_rectangle() {
    let (d, h) = drawing(object(), true);
    let list = build(&d, &Space::Model, &Options::default());
    assert_eq!(list.images.len(), 1);
    let p = list.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).unwrap();
    assert_eq!(p.handle, h);
    assert_eq!(list.points(p), [Vec2::new(0.0, 10.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 0.0)]);
    assert_eq!(polylines(&list), 1, "the frame, not crossed out");

    // Top left, top right, bottom left, bottom right.
    let view = View { center: Vec2::new(5.0, 5.0), scale: 10.0, width: 100, height: 100 };
    let pm = render(&list, &view, &RasterOptions { antialias: false, ..Default::default() }).unwrap();
    let got = [(25, 25), (75, 25), (25, 75), (75, 75)].map(|(x, y)| {
        let c = pm.pixel(x, y).unwrap();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    let near = got.iter().flatten().zip([RED, GREEN, BLUE, WHITE].iter().flatten()).all(|(a, b)| a.abs_diff(*b) <= 6);
    assert!(near, "{got:?}");
}

#[test]
fn an_object_without_a_picture_is_a_crossed_out_frame() {
    let (d, h) = drawing(object(), false);
    let list = build(&d, &Space::Model, &Options::default());
    assert!(list.images.is_empty());
    assert_eq!(polylines(&list), 3, "frame and two diagonals");
    assert!(list.prims.iter().all(|p| p.handle == h));
    assert_eq!((list.bounds.min, list.bounds.max), (Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)));
    // In a plot too: the object keeps its place.
    assert_eq!(polylines(&build_plot(&d, &Space::Model, &Options::default())), 3);
    // The drawing's extents take the object in.
    let e = d.model.iter().next().unwrap();
    let b = cadcraft_doc::entity_bounds(&d, e, 0);
    assert_eq!((b.min, b.max), (Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)));
}

#[test]
fn the_frame_follows_oleframe() {
    let frames = |setting: Option<i64>, plot: bool| {
        let (mut d, _) = drawing(object(), true);
        if let Some(v) = setting {
            d.header.set_i64("OLEFRAME", v);
        }
        let list = if plot { build_plot(&d, &Space::Model, &Options::default()) } else { build(&d, &Space::Model, &Options::default()) };
        assert_eq!(list.images.len(), 1);
        polylines(&list)
    };
    // 2, the default: shown, not plotted. 1: both. 0: neither.
    assert_eq!((frames(None, false), frames(None, true)), (1, 0));
    assert_eq!((frames(Some(2), false), frames(Some(2), true)), (1, 0));
    assert_eq!((frames(Some(1), false), frames(Some(1), true)), (1, 1));
    assert_eq!((frames(Some(0), false), frames(Some(0), true)), (0, 0));
}

#[test]
fn other_unknown_entities_and_broken_frames_draw_nothing() {
    for u in [
        Unknown { dxf_type: "LIGHT".into(), ..object() },
        Unknown { dxf_type: "OLE2FRAME".into(), tags: Vec::new() },
        Unknown { dxf_type: "OLE2FRAME".into(), tags: tags(&[(10, "0"), (20, "0"), (11, "0"), (21, "0")]) },
        Unknown { dxf_type: "OLE2FRAME".into(), tags: tags(&[(10, "nan"), (20, "inf"), (11, "1e999"), (21, "x")]) },
        Unknown { dxf_type: "OLE2FRAME".into(), tags: tags(&[(10, "-1e308"), (20, "-1e308"), (11, "1e308"), (21, "1e308")]) },
    ] {
        for picture in [false, true] {
            let (d, _) = drawing(u.clone(), picture);
            let list = build(&d, &Space::Model, &Options::default());
            if u.ole_frame().is_none() {
                assert!(list.prims.is_empty(), "{u:?}");
            }
            let _ = build_plot(&d, &Space::Model, &Options::default());
        }
    }
}
