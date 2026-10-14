//! Copy, paste and clear table cells (Ctrl+C, Ctrl+V and Delete on picked cells).

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, Handle, Table};
use serde_json::json;

fn table(s: &Session, h: &str) -> Table {
    match &s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind {
        EntityKind::Table(t) => t.clone(),
        _ => panic!("not a table"),
    }
}

/// A 3×3 table, 10 × 4 cells: row 0 "a b c", row 1 "1 2 =A2+B2".
fn session() -> (Session, String) {
    let mut s = Session::new();
    let r = s
        .execute(
            "table",
            &json!({ "at": [0, 0], "rows": 3, "cols": 3, "rowHeight": 4.0, "colWidth": 10.0, "cells": [["a", "b", "c"], ["1", "2", ""]] }),
        )
        .unwrap();
    let h = r["handle"].as_str().unwrap().to_string();
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 2, "text": "=A2+B2" })).unwrap();
    (s, h)
}

fn texts(t: &Table) -> Vec<Vec<String>> {
    t.cells.iter().map(|r| r.iter().map(|c| c.text.clone()).collect()).collect()
}

#[test]
fn copied_cells_paste_with_their_format_and_moved_formulas() {
    let (mut s, h) = session();
    s.execute("tableformat", &json!({ "handle": h, "row": 1, "col": 0, "align": "MC" })).unwrap();
    let r = s.execute("table.copycells", &json!({ "handle": h, "row": 1, "col": 0, "cols": 3 })).unwrap();
    assert_eq!(r["text"], "1\t2\t3\n");
    s.execute("table.pastecells", &json!({ "handle": h, "row": 2, "col": 0 })).unwrap();
    let t = table(&s, &h);
    assert_eq!(texts(&t)[2], ["1", "2", "3"]);
    assert_eq!(t.cells[2][2].formula.as_deref(), Some("=A3+B3"));
    assert_eq!(t.cells[2][0].look, t.cells[1][0].look);
    // Past the last row: rows are added for the copy.
    s.execute("table.pastecells", &json!({ "handle": h, "row": 2, "col": 0, "text": "x\ty\nz\tw\n" })).unwrap();
    let t = table(&s, &h);
    assert_eq!(t.row_heights.len(), 4);
    assert_eq!(texts(&t)[3], ["z", "w", ""]);
    // One cell copied fills a larger target.
    s.execute("table.copycells", &json!({ "handle": h, "row": 0, "col": 0 })).unwrap();
    s.execute("table.pastecells", &json!({ "handle": h, "row": 0, "col": 1, "rows": 2, "cols": 2 })).unwrap();
    assert_eq!(texts(&table(&s, &h))[1], ["1", "a", "a"]);
}

#[test]
fn cut_and_clear_empty_the_cells_and_keep_their_format() {
    let (mut s, h) = session();
    s.execute("tableformat", &json!({ "handle": h, "row": 0, "col": 0, "align": "MR" })).unwrap();
    s.execute("table.clearcells", &json!({ "handle": h, "row": 0, "col": 0, "rows": 2, "cols": 2 })).unwrap();
    let t = table(&s, &h);
    assert_eq!(texts(&t)[0], ["", "", "c"]);
    assert_eq!(texts(&t)[1][..2], ["", ""]);
    assert!(t.cells[0][0].look.is_some());
    s.execute("table.copycells", &json!({ "handle": h, "row": 0, "col": 2, "cut": true })).unwrap();
    assert_eq!(texts(&table(&s, &h))[0][2], "");
    s.execute("table.pastecells", &json!({ "handle": h, "row": 2, "col": 2 })).unwrap();
    assert_eq!(texts(&table(&s, &h))[2][2], "c");
}

#[test]
fn a_merge_is_cleared_and_copied_whole() {
    let (mut s, h) = session();
    s.execute("table.merge", &json!({ "handle": h, "row": 0, "col": 0, "rows": 1, "cols": 2 })).unwrap();
    // Picking the merge's second cell takes in the whole merge.
    s.execute("table.clearcells", &json!({ "handle": h, "row": 0, "col": 1 })).unwrap();
    assert_eq!(texts(&table(&s, &h))[0][0], "");
}

#[test]
fn hostile_cell_clipboard_params_never_panic() {
    let (mut s, h) = session();
    assert!(s.execute("table.pastecells", &json!({ "handle": h, "row": 0, "col": 0 })).is_err(), "nothing copied yet");
    for p in [
        json!({ "handle": h }),
        json!({ "handle": h, "row": 99, "col": 0 }),
        json!({ "handle": h, "row": 0, "col": 0, "rows": u64::MAX, "cols": u64::MAX }),
        json!({ "handle": h, "row": 0, "col": 0, "text": "" }),
        json!({ "handle": h, "row": 0, "col": 2, "text": "\t".repeat(5000) }),
        json!({ "handle": h, "row": 0, "col": 0, "text": "\"unterminated\n".repeat(100) }),
        json!({ "handle": "zz", "row": 0, "col": 0 }),
    ] {
        for id in ["table.copycells", "table.pastecells", "table.clearcells"] {
            let _ = s.execute(id, &p);
        }
    }
    let t = table(&s, &h);
    assert!(t.cells.len() <= 2000 && t.cells.iter().all(|r| r.len() == 3));
}
