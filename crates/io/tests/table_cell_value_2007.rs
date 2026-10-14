//! ACAD_TABLE cells in DXF 2007 and later (#1162): the cell text also goes in the cell value
//! block the DXF Reference documents from AutoCAD 2007 (301 block begin, 302 text, 303 chunks),
//! files before 2007 keep the plain group 1/2 text, and every version reads back the same.

use cadcraft_doc::*;
use cadcraft_dxf::Tag;
use cadcraft_geom::Vec3;
use cadcraft_io::{SaveOptions, Version, read, write_with};

/// The table of the issue: `table {"rows":2,"cols":2,"cells":[["a","b"],["c","d"]],"title":"T","header":["H1","H2"]}`.
fn issue_table() -> Table {
    let cell = |t: &str| TableCell { text: t.into(), ..TableCell::default() };
    let mut cells = vec![vec![cell("T"), cell("")], vec![cell("H1"), cell("H2")], vec![cell("a"), cell("b")], vec![cell("c"), cell("d")]];
    cells[0][0].merged = Some((1, 2));
    Table {
        insert: Vec3::ZERO,
        col_widths: vec![2.5, 2.5],
        row_heights: vec![0.27; 4],
        cells,
        style: "Standard".into(),
        text_height: 0.18,
        title: true,
        header: true,
        ..Table::default()
    }
}

fn drawing(t: Table) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Table(t)).unwrap();
    d
}

fn first_table(d: &Drawing) -> Table {
    d.model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).unwrap()
}

fn save(d: &Drawing, v: Version, binary: bool) -> Vec<u8> {
    write_with(d, "t.dxf", &SaveOptions { version: Some(v), binary }).unwrap()
}

/// The groups of each cell (from its 171) of the first ACAD_TABLE in the ENTITIES section.
fn cells_of(bytes: &[u8]) -> Vec<Vec<Tag>> {
    let tags = cadcraft_dxf::parse(bytes).unwrap();
    let ents = tags.iter().position(|t| t.code == 2 && t.str() == "ENTITIES").unwrap();
    let start = ents + tags[ents..].iter().position(|t| t.code == 0 && t.str() == "ACAD_TABLE").unwrap();
    let end = start + 1 + tags[start + 1..].iter().position(|t| t.code == 0).unwrap();
    let ent = &tags[start..end];
    let body = ent.iter().position(|t| t.code == 100 && t.str() == "AcDbTable").unwrap();
    let body = &ent[body..];
    let body = &body[..body.iter().position(|t| t.code >= 1000).unwrap_or(body.len())];
    let mut cells: Vec<Vec<Tag>> = Vec::new();
    for t in body {
        if t.code == 171 {
            cells.push(Vec::new());
        } else if let Some(c) = cells.last_mut() {
            c.push(t.clone());
        }
    }
    cells
}

fn strings(cell: &[Tag], code: i32) -> Vec<String> {
    cell.iter().filter(|t| t.code == code).map(Tag::str).collect()
}

fn pos(cell: &[Tag], code: i32, value: &str) -> Option<usize> {
    cell.iter().position(|t| t.code == code && t.str() == value)
}

const ISSUE_TEXTS: [&str; 8] = ["T", "", "H1", "H2", "a", "b", "c", "d"];

/// From 2007 each text cell carries, after its plain group 1 text, the cell value block: 301
/// begin, the text in 302 and 304 end, and the extended cell flags group 92 the Reference adds in
/// 2007. The value block also holds the text as its group 1 value, as the DWG bridge gives it.
#[test]
fn dxf_2007_and_later_put_the_cell_text_in_the_value_block() {
    let d = drawing(issue_table());
    for v in [Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        for binary in [false, true] {
            let cells = cells_of(&save(&d, v, binary));
            assert_eq!(cells.len(), 8, "{v} binary={binary}");
            for (cell, want) in cells.iter().zip(ISSUE_TEXTS) {
                let what = format!("{v} binary={binary} cell {want:?}: {cell:?}");
                assert_eq!(strings(cell, 92), vec!["0"], "{what}: extended cell flags");
                let begin = pos(cell, 301, "CELL_VALUE").expect(&what);
                let end = pos(cell, 304, "ACVALUE_END").expect(&what);
                assert!(begin < end, "{what}");
                let block = &cell[begin..end];
                assert_eq!(strings(block, 302), vec![want], "{what}: 302 text in the value block");
                assert!(strings(block, 303).is_empty(), "{what}: short text has no 303 chunks");
                assert_eq!(strings(block, 1), vec![want], "{what}: the block's group 1 value");
                // The plain pre-2007 text stays, before the block, for readers of the old groups.
                let plain: Vec<String> = strings(&cell[..begin], 1);
                assert_eq!(plain, vec![want], "{what}: plain group 1 text before the block");
                assert!(strings(&cell[..begin], 2).is_empty(), "{what}");
            }
        }
    }
}

