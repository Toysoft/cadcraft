//! Table cell content: blocks in cells (TINSERT) with attribute values, fitted into the cell, and
//! cells' own borders (`table.cellborders`), drawn, and kept through DXF.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, Handle, Lineweight, Table};
use cadcraft_engine::render;
use serde_json::json;

fn table(s: &Session, h: &str) -> Table {
    match &s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind {
        EntityKind::Table(t) => t.clone(),
        _ => panic!("not a table"),
    }
}

/// A 2×2 table with 10-wide, 4-high cells and a block "TAG" (a 2×1 rectangle and an attribute
/// NUMBER), both from the origin.
fn session() -> (Session, String) {
    let mut s = Session::new();
    let r = s.execute("table", &json!({ "at": [0, 0], "rows": 2, "cols": 2, "rowHeight": 4.0, "colWidth": 10.0 })).unwrap();
    let h = r["handle"].as_str().unwrap().to_string();
    let rect = s.execute("rectang", &json!({ "p1": [100, 100], "p2": [102, 101] })).unwrap();
    let att = s.execute("attdef", &json!({ "tag": "NUMBER", "default": "0", "at": [100.5, 100.25], "height": 0.5 })).unwrap();
    let hs: Vec<String> = [rect, att].iter().map(|v| v["handle"].as_str().unwrap().to_string()).collect();
    s.execute("block", &json!({ "name": "TAG", "base": [100, 100], "handles": hs })).unwrap();
    (s, h)
}

#[test]
fn a_block_fills_its_cell_with_its_attribute_values() {
    let (mut s, h) = session();
    s.execute("table.set", &json!({ "handle": h, "row": 0, "col": 1, "text": "gone" })).unwrap();
    s.execute("tinsert", &json!({ "handle": h, "row": 0, "col": 1, "block": "tag", "attribs": { "number": 7 } })).unwrap();
    let t = table(&s, &h);
    let cb = t.cells[0][1].block.as_ref().unwrap();
    assert_eq!(cb.name, "TAG");
    assert_eq!(cb.attrib("NUMBER"), Some("7"));
    // A cell holds a block or text.
    assert!(t.cells[0][1].text.is_empty());
    let g = render::table_geometry(s.doc().unwrap(), &t);
    assert_eq!(g.blocks.len(), 1);
    let b = &g.blocks[0].block.insert;
    // Fitted: the 2 × 1 block in a 10 × 4 cell is limited by the height inside the margins.
    let style = render::table_style_of(s.doc().unwrap(), &t);
    let inner = 4.0 - 2.0 * style.cell_style("Data").margin_v;
    assert!((b.scale.x - inner).abs() < 1e-6, "scale {}", b.scale.x);
    // Inside the cell (x 10..20, y -4..0).
    assert!(b.insert.x >= 10.0 - 1e-9 && b.insert.x <= 20.0 && b.insert.y <= 0.0 && b.insert.y >= -4.0, "{:?}", b.insert);
    assert_eq!(b.attribs.len(), 1);
    assert_eq!(b.attribs[0].text.value, "7");
    // No text is drawn for it.
    assert!(g.texts.iter().all(|x| x.text != "gone"));
}

#[test]
fn a_fixed_scale_is_kept_and_an_empty_name_removes_the_block() {
    let (mut s, h) = session();
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "TAG", "scale": 0.5, "rotation": 90 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let b = &g.blocks[0].block.insert;
    assert_eq!(b.scale.x, 0.5);
    assert!((b.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "" })).unwrap();
    assert!(table(&s, &h).cells[1][0].block.is_none());
    // Typing text into a block's cell replaces the block.
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "TAG" })).unwrap();
    s.execute("tabledit", &json!({ "at": [5, -6], "text": "text" })).unwrap();
    let c = &table(&s, &h).cells[1][0];
    assert_eq!((c.text.as_str(), c.block.is_none()), ("text", true));
}

