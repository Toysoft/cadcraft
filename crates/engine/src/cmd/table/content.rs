//! What table cells hold beyond text: TINSERT puts a block in a cell (with attribute values and
//! a dynamic block's visibility state); TABLEBORDERS gives cells their own borders and
//! TABLEFORMAT their own alignment, margins and text rotation. At the command line both pick a
//! cell and what it stands for (the cell, its column, its row, the table, or a range).

use std::collections::BTreeMap;

use cadcraft_doc::dynblock::{ParameterKind, dynamic_block};
use cadcraft_doc::{CellBlock, CellBorder, CellLook, Drawing, EntityKind, Handle, MAX_CELL_BLOCK_VALUES, Table};
use cadcraft_geom::Vec2;
use serde_json::{Value, json};

use super::cells::{align_value, apply_border, cell_under};
use super::{anchors, cell_at, edit, row_grow_param, usize_param};
use crate::cmd::{CommandSpec, bad, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

const TINSERT_PARAMS: &str = "{handle?, row+col | at: [x,y], block: name (\"\" removes the block), scale?: number | \"fit\" (default fit), \
     rotation?: degrees, attribs?: {TAG: value}, props?: {Visibility1: state}} → {handle}";

const BORDERS_PARAMS: &str = "{handle?, row+col | at: [x,y], scope?: cell|column|row|table|range (default range: rows, cols), rows? (default 1), cols? (default 1), \
     edges?: all|outside|inside|top|bottom|left|right|insideH|insideV or a list (default all), \
     visible?: bool, color?, lineweight? (mm or ByBlock/ByLayer), clear?: bool (back to the cell style)} → {handle}";

const FORMAT_PARAMS: &str = "{handle?, row+col | at: [x,y], scope?: cell|column|row|table|range (default cell; column, row and table: \
     their data rows, below the title and header), rows?, cols? (range), align?: TL|TC|TR|ML|MC|MR|BL|BC|BR, textHeight?, margin?, marginH?, marginV?, \
     rotation?: 0|90|180|270, clear?: bool (back to the cell style)} → {handle, cells}";

const SIZE_PARAMS: &str = "{handle?, row+col | at: [x,y], scope?: cell|column|row|table|range (default cell), rows?, cols? (range), \
     width? (the range's columns), height? (its rows), widths?: [w, …] (columns from `col`, default 0), heights?: [h, …] (rows from `row`, default 0), \
     rowGrow?: free|fixed|step (how rows grow for their text: freely, never, or to multiples of the step)} → {handle}";

/// Largest column width or row height (drawing units).
const MAX_SIZE: f64 = 1e6;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tinsert", "Insert Block in Table Cell", run_tinsert)
            .params(TINSERT_PARAMS)
            .interactive(|_| Ok(Box::new(TinsertM::default()))),
        CommandSpec::new("tableborders", "Set Table Cell Borders", run_borders)
            .alias(&["tbo"])
            .params(BORDERS_PARAMS)
            .interactive(|_| Ok(Box::new(BordersM::default()))),
        CommandSpec::new("tablesize", "Table Column Width and Row Height", run_size)
            .alias(&["tsi"])
            .params(SIZE_PARAMS)
            .interactive(|_| Ok(Box::new(SizeM::default()))),
        CommandSpec::new("tableformat", "Format Table Cells", run_format)
            .alias(&["tfo"])
            .params(FORMAT_PARAMS)
            .interactive(|_| Ok(Box::new(FormatM::default()))),
    ]
}

// ---------------------------------------------------------------- TINSERT

