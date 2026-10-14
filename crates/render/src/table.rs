//! TABLE geometry from the table's cell styles: cell fills, borders and texts. Shared by the
//! display list and the DXF writer (which stores the same geometry as the table's block).

use std::collections::HashMap;

use cadcraft_color::Color;
use cadcraft_doc::{CellBlock, CellBorder, CellStyle, CellVisibility, Drawing, Lineweight, PlacedCellBlock, Table, TableStyle, heavier};
use cadcraft_geom::Vec2;

use crate::{Builder, Ctx, Ink, ink};

/// The most cells laid out (rows × columns); larger tables draw only their outline.
pub const MAX_LAYOUT_CELLS: usize = 1_000_000;

/// A border segment.
#[derive(Clone, Debug, PartialEq)]
pub struct TableLine {
    pub a: Vec2,
    pub b: Vec2,
    pub color: Color,
    pub lineweight: Lineweight,
}

/// A cell background.
#[derive(Clone, Debug, PartialEq)]
pub struct TableFill {
    /// Corners, counter-clockwise from the bottom left.
    pub corners: [Vec2; 4],
    pub color: Color,
}

/// A cell's text, placed at its alignment point.
#[derive(Clone, Debug, PartialEq)]
pub struct TableText {
    pub row: usize,
    pub col: usize,
    pub at: Vec2,
    /// MTEXT attachment (1 top left … 9 bottom right).
    pub attach: u8,
    /// Rotation in radians (vertical header text).
    pub rotation: f64,
    pub height: f64,
    pub color: Color,
    pub style: String,
    pub text: String,
    /// The width the text wraps at (the cell's inside its margins); 0: no wrapping.
    pub width: f64,
    /// The cell's margin across the text (above and below it).
    pub pad: f64,
}

/// A cell's block, placed in the cell.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBlock {
    pub row: usize,
    pub col: usize,
    pub block: PlacedCellBlock,
}

/// Everything a table draws, in world coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableGeom {
    pub fills: Vec<TableFill>,
    pub lines: Vec<TableLine>,
    pub texts: Vec<TableText>,
    pub blocks: Vec<TableBlock>,
}

/// Which cells are covered by a merge (and so not drawn on their own).
pub fn table_covered(t: &Table) -> Vec<Vec<bool>> {
    let rows = t.row_heights.len().min(10_000);
    let cols = t.col_widths.len().min(10_000);
    if rows.saturating_mul(cols) > MAX_LAYOUT_CELLS {
        return vec![Vec::new(); rows];
    }
    let mut cov = vec![vec![false; cols]; rows];
    for (r, row) in t.cells.iter().enumerate().take(rows) {
        for (c, cell) in row.iter().enumerate().take(cols) {
            if let Some((rs, cs)) = cell.merged {
                for rr in r..(r + rs.max(1) as usize).min(rows) {
                    for cc in c..(c + cs.max(1) as usize).min(cols) {
                        if (rr, cc) != (r, c)
                            && let Some(x) = cov.get_mut(rr).and_then(|v| v.get_mut(cc))
                        {
                            *x = true;
                        }
                    }
                }
            }
        }
    }
    cov
}

/// The table style a table uses (Standard defaults when missing).
pub fn table_style_of(d: &Drawing, t: &Table) -> TableStyle {
    d.table_styles.iter().find(|s| s.name.eq_ignore_ascii_case(&t.style)).cloned().unwrap_or_default()
}

/// The scale of a table relative to its style: SCALE changes the table's text height, and every
/// cell style height and margin follows.
fn style_scale(st: &TableStyle, t: &Table) -> f64 {
    // A table from another writer is resolved to its style's height on reading (scale 1); a
    // non-positive height here is the same "no scale of its own".
    let k = t.text_height / st.text_height;
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

/// Column and row edge offsets (x right, y down from the insertion point).
fn edges(sizes: &[f64], n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n + 1);
    out.push(0.0);
    let mut acc = 0.0;
    for s in sizes.iter().take(n) {
        acc += if s.is_finite() { *s } else { 0.0 };
        out.push(acc);
    }
    out
}

/// A merged cell (or a plain one): its anchor, span and cell style.
struct Anchor {
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
    style: usize,
}

