//! XCLIP's standard DXF extension dictionary: INSERT → ACAD_FILTER → SPATIAL → SPATIAL_FILTER.
//!
//! The raw-data keeper (#528) already preserves this graph on an unchanged drawing. This module
//! reads planar filters into INSERTs for rendering, and overlays changed filters on a temporary
//! drawing before writing. Other entries in an INSERT's extension dictionary remain untouched.

use std::sync::Arc;

use cadcraft_doc::foreign::{Foreign, ForeignObject};
use cadcraft_doc::{ClipBoundary, Drawing, EntityKind, Handle, RawTag};
use cadcraft_geom::Mat3;

const MAX_POINTS: usize = cadcraft_render::clip::MAX_CLIP_POINTS;
const MAX_HANDLE: u64 = 1 << 48;

fn raw(code: i32, value: impl ToString) -> RawTag {
    RawTag { code, value: value.to_string() }
}

fn entry(tags: &[RawTag], name: &str) -> Option<Handle> {
    tags.windows(2)
        .find(|pair| pair[0].code == 3 && pair[0].value.eq_ignore_ascii_case(name) && matches!(pair[1].code, 350 | 360))
        .and_then(|pair| Handle::parse_hex(&pair[1].value))
}

fn remove_entry(tags: &mut Vec<RawTag>, name: &str) -> Option<Handle> {
    let mut old = None;
    let mut i = 0;
    while i + 1 < tags.len() {
        if tags[i].code == 3 && tags[i].value.eq_ignore_ascii_case(name) && matches!(tags[i + 1].code, 350 | 360) {
            old = old.or_else(|| Handle::parse_hex(&tags[i + 1].value));
            tags.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
    old
}

fn number(tags: &[RawTag], code: i32) -> Option<f64> {
    tags.iter().find(|t| t.code == code)?.value.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn object(f: &Foreign, h: Handle) -> Option<&ForeignObject> {
    f.objects.iter().find(|o| o.handle == h)
}

fn filter(f: &Foreign, h: Handle) -> Option<&ForeignObject> {
    let xd = f.entities.get(&h)?.xdict?;
    let fd = entry(&object(f, xd)?.tags, "ACAD_FILTER")?;
    let sf = entry(&object(f, fd)?.tags, "SPATIAL")?;
    object(f, sf).filter(|o| o.kind.eq_ignore_ascii_case("SPATIAL_FILTER"))
}

fn parse_filter(o: &ForeignObject) -> Option<ClipBoundary> {
    let count = number(&o.tags, 70)? as usize;
    if !(2..=MAX_POINTS).contains(&count)
        || number(&o.tags, 210).unwrap_or(0.0).abs() > 1e-9
        || number(&o.tags, 220).unwrap_or(0.0).abs() > 1e-9
        || (number(&o.tags, 230).unwrap_or(1.0) - 1.0).abs() > 1e-9
        || number(&o.tags, 11).unwrap_or(0.0).abs() > 1e-9
        || number(&o.tags, 21).unwrap_or(0.0).abs() > 1e-9
        || number(&o.tags, 31).unwrap_or(0.0).abs() > 1e-9
        || number(&o.tags, 72).unwrap_or(0.0) != 0.0
        || number(&o.tags, 73).unwrap_or(0.0) != 0.0
    {
        return None;
    }
    // A nontrivial boundary-coordinate transform cannot be represented by the current 2D model.
    // Keep such a filter raw so a later save does not discard or alter it.
    let matrix: Vec<f64> = o.tags.iter().filter(|t| t.code == 40).filter_map(|t| t.value.parse::<f64>().ok()).collect();
    let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    // Earlier CADCraft builds wrote these 4×3 matrices as 3×4. Read that legacy
    // form so saving an old drawing can rewrite it with the correct layout.
    let legacy_identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    if matrix.len() != 24
        || matrix.get(12..24).is_none_or(|m| {
            let matches = |expected: [f64; 12]| m.iter().zip(expected).all(|(a, b)| a.is_finite() && (a - b).abs() <= 1e-9);
            !matches(identity) && !matches(legacy_identity)
        })
    {
        return None;
    }
    let legacy = matrix[12..24].iter().zip(legacy_identity).all(|(a, b)| (a - b).abs() <= 1e-9);
    let inverse = Mat3 { a: matrix[0], b: matrix[1], c: matrix[4], d: matrix[5], e: matrix[3], f: matrix[7] };
    if !matrix.iter().all(|n| n.is_finite()) || (!legacy && inverse.inverse().is_none()) {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for (i, t) in o.tags.iter().enumerate() {
        if t.code != 10 {
            continue;
        }
        let x = t.value.parse::<f64>().ok()?;
        let y = o.tags.get(i + 1).filter(|t| t.code == 20)?.value.parse::<f64>().ok()?;
        let world = cadcraft_geom::Vec2::new(x, y);
        // Standard filters store boundary vertices in drawing coordinates.
        // Legacy CADCraft filters stored them in block coordinates already.
        let p = if legacy { world } else { inverse.apply(world) };
        if !p.is_finite() {
            return None;
        }
        points.push(p);
        if points.len() > count {
            return None;
        }
    }
    if points.len() != count
        || cadcraft_render::clip::PolyClip::new(&ClipBoundary { points: points.clone(), enabled: true, inverted: false, deleted: false }.polygon())
            .is_none()
    {
        return None;
    }
    Some(ClipBoundary { points, enabled: number(&o.tags, 71).unwrap_or(1.0) != 0.0, inverted: false, deleted: false })
}

/// Resolve supported planar SPATIAL_FILTER objects after the raw-data keeper has read them.
pub(crate) fn read(d: &mut Drawing) {
    if d.foreign.objects.is_empty() {
        return;
    }
    let clips: Vec<(Handle, ClipBoundary)> = d.foreign.entities.keys().filter_map(|h| Some((*h, parse_filter(filter(&d.foreign, *h)?)?))).collect();
    for (h, clip) in clips {
        if d.modify_entity(h, |e| {
            if let EntityKind::Insert(i) = &mut e.kind {
                i.clip = Some(clip.clone());
            }
        })
        .is_ok()
        {
            continue;
        }
        for block in d.blocks.values_mut() {
            if block.entities.iter().any(|e| e.handle == h) {
                Arc::make_mut(block).entities.modify(h, |e| {
                    if let EntityKind::Insert(i) = &mut e.kind {
                        i.clip = Some(clip.clone());
                    }
                });
                break;
            }
        }
    }
}

fn dict(h: Handle, owner: Handle) -> ForeignObject {
    ForeignObject {
        handle: h,
        kind: "DICTIONARY".into(),
        tags: vec![
            raw(5, h.hex()),
            raw(102, "{ACAD_REACTORS"),
            raw(330, owner.hex()),
            raw(102, "}"),
            raw(330, owner.hex()),
            raw(100, "AcDbDictionary"),
            raw(280, 1),
            raw(281, 1),
        ],
    }
}

fn spatial(h: Handle, owner: Handle, clip: &ClipBoundary, transform: Mat3, inverse: Mat3) -> ForeignObject {
    let mut tags = vec![
        raw(5, h.hex()),
        raw(102, "{ACAD_REACTORS"),
        raw(330, owner.hex()),
        raw(102, "}"),
        raw(330, owner.hex()),
        raw(100, "AcDbFilter"),
        raw(100, "AcDbSpatialFilter"),
        raw(70, clip.points.len()),
    ];
    for local in &clip.points {
        let p = transform.apply(*local);
        tags.push(raw(10, p.x));
        tags.push(raw(20, p.y));
    }
    tags.extend([
        raw(210, 0),
        raw(220, 0),
        raw(230, 1),
        raw(11, 0),
        raw(21, 0),
        raw(31, 0),
        raw(71, i32::from(clip.enabled)),
        raw(72, 0),
        raw(73, 0),
    ]);
    // Each 4×3 matrix has four rows and three columns, stored column-major.
    // The fourth row contains translation, so its values are entries 4, 8, and 12.
    // The first matrix is the inverse block reference; the second is identity for
    // boundary points already expressed in drawing coordinates.
    for n in [inverse.a, inverse.b, 0.0, inverse.e, inverse.c, inverse.d, 0.0, inverse.f, 0.0, 0.0, 1.0, 0.0] {
        tags.push(raw(40, n));
    }
    for n in [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        tags.push(raw(40, n));
    }
    ForeignObject { handle: h, kind: "SPATIAL_FILTER".into(), tags }
}

/// A temporary drawing with each modelled XCLIP overlaid on its preserved extension dictionary.
/// Deleted clips remove the old SPATIAL entry while retaining unrelated dictionary content.
pub(crate) fn prepare(d: &Drawing) -> Option<Drawing> {
    let mut clips = Vec::new();
    let mut take = |e: &cadcraft_doc::Entity| {
        if let EntityKind::Insert(i) = &e.kind
            && let Some(c) = &i.clip
            && let Some(base) = d.block(&i.block).map(|b| b.base.xy())
        {
            clips.push((e.handle, c.clone(), i.transform(base)));
        }
    };
    for e in d.model.iter() {
        take(e);
    }
    for layout in &d.layouts {
        for e in layout.entities.iter() {
            take(e);
        }
    }
    for block in d.blocks.values() {
        for e in block.entities.iter() {
            take(e);
        }
    }
    if clips.is_empty() {
        return None;
    }
    let mut out = d.clone();
    let f = Arc::make_mut(&mut out.foreign);
    let mut next = d.handseed.max(f.objects.iter().map(|o| o.handle.0).max().unwrap_or(0)).saturating_add(1);
    fn fresh(next: &mut u64) -> Handle {
        let h = Handle(*next);
        *next = (*next).saturating_add(1);
        h
    }
    for (insert, clip, xf) in clips {
        if clip.deleted && filter(f, insert).is_none() {
            continue;
        }
        let inverse = xf.inverse();
        if !clip.deleted
            && (inverse.is_none()
                || !(2..=MAX_POINTS).contains(&clip.points.len())
                || cadcraft_render::clip::PolyClip::new(&clip.polygon()).is_none())
        {
            continue;
        }
        if next.saturating_add(3) >= MAX_HANDLE {
            break;
        }
        let old_xd = f.entities.get(&insert).and_then(|x| x.xdict);
        let xd = old_xd.filter(|h| object(f, *h).is_some()).unwrap_or_else(|| fresh(&mut next));
        if old_xd != Some(xd) {
            f.objects.push(dict(xd, insert));
            f.entities.entry(insert).or_default().xdict = Some(xd);
        }
        let old_fd = object(f, xd).and_then(|o| entry(&o.tags, "ACAD_FILTER"));
        let fd = old_fd.filter(|h| object(f, *h).is_some()).unwrap_or_else(|| fresh(&mut next));
        if old_fd != Some(fd) {
            if let Some(o) = f.objects.iter_mut().find(|o| o.handle == xd) {
                remove_entry(&mut o.tags, "ACAD_FILTER");
                o.tags.extend([raw(3, "ACAD_FILTER"), raw(350, fd.hex())]);
            }
            f.objects.push(dict(fd, xd));
        }
        let old_sf = f.objects.iter_mut().find(|o| o.handle == fd).and_then(|o| remove_entry(&mut o.tags, "SPATIAL"));
        if let Some(h) = old_sf {
            f.objects.retain(|o| o.handle != h);
        }
        if !clip.deleted {
            let h = fresh(&mut next);
            if let Some(o) = f.objects.iter_mut().find(|o| o.handle == fd) {
                o.tags.extend([raw(3, "SPATIAL"), raw(350, h.hex())]);
            }
            if let Some(inv) = inverse {
                f.objects.push(spatial(h, fd, &clip, xf, inv));
            }
        }
    }
    Some(out)
}