/// A JSON object of strings (numbers and booleans as typed), at most [`MAX_CELL_BLOCK_VALUES`].
fn string_map(p: &Value, key: &str) -> std::result::Result<BTreeMap<String, String>, String> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(BTreeMap::new()) };
    let o = v.as_object().ok_or_else(|| format!("`{key}` must be an object of names and values"))?;
    if o.len() > MAX_CELL_BLOCK_VALUES {
        return Err(format!("`{key}` has more than {MAX_CELL_BLOCK_VALUES} entries"));
    }
    Ok(o.iter()
        .map(|(k, v)| (k.trim().to_string(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
        .filter(|(k, _)| !k.is_empty())
        .collect())
}

/// Check custom properties `props` against block `name`: each must name a visibility parameter
/// of the dynamic block and one of its states (other properties change the block's geometry,
/// which cells can't show yet).
fn check_props(d: &Drawing, name: &str, props: &BTreeMap<String, String>) -> std::result::Result<(), String> {
    if props.is_empty() {
        return Ok(());
    }
    let m = dynamic_block(d, name).ok_or_else(|| format!("Block \"{name}\" is not a dynamic block."))?;
    let vis: Vec<(&str, Vec<&str>)> = m
        .parameters
        .iter()
        .filter_map(|p| match &p.kind {
            ParameterKind::Visibility { label, states, .. } => Some((label.as_str(), states.iter().map(|s| s.name.as_str()).collect())),
            _ => None,
        })
        .collect();
    for (k, v) in props {
        let found =
            vis.iter().find(|(l, _)| l.eq_ignore_ascii_case(k)).or_else(|| k.eq_ignore_ascii_case("Visibility").then(|| vis.first()).flatten());
        let Some((label, states)) = found else {
            return Err(format!("A table cell can set only a visibility state of \"{name}\"; it has no visibility property \"{k}\"."));
        };
        if !states.iter().any(|s| s.eq_ignore_ascii_case(v.trim())) {
            return Err(format!("\"{v}\" is not a state of {label}: {}.", states.join(", ")));
        }
    }
    Ok(())
}

/// The table and cell a command names: `at` (a point in a cell), else `handle` + `row` + `col`.
fn target(s: &Session, p: &Value, id: &str) -> Result<Value> {
    let mut q = p.clone();
    if let Some(at) = p.get("at").and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok()) {
        let (h, r, c) = cell_under(s, Vec2::new(at[0], at[1])).ok_or_else(|| bad(id, "no table cell at that point"))?;
        q["handle"] = json!(h.hex());
        q["row"] = json!(r);
        q["col"] = json!(c);
    }
    Ok(q)
}

/// The anchor of the merge covering cell (`r`, `c`), or the cell itself.
fn anchor_of(t: &Table, r: usize, c: usize) -> (usize, usize) {
    anchors(t).into_iter().find(|&(ar, ac, rs, cs)| r >= ar && r < ar + rs && c >= ac && c < ac + cs).map(|(ar, ac, ..)| (ar, ac)).unwrap_or((r, c))
}

fn run_tinsert(s: &mut Session, p: &Value) -> Result<Value> {
    let q = target(s, p, "tinsert")?;
    let name = str_param(&q, "block").map(str::trim).unwrap_or("").to_string();
    let block = if name.is_empty() {
        None
    } else {
        let d = s.doc()?;
        let real = d.block(&name).map(|b| b.name.clone()).ok_or_else(|| bad("tinsert", format!("Block \"{name}\" not found.")))?;
        let scale = match q.get("scale") {
            None | Some(Value::Null) => None,
            Some(Value::String(f)) if f.trim().eq_ignore_ascii_case("fit") => None,
            Some(v) => {
                Some(v.as_f64().filter(|x| x.is_finite() && *x > 0.0 && *x <= 1e9).ok_or_else(|| bad("tinsert", format!("invalid scale {v}")))?)
            }
        };
        let rotation = match q.get("rotation") {
            None | Some(Value::Null) => 0.0,
            Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad("tinsert", format!("invalid rotation {v}")))?.to_radians(),
        };
        let attribs = string_map(&q, "attribs").map_err(|m| bad("tinsert", m))?;
        let props = string_map(&q, "props").map_err(|m| bad("tinsert", m))?;
        check_props(d, &real, &props).map_err(|m| bad("tinsert", m))?;
        Some(CellBlock { name: real, scale, rotation, attribs, props }.sanitized())
    };
    edit(s, &q, "tinsert", |t| {
        let (r, c) = cell_at(&q, t)?;
        let (r, c) = anchor_of(t, r, c);
        if let Some(cell) = t.cells.get_mut(r).and_then(|row| row.get_mut(c)) {
            if block.is_some() {
                // A cell holds a block or text, not both (as in AutoCAD).
                cell.text.clear();
                cell.formula = None;
            }
            cell.block = block;
        }
        Ok(())
    })
}

/// TINSERT at the command line: pick a cell, then name the block (fitted into the cell).
#[derive(Default)]
struct TinsertM {
    cell: Option<(Handle, usize, usize)>,
}