/// Lay out table `t` of drawing `d`, every part of a broken table.
pub fn table_geometry(d: &Drawing, t: &Table) -> TableGeom {
    let parts = t.parts();
    if parts.len() <= 1 {
        return layout(d, t);
    }
    let mut g = TableGeom::default();
    for p in parts {
        let mut pg = layout(d, &part_table(t, &p.rows, p.origin));
        for tx in &mut pg.texts {
            tx.row = p.rows.get(tx.row).copied().unwrap_or(tx.row);
        }
        for bl in &mut pg.blocks {
            bl.row = p.rows.get(bl.row).copied().unwrap_or(bl.row);
        }
        g.fills.extend(pg.fills);
        g.lines.extend(pg.lines);
        g.texts.extend(pg.texts);
        g.blocks.extend(pg.blocks);
    }
    g
}

/// One part of a broken table as a table of its own: its rows (with their cell styles) at
/// `origin`; merges are cut at the part's edge.
fn part_table(t: &Table, rows: &[usize], origin: Vec2) -> Table {
    let cols = t.col_widths.len();
    let mut cells: Vec<Vec<cadcraft_doc::TableCell>> = rows.iter().map(|r| t.cells.get(*r).cloned().unwrap_or_default()).collect();
    for (i, row) in cells.iter_mut().enumerate() {
        row.resize_with(cols, Default::default);
        for cell in row.iter_mut() {
            if let Some((rs, cs)) = cell.merged {
                // Rows of the merge that follow in this part.
                let start = rows.get(i).copied().unwrap_or(0);
                let mut n = 1u32;
                while n < rs && rows.get(i + n as usize).is_some_and(|r| *r == start + n as usize) {
                    n += 1;
                }
                cell.merged = (n > 1 || cs > 1).then_some((n, cs));
            }
        }
    }
    Table {
        insert: origin.to3(t.insert.z),
        col_widths: t.col_widths.clone(),
        row_heights: rows.iter().map(|r| t.row_heights.get(*r).copied().unwrap_or(0.0)).collect(),
        row_styles: rows.iter().map(|r| t.row_style(*r).to_string()).collect(),
        cells,
        style: t.style.clone(),
        text_height: t.text_height,
        title: false,
        header: false,
        breaks: None,
        data_link: None,
        row_grow: t.row_grow,
        row_min: Vec::new(),
    }
}

