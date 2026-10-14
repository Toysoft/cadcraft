//! Table cells picked with the mouse, as in AutoCAD: a click inside a cell selects it, Shift+click
//! or a drag across cells selects a range, Esc lets go. The selected cells are highlighted, and
//! the right-click menu acts on them (alignment, borders, text rotation, a block, merging, rows
//! and columns) through the table commands. A click on the table's lines still selects the
//! table itself.

use cadcraft_doc::{EntityKind, Handle, Table};
use cadcraft_geom::{Bounds2, Vec2};
use egui::{Color32, Rect, Stroke};
use serde_json::{Value, json};

use crate::CadApp;
use crate::canvas::Xf;
use crate::menus::Entry;

/// Menu item ids of cell actions start with this ([`act`]).
pub const CELL: &str = "cell:";

/// Selected cells of one table: the cell the selection started at and the one it reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSel {
    pub table: Handle,
    pub anchor: (usize, usize),
    pub end: (usize, usize),
}

impl CellSel {
    /// The cells from `anchor` to `end`: (first row, rows, first column, columns).
    pub fn range(&self) -> (usize, usize, usize, usize) {
        let (r0, r1) = (self.anchor.0.min(self.end.0), self.anchor.0.max(self.end.0));
        let (c0, c1) = (self.anchor.1.min(self.end.1), self.anchor.1.max(self.end.1));
        (r0, r1 - r0 + 1, c0, c1 - c0 + 1)
    }

    /// [`Self::range`] grown to take in every merged cell it meets, as a selection of cells is.
    pub fn range_in(&self, t: &Table) -> (usize, usize, usize, usize) {
        merged_range(t, self.range())
    }
}

/// `range` grown until no merged cell of `t` sticks out of it.
pub fn merged_range(t: &Table, range: (usize, usize, usize, usize)) -> (usize, usize, usize, usize) {
    let (mut r0, mut r1, mut c0, mut c1) = (range.0, range.0 + range.1, range.2, range.2 + range.3);
    let merges: Vec<(usize, usize, usize, usize)> = t
        .cells
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter().enumerate().filter_map(move |(c, cell)| cell.merged.map(|(rs, cs)| (r, c, rs.max(1) as usize, cs.max(1) as usize)))
        })
        .collect();
    // Each pass can only grow the range; a pass that doesn't ends it.
    for _ in 0..=merges.len() {
        let mut grew = false;
        for &(ar, ac, rs, cs) in &merges {
            if ar < r1 && r0 < ar + rs && ac < c1 && c0 < ac + cs {
                let next = (r0.min(ar), r1.max(ar + rs), c0.min(ac), c1.max(ac + cs));
                grew |= next != (r0, r1, c0, c1);
                (r0, r1, c0, c1) = next;
            }
        }
        if !grew {
            break;
        }
    }
    let (r1, c1) = (r1.min(t.row_heights.len()), c1.min(t.col_widths.len()));
    (r0.min(r1), r1.saturating_sub(r0).max(1), c0.min(c1), c1.saturating_sub(c0).max(1))
}

/// Whether `a` and `b` overlap or share an edge (cells of one part of a table).
fn touches(a: &Bounds2, b: &Bounds2) -> bool {
    let e = 1e-9 * (1.0 + a.width().max(a.height()));
    b.min.x <= a.max.x + e && a.min.x <= b.max.x + e && b.min.y <= a.max.y + e && a.min.y <= b.max.y + e
}

/// The table at `h`, if it still is one.
fn table(app: &CadApp, h: Handle) -> Option<Table> {
    match &app.session.doc().ok()?.entity(h)?.kind {
        EntityKind::Table(t) => Some(t.clone()),
        _ => None,
    }
}

/// The table cell whose inside `p` is in, `tol` (drawing units) away from its lines: clicks on
/// the lines pick the table itself.
pub fn cell_inside(app: &CadApp, p: Vec2, tol: f64) -> Option<(Handle, usize, usize)> {
    let d = app.session.doc().ok()?;
    let space = d.space(&app.session.space())?;
    for e in space.iter() {
        let EntityKind::Table(t) = &e.kind else { continue };
        let Some((r, c)) = t.cell_at_point(p) else { continue };
        // The whole merged cell around it (its inner grid lines are not drawn), in this part.
        let Some(mut area) = t.cell_rects(r, c).into_iter().find(|b| b.contains(p)) else { continue };
        let (r0, rows, c0, cols) = merged_range(t, (r, 1, c, 1));
        let rects: Vec<Bounds2> =
            (r0..r0 + rows).flat_map(|rr| (c0..c0 + cols).map(move |cc| (rr, cc))).flat_map(|(rr, cc)| t.cell_rects(rr, cc)).collect();
        for _ in 0..rects.len().min(10_000) {
            let before = area;
            for b in &rects {
                if touches(&area, b) {
                    area = area.union(b);
                }
            }
            if area == before {
                break;
            }
        }
        let inside = p.x >= area.min.x + tol && p.x <= area.max.x - tol && p.y >= area.min.y + tol && p.y <= area.max.y - tol;
        if inside {
            return Some((e.handle, r, c));
        }
    }
    None
}