impl Interactive for TinsertM {
    fn name(&self) -> &'static str {
        "TINSERT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.cell {
            None => Prompt::new("Pick a table cell", Accept::POINT),
            Some(_) => Prompt::new("Enter block name", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.cell, i) {
            (_, Input::Cancel) | (None, Input::Enter) | (Some(_), Input::Enter) => Ok(Step::Cancel),
            (None, Input::Point(p)) => {
                match cell_under(s, p) {
                    Some(c) => self.cell = Some(c),
                    None => s.echo("No table cell there."),
                }
                Ok(Step::Continue)
            }
            (Some((h, r, c)), Input::Text(name) | Input::Keyword(name)) => {
                let q = json!({ "handle": h.hex(), "row": r, "col": c, "block": name });
                match run_tinsert(s, &q) {
                    Ok(_) => Ok(Step::Done),
                    Err(EngineError::BadParams { msg, .. }) => {
                        s.echo(msg);
                        Ok(Step::Continue)
                    }
                    Err(e) => Err(e),
                }
            }
            _ => Ok(Step::Continue),
        }
    }
}

// ---------------------------------------------------------------- cell borders

/// Which edges of a range `edges` names: (top, bottom, left, right, inside horizontal, inside
/// vertical).
fn edge_set(v: Option<&Value>) -> std::result::Result<[bool; 6], String> {
    let names: Vec<String> = match v {
        None | Some(Value::Null) => vec!["all".into()],
        Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_ascii_lowercase()).collect(),
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().unwrap_or("").trim().to_ascii_lowercase()).collect(),
        Some(v) => return Err(format!("invalid edges {v}")),
    };
    let mut out = [false; 6];
    for n in names {
        let set: &[usize] = match n.as_str() {
            "all" => &[0, 1, 2, 3, 4, 5],
            "outside" => &[0, 1, 2, 3],
            "inside" => &[4, 5],
            "top" => &[0],
            "bottom" => &[1],
            "left" => &[2],
            "right" => &[3],
            "insideh" => &[4],
            "insidev" => &[5],
            _ => return Err(format!("unknown edge {n} (all, outside, inside, top, bottom, left, right, insideH, insideV)")),
        };
        for i in set {
            if let Some(x) = out.get_mut(*i) {
                *x = true;
            }
        }
    }
    Ok(out)
}

