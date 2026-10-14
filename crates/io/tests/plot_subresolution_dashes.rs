//! Plots draw a dash pattern that repeats in less than the chord tolerance (0.05 mm on paper) as a
//! continuous line (#1068): one path instead of one per dash. Dashes that are visible on paper
//! still come out one by one.

use cadcraft_doc::Space;
use cadcraft_io::{PdfOptions, pdf::pdf, read_dxf};

/// N horizontal lines on a 0.5/-0.25 DASHED linetype, each `len` long (as in the #1068 control).
fn dashed_lines(n: usize, len: f64) -> Vec<u8> {
    let mut g = String::new();
    let mut p = |code: i32, val: &str| g.push_str(&format!("{code:>3}\n{val}\n"));
    for (c, v) in [(0, "SECTION"), (2, "HEADER"), (9, "$ACADVER"), (1, "AC1032"), (9, "$INSUNITS"), (70, "4"), (0, "ENDSEC")] {
        p(c, v);
    }
    for (c, v) in [(0, "SECTION"), (2, "TABLES"), (0, "TABLE"), (2, "LTYPE"), (70, "1")] {
        p(c, v);
    }
    for (c, v) in
        [(0, "LTYPE"), (2, "DASHED"), (70, "0"), (3, "Dashed"), (72, "65"), (73, "2"), (40, "0.75"), (49, "0.5"), (74, "0"), (49, "-0.25"), (74, "0")]
    {
        p(c, v);
    }
    for (c, v) in [(0, "ENDTAB"), (0, "ENDSEC"), (0, "SECTION"), (2, "ENTITIES")] {
        p(c, v);
    }
    for i in 0..n {
        let y = format!("{}", i as f64 * 2.0);
        let l = format!("{len}");
        for (c, v) in [(0, "LINE"), (8, "0"), (6, "DASHED"), (10, "0"), (20, y.as_str()), (30, "0"), (11, l.as_str()), (21, y.as_str()), (31, "0")] {
            p(c, v);
        }
    }
    p(0, "ENDSEC");
    p(0, "EOF");
    g.into_bytes()
}

/// Path segments drawn in an uncompressed PDF ("x y m" starts one).
fn moves(d: &cadcraft_doc::Drawing) -> usize {
    let o = PdfOptions { compress: false, ..PdfOptions::default() };
    let bytes = pdf(d, &Space::Model, &o).expect("pdf");
    String::from_utf8_lossy(&bytes).lines().filter(|l| l.ends_with(" m")).count()
}

#[test]
fn dashes_below_paper_resolution_plot_as_one_line() {
    // 10 lines with 40,000 periods each, fitted to the sheet: a period is about 0.007 mm on paper.
    let d = read_dxf(&dashed_lines(10, 30_000.0)).expect("dxf");
    let n = moves(&d);
    assert!(n <= 20, "{n} path segments for 10 sub-resolution dashed lines");
}

#[test]
fn visible_dashes_still_plot_one_by_one() {
    // 10 lines of 20 periods: a period is several millimetres on paper.
    let d = read_dxf(&dashed_lines(10, 15.0)).expect("dxf");
    let n = moves(&d);
    assert!(n >= 10 * 20, "{n} path segments; expected every dash");
}