/// A click at `p` with no command running: select the cell there (Shift extends a selection in
/// the same table). `false` when there is no cell there, and the selection of cells is dropped.
pub fn click(app: &mut CadApp, p: Vec2, tol: f64, shift: bool) -> bool {
    let Some((h, r, c)) = cell_inside(app, p, tol) else {
        app.canvas.cells = None;
        return false;
    };
    app.canvas.cells = match app.canvas.cells {
        Some(sel) if shift && sel.table == h => Some(CellSel { end: (r, c), ..sel }),
        _ => Some(CellSel { table: h, anchor: (r, c), end: (r, c) }),
    };
    app.session.set_selection(Vec::new());
    true
}

/// A drag over the cells reaches the cell at `p` (in the selection's table).
pub fn drag_to(app: &mut CadApp, p: Vec2) {
    let Some(sel) = app.canvas.cells else { return };
    let Some(t) = table(app, sel.table) else { return };
    if let Some(cell) = t.cell_at_point(p) {
        app.canvas.cells = Some(CellSel { end: cell, ..sel });
    }
}

/// Drop a selection whose table is gone or no longer has those cells, or when objects are
/// selected instead.
pub fn validate(app: &mut CadApp) {
    if !app.session.selection().is_empty() {
        app.canvas.cells = None;
    }
    if let Some(sel) = app.canvas.cells {
        let ok = table(app, sel.table).is_some_and(|t| {
            let (r0, rows, c0, cols) = sel.range();
            r0 + rows <= t.row_heights.len() && c0 + cols <= t.col_widths.len()
        });
        if !ok {
            app.canvas.cells = None;
        }
    }
}

/// Highlight the selected cells.
pub fn paint(app: &CadApp, painter: &egui::Painter, xf: &Xf) {
    let Some(sel) = app.canvas.cells else { return };
    let Some(t) = table(app, sel.table) else { return };
    let tk = crate::theme::Tokens::get();
    let stroke = Stroke::new(2.0, tk.window_stroke);
    let fill = Color32::from_rgba_unmultiplied(tk.window_stroke.r(), tk.window_stroke.g(), tk.window_stroke.b(), 40);
    let (r0, rows, c0, cols) = sel.range_in(&t);
    for r in r0..r0 + rows {
        for c in c0..c0 + cols {
            for b in t.cell_rects(r, c) {
                let rect = Rect::from_two_pos(xf.to_screen(b.min), xf.to_screen(b.max));
                painter.rect_filled(rect, 0.0, fill);
            }
        }
    }
    // The outline of the whole range in each part.
    let mut outline = Bounds2::EMPTY;
    for r in r0..r0 + rows {
        for c in c0..c0 + cols {
            for b in t.cell_rects(r, c) {
                outline = outline.union(&b);
            }
        }
    }
    if !outline.is_empty() && !t.is_broken() {
        let rect = Rect::from_two_pos(xf.to_screen(outline.min), xf.to_screen(outline.max));
        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
    }
}

/// A grid line of a table: the right edge of a column or the bottom edge of a row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridLine {
    Col(usize),
    Row(usize),
}

/// A grid line being dragged to `at` (x for a column's edge, y for a row's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineDrag {
    pub table: Handle,
    pub line: GridLine,
    pub at: f64,
}

/// Where the column edges (x) and row edges (y) of an unbroken table are.
fn grid(t: &Table) -> Option<(Vec<f64>, Vec<f64>)> {
    if t.is_broken() || t.col_widths.is_empty() || t.row_heights.is_empty() {
        return None;
    }
    let o = t.insert.xy();
    let xs = t.col_widths.iter().scan(o.x, |x, w| {
        *x += w;
        Some(*x)
    });
    let ys = t.row_heights.iter().scan(o.y, |y, h| {
        *y -= h;
        Some(*y)
    });
    Some((xs.collect(), ys.collect()))
}