fn run_borders(s: &mut Session, p: &Value) -> Result<Value> {
    let q = target(s, p, "tableborders")?;
    let p = &q;
    let which = edge_set(p.get("edges")).map_err(|m| bad("tableborders", m))?;
    let scope = str_param(p, "scope").unwrap_or("range").to_string();
    let clear = p.get("clear").and_then(Value::as_bool).unwrap_or(false);
    let mut border = CellBorder::default();
    if !clear {
        apply_border(&mut border, p).map_err(|m| bad("tableborders", m))?;
    }
    edit(s, p, "tableborders", |t| {
        let (r, c) = cell_at(p, t)?;
        // Borders of a column or the table take in the title and header rows too.
        let (r0, rows, c0, cols) = scope_cells(t, &scope, r, c, usize_param(p, "rows").unwrap_or(1), usize_param(p, "cols").unwrap_or(1), true)?;
        let [top, bottom, left, right, inside_h, inside_v] = which;
        let value = (!clear).then_some(border);
        for r in r0..r0 + rows {
            for c in c0..c0 + cols {
                let (ar, ac) = anchor_of(t, r, c);
                let Some(cell) = t.cells.get_mut(ar).and_then(|row| row.get_mut(ac)) else { continue };
                let mut e = cell.edges.unwrap_or_default();
                if (r == r0 && top) || (r > r0 && inside_h) {
                    e.top = value;
                }
                if (r + 1 == r0 + rows && bottom) || (r + 1 < r0 + rows && inside_h) {
                    e.bottom = value;
                }
                if (c == c0 && left) || (c > c0 && inside_v) {
                    e.left = value;
                }
                if (c + 1 == c0 + cols && right) || (c + 1 < c0 + cols && inside_v) {
                    e.right = value;
                }
                cell.edges = (!e.is_empty()).then_some(e);
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------- sizes

/// A positive size no larger than [`MAX_SIZE`].
fn size_value(v: &Value, what: &str) -> std::result::Result<f64, String> {
    v.as_f64().filter(|x| x.is_finite() && *x > 0.0 && *x <= MAX_SIZE).ok_or_else(|| format!("invalid {what} {v} (more than 0, at most {MAX_SIZE})"))
}

/// A list of sizes, at most `max` of them.
fn size_list(p: &Value, key: &str, max: usize) -> std::result::Result<Option<Vec<f64>>, String> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let a = v.as_array().ok_or_else(|| format!("`{key}` must be a list of sizes"))?;
    if a.len() > max {
        return Err(format!("`{key}` has more entries than the table has"));
    }
    a.iter().map(|x| size_value(x, key)).collect::<std::result::Result<Vec<_>, _>>().map(Some)
}

fn run_size(s: &mut Session, p: &Value) -> Result<Value> {
    let q = target(s, p, "tablesize")?;
    let one = |k: &str| q.get(k).filter(|v| !v.is_null()).map(|v| size_value(v, k)).transpose();
    let width = one("width").map_err(|m| bad("tablesize", m))?;
    let height = one("height").map_err(|m| bad("tablesize", m))?;
    let scope = str_param(&q, "scope").unwrap_or("cell").to_string();
    let grow = row_grow_param(&q).map_err(|m| bad("tablesize", m))?;
    edit(s, &q, "tablesize", |t| {
        let (nr, nc) = (t.row_heights.len(), t.col_widths.len());
        let widths = size_list(&q, "widths", nc)?;
        let heights = size_list(&q, "heights", nr)?;
        if width.is_none() && height.is_none() && widths.is_none() && heights.is_none() && grow.is_none() {
            return Err("nothing to set: give width, height, widths, heights or rowGrow".into());
        }
        if let Some(g) = grow {
            t.row_grow = g;
        }
        if let Some(ws) = widths {
            let c0 = usize_param(&q, "col").unwrap_or(0).min(nc);
            for (w, slot) in ws.iter().zip(t.col_widths.iter_mut().skip(c0)) {
                *slot = *w;
            }
        }
        if let Some(hs) = heights {
            let r0 = usize_param(&q, "row").unwrap_or(0).min(nr);
            for (i, h) in hs.iter().enumerate() {
                t.set_row_height(r0 + i, *h);
            }
        }
        if width.is_some() || height.is_some() {
            let (r, c) = cell_at(&q, t)?;
            let (r0, rows, c0, cols) =
                scope_cells(t, &scope, r, c, usize_param(&q, "rows").unwrap_or(1), usize_param(&q, "cols").unwrap_or(1), true)?;
            if let Some(w) = width {
                for slot in t.col_widths.iter_mut().skip(c0).take(cols) {
                    *slot = w;
                }
            }
            if let Some(h) = height {
                for r in r0..(r0 + rows).min(nr) {
                    t.set_row_height(r, h);
                }
            }
        }
        Ok(())
    })
}

/// `v` with at most 4 decimals and no trailing zeros ("8", "6.5").
fn short_number(v: f64) -> String {
    let t = format!("{v:.4}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// TABLESIZE at the command line: pick the cells, then their columns' width and their rows'
/// height (Enter keeps each); `Grow` at the height sets how the table's rows grow for their
/// text.
#[derive(Default)]
struct SizeM {
    pick: CellPick,
    /// 0 width, 1 height, 2 row growth.
    step: u8,
    params: serde_json::Map<String, Value>,
}

impl SizeM {
    /// The first picked column's width or row's height, as the prompt's default.
    fn current(&self, s: &Session) -> Option<f64> {
        let (h, r, c) = self.pick.cell?;
        let d = s.doc().ok()?;
        let EntityKind::Table(t) = &d.entity(h)?.kind else { return None };
        if self.step == 0 { t.col_widths.get(c).copied() } else { Some(t.row_base(r)) }
    }
}

impl Interactive for SizeM {
    fn name(&self) -> &'static str {
        "TABLESIZE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if let Some(p) = self.pick.prompt() {
            return p;
        }
        if self.step == 2 {
            return Prompt::new("Rows grow for their text: Free, Fixed or a step (Enter: keep)", Accept::TEXT).kw(GROWS).default(self.grow_now(s));
        }
        let p = if self.step == 0 {
            Prompt::new("Column width (Enter: keep)", Accept::NUMBER)
        } else {
            Prompt::new("Row height (Enter: keep)", Accept::NUMBER).kw(&["Grow"])
        };
        match self.current(s) {
            Some(v) => p.default(short_number(v)),
            None => p,
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.pick.prompt().is_some() {
            return Ok(match self.pick.input(s, i) {
                Picked::Cancel => Step::Cancel,
                _ => Step::Continue,
            });
        }
        if self.step == 2 {
            match i {
                Input::Cancel => return Ok(Step::Cancel),
                Input::Text(t) | Input::Keyword(t) => match row_grow_param(&json!({ "rowGrow": t.trim() })) {
                    Ok(Some(g)) => {
                        self.params.insert("rowGrow".into(), json!(grow_word(g)));
                    }
                    _ => {
                        s.echo("Type Free, Fixed or a step more than 0.");
                        return Ok(Step::Continue);
                    }
                },
                _ => {}
            }
            return Ok(self.finish(s));
        }
        let key = if self.step == 0 { "width" } else { "height" };
        match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) if self.step == 1 && t.trim().eq_ignore_ascii_case("grow") => {
                self.step = 2;
                return Ok(Step::Continue);
            }
            Input::Enter => {}
            Input::Text(t) | Input::Keyword(t) => match t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_SIZE) {
                Some(v) => {
                    self.params.insert(key.into(), json!(v));
                }
                None => {
                    s.echo("Type a size more than 0.");
                    return Ok(Step::Continue);
                }
            },
            _ => return Ok(Step::Continue),
        }
        if self.step == 0 {
            self.step = 1;
            return Ok(Step::Continue);
        }
        Ok(self.finish(s))
    }
}

const GROWS: &[&str] = &["Free", "Fixed"];

/// A row growth as TABLESIZE's `rowGrow` takes it.
fn grow_word(g: cadcraft_doc::RowGrow) -> String {
    match g {
        cadcraft_doc::RowGrow::Free => "free".into(),
        cadcraft_doc::RowGrow::Fixed => "fixed".into(),
        cadcraft_doc::RowGrow::Step(s) => short_number(s),
    }
}

impl SizeM {
    /// The picked table's row growth, as the prompt's default ("Free", "Fixed", "8").
    fn grow_now(&self, s: &Session) -> String {
        let g = self.pick.cell.and_then(|(h, ..)| match s.doc().ok()?.entity(h).map(|e| &e.kind) {
            Some(EntityKind::Table(t)) => Some(t.row_grow),
            _ => None,
        });
        match g.unwrap_or_default() {
            cadcraft_doc::RowGrow::Free => "Free".into(),
            cadcraft_doc::RowGrow::Fixed => "Fixed".into(),
            cadcraft_doc::RowGrow::Step(v) => short_number(v),
        }
    }

    /// Apply what was typed.
    fn finish(&self, s: &mut Session) -> Step {
        if !self.params.is_empty() {
            let mut q = self.pick.params();
            q.extend(self.params.clone());
            if let Err(e) = run_size(s, &Value::Object(q)) {
                s.echo(e.to_string());
            }
        }
        Step::Done
    }
}

// ---------------------------------------------------------------- cell format

/// Which cells a scope covers, from cell (`r`, `c`): (first row, rows, first column, columns).
/// Column, row and table scopes cover data rows only (below the title and header rows).
/// With `labels`, column and table scopes take in the title and header rows too.
fn scope_cells(
    t: &Table,
    scope: &str,
    r: usize,
    c: usize,
    rows: usize,
    cols: usize,
    labels: bool,
) -> std::result::Result<(usize, usize, usize, usize), String> {
    let (nr, nc) = (t.cells.len(), t.col_widths.len());
    let data = if labels { 0 } else { (usize::from(t.title) + usize::from(t.header)).min(nr) };
    Ok(match scope.trim().to_ascii_lowercase().as_str() {
        "" | "cell" => (r, 1, c, 1),
        "column" | "col" => (data, nr - data, c, 1),
        "row" => (r, 1, 0, nc),
        "table" => (data, nr - data, 0, nc),
        "range" => (r, rows.max(1).min(nr - r), c, cols.max(1).min(nc - c)),
        other => return Err(format!("unknown scope {other} (cell, column, row, table, range)")),
    })
}

/// The look a format command sets: its alignment, margins and rotation (each when given).
fn look_param(p: &Value) -> std::result::Result<CellLook, String> {
    let margin = |k: &str| -> std::result::Result<Option<f64>, String> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|x| x.is_finite() && (0.0..=1e9).contains(x)).map(Some).ok_or_else(|| format!("invalid {k}: {v}")),
        }
    };
    let both = margin("margin")?;
    let align = match p.get("align") {
        None | Some(Value::Null) => None,
        Some(a) => Some(align_value(a).ok_or_else(|| format!("invalid align {a} (TL, TC, TR, ML, MC, MR, BL, BC, BR)"))?),
    };
    let rotation = match p.get("rotation") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|r| u16::try_from(r).ok())
                .filter(|r| [0, 90, 180, 270].contains(r))
                .ok_or_else(|| format!("invalid rotation {v} (0, 90, 180, 270)"))?,
        ),
    };
    Ok(CellLook { align, margin_h: margin("marginH")?.or(both), margin_v: margin("marginV")?.or(both), rotation })
}