#[test]
fn bad_tinsert_parameters_are_refused() {
    let (mut s, h) = session();
    for p in [
        json!({ "handle": h, "row": 0, "col": 0, "block": "NOPE" }),
        json!({ "handle": h, "row": 9, "col": 0, "block": "TAG" }),
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "scale": -1 }),
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "scale": "huge" }),
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "attribs": [1, 2] }),
        // TAG is not a dynamic block: it has no visibility states.
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "props": { "Visibility1": "A" } }),
        json!({ "at": [500, 500], "block": "TAG" }),
    ] {
        assert!(s.execute("tinsert", &p).is_err(), "{p}");
    }
    assert!(table(&s, &h).cells.iter().flatten().all(|c| c.block.is_none()));
}

#[test]
fn hostile_tinsert_params_never_panic() {
    let (mut s, h) = session();
    let long = "x".repeat(100_000);
    for p in [
        json!({}),
        json!({ "handle": h }),
        json!({ "handle": h, "row": u64::MAX, "col": u64::MAX, "block": "TAG" }),
        json!({ "handle": h, "row": 0, "col": 0, "block": long }),
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "rotation": 1e308, "scale": 1e308 }),
        json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "attribs": { "NUMBER": long } }),
        json!({ "handle": h, "row": 0, "col": 0, "rows": u64::MAX, "cols": u64::MAX, "edges": ["all"], "lineweight": 99 }),
    ] {
        let _ = s.execute("tinsert", &p);
        let _ = s.execute("tableborders", &p);
    }
    let t = table(&s, &h);
    let _ = render::table_geometry(s.doc().unwrap(), &t);
}