/// The grid line of the picked cells' table within `tol` of `p` (the nearest), that a drag
/// resizes: a column's right edge or a row's bottom edge.
pub fn line_at(app: &CadApp, p: Vec2, tol: f64) -> Option<(Handle, GridLine)> {
    let sel = app.canvas.cells?;
    let t = table(app, sel.table)?;
    let (xs, ys) = grid(&t)?;
    let o = t.insert.xy();
    let (x1, y1) = (xs.last().copied()?, ys.last().copied()?);
    let within_x = p.x >= o.x - tol && p.x <= x1 + tol;
    let within_y = p.y <= o.y + tol && p.y >= y1 - tol;
    let near =
        |v: &[f64], at: f64| v.iter().enumerate().map(|(i, x)| (i, (x - at).abs())).filter(|(_, d)| *d <= tol).min_by(|a, b| a.1.total_cmp(&b.1));
    let col = near(&xs, p.x).filter(|_| within_y);
    let row = near(&ys, p.y).filter(|_| within_x);
    let line = match (col, row) {
        (Some(c), Some(r)) if r.1 < c.1 => GridLine::Row(r.0),
        (Some(c), _) => GridLine::Col(c.0),
        (None, Some(r)) => GridLine::Row(r.0),
        (None, None) => return None,
    };
    Some((sel.table, line))
}

/// Start dragging the grid line at `p`, if there is one.
pub fn start_line_drag(app: &mut CadApp, p: Vec2, tol: f64) -> bool {
    let Some((table, line)) = line_at(app, p, tol) else { return false };
    let at = match line {
        GridLine::Col(_) => p.x,
        GridLine::Row(_) => p.y,
    };
    app.canvas.line_drag = Some(LineDrag { table, line, at });
    true
}

/// The column width or row height a dragged line gives (more than 0), with the cell it is of.
fn dragged_size(t: &Table, d: &LineDrag) -> Option<(usize, usize, f64)> {
    let (xs, ys) = grid(t)?;
    let o = t.insert.xy();
    let (r, c, size) = match d.line {
        GridLine::Col(c) => (0, c, d.at - if c == 0 { o.x } else { *xs.get(c - 1)? }),
        GridLine::Row(r) => (r, 0, if r == 0 { o.y } else { *ys.get(r - 1)? } - d.at),
    };
    // At least a hundredth of the table's text height, never past its far side.
    let min = (t.text_height * 0.01).max(1e-6);
    (size.is_finite()).then_some((r, c, size.max(min)))
}

/// Let go of a dragged grid line: TABLESIZE gives its column the new width or its row the new
/// height (the rest of the table moves along; rows still grow for their text).
pub fn end_line_drag(app: &mut CadApp) {
    let Some(d) = app.canvas.line_drag.take() else { return };
    let Some(t) = table(app, d.table) else { return };
    let Some((r, c, size)) = dragged_size(&t, &d) else { return };
    let key = if matches!(d.line, GridLine::Col(_)) { "width" } else { "height" };
    if let Err(e) = app.run("tablesize", json!({ "handle": d.table.hex(), "row": r, "col": c, key: size })) {
        app.session.echo(e);
    }
}

/// The dragged grid line, across the table, with its new size by it.
pub fn paint_line_drag(app: &CadApp, painter: &egui::Painter, xf: &Xf) {
    let Some(d) = app.canvas.line_drag else { return };
    let Some(t) = table(app, d.table) else { return };
    let Some((xs, ys)) = grid(&t) else { return };
    let o = t.insert.xy();
    let (Some(x1), Some(y1)) = (xs.last().copied(), ys.last().copied()) else { return };
    let (a, b) = match d.line {
        GridLine::Col(_) => (Vec2::new(d.at, o.y), Vec2::new(d.at, y1)),
        GridLine::Row(_) => (Vec2::new(o.x, d.at), Vec2::new(x1, d.at)),
    };
    let tk = crate::theme::Tokens::get();
    let (sa, sb) = (xf.to_screen(a), xf.to_screen(b));
    painter.extend(egui::Shape::dashed_line(&[sa, sb], Stroke::new(1.5, tk.window_stroke), 6.0, 4.0));
    if let Some((_, _, size)) = dragged_size(&t, &d) {
        let text = format!("{size:.2}").trim_end_matches('0').trim_end_matches('.').to_string();
        painter.text(sa + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, text, egui::FontId::proportional(13.0), tk.window_stroke);
    }
}

fn item(label: &str, id: String) -> Entry {
    Entry::Item { label: label.into(), id, shortcut: None, enabled: true }
}

fn sub(label: &str, children: Vec<Entry>) -> Entry {
    Entry::Sub { label: label.into(), children }
}