/// The scale of the table a command names: its text height over its table style's (1 when
/// unknown), as the renderer applies it to cell styles and cells' own text heights.
fn table_scale(s: &Session, q: &Value) -> f64 {
    let Some(h) = q.get("handle").and_then(|v| v.as_str().and_then(Handle::parse_hex).or_else(|| v.as_u64().map(Handle))) else { return 1.0 };
    let Ok(d) = s.doc() else { return 1.0 };
    let Some(EntityKind::Table(t)) = d.entity(h).map(|e| &e.kind) else { return 1.0 };
    let k = t.text_height / cadcraft_render::table_style_of(d, t).text_height;
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

fn run_format(s: &mut Session, p: &Value) -> Result<Value> {
    let q = target(s, p, "tableformat")?;
    let clear = q.get("clear").and_then(Value::as_bool).unwrap_or(false);
    let set = look_param(&q).map_err(|m| bad("tableformat", m))?;
    let height = match q.get("textHeight") {
        None | Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_f64().filter(|h| h.is_finite() && *h > 0.0 && *h <= 1e6).ok_or_else(|| bad("tableformat", format!("invalid textHeight {v}")))?)
        }
    };
    if !clear && set.is_empty() && height.is_none() {
        return Err(bad("tableformat", "nothing to set: give align, textHeight, margin, marginH, marginV or rotation (or clear)"));
    }
    // A cell's own text height is kept before the table's scale (its text height over its
    // style's), so the height asked for is the one drawn.
    let scale = table_scale(s, &q);
    let scope = str_param(&q, "scope").unwrap_or("cell").to_string();
    let mut n = 0usize;
    let r = edit(s, &q, "tableformat", |t| {
        let (r, c) = cell_at(&q, t)?;
        let (r0, rows, c0, cols) = scope_cells(t, &scope, r, c, usize_param(&q, "rows").unwrap_or(1), usize_param(&q, "cols").unwrap_or(1), false)?;
        for r in r0..r0 + rows {
            for c in c0..c0 + cols {
                let (ar, ac) = anchor_of(t, r, c);
                let Some(cell) = t.cells.get_mut(ar).and_then(|row| row.get_mut(ac)) else { continue };
                let mut l = if clear { CellLook::default() } else { cell.look.unwrap_or_default() };
                l.align = set.align.or(l.align);
                l.margin_h = set.margin_h.or(l.margin_h);
                l.margin_v = set.margin_v.or(l.margin_v);
                l.rotation = set.rotation.or(l.rotation);
                cell.look = (!l.is_empty()).then_some(l);
                if clear {
                    cell.text_height = None;
                }
                if let Some(h) = height {
                    cell.text_height = Some(h / scale);
                }
                n += 1;
            }
        }
        Ok(())
    })?;
    Ok(json!({ "handle": r["handle"], "cells": n }))
}