#[test]
fn cell_borders_override_the_style_and_the_heavier_wins() {
    let (mut s, h) = session();
    // The whole table's outline heavy, the top row's bottom thin: on the edge between the rows
    // the second row's top (not set) leaves the first row's thin bottom.
    s.execute("tableborders", &json!({ "handle": h, "row": 0, "col": 0, "rows": 2, "cols": 2, "edges": "outside", "lineweight": 0.5 })).unwrap();
    s.execute("tableborders", &json!({ "handle": h, "row": 0, "col": 0, "cols": 2, "edges": "bottom", "lineweight": 0.18 })).unwrap();
    // The lower left cell's top heavier than the upper left cell's bottom: it wins there.
    s.execute("tableborders", &json!({ "handle": h, "row": 1, "col": 0, "edges": "top", "lineweight": 0.7 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    // The lineweights of the horizontal segments at height `y` overlapping x0..x1.
    let at = |y: f64, x0: f64, x1: f64| {
        g.lines
            .iter()
            .filter(|l| (l.a.y - y).abs() < 1e-9 && (l.b.y - y).abs() < 1e-9 && l.a.x.min(l.b.x) < x1 - 1e-9 && l.a.x.max(l.b.x) > x0 + 1e-9)
            .map(|l| l.lineweight)
            .collect::<Vec<_>>()
    };
    assert_eq!(at(0.0, 0.0, 20.0), [Lineweight::Mm100(50)], "top");
    assert_eq!(at(-4.0, 0.0, 10.0), [Lineweight::Mm100(70)], "middle, left");
    assert_eq!(at(-4.0, 10.0, 20.0), [Lineweight::Mm100(18)], "middle, right");
    // Clearing goes back to the cell style.
    s.execute("tableborders", &json!({ "handle": h, "row": 0, "col": 0, "rows": 2, "cols": 2, "clear": true })).unwrap();
    assert!(table(&s, &h).cells.iter().flatten().all(|c| c.edges.is_none()));
}

#[test]
fn block_cells_and_borders_survive_dxf() {
    let (mut s, h) = session();
    s.execute("tinsert", &json!({ "handle": h, "row": 0, "col": 0, "block": "TAG", "attribs": { "NUMBER": "12" } })).unwrap();
    s.execute("tableborders", &json!({ "handle": h, "row": 1, "col": 1, "edges": "left", "lineweight": 0.5, "color": 1 })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": "a long text that wraps in its cell" })).unwrap();
    let d = s.doc().unwrap();
    let dxf = cadcraft_io::write_dxf(d);
    // AutoCAD sees a block cell: type 2 with the block's record and the attribute value.
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    let pair = |code: &str, value: &str| lines.windows(2).any(|w| w[0] == code && w[1] == value);
    assert!(pair("171", "2"), "block cell type");
    assert!(pair("300", "12"), "attribute value");
    let back = cadcraft_io::read_dxf(dxf.as_bytes()).unwrap();
    let t2 = back
        .model
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Table(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap();
    let t = table(&s, &h);
    assert_eq!(t2.cells[0][0].block, t.cells[0][0].block);
    assert_eq!(t2.cells[1][1].edges, t.cells[1][1].edges);
    // The wrapped text keeps its cell width.
    let g = render::table_geometry(&back, &t2);
    let w = g.texts.iter().find(|x| x.text.starts_with("a long")).unwrap().width;
    assert!(w > 0.0 && w < 10.0, "{w}");
}

/// A session with a table and a dynamic block "Door" (kept foreign data, built by hand): a
/// visibility parameter whose state Open shows its line and Closed hides it; a circle is shown
/// in both.
fn dynamic_session() -> (Session, String, Handle) {
    use cadcraft_engine::doc::RawTag;
    use cadcraft_engine::doc::foreign::{Foreign, ForeignExtra, ForeignObject};
    let obj = |handle: u64, kind: &str, tags: &[(i32, &str)]| ForeignObject {
        handle: Handle(handle),
        kind: kind.into(),
        tags: tags.iter().map(|(c, v)| RawTag { code: *c, value: (*v).into() }).collect(),
    };
    let mut s = Session::new();
    let h = s.execute("table", &json!({ "at": [0, 0], "rows": 2, "cols": 1, "rowHeight": 4.0, "colWidth": 10.0 })).unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    let l = s.execute("line", &json!({ "points": [[0, 0], [4, 0]] })).unwrap()["handles"][0].clone();
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap()["handle"].clone();
    s.execute("block", &json!({ "name": "Door", "base": [0, 0], "handles": [l, c] })).unwrap();
    let blk = s.doc().unwrap().block("Door").unwrap().clone();
    let line = blk.entities.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).unwrap().handle;
    let lh = line.hex();
    let mut f = Foreign::default();
    f.records.insert(("BLOCK_RECORD".into(), "DOOR".into()), ForeignExtra { xdict: Some(Handle(0xF00)), ..Default::default() });
    f.objects.push(obj(0xF00, "DICTIONARY", &[(100, "AcDbDictionary"), (3, "ACAD_ENHANCEDBLOCK"), (360, "F01")]));
    f.objects.push(obj(0xF01, "ACAD_EVALUATION_GRAPH", &[(100, "AcDbEvalGraph"), (91, "0"), (95, "1"), (360, "F02")]));
    f.objects.push(obj(
        0xF02,
        "BLOCKVISIBILITYPARAMETER",
        &[
            (100, "AcDbEvalExpr"),
            (90, "1"),
            (100, "AcDbBlockElement"),
            (300, "Visibility State"),
            (100, "AcDbBlockParameter"),
            (280, "1"),
            (100, "AcDbBlock1PtParameter"),
            (1010, "2"),
            (1020, "3"),
            (100, "AcDbBlockVisibilityParameter"),
            (301, "Visibility1"),
            (302, ""),
            (93, "1"),
            (331, &lh),
            (92, "2"),
            (303, "Open"),
            (94, "1"),
            (332, &lh),
            (95, "0"),
            (303, "Closed"),
            (94, "0"),
            (95, "0"),
        ],
    ));
    let mut d = s.doc().unwrap().clone();
    d.foreign = std::sync::Arc::new(f);
    s.open_drawing(d, "dynamic", None);
    (s, h, line)
}

#[test]
fn a_dynamic_block_shows_the_cells_visibility_state() {
    let (mut s, h, line) = dynamic_session();
    s.execute("tinsert", &json!({ "handle": h, "row": 0, "col": 0, "block": "Door", "props": { "Visibility1": "open" } })).unwrap();
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "Door", "props": { "visibility": "Closed" } })).unwrap();
    // An unknown state is refused with the states it may be.
    let e = s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "Door", "props": { "Visibility1": "Ajar" } })).unwrap_err();
    assert!(e.to_string().contains("Open, Closed"), "{e}");
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let blk = s.doc().unwrap().block("Door").unwrap().clone();
    let shown = |row: usize| {
        let v = &g.blocks.iter().find(|b| b.row == row).unwrap().block.visibility;
        blk.entities.iter().filter(|e| v.shows(e)).map(|e| e.handle).collect::<Vec<_>>()
    };
    assert!(shown(0).contains(&line), "Open shows the line");
    assert!(!shown(1).contains(&line), "Closed hides the line");
    assert_eq!(shown(1).len(), 1, "the circle stays");
    // The *T block AutoCAD draws shows the states too: switching the closed door open adds its
    // line there.
    // (LINEs, CIRCLEs) in the written file: the door's definition has one of each.
    let lines_in_t = |s: &Session| {
        let dxf = cadcraft_io::write_dxf(s.doc().unwrap());
        let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
        let n = |kind: &str| lines.windows(2).filter(|w| w[0] == "0" && w[1] == kind).count();
        (n("LINE"), n("CIRCLE"))
    };
    let (closed, circles) = lines_in_t(&s);
    assert_eq!(circles, 3);
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 0, "block": "Door", "props": { "Visibility1": "Open" } })).unwrap();
    assert_eq!(lines_in_t(&s), (closed + 1, 3));
}