/// The right-click menu's groups for the selected cells (`None` without a selection).
pub fn entries(app: &CadApp) -> Option<Vec<Vec<Entry>>> {
    let sel = app.canvas.cells?;
    // One cell, or one merged cell.
    let one = table(app, sel.table).is_some_and(|t| {
        let (r0, rows, c0, cols) = sel.range_in(&t);
        (rows, cols) == (1, 1) || t.cells.get(r0).and_then(|row| row.get(c0)).and_then(|c| c.merged) == Some((rows as u32, cols as u32))
    });
    let align = [
        ("Top Left", 1),
        ("Top Center", 2),
        ("Top Right", 3),
        ("Middle Left", 4),
        ("Middle Center", 5),
        ("Middle Right", 6),
        ("Bottom Left", 7),
        ("Bottom Center", 8),
        ("Bottom Right", 9),
    ]
    .into_iter()
    .map(|(l, a)| item(l, format!("{CELL}align:{a}")))
    .collect();
    let rotation = [("Horizontal", 0), ("Vertical, Upwards", 90), ("Vertical, Downwards", 270)]
        .into_iter()
        .map(|(l, r)| item(l, format!("{CELL}rotate:{r}")))
        .collect();
    // GOST 2.304's text heights, then one typed at the command line.
    let mut heights: Vec<Entry> = ["1.8", "2.5", "3.5", "5", "7", "10", "14"].iter().map(|h| item(h, format!("{CELL}height:{h}"))).collect();
    heights.push(item("Other...", format!("{CELL}height:other")));
    // How the table's rows grow for their text (ESKD forms: in steps of 8 mm).
    let grow = vec![
        item("Freely", format!("{CELL}grow:free")),
        item("In Steps of 8", format!("{CELL}grow:8")),
        item("In Steps of...", format!("{CELL}grow:other")),
        item("Fixed Height", format!("{CELL}grow:fixed")),
    ];
    let weights = |edges: &str| -> Vec<Entry> {
        let mut v: Vec<Entry> =
            ["0.18", "0.25", "0.35", "0.5", "0.7", "1.0"].iter().map(|w| item(&format!("{w} mm"), format!("{CELL}borders:{edges}:{w}"))).collect();
        v.push(item("ByBlock", format!("{CELL}borders:{edges}:byblock")));
        v.push(item("Hidden", format!("{CELL}borders:{edges}:hidden")));
        v.push(item("As the Cell Style", format!("{CELL}borders:{edges}:clear")));
        v
    };
    let borders = [
        ("All Borders", "all"),
        ("Outside Borders", "outside"),
        ("Inside Borders", "inside"),
        ("Inside Horizontal", "insideH"),
        ("Inside Vertical", "insideV"),
        ("Top Border", "top"),
        ("Bottom Border", "bottom"),
        ("Left Border", "left"),
        ("Right Border", "right"),
    ]
    .into_iter()
    .map(|(l, e)| sub(l, weights(e)))
    .collect();
    let mut cell = vec![
        sub("Alignment", align),
        sub("Borders", borders),
        sub("Text Height", heights),
        sub("Text Rotation", rotation),
        item("Column Width and Row Height...", format!("{CELL}size")),
        sub("Rows Grow for Text", grow),
        item("Remove Formatting", format!("{CELL}clearformat")),
    ];
    if one {
        cell.insert(0, item("Edit Text", format!("{CELL}edit")));
    }
    let mut clip = vec![item("Cut", format!("{CELL}cut")), item("Copy", format!("{CELL}copy"))];
    if app.session.cell_clipboard.is_some() {
        clip.push(item("Paste", format!("{CELL}paste")));
    }
    let content = vec![item("Insert Block...", format!("{CELL}block")), item("Delete Content", format!("{CELL}delete"))];
    let merge = vec![item("Merge Cells", format!("{CELL}merge")), item("Unmerge Cells", format!("{CELL}unmerge"))];
    let rows = vec![
        item("Insert Above", format!("{CELL}row:above")),
        item("Insert Below", format!("{CELL}row:below")),
        item("Delete Rows", format!("{CELL}row:delete")),
    ];
    let cols = vec![
        item("Insert Left", format!("{CELL}col:left")),
        item("Insert Right", format!("{CELL}col:right")),
        item("Delete Columns", format!("{CELL}col:delete")),
    ];
    Some(vec![clip, cell, content, merge, vec![sub("Rows", rows), sub("Columns", cols)]])
}