/// 2000 and 2004 files keep the cell groups as before: no value block, no group 92.
#[test]
fn dxf_2004_and_earlier_keep_the_cells_as_before() {
    let d = drawing(issue_table());
    for v in [Version::R2000, Version::R2004] {
        for binary in [false, true] {
            let cells = cells_of(&save(&d, v, binary));
            assert_eq!(cells.len(), 8, "{v}");
            for (cell, want) in cells.iter().zip(ISSUE_TEXTS) {
                let what = format!("{v} binary={binary} cell {want:?}: {cell:?}");
                assert_eq!(strings(cell, 1), vec![want], "{what}");
                for code in [92, 301, 302, 303, 304] {
                    assert!(strings(cell, code).is_empty(), "{what}: no group {code} before 2007");
                }
            }
        }
    }
}

/// Text longer than 250 characters: 250-character chunks in 303 and the rest in 302 (DXF
/// Reference, from 2007), beside the 2/1 chunks of the plain text, and it reads back whole.
#[test]
fn long_cell_text_is_chunked_into_303_groups() {
    let long: String = (0..600).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    let mut t = issue_table();
    t.cells[2][1].text = long.clone();
    let d = drawing(t);
    let cells = cells_of(&save(&d, Version::R2018, false));
    let cell = &cells[5];
    let begin = pos(cell, 301, "CELL_VALUE").unwrap();
    let end = pos(cell, 304, "ACVALUE_END").unwrap();
    let block = &cell[begin..end];
    let chunks = strings(block, 303);
    assert_eq!(chunks.iter().map(String::len).collect::<Vec<_>>(), vec![250, 250]);
    let rest = strings(block, 302);
    assert_eq!(rest.iter().map(String::len).collect::<Vec<_>>(), vec![100]);
    assert_eq!(format!("{}{}", chunks.concat(), rest.concat()), long);
    assert_eq!(strings(&cell[..begin], 2).iter().map(String::len).collect::<Vec<_>>(), vec![250, 250]);
    assert_eq!(strings(&cell[..begin], 1).concat().len(), 100);
    assert_eq!(first_table(&read(&save(&d, Version::R2018, false), "t.dxf").unwrap()).cells[2][1].text, long);
}

/// The issue's table reads back the same from a file of every version, ASCII and binary.
#[test]
fn the_table_reads_back_the_same_at_every_version() {
    let t = issue_table();
    let d = drawing(t.clone());
    for v in Version::ALL {
        for binary in [false, true] {
            let back = read(&save(&d, v, binary), "t.dxf").unwrap();
            assert_eq!(first_table(&back), t, "{v} binary={binary}");
        }
    }
}

fn table_dxf(cells: &str) -> String {
    format!(
        "0\nSECTION\n2\nENTITIES\n0\nACAD_TABLE\n100\nAcDbTable\n91\n1\n92\n4\n141\n0.3\n142\n2\n142\n2\n142\n2\n142\n2\n{cells}0\nENDSEC\n0\nEOF\n"
    )
}

/// Cells written the 2007 way by other programs: the text read is the value block's 302 (with
/// its 303 chunks) when it has one, else the block's group 1 value (the DWG bridge's form), else
/// the plain groups; a plain 302 with a different plain group 1 reads as the 302.
#[test]
fn text_in_the_value_block_is_read() {
    let dxf = table_dxf(
        "171\n1\n92\n0\n301\nCELL_VALUE\n93\n0\n90\n4\n303\nfirst-\n302\nsecond\n304\nACVALUE_END\n\
         171\n1\n301\nCELL_VALUE\n93\n0\n90\n4\n1\nbridge\n94\n0\n300\n\n302\n\n304\nACVALUE_END\n\
         171\n1\n1\nplain\n\
         171\n1\n1\nraw\n301\nCELL_VALUE\n93\n0\n90\n4\n1\nraw\n302\nformatted\n304\nACVALUE_END\n",
    );
    let d = read(dxf.as_bytes(), "t.dxf").unwrap();
    let texts: Vec<String> = first_table(&d).cells[0].iter().map(|c| c.text.clone()).collect();
    assert_eq!(texts, ["first-second", "bridge", "plain", "formatted"]);
}

/// Broken value blocks never panic: a block with no end, an end with no begin, chunks alone,
/// nested begins, a block in the last cell running into the xdata.
#[test]
fn broken_value_blocks_do_not_panic() {
    for cells in [
        "171\n1\n301\nCELL_VALUE\n302\nx\n",
        "171\n1\n304\nACVALUE_END\n302\ny\n1\nz\n",
        "171\n1\n303\nabc\n303\ndef\n",
        "171\n1\n301\nCELL_VALUE\n301\nCELL_VALUE\n302\nq\n304\nACVALUE_END\n304\nACVALUE_END\n",
        "171\n1\n301\nCELL_VALUE\n302\nw\n1001\nCADCRAFT\n1000\nTABLE\n",
        "171\n1\n301\n\n302\n\n303\n\n304\n\n",
    ] {
        let _ = read(table_dxf(cells).as_bytes(), "t.dxf");
    }
}