#[test]
fn a_columns_alignment_and_margin_override_the_cell_style() {
    let mut s = Session::new();
    let h = s
        .execute("table", &json!({ "at": [0, 0], "header": ["N", "Name"], "cells": [["1", "a"], ["2", "b"]], "rowHeight": 4.0, "colWidth": 10.0 }))
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    // Column 0's data rows: middle centre; column 1's: middle right, 2 from the right side.
    let r = s.execute("tableformat", &json!({ "handle": h, "row": 1, "col": 0, "scope": "column", "align": "MC" })).unwrap();
    assert_eq!(r["cells"], 2);
    s.execute("tableformat", &json!({ "handle": h, "row": 1, "col": 1, "scope": "column", "align": "MR", "marginH": 2.0 })).unwrap();
    let t = table(&s, &h);
    assert!(t.cells[0][0].look.is_none(), "the header row keeps its style");
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let at = |text: &str| g.texts.iter().find(|x| x.text == text).unwrap().clone();
    let one = at("1");
    assert_eq!(one.attach, 5);
    // Row 1 spans y -4..-8: its middle is -6; column 0 spans x 0..10.
    assert!((one.at.x - 5.0).abs() < 1e-9 && (one.at.y + 6.0).abs() < 1e-9, "{:?}", one.at);
    let a = at("a");
    assert_eq!(a.attach, 6);
    assert!((a.at.x - 18.0).abs() < 1e-9, "{:?}", a.at);
    // Clearing goes back to the style.
    s.execute("tableformat", &json!({ "handle": h, "row": 1, "col": 0, "scope": "table", "clear": true })).unwrap();
    assert!(table(&s, &h).cells.iter().flatten().all(|c| c.look.is_none()));
}