/// Run cell action `action` (an item id without [`CELL`]) on the selected cells.
pub fn act(app: &mut CadApp, action: &str) {
    let Some(sel) = app.canvas.cells else { return };
    let Some(t) = table(app, sel.table) else { return };
    let (r0, rows, c0, cols) = sel.range_in(&t);
    let h = sel.table.hex();
    let range = json!({ "handle": h, "row": r0, "col": c0, "scope": "range", "rows": rows, "cols": cols });
    let with = |extra: Value| {
        let mut q = range.clone();
        if let (Some(o), Some(e)) = (q.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        q
    };
    // The first cell's centre, for the commands that pick a cell.
    let centre = t.cell_rects(r0, c0).first().map(|b| (b.min + b.max) * 0.5);
    let parts: Vec<&str> = action.split(':').collect();
    let result: Result<Value, String> = match parts.as_slice() {
        ["align", a] => app.run("tableformat", with(json!({ "align": a.parse::<u8>().unwrap_or(5) }))),
        ["rotate", r] => app.run("tableformat", with(json!({ "rotation": r.parse::<u16>().unwrap_or(0) }))),
        ["clearformat"] => {
            let _ = app.run("tableborders", with(json!({ "clear": true })));
            app.run("tableformat", with(json!({ "clear": true })))
        }
        ["borders", edges, w] => {
            let extra = match *w {
                "clear" => json!({ "edges": edges, "clear": true }),
                "hidden" => json!({ "edges": edges, "visible": false }),
                "byblock" => json!({ "edges": edges, "lineweight": "ByBlock" }),
                mm => json!({ "edges": edges, "lineweight": mm.parse::<f64>().unwrap_or(0.25) }),
            };
            app.run("tableborders", with(extra))
        }
        ["edit"] => {
            start_at(app, "tabledit", centre);
            Ok(Value::Null)
        }
        ["block"] => {
            start_at(app, "tinsert", centre);
            Ok(Value::Null)
        }
        ["size"] => {
            start_picked(app, "tablesize", &t, (r0, rows, c0, cols), &[]);
            Ok(Value::Null)
        }
        // TABLESIZE with the width kept, at its row growth: it asks for the step.
        ["grow", "other"] => {
            start_picked(
                app,
                "tablesize",
                &t,
                (r0, rows, c0, cols),
                &[cadcraft_engine::Input::Enter, cadcraft_engine::Input::Keyword("Grow".into())],
            );
            Ok(Value::Null)
        }
        ["grow", g] => app.run("tablesize", json!({ "handle": h, "rowGrow": g })),
        // TABLEFORMAT with the selection picked and the alignment kept: it asks for the height.
        ["height", "other"] => {
            start_picked(app, "tableformat", &t, (r0, rows, c0, cols), &[cadcraft_engine::Input::Enter]);
            Ok(Value::Null)
        }
        ["height", hgt] => match hgt.parse::<f64>() {
            Ok(v) => app.run("tableformat", with(json!({ "textHeight": v }))),
            Err(_) => Ok(Value::Null),
        },
        ["delete"] => app.run("table.clearcells", range.clone()),
        ["copy"] => {
            copy(app, false);
            Ok(Value::Null)
        }
        ["cut"] => {
            copy(app, true);
            Ok(Value::Null)
        }
        ["paste"] => {
            paste(app, None);
            Ok(Value::Null)
        }
        ["merge"] => app.run("table.merge", json!({ "handle": h, "row": r0, "col": c0, "rows": rows, "cols": cols })),
        ["unmerge"] => app.run("table.unmerge", json!({ "handle": h, "row": r0, "col": c0, "rows": rows, "cols": cols })),
        ["row", "above"] => app.run("table.insertrow", json!({ "handle": h, "row": r0 })),
        ["row", "below"] => app.run("table.insertrow", json!({ "handle": h, "row": r0 + rows })),
        ["row", "delete"] => {
            app.canvas.cells = None;
            let mut last = Ok(Value::Null);
            for r in (r0..r0 + rows).rev() {
                last = app.run("table.deleterow", json!({ "handle": h, "row": r }));
            }
            last
        }
        ["col", "left"] => app.run("table.insertcol", json!({ "handle": h, "col": c0 })),
        ["col", "right"] => app.run("table.insertcol", json!({ "handle": h, "col": c0 + cols })),
        ["col", "delete"] => {
            app.canvas.cells = None;
            let mut last = Ok(Value::Null);
            for c in (c0..c0 + cols).rev() {
                last = app.run("table.deletecol", json!({ "handle": h, "col": c }));
            }
            last
        }
        _ => Ok(Value::Null),
    };
    if let Err(e) = result {
        app.session.echo(e);
    }
    validate(app);
}

/// Copy the picked cells (and empty them for `cut`); their text goes on the system clipboard.
pub fn copy(app: &mut CadApp, cut: bool) {
    let Some(sel) = app.canvas.cells else { return };
    let Some(t) = table(app, sel.table) else { return };
    let (r0, rows, c0, cols) = sel.range_in(&t);
    let p = json!({ "handle": sel.table.hex(), "row": r0, "col": c0, "rows": rows, "cols": cols, "cut": cut });
    match app.run("table.copycells", p) {
        Ok(v) => app.canvas.cell_text = v.get("text").and_then(Value::as_str).map(str::to_string),
        Err(e) => app.session.echo(e),
    }
    validate(app);
}

/// Paste into the picked cells: `text` from another program (tab-separated rows), else the
/// copied cells. The pasted cells become the picked ones.
pub fn paste(app: &mut CadApp, text: Option<&str>) {
    let Some(sel) = app.canvas.cells else { return };
    let Some(t) = table(app, sel.table) else { return };
    let (r0, rows, c0, cols) = sel.range_in(&t);
    let mut p = json!({ "handle": sel.table.hex(), "row": r0, "col": c0, "rows": rows, "cols": cols });
    if let Some(text) = text {
        p["text"] = json!(text);
    }
    match app.run("table.pastecells", p) {
        Ok(v) => {
            let n = |k: &str| v.get(k).and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).unwrap_or(1).max(1);
            app.canvas.cells = Some(CellSel { table: sel.table, anchor: (r0, c0), end: (r0 + n("rows") - 1, c0 + n("cols") - 1) });
        }
        Err(e) => app.session.echo(e),
    }
    validate(app);
}

