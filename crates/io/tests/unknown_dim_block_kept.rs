//! Saving keeps the anonymous `*D` block an entity of a type CADCraft doesn't model names
//! (issue #1161): an AutoCAD arc-length dimension (ARC_DIMENSION) is kept as read (#528) and
//! still names its block in group 2, so the `*D` block clean-up of a save (#495) must count it as
//! a user. Without that the block went and the reference stayed, and AutoCAD refused the file.
//!
//! The fixture is the one of the issue, written by hand from the DXF Reference.

use cadcraft_doc::EntityKind;
use cadcraft_io::{read_dxf, write_dxf};

/// DXF text from lines of `code value`.
fn dxf(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (code, value) = line.split_once(' ').unwrap_or((line, ""));
        out.push_str(&format!("{code}\n{value}\n"));
    }
    out
}

/// An arc and an arc-length dimension on it, with the dimension's `*D1` block holding one line.
const ARC_DIM: &str = "
0 SECTION
2 BLOCKS
0 BLOCK
8 0
2 *D1
70 1
10 0
20 0
30 0
3 *D1
0 LINE
8 0
10 320
20 150
11 330
21 175
0 ENDBLK
0 ENDSEC
0 SECTION
2 ENTITIES
0 ARC
8 0
10 300
20 150
40 20
50 0
51 90
0 ARC_DIMENSION
5 41
100 AcDbEntity
8 0
100 AcDbDimension
2 *D1
10 333.8
20 169.5
11 327.6
21 177.6
70 5
42 31.4159
100 AcDbArcDimension
13 320
23 150
14 300
24 170
15 300
25 150
40 0
41 1.5708
0 ENDSEC
0 EOF
";

/// (code, value) pairs of an ASCII DXF.
fn pairs(dxf: &str) -> Vec<(i32, String)> {
    let lines: Vec<&str> = dxf.lines().collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.trim().parse().ok()?, c.get(1)?.trim().to_string()))).collect()
}

/// The records of a DXF: (type, groups after it).
fn records(dxf: &str) -> Vec<(String, Vec<(i32, String)>)> {
    let mut out: Vec<(String, Vec<(i32, String)>)> = Vec::new();
    for (code, v) in pairs(dxf) {
        if code == 0 {
            out.push((v, Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push((code, v));
        }
    }
    out
}

fn value(groups: &[(i32, String)], code: i32) -> Option<&str> {
    groups.iter().find(|(c, _)| *c == code).map(|(_, v)| v.as_str())
}

/// Names of the BLOCK definitions.
fn block_names(dxf: &str) -> Vec<String> {
    records(dxf).iter().filter(|(k, _)| k == "BLOCK").filter_map(|(_, g)| value(g, 2).map(str::to_string)).collect()
}

/// The ARC_DIMENSION records' block names (group 2).
fn arc_dim_blocks(dxf: &str) -> Vec<String> {
    records(dxf).iter().filter(|(k, _)| k == "ARC_DIMENSION").filter_map(|(_, g)| value(g, 2).map(str::to_string)).collect()
}

/// The entities of BLOCK `name` as (type, groups).
fn block_entities(dxf: &str, name: &str) -> Vec<(String, Vec<(i32, String)>)> {
    let recs = records(dxf);
    let start = recs.iter().position(|(k, g)| k == "BLOCK" && value(g, 2) == Some(name)).unwrap_or_else(|| panic!("no block {name}"));
    recs.iter().skip(start + 1).take_while(|(k, _)| k != "ENDBLK").cloned().collect()
}

fn resave(dxf: &str) -> String {
    write_dxf(&read_dxf(dxf.as_bytes()).unwrap())
}

/// The saved file still defines `*D1`, with its line, and the ARC_DIMENSION still names it.
fn check(out: &str) {
    assert_eq!(arc_dim_blocks(out), vec!["*D1".to_string()], "ARC_DIMENSION not written or not naming *D1:\n{out}");
    assert!(block_names(out).iter().any(|n| n == "*D1"), "*D1 block dropped; blocks: {:?}", block_names(out));
    let ents = block_entities(out, "*D1");
    assert_eq!(ents.len(), 1, "*D1 entities: {ents:?}");
    let (kind, g) = &ents[0];
    assert_eq!(kind, "LINE");
    assert_eq!(value(g, 10).and_then(|v| v.parse::<f64>().ok()), Some(320.0));
    assert_eq!(value(g, 21).and_then(|v| v.parse::<f64>().ok()), Some(175.0));
}

#[test]
fn the_fixture_reads_as_an_unknown_entity_with_its_block() {
    let d = read_dxf(dxf(ARC_DIM).as_bytes()).unwrap();
    assert!(d.block("*D1").is_some(), "*D1 not read");
    let unknown: Vec<&str> =
        d.model.iter().filter_map(|e| if let EntityKind::Unknown(u) = &e.kind { Some(u.dxf_type.as_str()) } else { None }).collect();
    assert_eq!(unknown, vec!["ARC_DIMENSION"]);
}

#[test]
fn save_keeps_the_block_an_arc_dimension_names() {
    let out = resave(&dxf(ARC_DIM));
    check(&out);
}

#[test]
fn the_block_survives_a_second_save() {
    let once = resave(&dxf(ARC_DIM));
    let twice = resave(&once);
    check(&twice);
    // Reopened, the dimension's block is there to be found.
    let d = read_dxf(twice.as_bytes()).unwrap();
    assert!(d.block("*D1").is_some(), "*D1 missing after two saves");
}

#[test]
fn a_dim_block_nothing_names_is_still_dropped() {
    // The same file with the dimension's block renamed: `*D1` has no user and goes (#495).
    let src = dxf(ARC_DIM).replace("*D1\n10\n333.8", "*D2\n10\n333.8");
    assert!(src.contains("*D2"), "fixture edit failed");
    let out = resave(&src);
    assert_eq!(arc_dim_blocks(out.as_str()), vec!["*D2".to_string()]);
    assert!(!block_names(&out).iter().any(|n| n == "*D1"), "unreferenced *D1 kept: {:?}", block_names(&out));
}

#[test]
fn a_dimension_without_or_with_another_case_of_its_marker_keeps_its_block() {
    let base = dxf(ARC_DIM);
    let marker = "100\nAcDbDimension\n";
    assert!(base.contains(marker), "fixture edit failed");
    // No `100 AcDbDimension` line at all, and the marker written in lower case: the entity is
    // still a dimension by its type, so its block stays.
    for src in [base.replace(marker, ""), base.replace("AcDbDimension", "acdbdimension")] {
        assert_ne!(src, base);
        check(&resave(&src));
    }
}

#[test]
fn a_raw_shape_without_a_marker_is_not_a_dimension() {
    use cadcraft_doc::{RawTag, Unknown};
    let tag = |code, value: &str| RawTag { code, value: value.into() };
    let shape = Unknown { dxf_type: "SHAPE".into(), tags: vec![tag(100, "AcDbShape"), tag(2, "BOX")] };
    assert!(!shape.is_dimension());
    assert_eq!(shape.block_names().count(), 0);
    // A lower-case marker on an unmodelled type still counts.
    let odd = Unknown { dxf_type: "SOMETHING".into(), tags: vec![tag(100, " acdbdimension "), tag(2, "*D9")] };
    assert!(odd.is_dimension());
    let bare = Unknown { dxf_type: " arc_dimension ".into(), tags: vec![tag(2, "*D9")] };
    assert_eq!(bare.block_names().collect::<Vec<_>>(), ["*D9"]);
}
