//! Copy, paste and clear table cells: what Ctrl+C, Ctrl+V and Delete do on picked cells.
//! Copied cells keep their text, formulas (moved like a spreadsheet's), blocks and formats; text
//! from another program (a spreadsheet's tab-separated rows) pastes as cell texts.

use cadcraft_doc::{Table, TableCell};
use serde_json::{Value, json};

use super::{MAX_COLS, MAX_ROWS, anchors, cell_at, edit, find, formula, normalize, usize_param};
use crate::Result;
use crate::Session;
use crate::cmd::{CommandSpec, bad, bool_or, str_param};

/// The most characters of pasted text read (hostile input).
const MAX_PASTE_TEXT: usize = 4 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("table.copycells", "Copy Table Cells", run_copy)
            .params("{handle, row, col, rows?, cols? (default 1), cut?: bool (then clear them)} → {text (tab-separated rows)}"),
        CommandSpec::new("table.pastecells", "Paste Table Cells", run_paste).params(
            "{handle, row, col, rows?, cols? (the target: the copy repeats over a larger one), text? (tab-separated rows; default: \
             the copied cells)} → {rows, cols}",
        ),
        CommandSpec::new("table.clearcells", "Clear Table Cells", run_clear)
            .params("{handle, row, col, rows?, cols?} (texts, formulas and blocks; formats stay)"),
    ]
}

/// The range a command names: (first row, rows, first column, columns), inside the table and
/// taking in whole merges.
fn range(p: &Value, t: &Table) -> std::result::Result<(usize, usize, usize, usize), String> {
    let (r, c) = cell_at(p, t)?;
    let rows = usize_param(p, "rows").unwrap_or(1).clamp(1, t.cells.len() - r);
    let cols = usize_param(p, "cols").unwrap_or(1).clamp(1, t.col_widths.len() - c);
    let (mut r0, mut r1, mut c0, mut c1) = (r, r + rows, c, c + cols);
    // Grow to whole merges until no merge sticks out.
    loop {
        let mut grown = false;
        for (ar, ac, rs, cs) in anchors(t) {
            let meets = ar < r1 && ar + rs > r0 && ac < c1 && ac + cs > c0;
            if meets && (ar < r0 || ar + rs > r1 || ac < c0 || ac + cs > c1) {
                (r0, r1, c0, c1) = (r0.min(ar), r1.max(ar + rs).min(t.cells.len()), c0.min(ac), c1.max(ac + cs).min(t.col_widths.len()));
                grown = true;
            }
        }
        if !grown {
            return Ok((r0, r1 - r0, c0, c1 - c0));
        }
    }
}

/// Empty the cells of a range: their texts, formulas and blocks.
fn clear(t: &mut Table, (r0, rows, c0, cols): (usize, usize, usize, usize)) {
    for row in t.cells.iter_mut().skip(r0).take(rows) {
        for cell in row.iter_mut().skip(c0).take(cols) {
            cell.text.clear();
            cell.formula = None;
            cell.block = None;
        }
    }
}

/// A cell's text as a spreadsheet takes it: plain, quoted when it holds tabs, line breaks or
/// quotes.
fn tsv_field(cell: &TableCell) -> String {
    let t = cadcraft_fonts::plain_mtext(&cell.text);
    if t.contains(['\t', '\n', '\r', '"']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
}

/// Tab-separated rows (quoted fields may hold tabs and line breaks) as cell texts; a trailing
/// line break ends the last row. At most [`MAX_ROWS`] × [`MAX_COLS`].
fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().take(MAX_PASTE_TEXT).peekable();
    let mut quoted = false;
    let mut at_start = true;
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\r' => {}
                '\n' => field.push_str("\\P"),
                c => field.push(c),
            }
            continue;
        }
        match ch {
            '"' if at_start => quoted = true,
            '\t' => {
                row.push(std::mem::take(&mut field));
                at_start = true;
                continue;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                if rows.len() >= MAX_ROWS {
                    break;
                }
                at_start = true;
                continue;
            }
            c => field.push(c),
        }
        at_start = false;
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    for r in &mut rows {
        r.truncate(MAX_COLS);
    }
    rows
}