/// Start command `id` (one that picks cells, see the engine's TABLEFORMAT) with the cells of
/// `range` picked: its first cell, then the range to its last one; then `more` inputs.
fn start_picked(app: &mut CadApp, id: &str, t: &Table, range: (usize, usize, usize, usize), more: &[cadcraft_engine::Input]) {
    let (r0, rows, c0, cols) = range;
    let centre = |r: usize, c: usize| t.cell_rects(r, c).first().map(|b| (b.min + b.max) * 0.5);
    start_at(app, id, centre(r0, c0));
    let mut inputs = match centre(r0 + rows - 1, c0 + cols - 1) {
        Some(p) if rows > 1 || cols > 1 => vec![cadcraft_engine::Input::Keyword("rAnge".into()), cadcraft_engine::Input::Point(p)],
        _ => vec![cadcraft_engine::Input::Enter],
    };
    inputs.extend(more.iter().cloned());
    for i in inputs {
        if let Err(e) = app.session.input(i) {
            app.session.echo(e.to_string());
        }
    }
}

/// Start command `id` and give it the point `at` (the cell it asks for first).
fn start_at(app: &mut CadApp, id: &str, at: Option<Vec2>) {
    app.start(id);
    if let Some(p) = at
        && let Err(e) = app.session.input(cadcraft_engine::Input::Point(p))
    {
        app.session.echo(e.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use cadcraft_engine::Session;

    /// An app with a 3×3 table of 10 × 4 cells at the origin (no title or header).
    fn app() -> (CadApp, Handle) {
        let mut a = CadApp::new(Session::new(), Services::default());
        let r = a.run("table", json!({ "at": [0, 0], "rows": 3, "cols": 3, "rowHeight": 4.0, "colWidth": 10.0 })).unwrap_or_default();
        let h = r["handle"].as_str().and_then(Handle::parse_hex).unwrap_or(Handle(0));
        (a, h)
    }

    fn cells(a: &CadApp, h: Handle) -> Vec<Vec<cadcraft_doc::TableCell>> {
        table(a, h).map(|t| t.cells).unwrap_or_default()
    }

    #[test]
    fn clicks_pick_cells_and_shift_extends_the_range() {
        let (mut a, h) = app();
        // Inside cell (0, 0); on a line (x 10) there is no cell, and the selection goes.
        assert!(click(&mut a, Vec2::new(5.0, -2.0), 0.1, false));
        assert_eq!(a.canvas.cells.map(|s| s.range()), Some((0, 1, 0, 1)));
        assert!(click(&mut a, Vec2::new(25.0, -10.0), 0.1, true));
        assert_eq!(a.canvas.cells.map(|s| s.range()), Some((0, 3, 0, 3)));
        assert_eq!(a.canvas.cells.map(|s| s.table), Some(h));
        assert!(!click(&mut a, Vec2::new(10.0, -2.0), 0.1, false));
        assert!(a.canvas.cells.is_none());
        // A drag reaches the cell under the pointer.
        assert!(click(&mut a, Vec2::new(15.0, -6.0), 0.1, false));
        drag_to(&mut a, Vec2::new(25.0, -2.0));
        assert_eq!(a.canvas.cells.map(|s| s.range()), Some((0, 2, 1, 2)));
        // Selecting objects lets the cells go.
        a.session.set_selection(vec![h]);
        validate(&mut a);
        assert!(a.canvas.cells.is_none());
    }

    #[test]
    fn the_menu_formats_and_borders_the_selected_range() {
        let (mut a, h) = app();
        assert!(entries(&a).is_none(), "no menu without cells");
        click(&mut a, Vec2::new(5.0, -6.0), 0.1, false);
        click(&mut a, Vec2::new(15.0, -10.0), 0.1, true);
        assert!(entries(&a).is_some());
        act(&mut a, "align:5");
        act(&mut a, "rotate:90");
        act(&mut a, "borders:outside:0.7");
        let c = cells(&a, h);
        let l = c[1][0].look.unwrap_or_default();
        assert_eq!((l.align, l.rotation), (Some(5), Some(90)));
        assert!(c[0][0].look.is_none() && c[1][2].look.is_none(), "outside the range");
        let top = c[1][0].edges.and_then(|e| e.top).map(|b| b.lineweight);
        assert_eq!(top, Some(cadcraft_doc::Lineweight::Mm100(70)));
        assert!(c[1][0].edges.and_then(|e| e.bottom).is_none(), "an inside edge");
        act(&mut a, "clearformat");
        let c = cells(&a, h);
        assert!(c.iter().flatten().all(|x| x.look.is_none() && x.edges.is_none()));
    }

    #[test]
    fn the_menu_merges_inserts_and_deletes() {
        let (mut a, h) = app();
        a.run("table.set", json!({ "handle": h.hex(), "row": 0, "col": 0, "text": "x" })).unwrap_or_default();
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        act(&mut a, "delete");
        assert!(cells(&a, h)[0][0].text.is_empty());
        click(&mut a, Vec2::new(15.0, -2.0), 0.1, true);
        act(&mut a, "merge");
        assert_eq!(cells(&a, h)[0][0].merged, Some((1, 2)));
        act(&mut a, "unmerge");
        assert_eq!(cells(&a, h)[0][0].merged, None);
        act(&mut a, "row:below");
        act(&mut a, "col:left");
        let t = table(&a, h).unwrap_or_default();
        assert_eq!((t.row_heights.len(), t.col_widths.len()), (4, 4));
        // Deleting the rows of the selection (row 0) lets the cells go.
        act(&mut a, "row:delete");
        assert_eq!(table(&a, h).map(|t| t.row_heights.len()), Some(3));
        assert!(a.canvas.cells.is_none());
        // Unknown actions do nothing.
        act(&mut a, "nonsense:1");
    }

    #[test]
    fn merged_cells_are_picked_and_unmerged_whole() {
        let (mut a, h) = app();
        // Cells (0, 0)–(1, 1) merged: 20 × 8 at the origin.
        a.run("table.merge", json!({ "handle": h.hex(), "row": 0, "col": 0, "rows": 2, "cols": 2 })).unwrap_or_default();
        // A click next to the hidden inner line (x 10) is inside the merged cell, and selects it whole.
        assert!(click(&mut a, Vec2::new(10.05, -4.05), 0.1, false));
        let t = table(&a, h).unwrap_or_default();
        assert_eq!(a.canvas.cells.map(|s| s.range_in(&t)), Some((0, 2, 0, 2)));
        assert!(
            entries(&a).is_some_and(|g| g.iter().flatten().any(|e| matches!(e, Entry::Item { id, .. } if id == "cell:edit"))),
            "one merged cell can be edited"
        );
        // A range from (2, 1) up into the merge grows to take it in.
        click(&mut a, Vec2::new(15.0, -10.0), 0.1, false);
        click(&mut a, Vec2::new(25.0, -6.0), 0.1, true);
        let t = table(&a, h).unwrap_or_default();
        assert_eq!(a.canvas.cells.map(|s| s.range_in(&t)), Some((0, 3, 0, 3)));
        // A second merge, then Unmerge on the whole table takes both apart.
        a.run("table.merge", json!({ "handle": h.hex(), "row": 2, "col": 1, "cols": 2 })).unwrap_or_default();
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        click(&mut a, Vec2::new(25.0, -10.0), 0.1, true);
        act(&mut a, "unmerge");
        assert!(cells(&a, h).iter().flatten().all(|c| c.merged.is_none()));
    }

    #[test]
    fn the_size_item_asks_for_the_widths_and_heights_of_the_selection() {
        let (mut a, h) = app();
        click(&mut a, Vec2::new(15.0, -2.0), 0.1, false);
        click(&mut a, Vec2::new(25.0, -6.0), 0.1, true);
        act(&mut a, "size");
        // Range (0..2, 1..3) picked: the command asks for the width, then the height.
        assert!(a.session.prompting());
        let _ = a.session.input(cadcraft_engine::Input::Text("25".into()));
        let _ = a.session.input(cadcraft_engine::Input::Enter);
        assert!(!a.session.prompting());
        let t = table(&a, h).unwrap_or_default();
        assert_eq!(t.col_widths, vec![10.0, 25.0, 25.0]);
        assert_eq!(t.row_heights, vec![4.0, 4.0, 4.0]);
    }

    #[test]
    fn the_menu_sets_the_text_height_of_the_selection() {
        let (mut a, h) = app();
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        click(&mut a, Vec2::new(25.0, -2.0), 0.1, true);
        act(&mut a, "height:2.5");
        let t = table(&a, h).unwrap_or_default();
        assert!(t.cells[0].iter().all(|c| c.text_height == Some(2.5)));
        assert!(t.cells[1].iter().all(|c| c.text_height.is_none()));
        // Other...: the command asks for the height, the alignment kept.
        act(&mut a, "height:other");
        assert!(a.session.prompting());
        let _ = a.session.input(cadcraft_engine::Input::Text("7".into()));
        let _ = a.session.input(cadcraft_engine::Input::Enter);
        let _ = a.session.input(cadcraft_engine::Input::Enter);
        assert!(!a.session.prompting());
        assert!(table(&a, h).is_some_and(|t| t.cells[0].iter().all(|c| c.text_height == Some(7.0))));
        // Remove Formatting goes back to the style's height.
        act(&mut a, "clearformat");
        assert!(table(&a, h).is_some_and(|t| t.cells.iter().flatten().all(|c| c.text_height.is_none())));
    }

    #[test]
    fn the_menu_sets_how_rows_grow() {
        let (mut a, h) = app();
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        act(&mut a, "grow:8");
        assert!(table(&a, h).is_some_and(|t| t.row_grow == cadcraft_engine::doc::RowGrow::Step(8.0)));
        act(&mut a, "grow:fixed");
        assert!(table(&a, h).is_some_and(|t| t.row_grow == cadcraft_engine::doc::RowGrow::Fixed));
        // In Steps of...: the command asks for the step.
        act(&mut a, "grow:other");
        assert!(a.session.prompting());
        let _ = a.session.input(cadcraft_engine::Input::Text("5".into()));
        assert!(!a.session.prompting());
        assert!(table(&a, h).is_some_and(|t| t.row_grow == cadcraft_engine::doc::RowGrow::Step(5.0)));
    }

    #[test]
    fn picked_cells_copy_paste_and_delete() {
        let (mut a, h) = app();
        let _ = a.run("table.set", json!({ "handle": h.hex(), "row": 0, "col": 0, "text": "x" }));
        let _ = a.run("table.set", json!({ "handle": h.hex(), "row": 0, "col": 1, "text": "y" }));
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        click(&mut a, Vec2::new(15.0, -2.0), 0.1, true);
        act(&mut a, "copy");
        assert_eq!(a.canvas.cell_text.as_deref(), Some("x\ty\n"), "for the system clipboard");
        assert!(entries(&a).is_some_and(|g| g.iter().flatten().any(|e| matches!(e, Entry::Item { id, .. } if id == "cell:paste"))));
        // Pasted at one cell: the copy takes its size, and becomes the picked cells.
        click(&mut a, Vec2::new(5.0, -6.0), 0.1, false);
        act(&mut a, "paste");
        assert_eq!((cells(&a, h)[1][0].text.as_str(), cells(&a, h)[1][1].text.as_str()), ("x", "y"));
        assert_eq!(a.canvas.cells.map(|c| c.range()), Some((1, 1, 0, 2)));
        // Text from a spreadsheet.
        click(&mut a, Vec2::new(5.0, -10.0), 0.1, false);
        paste(&mut a, Some("1\t2\t3\n"));
        assert_eq!(cells(&a, h)[2].iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        // Delete empties the picked cells.
        act(&mut a, "delete");
        assert!(cells(&a, h)[2].iter().all(|c| c.text.is_empty()));
        assert_eq!(cells(&a, h)[1][0].text, "x");
    }

    #[test]
    fn dragging_a_grid_line_resizes_its_column_or_row() {
        let (mut a, h) = app();
        // No cells picked: the lines are the table's (a click there picks it).
        assert!(line_at(&a, Vec2::new(10.0, -2.0), 0.2).is_none());
        click(&mut a, Vec2::new(5.0, -2.0), 0.1, false);
        assert_eq!(line_at(&a, Vec2::new(10.1, -2.0), 0.2), Some((h, GridLine::Col(0))));
        assert_eq!(line_at(&a, Vec2::new(5.0, -3.9), 0.2), Some((h, GridLine::Row(0))));
        assert_eq!(line_at(&a, Vec2::new(29.9, -12.0), 0.2).map(|l| l.1), Some(GridLine::Row(2)), "the corner: the nearer line");
        assert!(line_at(&a, Vec2::new(50.0, -2.0), 0.2).is_none(), "outside the table");
        assert!(start_line_drag(&mut a, Vec2::new(10.0, -2.0), 0.2));
        if let Some(d) = &mut a.canvas.line_drag {
            d.at = 14.0;
        }
        end_line_drag(&mut a);
        assert!(a.canvas.line_drag.is_none());
        assert_eq!(table(&a, h).map(|t| t.col_widths), Some(vec![14.0, 10.0, 10.0]));
        assert!(start_line_drag(&mut a, Vec2::new(5.0, -8.0), 0.2));
        if let Some(d) = &mut a.canvas.line_drag {
            d.at = -15.0;
        }
        end_line_drag(&mut a);
        assert_eq!(table(&a, h).map(|t| t.row_heights), Some(vec![4.0, 11.0, 4.0]));
        // Dragged past the column's other side: the smallest width, never a negative one.
        assert!(start_line_drag(&mut a, Vec2::new(24.0, -2.0), 0.2));
        if let Some(d) = &mut a.canvas.line_drag {
            d.at = -100.0;
        }
        end_line_drag(&mut a);
        assert!(table(&a, h).is_some_and(|t| t.col_widths[1] > 0.0 && t.col_widths[1] < 0.1));
    }
}