/// Lay out one part.
fn layout(d: &Drawing, t: &Table) -> TableGeom {
    let st = table_style_of(d, t);
    let rows = t.row_heights.len().min(10_000);
    let cols = t.col_widths.len().min(10_000);
    let xs = edges(&t.col_widths, cols);
    let ys = edges(&t.row_heights, rows);
    let o = t.insert.xy();
    let p = |x: f64, y: f64| o + Vec2::new(x, -y);
    let x_at = |c: usize| xs.get(c).copied().unwrap_or(0.0);
    let y_at = |r: usize| ys.get(r).copied().unwrap_or(0.0);
    let mut g = TableGeom::default();
    if rows == 0 || cols == 0 {
        return g;
    }
    if rows.saturating_mul(cols) > MAX_LAYOUT_CELLS {
        let (w, h) = (x_at(cols), y_at(rows));
        let corners = [p(0.0, 0.0), p(w, 0.0), p(w, h), p(0.0, h), p(0.0, 0.0)];
        for s in corners.windows(2) {
            if let [a, b] = s {
                g.lines.push(TableLine { a: *a, b: *b, color: Color::ByBlock, lineweight: Lineweight::ByBlock });
            }
        }
        return g;
    }
    let k = style_scale(&st, t);
    // Cell styles by name, scaled to the table.
    let mut styles: Vec<CellStyle> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut style_index = |name: &str| -> usize {
        let key = name.to_ascii_lowercase();
        if let Some(i) = by_name.get(&key) {
            return *i;
        }
        let mut cs = st.cell_style(name);
        cs.text_height *= k;
        cs.margin_h *= k;
        cs.margin_v *= k;
        styles.push(cs);
        let i = styles.len() - 1;
        by_name.insert(key, i);
        i
    };
    // Anchors and the anchor owning each grid cell.
    let cov = table_covered(t);
    let mut anchors: Vec<Anchor> = Vec::new();
    let mut owner = vec![usize::MAX; rows * cols];
    for r in 0..rows {
        for c in 0..cols {
            if cov.get(r).and_then(|v| v.get(c)).copied().unwrap_or(false) {
                continue;
            }
            let cell = t.cells.get(r).and_then(|row| row.get(c));
            let (rs, cs) = cell.and_then(|x| x.merged).map(|(a, b)| (a.max(1) as usize, b.max(1) as usize)).unwrap_or((1, 1));
            let (rs, cs) = (rs.min(rows - r), cs.min(cols - c));
            let style = style_index(t.cell_style_name(r, c));
            let id = anchors.len();
            for rr in r..r + rs {
                for cc in c..c + cs {
                    if let Some(x) = owner.get_mut(rr * cols + cc)
                        && *x == usize::MAX
                    {
                        *x = id;
                    }
                }
            }
            anchors.push(Anchor { row: r, col: c, rows: rs, cols: cs, style });
        }
    }
    let own = |r: usize, c: usize| owner.get(r * cols + c).copied().filter(|x| *x != usize::MAX);
    let style_of = |a: Option<usize>| a.and_then(|a| anchors.get(a)).and_then(|a| styles.get(a.style));
    let same_style = |a: Option<usize>, b: Option<usize>| matches!((a.and_then(|a| anchors.get(a)), b.and_then(|b| anchors.get(b))), (Some(x), Some(y)) if x.style == y.style);
    // A cell's own borders: of the anchor cell of a merge.
    let edges = |a: Option<usize>| a.and_then(|a| anchors.get(a)).and_then(|a| t.cells.get(a.row).and_then(|r| r.get(a.col))).and_then(|c| c.edges);
    // Two cells' own borders on the edge they share: the heavier wins.
    let either = |a: Option<CellBorder>, b: Option<CellBorder>| match (a, b) {
        (Some(a), Some(b)) => Some(heavier(a, b)),
        (a, b) => a.or(b),
    };
    // Fills first, under the borders and texts.
    for a in &anchors {
        let Some(cs) = styles.get(a.style) else { continue };
        if let Some(fill) = cs.fill {
            let (x0, x1, y0, y1) = (x_at(a.col), x_at(a.col + a.cols), y_at(a.row), y_at(a.row + a.rows));
            g.fills.push(TableFill { corners: [p(x0, y1), p(x1, y1), p(x1, y0), p(x0, y0)], color: fill });
        }
    }
    // Borders: one segment per grid edge between different cells, joined while unchanged.
    let push = |g: &mut TableGeom, a: Vec2, b: Vec2, br: Option<CellBorder>| {
        let Some(br) = br.filter(|b| b.visible) else { return };
        if let Some(last) = g.lines.last_mut()
            && last.b == a
            && last.color == br.color
            && last.lineweight == br.lineweight
            && (last.b - last.a).cross(b - a).abs() <= 1e-12 * (1.0 + (last.b - last.a).len() * (b - a).len())
        {
            last.b = b;
            return;
        }
        g.lines.push(TableLine { a, b, color: br.color, lineweight: br.lineweight });
    };
    for i in 0..=rows {
        for c in 0..cols {
            let above = if i > 0 { own(i - 1, c) } else { None };
            let below = if i < rows { own(i, c) } else { None };
            if above.is_some() && above == below {
                continue;
            }
            let br = match (style_of(above), style_of(below)) {
                (None, Some(b)) => Some(b.borders.top),
                (Some(a), None) => Some(a.borders.bottom),
                (Some(a), Some(_)) if same_style(above, below) => Some(a.borders.inside_h),
                (Some(a), Some(b)) => Some(CellBorder { visible: a.borders.bottom.visible && b.borders.top.visible, ..b.borders.top }),
                (None, None) => None,
            };
            let own_border = either(edges(above).and_then(|e| e.bottom), edges(below).and_then(|e| e.top));
            let br = own_border.or(br);
            push(&mut g, p(x_at(c), y_at(i)), p(x_at(c + 1), y_at(i)), br);
        }
    }
    for j in 0..=cols {
        for r in 0..rows {
            let left = if j > 0 { own(r, j - 1) } else { None };
            let right = if j < cols { own(r, j) } else { None };
            if left.is_some() && left == right {
                continue;
            }
            let br = match (style_of(left), style_of(right)) {
                (None, Some(b)) => Some(b.borders.left),
                (Some(a), None) => Some(a.borders.right),
                (Some(a), Some(_)) if same_style(left, right) => Some(a.borders.inside_v),
                (Some(a), Some(b)) => Some(CellBorder { visible: a.borders.right.visible && b.borders.left.visible, ..b.borders.left }),
                (None, None) => None,
            };
            let own_border = either(edges(left).and_then(|e| e.right), edges(right).and_then(|e| e.left));
            let br = own_border.or(br);
            push(&mut g, p(x_at(j), y_at(r)), p(x_at(j), y_at(r + 1)), br);
        }
    }
    // Blocks, fitted in their cells (one visibility look-up per block and state).
    let mut shown: HashMap<(String, Vec<(String, String)>), CellVisibility> = HashMap::new();
    let mut visibility = |cb: &CellBlock| -> CellVisibility {
        let key = (cb.name.to_ascii_lowercase(), cb.props.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect());
        shown.entry(key).or_insert_with(|| cadcraft_doc::cell_visibility(d, cb)).clone()
    };
    // A cell's own alignment and margins over its cell style's (margins scale with the table).
    let look = |a: &Anchor, cs: &CellStyle| -> (u8, f64, f64, u16) {
        let l = t.cells.get(a.row).and_then(|row| row.get(a.col)).and_then(|c| c.look).map(|l| l.sanitized()).unwrap_or_default();
        (
            l.align.unwrap_or(cs.align).clamp(1, 9),
            l.margin_h.map_or(cs.margin_h, |m| m * k),
            l.margin_v.map_or(cs.margin_v, |m| m * k),
            l.rotation.unwrap_or(0),
        )
    };
    for a in &anchors {
        let Some(cb) = t.cells.get(a.row).and_then(|row| row.get(a.col)).and_then(|c| c.block.as_ref()) else { continue };
        let Some(cs) = styles.get(a.style) else { continue };
        let (align, margin_h, margin_v, _) = look(a, cs);
        let (x0, x1, y0, y1) = (x_at(a.col), x_at(a.col + a.cols), y_at(a.row), y_at(a.row + a.rows));
        let (min, max) = (p(x0, y1), p(x1, y0));
        if let Some(block) = cadcraft_doc::place_cell_block(d, cb, visibility(cb), min, max, Vec2::new(margin_h, margin_v), align) {
            g.blocks.push(TableBlock { row: a.row, col: a.col, block });
        }
    }
    // Texts at their alignment point inside the margins (a cell with a block shows the block).
    for a in &anchors {
        let Some(cell) = t.cells.get(a.row).and_then(|row| row.get(a.col)).filter(|c| !c.text.is_empty() && c.block.is_none()) else { continue };
        let Some(cs) = styles.get(a.style) else { continue };
        let (x0, x1, y0, y1) = (x_at(a.col), x_at(a.col + a.cols), y_at(a.row), y_at(a.row + a.rows));
        let (attach, mh, mv, rotation) = look(a, cs);
        let (min, max) = (p(x0, y1), p(x1, y0));
        let at = text_anchor(min, max, attach, mh, mv, rotation);
        // Along the text: the cell's width, or its height for vertical text.
        let along = if rotation % 180 == 0 { x1 - x0 } else { y1 - y0 };
        g.texts.push(TableText {
            row: a.row,
            col: a.col,
            at,
            attach,
            rotation: f64::from(rotation).to_radians(),
            height: cell.text_height.filter(|h| h.is_finite() && *h > 0.0).map_or(cs.text_height, |h| h * k),
            color: cs.text_color,
            style: cs.text_style.clone(),
            text: cell.text.clone(),
            width: (along - 2.0 * mh).max(0.0),
            pad: mv,
        });
    }
    g
}