#[test]
fn vertical_header_text_reads_upwards_and_wraps_along_the_cell_height() {
    let (mut s, h) = session();
    s.execute("table.set", &json!({ "handle": h, "row": 0, "col": 0, "text": "Format" })).unwrap();
    s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "align": "MC", "rotation": 90 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let x = g.texts.iter().find(|x| x.text == "Format").unwrap();
    assert!((x.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    // The cell is 10 wide and 4 high: the text wraps at the height.
    let mh = render::table_style_of(s.doc().unwrap(), &t).cell_style("Data").margin_h;
    assert!((x.width - (4.0 - 2.0 * mh)).abs() < 1e-9, "{}", x.width);
    assert!((x.at.x - 5.0).abs() < 1e-9 && (x.at.y + 2.0).abs() < 1e-9, "{:?}", x.at);
    // Kept through DXF.
    let back = cadcraft_io::read_dxf(cadcraft_io::write_dxf(s.doc().unwrap()).as_bytes()).unwrap();
    let t2 = back.model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    assert_eq!(t2.cells[0][0].look, t.cells[0][0].look);
}

#[test]
fn cellformat_at_the_command_line() {
    let (mut s, h) = session();
    s.cmdline("tableformat").unwrap();
    // Cell (0, 1): x 10..20, y 0..-4; its column, middle right, margin 1.5, no rotation change.
    s.cmdline("15,-2").unwrap();
    s.cmdline("o").unwrap();
    s.cmdline("mr").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("1.5").unwrap();
    s.cmdline("").unwrap();
    assert!(!s.prompting());
    let t = table(&s, &h);
    for r in 0..2 {
        let l = t.cells[r][1].look.unwrap();
        assert_eq!((l.align, l.margin_h, l.rotation), (Some(6), Some(1.5), None));
    }
    assert!(t.cells[0][0].look.is_none());
    // A range by its two corners.
    s.cmdline("tableformat").unwrap();
    s.cmdline("5,-6").unwrap();
    s.cmdline("a").unwrap();
    s.cmdline("15,-2").unwrap();
    s.cmdline("tc").unwrap();
    s.cmdline("2.5").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("90").unwrap();
    let t = table(&s, &h);
    assert!(t.cells.iter().flatten().all(|c| c.look.is_some_and(|l| l.align == Some(2) && l.rotation == Some(90))));
    assert!(t.cells.iter().flatten().all(|c| c.text_height == Some(2.5)));
}

#[test]
fn a_cells_text_height_is_drawn_and_kept() {
    let (mut s, h) = session();
    s.execute("table.set", &json!({ "handle": h, "row": 0, "col": 0, "text": "A" })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": "B" })).unwrap();
    s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "textHeight": 2.5 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let height = |x: &str| g.texts.iter().find(|t| t.text == x).unwrap().height;
    assert_eq!(height("A"), 2.5);
    assert_ne!(height("B"), 2.5, "the other cell keeps its style's height");
    let back = cadcraft_io::read_dxf(cadcraft_io::write_dxf(s.doc().unwrap()).as_bytes()).unwrap();
    let t2 = back.model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    assert_eq!(t2.cells[0][0].text_height, Some(2.5));
    for bad in [json!(0), json!(-1), json!("big"), json!(1e300)] {
        assert!(s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "textHeight": bad })).is_err(), "{bad}");
    }
    s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "clear": true })).unwrap();
    assert_eq!(table(&s, &h).cells[0][0].text_height, None);
}

#[test]
fn bad_cellformat_parameters_are_refused() {
    let (mut s, h) = session();
    for p in [
        json!({ "handle": h, "row": 0, "col": 0 }),
        json!({ "handle": h, "row": 0, "col": 0, "align": "XX" }),
        json!({ "handle": h, "row": 0, "col": 0, "rotation": 45 }),
        json!({ "handle": h, "row": 0, "col": 0, "margin": -1 }),
        json!({ "handle": h, "row": 0, "col": 0, "align": "MC", "scope": "diagonal" }),
        json!({ "handle": h, "row": 7, "col": 0, "align": "MC" }),
        json!({ "handle": h, "row": 0, "col": 0, "align": "MC", "scope": "range", "rows": u64::MAX, "cols": u64::MAX, "rotation": u64::MAX }),
    ] {
        assert!(s.execute("tableformat", &p).is_err(), "{p}");
    }
    let r = s.execute("tableformat", &json!({ "handle": h, "row": 1, "col": 1, "align": 5, "scope": "range", "rows": u64::MAX, "cols": u64::MAX }));
    assert_eq!(r.unwrap()["cells"], 1, "a range is cut at the table's edge");
}