/// Picking cells at the command line: a cell, what it stands for (the cell, its column, its row,
/// the table, or a range to a second cell).
#[derive(Default)]
struct CellPick {
    cell: Option<(Handle, usize, usize)>,
    scope: Option<String>,
    corner: Option<(usize, usize)>,
}

/// Where a [`CellPick`] is after an input.
enum Picked {
    Asking,
    Done,
    Cancel,
}

const SCOPES: &[&str] = &["Cell", "cOlumn", "Row", "Table", "rAnge"];

impl CellPick {
    /// The prompt while picking, or `None` once picked.
    fn prompt(&self) -> Option<Prompt> {
        match (&self.cell, self.scope.as_deref()) {
            (None, _) => Some(Prompt::new("Pick a table cell", Accept::POINT)),
            (Some(_), None) => Some(Prompt::new("Apply to", Accept::TEXT).kw(SCOPES).default("Cell")),
            (Some(_), Some("range")) if self.corner.is_none() => Some(Prompt::new("Pick the opposite cell of the range", Accept::POINT)),
            _ => None,
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Picked {
        match (&self.cell, self.scope.as_deref(), i) {
            (_, _, Input::Cancel) | (None, _, Input::Enter) => return Picked::Cancel,
            (None, _, Input::Point(p)) => match cell_under(s, p) {
                Some(c) => self.cell = Some(c),
                None => s.echo("No table cell there."),
            },
            (Some(_), None, Input::Enter) => self.scope = Some("cell".into()),
            (Some(_), None, Input::Keyword(k) | Input::Text(k)) => {
                let k = k.trim().to_ascii_lowercase();
                let scope = match k.as_str() {
                    "o" => Some("column"),
                    "a" => Some("range"),
                    _ => ["cell", "column", "row", "table", "range"].into_iter().find(|x| !k.is_empty() && x.starts_with(&k)),
                };
                match scope {
                    Some(sc) => self.scope = Some(sc.into()),
                    None => s.echo("Choose Cell, cOlumn, Row, Table or rAnge."),
                }
            }
            (Some((h, ..)), Some("range"), Input::Point(p)) => match cell_under(s, p) {
                Some((h2, r2, c2)) if h2 == *h => self.corner = Some((r2, c2)),
                _ => s.echo("Pick a cell of the same table."),
            },
            _ => {}
        }
        if self.prompt().is_none() { Picked::Done } else { Picked::Asking }
    }
    /// The picked cells as command parameters: handle, row, col, scope (and rows, cols).
    fn params(&self) -> serde_json::Map<String, Value> {
        let mut q = serde_json::Map::new();
        let Some((h, r, c)) = self.cell else { return q };
        let (r0, c0) = self.corner.map_or((r, c), |(r2, c2)| (r.min(r2), c.min(c2)));
        q.insert("handle".into(), json!(h.hex()));
        q.insert("row".into(), json!(r0));
        q.insert("col".into(), json!(c0));
        q.insert("scope".into(), json!(self.scope.clone().unwrap_or_else(|| "cell".into())));
        if let Some((r2, c2)) = self.corner {
            q.insert("rows".into(), json!(r.abs_diff(r2) + 1));
            q.insert("cols".into(), json!(c.abs_diff(c2) + 1));
        }
        q
    }
}

/// TABLEFORMAT at the command line: pick the cells, then the alignment, margin and rotation
/// (Enter keeps each as it is).
#[derive(Default)]
struct FormatM {
    pick: CellPick,
    /// 0 alignment, 1 text height, 2 margin, 3 rotation.
    step: u8,
    params: serde_json::Map<String, Value>,
}

const ALIGNS: &[&str] = &["TL", "TC", "TR", "ML", "MC", "MR", "BL", "BC", "BR"];
const ROTATIONS: &[&str] = &["0", "90", "180", "270"];

impl FormatM {
    fn apply(&mut self, s: &mut Session) -> Result<Step> {
        if self.params.is_empty() {
            return Ok(Step::Done);
        }
        let mut q = self.pick.params();
        q.extend(self.params.clone());
        if let Err(e) = run_format(s, &Value::Object(q)) {
            s.echo(e.to_string());
        }
        Ok(Step::Done)
    }
}

impl Interactive for FormatM {
    fn name(&self) -> &'static str {
        "TABLEFORMAT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if let Some(p) = self.pick.prompt() {
            return p;
        }
        match self.step {
            0 => Prompt::new("Alignment (Enter: keep)", Accept::TEXT).kw(ALIGNS),
            1 => Prompt::new("Text height (Enter: keep)", Accept::NUMBER),
            2 => Prompt::new("Margin from the cell's sides (Enter: keep)", Accept::NUMBER),
            _ => Prompt::new("Text rotation (Enter: keep)", Accept::TEXT).kw(ROTATIONS),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.pick.prompt().is_some() {
            return Ok(match self.pick.input(s, i) {
                Picked::Cancel => Step::Cancel,
                _ => Step::Continue,
            });
        }
        match (self.step, i) {
            (_, Input::Cancel) => return Ok(Step::Cancel),
            (3, Input::Enter) => return self.apply(s),
            (_, Input::Enter) => self.step += 1,
            (0, Input::Keyword(k) | Input::Text(k)) => match align_value(&json!(k)) {
                Some(a) => {
                    self.params.insert("align".into(), json!(a));
                    self.step = 1;
                }
                None => s.echo("Choose TL, TC, TR, ML, MC, MR, BL, BC or BR."),
            },
            (1, Input::Keyword(t) | Input::Text(t)) => match t.trim().parse::<f64>().ok().filter(|h| h.is_finite() && *h > 0.0 && *h <= 1e6) {
                Some(h) => {
                    self.params.insert("textHeight".into(), json!(h));
                    self.step = 2;
                }
                None => s.echo("The text height is more than 0."),
            },
            (2, Input::Keyword(t) | Input::Text(t)) => match t.trim().parse::<f64>().ok().filter(|m| m.is_finite() && (0.0..=1e9).contains(m)) {
                Some(m) => {
                    self.params.insert("marginH".into(), json!(m));
                    self.step = 3;
                }
                None => s.echo("The margin is a distance of 0 or more."),
            },
            (_, Input::Keyword(k) | Input::Text(k)) => match k.trim().parse::<u16>().ok().filter(|r| [0, 90, 180, 270].contains(r)) {
                Some(r) => {
                    self.params.insert("rotation".into(), json!(r));
                    return self.apply(s);
                }
                None => s.echo("Choose 0, 90, 180 or 270."),
            },
            _ => {}
        }
        Ok(Step::Continue)
    }
}

/// TABLEBORDERS at the command line: pick the cells, which of their edges, then the lineweight
/// (millimetres, ByBlock, or Clear to go back to the cell style).
#[derive(Default)]
struct BordersM {
    pick: CellPick,
    edges: Option<String>,
}

const EDGES: &[&str] = &["All", "Outside", "Inside", "Top", "Bottom", "Left", "Right", "insideH", "insideV"];

impl Interactive for BordersM {
    fn name(&self) -> &'static str {
        "TABLEBORDERS"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if let Some(p) = self.pick.prompt() {
            return p;
        }
        match self.edges {
            None => Prompt::new("Edges", Accept::TEXT).kw(EDGES).default("All"),
            Some(_) => Prompt::new("Lineweight in mm, ByBlock or Clear", Accept::TEXT).kw(&["Byblock", "Clear"]).default("0.5"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.pick.prompt().is_some() {
            return Ok(match self.pick.input(s, i) {
                Picked::Cancel => Step::Cancel,
                _ => Step::Continue,
            });
        }
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let Some(edges) = self.edges.clone() else {
            let k = match i {
                Input::Enter => "all".to_string(),
                Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase(),
                _ => return Ok(Step::Continue),
            };
            let name = match k.as_str() {
                "a" => Some("all"),
                "o" => Some("outside"),
                "i" => Some("inside"),
                "t" => Some("top"),
                "b" => Some("bottom"),
                "l" => Some("left"),
                "r" => Some("right"),
                "h" => Some("insideH"),
                "v" => Some("insideV"),
                _ => EDGES.iter().copied().find(|e| e.eq_ignore_ascii_case(&k)),
            };
            match name {
                Some(n) => self.edges = Some(n.to_string()),
                None => s.echo("Choose All, Outside, Inside, Top, Bottom, Left, Right, insideH or insideV."),
            }
            return Ok(Step::Continue);
        };
        let v = match i {
            Input::Enter => "0.5".to_string(),
            Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase(),
            _ => return Ok(Step::Continue),
        };
        let mut q = self.pick.params();
        q.insert("edges".into(), json!(edges));
        match v.as_str() {
            "c" | "clear" => {
                q.insert("clear".into(), json!(true));
            }
            "b" | "byblock" => {
                q.insert("lineweight".into(), json!("ByBlock"));
            }
            mm => match mm.trim_end_matches("mm").trim().parse::<f64>() {
                Ok(w) => {
                    q.insert("lineweight".into(), json!(w));
                }
                Err(_) => {
                    s.echo("Type a lineweight in millimetres (0.18, 0.5, 1), ByBlock or Clear.");
                    return Ok(Step::Continue);
                }
            },
        }
        if let Err(e) = run_borders(s, &Value::Object(q)) {
            s.echo(e.to_string());
            return Ok(Step::Continue);
        }
        Ok(Step::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_names() {
        assert_eq!(edge_set(None), Ok([true; 6]));
        assert_eq!(edge_set(Some(&json!("outside"))), Ok([true, true, true, true, false, false]));
        assert_eq!(edge_set(Some(&json!(["top", "insideV"]))), Ok([true, false, false, false, false, true]));
        assert!(edge_set(Some(&json!("diagonal"))).is_err());
        assert!(edge_set(Some(&json!(3))).is_err());
    }

    #[test]
    fn string_maps_are_bounded() {
        let mut big = serde_json::Map::new();
        for i in 0..=MAX_CELL_BLOCK_VALUES {
            big.insert(format!("T{i}"), json!(i));
        }
        assert!(string_map(&json!({ "attribs": big }), "attribs").is_err());
        let m = string_map(&json!({ "attribs": { "NUMBER": 5, " ": "x", "VAL": "a" } }), "attribs").unwrap_or_default();
        assert_eq!(m.get("NUMBER").map(String::as_str), Some("5"));
        assert_eq!(m.len(), 2);
        assert!(string_map(&json!({ "attribs": [1] }), "attribs").is_err());
    }
}