fn run_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "table.copycells";
    let (h, mut t) = find(s, p, id)?;
    normalize(&mut t);
    let (r0, rows, c0, cols) = range(p, &t).map_err(|m| bad(id, m))?;
    let cells: Vec<Vec<TableCell>> = t
        .cells
        .iter()
        .skip(r0)
        .take(rows)
        .map(|row| row.iter().skip(c0).take(cols).map(|c| TableCell { merged: None, ..c.clone() }).collect())
        .collect();
    let text: String = cells.iter().map(|row| row.iter().map(tsv_field).collect::<Vec<_>>().join("\t") + "\n").collect();
    s.cell_clipboard = Some((r0, c0, cells));
    // Cut: the cells are emptied (an undo step); a copy leaves the drawing as it is.
    if bool_or(p, "cut", false) {
        edit(s, &json!({ "handle": h.hex() }), id, |t| {
            clear(t, (r0, rows, c0, cols));
            Ok(())
        })?;
    }
    Ok(json!({ "handle": h.hex(), "text": text }))
}

fn run_paste(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "table.pastecells";
    // The source: text from another program, or the copied cells.
    let (from, src): ((usize, usize), Vec<Vec<TableCell>>) = match str_param(p, "text") {
        Some(text) => {
            let cells = parse_tsv(text)
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|t| {
                            if t.trim().starts_with('=') && formula::check(t.trim()).is_ok() {
                                TableCell { formula: Some(t.trim().to_string()), ..TableCell::default() }
                            } else {
                                TableCell { text: t, ..TableCell::default() }
                            }
                        })
                        .collect()
                })
                .collect();
            ((0, 0), cells)
        }
        None => match &s.cell_clipboard {
            Some((r, c, cells)) => ((*r, *c), cells.clone()),
            None => return Err(bad(id, "no cells copied")),
        },
    };
    let (src_rows, src_cols) = (src.len(), src.iter().map(Vec::len).max().unwrap_or(0));
    if src_rows == 0 || src_cols == 0 {
        return Err(bad(id, "nothing to paste"));
    }
    let mut size = (0, 0);
    let r = edit(s, p, id, |t| {
        let (r0, c0) = cell_at(p, t)?;
        // A larger target repeats the copy over it; a smaller one takes the whole copy, rows
        // added below for it (columns past the table's last are left out).
        let rows = usize_param(p, "rows").unwrap_or(1).max(src_rows).min(MAX_ROWS);
        let cols = usize_param(p, "cols").unwrap_or(1).max(src_cols).min(t.col_widths.len() - c0);
        let need = r0.saturating_add(rows).min(MAX_ROWS);
        while t.cells.len() < need {
            let h = t.row_heights.last().copied().unwrap_or(t.text_height * 5.0 / 3.0);
            if !t.row_min.is_empty() {
                t.row_min.push(h);
            }
            t.row_heights.push(h);
            t.cells.push(vec![TableCell::default(); t.col_widths.len()]);
        }
        let rows = rows.min(t.cells.len() - r0);
        // Cells a merge covers are left alone (the merge's first cell shows its content).
        let covered = cadcraft_render::table_covered(t);
        for i in 0..rows {
            for j in 0..cols {
                let (r, c) = (r0 + i, c0 + j);
                if covered.get(r).and_then(|v| v.get(c)).copied().unwrap_or(false) {
                    continue;
                }
                let Some(from_cell) = src.get(i % src_rows).and_then(|row| row.get(j % src_cols)) else { continue };
                let Some(cell) = t.cells.get_mut(r).and_then(|row| row.get_mut(c)) else { continue };
                let merged = cell.merged;
                *cell = TableCell { merged, ..from_cell.clone() };
                if let Some(f) = &mut cell.formula {
                    let dr = r as isize - (from.0 + i % src_rows) as isize;
                    let dc = c as isize - (from.1 + j % src_cols) as isize;
                    *f = formula::offset_refs(f, dr, dc);
                }
            }
        }
        size = (rows, cols);
        Ok(())
    })?;
    Ok(json!({ "handle": r["handle"], "rows": size.0, "cols": size.1 }))
}

fn run_clear(s: &mut Session, p: &Value) -> Result<Value> {
    edit(s, p, "table.clearcells", |t| {
        let r = range(p, t)?;
        clear(t, r);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreadsheet_rows_parse_with_quotes_tabs_and_line_breaks() {
        assert_eq!(parse_tsv("a\tb\nc\td\n"), vec![vec!["a", "b"], vec!["c", "d"]]);
        assert_eq!(parse_tsv("\"x\ty\"\t\"two\nlines\"\t\"say \"\"hi\"\"\""), vec![vec!["x\ty", "two\\Plines", "say \"hi\""]]);
        assert_eq!(parse_tsv("\r\n"), vec![vec![""]]);
        assert!(parse_tsv("").is_empty());
        assert!(parse_tsv(&"\n".repeat(10 * MAX_ROWS)).len() <= MAX_ROWS);
        assert!(parse_tsv(&"\t".repeat(10 * MAX_COLS)).iter().all(|r| r.len() <= MAX_COLS));
    }
}