#[test]
fn tableborders_at_the_command_line() {
    let (mut s, h) = session();
    // The whole table's outline at 1 mm, typed by alias; then column 1's left edge cleared.
    s.cmdline("tbo").unwrap();
    s.cmdline("5,-2").unwrap();
    s.cmdline("t").unwrap();
    s.cmdline("o").unwrap();
    s.cmdline("1").unwrap();
    assert!(!s.prompting());
    let t = table(&s, &h);
    let top = |r: usize, c: usize| t.cells[r][c].edges.and_then(|e| e.top).map(|b| b.lineweight);
    assert_eq!(top(0, 0), Some(Lineweight::Mm100(100)));
    assert_eq!(top(1, 0), None, "inside edges keep the style");
    assert_eq!(t.cells[1][1].edges.and_then(|e| e.right).map(|b| b.lineweight), Some(Lineweight::Mm100(100)));
    s.cmdline("tableborders").unwrap();
    s.cmdline("15,-2").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("l").unwrap();
    s.cmdline("c").unwrap();
    assert!(table(&s, &h).cells[0][1].edges.and_then(|e| e.left).is_none());
    // A bad lineweight asks again.
    s.cmdline("tableborders").unwrap();
    s.cmdline("15,-2").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("thick").unwrap();
    assert!(s.prompting());
    s.cancel();
}

#[test]
fn merging_keeps_a_block_and_unmerge_takes_a_range() {
    let (mut s, h) = session();
    s.execute("tinsert", &json!({ "handle": h, "row": 1, "col": 1, "block": "TAG" })).unwrap();
    s.execute("table.merge", &json!({ "handle": h, "row": 0, "col": 0, "rows": 2, "cols": 2 })).unwrap();
    let t = table(&s, &h);
    assert_eq!(t.cells[0][0].block.as_ref().map(|b| b.name.as_str()), Some("TAG"), "the block moves to the merged cell");
    assert!(t.cells[1][1].block.is_none());
    // Text in the anchor keeps it: no block then.
    s.execute("table.unmerge", &json!({ "handle": h, "row": 1, "col": 1 })).unwrap();
    assert!(table(&s, &h).cells[0][0].merged.is_none(), "any cell of a merge unmerges it");
    s.execute("table.merge", &json!({ "handle": h, "row": 0, "col": 0, "cols": 2 })).unwrap();
    s.execute("table.merge", &json!({ "handle": h, "row": 1, "col": 0, "cols": 2 })).unwrap();
    s.execute("table.unmerge", &json!({ "handle": h, "row": 0, "col": 0, "rows": 2, "cols": 1 })).unwrap();
    assert!(table(&s, &h).cells.iter().flatten().all(|c| c.merged.is_none()), "every merge meeting the range");
}

#[test]
fn tablesize_sets_widths_and_heights() {
    let (mut s, h) = session();
    // All the column widths at once (an ESKD header's, say), then one row's height.
    s.execute("tablesize", &json!({ "handle": h, "widths": [20, 8] })).unwrap();
    s.execute("tablesize", &json!({ "handle": h, "row": 1, "col": 0, "scope": "row", "height": 15 })).unwrap();
    let t = table(&s, &h);
    assert_eq!((t.col_widths.clone(), t.row_heights.clone()), (vec![20.0, 8.0], vec![4.0, 15.0]));
    // At the command line: pick a cell, its column, width 12, height kept.
    s.cmdline("tsi").unwrap();
    s.cmdline("25,-2").unwrap();
    s.cmdline("o").unwrap();
    s.cmdline("12").unwrap();
    s.cmdline("").unwrap();
    assert!(!s.prompting());
    assert_eq!(table(&s, &h).col_widths, vec![20.0, 12.0]);
    for p in [
        json!({ "handle": h }),
        json!({ "handle": h, "widths": [1, 2, 3] }),
        json!({ "handle": h, "widths": [0] }),
        json!({ "handle": h, "widths": "wide" }),
        json!({ "handle": h, "row": 0, "col": 0, "width": -5 }),
        json!({ "handle": h, "row": 0, "col": 0, "height": 1e300 }),
        json!({ "handle": h, "row": 9, "col": 0, "width": 5 }),
    ] {
        assert!(s.execute("tablesize", &p).is_err(), "{p}");
    }
}