/// The alignment point of a cell's text in the cell `min` … `max` (world): `attach` (MTEXT
/// numbering) read in the text's own frame, turned by `rotation` degrees, `mh` from the cell's
/// sides along the text and `mv` across it.
fn text_anchor(min: Vec2, max: Vec2, attach: u8, mh: f64, mv: f64, rotation: u16) -> Vec2 {
    let (h, v) = (usize::from((attach.clamp(1, 9) - 1) % 3), usize::from((attach.clamp(1, 9) - 1) / 3));
    let c = (min + max) * 0.5;
    let pick = |[first, mid, last]: [f64; 3], i: usize| match i {
        0 => first,
        2 => last,
        _ => mid,
    };
    match rotation {
        // Reading upwards: the text's left is the cell's bottom, its top the cell's left.
        90 => Vec2::new(pick([min.x + mv, c.x, max.x - mv], v), pick([min.y + mh, c.y, max.y - mh], h)),
        180 => Vec2::new(pick([max.x - mh, c.x, min.x + mh], h), pick([min.y + mv, c.y, max.y - mv], v)),
        // Reading downwards: the text's left is the cell's top, its top the cell's right.
        270 => Vec2::new(pick([max.x - mv, c.x, min.x + mv], v), pick([max.y - mh, c.y, min.y + mh], h)),
        _ => Vec2::new(pick([min.x + mh, c.x, max.x - mh], h), pick([max.y - mv, c.y, min.y + mv], v)),
    }
}