#[test]
fn rows_grow_for_their_text_freely_in_steps_or_not_at_all() {
    let (mut s, h) = session();
    let long = "a long text that wraps over several lines in its cell";
    s.execute("tableformat", &json!({ "handle": h, "scope": "table", "row": 0, "col": 0, "textHeight": 2.5 })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": long })).unwrap();
    let t = table(&s, &h);
    let (needs, _) = render::table_row_needs(s.doc().unwrap(), &t);
    assert!(needs[1] > 8.0, "several lines of 2.5: {needs:?}");
    assert_eq!(t.row_heights, vec![4.0, needs[1]], "free: as much as the text needs");
    assert_eq!(t.row_min, vec![4.0, 4.0]);
    // ESKD: rows grow to multiples of 8.
    s.execute("tablesize", &json!({ "handle": h, "rowGrow": 8 })).unwrap();
    let hh = table(&s, &h).row_heights[1];
    assert!(hh >= needs[1] && hh % 8.0 == 0.0 && hh < needs[1] + 8.0, "{hh}");
    // Fixed: the row keeps the height it was given.
    s.execute("tablesize", &json!({ "handle": h, "rowGrow": "fixed" })).unwrap();
    assert_eq!(table(&s, &h).row_heights, vec![4.0, 4.0]);
    // Kept through DXF.
    s.execute("tablesize", &json!({ "handle": h, "rowGrow": "8" })).unwrap();
    let back = cadcraft_io::read_dxf(cadcraft_io::write_dxf(s.doc().unwrap()).as_bytes()).unwrap();
    let t2 = back.model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    let t = table(&s, &h);
    assert_eq!((t2.row_grow, t2.row_min.clone()), (t.row_grow, t.row_min.clone()));
    // A row given a new height grows from it; without the text it shrinks back to it.
    s.execute("tablesize", &json!({ "handle": h, "row": 1, "col": 0, "height": 6 })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": "" })).unwrap();
    let t = table(&s, &h);
    assert_eq!(t.row_heights, vec![4.0, 6.0]);
    assert!(t.row_min.is_empty());
    for bad in [json!(0), json!(-8), json!("nan"), json!("huge"), json!({ "step": 8 }), json!(1e300)] {
        assert!(s.execute("tablesize", &json!({ "handle": h, "rowGrow": bad })).is_err(), "{bad}");
    }
}

#[test]
fn inserted_and_deleted_rows_keep_the_given_heights_in_step() {
    let (mut s, h) = session();
    s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "textHeight": 2.5 })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 0, "col": 0, "text": "a long text that wraps over several lines" })).unwrap();
    let grown = table(&s, &h).row_heights[0];
    assert!(grown > 4.0);
    s.execute("table.insertrow", &json!({ "handle": h, "row": 0, "height": 5 })).unwrap();
    let t = table(&s, &h);
    assert_eq!(t.row_heights, vec![5.0, grown, 4.0]);
    assert_eq!(t.row_min, vec![5.0, 4.0, 4.0]);
    s.execute("table.deleterow", &json!({ "handle": h, "row": 0 })).unwrap();
    assert_eq!(table(&s, &h).row_min, vec![4.0, 4.0]);
}

#[test]
fn row_growth_at_the_command_line() {
    let (mut s, h) = session();
    s.cmdline("tsi").unwrap();
    s.cmdline("5,-2").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("g").unwrap();
    s.cmdline("8").unwrap();
    assert!(!s.prompting());
    assert_eq!(table(&s, &h).row_grow, cadcraft_engine::doc::RowGrow::Step(8.0));
    // TABLE makes a table that grows by steps too.
    let r = s.execute("table", &json!({ "at": [50, 0], "rows": 1, "cols": 1, "rowHeight": 8, "colWidth": 10, "rowGrow": "fixed" })).unwrap();
    assert_eq!(table(&s, r["handle"].as_str().unwrap()).row_grow, cadcraft_engine::doc::RowGrow::Fixed);
}