/// A cell style colour as drawn: ByBlock / ByLayer take the table's colour.
fn style_ink(c: Color, table: Ink) -> Ink {
    match c {
        Color::ByBlock | Color::ByLayer => table,
        c => ink(c, Color::Index(7), Color::Index(7)),
    }
}

/// The heights the rows of table `t` need for their text, for [`Table::fit_rows`]: each
/// row's own cells, and (first row, rows, height) of cells merged over several rows. Text
/// running across the row counts; vertical text wraps along the row instead.
pub fn table_row_needs(d: &Drawing, t: &Table) -> (Vec<f64>, Vec<(usize, usize, f64)>) {
    let mut needs = vec![0.0; t.row_heights.len().min(10_000)];
    let mut spans = Vec::new();
    for tx in &layout(d, t).texts {
        if tx.rotation.sin().abs() > 0.5 {
            continue;
        }
        let need = text_layout(d, tx).bounds.height() + 2.0 * tx.pad;
        if !need.is_finite() {
            continue;
        }
        let rows = t.cells.get(tx.row).and_then(|r| r.get(tx.col)).and_then(|c| c.merged).map_or(1, |(rs, _)| rs.max(1) as usize);
        if rows > 1 {
            spans.push((tx.row, rows, need));
        } else if let Some(n) = needs.get_mut(tx.row) {
            *n = f64::max(*n, need);
        }
    }
    (needs, spans)
}

/// A cell's text laid out as MTEXT (`\P`, `{\Q15;…}`, `\F…;`) at the cell's alignment
/// point (its numbering is MTEXT's attachment), in its text style's font.
fn text_layout(d: &Drawing, tx: &TableText) -> cadcraft_fonts::MTextLayout {
    let ts = crate::text_style(d, &tx.style);
    let mp = cadcraft_fonts::MTextParams {
        attach: tx.attach,
        width: tx.width,
        rotation: tx.rotation,
        font: cadcraft_fonts::TextFont::resolve(&ts.font),
        width_factor: ts.width_factor,
        oblique: ts.oblique,
        ..cadcraft_fonts::MTextParams::new(tx.at, tx.height)
    };
    cadcraft_fonts::layout_mtext_with(&tx.text, &mp)
}

/// Draw a table into the display list.
pub(crate) fn draw(b: &mut Builder, ctx: &Ctx, e: &cadcraft_doc::Entity, t: &Table, rgb: Ink, lw: f32) {
    let g = table_geometry(ctx.d, t);
    if b.opts.fill {
        for f in &g.fills {
            let [a, bb, c, dd] = f.corners;
            b.tris(ctx, style_ink(f.color, rgb), &[a, bb, c, a, c, dd]);
        }
    }
    for l in &g.lines {
        let w = match l.lineweight {
            Lineweight::Mm100(v) => f32::from(v) / 100.0,
            Lineweight::Default => crate::default_lineweight(ctx.d),
            Lineweight::ByBlock | Lineweight::ByLayer => lw,
        };
        b.polyline(ctx, style_ink(l.color, rgb), w, &[l.a, l.b]);
    }
    for bl in &g.blocks {
        crate::insert_shown(b, ctx, e, &bl.block.insert, rgb, Some(&bl.block.visibility));
    }
    if !b.opts.text {
        return;
    }
    for tx in &g.texts {
        let layout = text_layout(ctx.d, tx);
        for piece in &layout.pieces {
            b.shaped(ctx, style_ink(tx.color, rgb), lw, &piece.shaped);
        }
    }
}
