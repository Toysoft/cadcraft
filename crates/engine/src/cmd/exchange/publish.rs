//! PUBLISH: plot a list of sheets from open drawings and drawing files to PDF, SVG or PNG files,
//! one per sheet or one combined PDF, with a page setup changed per run and an optional sheet
//! stamp. `action: "status"` checks the sheets without plotting.
//!
//! A run is a [`PublishJob`]: the request checked and the open drawings it uses taken as a
//! snapshot, so the plotting can run on a worker thread while the drawings stay editable.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadcraft_doc::{Drawing, Space};
use serde_json::{Value, json};

use super::super::file::{base64_encode, io, plot_book};
use super::MAX_SHEETS;
use crate::cmd::{bad, str_param};
use crate::{EngineError, Result, Session};

/// Most copies one PUBLISH asks for.
const MAX_COPIES: u64 = 100;
/// Longest file name pattern, in characters.
const MAX_NAME: usize = 200;
/// The file name pattern when none is given: the drawing's name, then the sheet's.
const DEFAULT_NAME: &str = "{drawing}-{sheet}";
/// Parameters PUBLISH takes (anything else is refused, so a misspelt name is never ignored).
const PUBLISH_KEYS: &[&str] = &["action", "sheets", "to", "folder", "name", "combine", "pdf", "copies", "stamp", "path", "layouts"];
/// The combined file's name pattern when none is given.
const DEFAULT_COMBINED_NAME: &str = "{drawing}";
/// What a sheet's `pageSetup` may change for one run.
const SETUP_KEYS: &[&str] =
    &["paper", "width", "height", "landscape", "fit", "scale", "margins", "plotStyleTable", "lineweights", "plotTransparency"];
/// What `pdf` may set.
const PDF_KEYS: &[&str] = &["layers", "offLayers", "text"];
/// What `stamp` may set.
const STAMP_KEYS: &[&str] = &["text", "corner", "height"];
/// The stamp's text when none is given.
const DEFAULT_STAMP: &str = "{drawing} · {sheet} · {date} {time}";
/// Longest stamp text, in characters; its height on paper in millimetres.
const MAX_STAMP: usize = 200;
const STAMP_HEIGHT_MM: (f64, f64, f64) = (0.5, 2.5, 50.0);
/// The stamp's distance from the printable area's edges, in millimetres.
const STAMP_INSET_MM: f64 = 3.0;

/// Where on the sheet the stamp goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Corner {
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

/// A sheet stamp: a line of text with `{drawing}`, `{sheet}`, `{n}`, `{date}` and `{time}` in a
/// corner of each sheet.
#[derive(Clone, Debug)]
pub(super) struct Stamp {
    text: String,
    corner: Corner,
    height_mm: f64,
}

/// `stamp`: `true` (the default text), or `{text?, corner?, height?}`.
pub(super) fn stamp_settings(cmd: &str, v: &Value) -> Result<Option<Stamp>> {
    let o = match v {
        Value::Null | Value::Bool(false) => return Ok(None),
        Value::Bool(true) => return Ok(Some(Stamp { text: DEFAULT_STAMP.into(), corner: Corner::BottomRight, height_mm: STAMP_HEIGHT_MM.1 })),
        Value::Object(o) => o,
        _ => return Err(bad(cmd, "`stamp` must be true, false or {text?, corner?, height?}")),
    };
    unknown_key(cmd, o, STAMP_KEYS, "stamp setting")?;
    let text = match o.get("text") {
        None | Some(Value::Null) => DEFAULT_STAMP.to_string(),
        Some(Value::String(t)) if !t.trim().is_empty() && t.chars().count() <= MAX_STAMP && !t.chars().any(char::is_control) => t.clone(),
        Some(_) => return Err(bad(cmd, format!("`stamp.text` must be one line of 1 to {MAX_STAMP} characters"))),
    };
    stamp_text(&text, "", "", 1, ("", "")).map_err(|e| bad(cmd, e))?;
    let corner = match o.get("corner").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None if o.get("corner").is_none_or(Value::is_null) => Corner::BottomRight,
        Some("bottom-left") => Corner::BottomLeft,
        Some("bottom-right") => Corner::BottomRight,
        Some("top-left") => Corner::TopLeft,
        Some("top-right") => Corner::TopRight,
        _ => return Err(bad(cmd, "`stamp.corner` must be bottom-left, bottom-right, top-left or top-right")),
    };
    let (lo, default, hi) = STAMP_HEIGHT_MM;
    let height_mm = match o.get("height") {
        None | Some(Value::Null) => default,
        Some(h) => h
            .as_f64()
            .filter(|h| h.is_finite() && (lo..=hi).contains(h))
            .ok_or_else(|| bad(cmd, format!("`stamp.height` must be {lo} to {hi} millimetres")))?,
    };
    Ok(Some(Stamp { text, corner, height_mm }))
}

/// Fill a pattern's `{token}`s from `values` (`{{` and `}}` are braces). `what` names the pattern
/// in errors ("`name`", "the stamp"). An unknown token, a lone `}` or a `{` without its `}` is
/// refused.
pub fn fill_tokens(what: &str, pattern: &str, values: &[(&str, &str)]) -> std::result::Result<String, String> {
    let known = || values.iter().map(|(k, _)| format!("{{{k}}}")).collect::<Vec<_>>().join(", ");
    let mut out = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut token = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    token.push(c);
                }
                if !closed {
                    return Err(format!("{what} has a {{ without its }}: write {{{{ for a brace"));
                }
                match values.iter().find(|(k, _)| *k == token) {
                    Some((_, v)) => out.push_str(v),
                    None => return Err(format!("{what} has an unknown token {{{token}}}: use {}", known())),
                }
            }
            '}' => return Err(format!("{what} has a }} without its {{: write }}}} for a brace")),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// The stamp's text for one sheet: `{drawing}`, `{sheet}`, `{n}`, `{date}` and `{time}`.
fn stamp_text(pattern: &str, drawing: &str, sheet: &str, n: usize, (date, time): (&str, &str)) -> std::result::Result<String, String> {
    let n = n.to_string();
    fill_tokens("the stamp", pattern, &[("drawing", drawing), ("sheet", sheet), ("n", &n), ("date", date), ("time", time)])
}

/// Today's date and the time, UTC (`2026-10-15`, `09:47 UTC`): the same on every sheet of a run.
fn utc_now() -> (String, String) {
    // The web build has no system clock (`SystemTime::now` panics there): no date or time.
    if cfg!(target_arch = "wasm32") {
        return (String::new(), String::new());
    }
    utc_at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// The date and time `secs` seconds after 1970-01-01, UTC.
fn utc_at(secs: u64) -> (String, String) {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (format!("{year:04}-{month:02}-{day:02}"), format!("{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60))
}

/// A copy of the drawing with the stamp in the sheet's corner. On a layout it sits inside the
/// printable area of the paper the sheet plots on (a page setup changed for the run included), at
/// its height on paper; in model space, which is fitted to the paper, it goes just outside the
/// drawing's extents at a fiftieth of their size.
fn stamped(d: &Drawing, space: &Space, setup: Option<&serde_json::Map<String, Value>>, stamp: &Stamp, text: String) -> Drawing {
    use cadcraft_doc::{Common, EntityKind, HAlign, Text, VAlign};
    use cadcraft_geom::{Vec2, Vec3};
    let right = matches!(stamp.corner, Corner::BottomRight | Corner::TopRight);
    let top = matches!(stamp.corner, Corner::TopLeft | Corner::TopRight);
    let at = match space {
        Space::Paper(name) => {
            let mut page = d.layout(name).map(|l| l.page.clone()).unwrap_or_default();
            if let Some(set) = setup {
                if let Some(ps) = set.get("paper").and_then(Value::as_str).and_then(cadcraft_render::paper_size) {
                    page.paper = ps.name.into();
                    page.width_mm = ps.width_mm;
                    page.height_mm = ps.height_mm;
                }
                if let (Some(w), Some(h)) = (set.get("width").and_then(Value::as_f64), set.get("height").and_then(Value::as_f64)) {
                    page.width_mm = w;
                    page.height_mm = h;
                }
                if let Some(l) = set.get("landscape").and_then(Value::as_bool) {
                    page.landscape = l;
                }
                // The run's margins move the printable area the stamp sits in.
                if let Some(m) = set.get("margins").and_then(Value::as_array).filter(|m| m.len() == 4) {
                    for (slot, v) in page.margins_mm.iter_mut().zip(m) {
                        if let Some(v) = v.as_f64().filter(|v| v.is_finite() && *v >= 0.0) {
                            *slot = v;
                        }
                    }
                }
            }
            let unit = cadcraft_render::paper::paper_unit_mm(d);
            let sheet = cadcraft_render::Sheet::from_page(&page, unit);
            let (h, inset) = (stamp.height_mm / unit, STAMP_INSET_MM / unit);
            let a = sheet.printable;
            let x = if right { a.max.x - inset } else { a.min.x + inset };
            let y = if top { a.max.y - inset - h } else { a.min.y + inset };
            (Vec2::new(x, y), h)
        }
        Space::Model => {
            let e = d.extents(space);
            let e = if e.is_empty() || !e.min.is_finite() || !e.max.is_finite() {
                cadcraft_geom::Bounds2::new(Vec2::ZERO, Vec2::new(1.0, 1.0))
            } else {
                e
            };
            let h = (e.width().max(e.height()) / 50.0).max(1e-6);
            let x = if right { e.max.x } else { e.min.x };
            let y = if top { e.max.y + 0.6 * h } else { e.min.y - 1.6 * h };
            (Vec2::new(x, y), h)
        }
    };
    let ((x, y), height) = ((at.0.x, at.0.y), at.1);
    let p = Vec3::new(x, y, 0.0);
    let text = Text {
        insert: p,
        align_pt: right.then_some(p),
        height,
        value: text,
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: if right { HAlign::Right } else { HAlign::Left },
        valign: VAlign::Baseline,
    };
    let mut c = d.clone();
    let _ = c.add(space, Common::default(), EntityKind::Text(text));
    c
}
/// Largest custom paper side and margin, in millimetres.
const MAX_PAPER_MM: f64 = 10_000.0;
const MAX_MARGIN_MM: f64 = 1_000.0;

/// A layout name made safe for a file name.
pub(in crate::cmd) fn file_part(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' }).collect()
}

/// A drawing's name without its folder or extension.
fn drawing_stem(name: &str) -> String {
    std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Drawing".into())
}

/// Where a sheet's drawing comes from.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// An open drawing, by its index in the session.
    Open(usize),
    /// A drawing file, read for this run only.
    File(String),
}

/// One sheet to publish, as asked for.
#[derive(Clone, Debug)]
struct SheetRequest {
    source: Source,
    layout: String,
    /// Plot settings replacing the sheet's page setup for this run (already checked).
    setup: Option<serde_json::Map<String, Value>>,
}

/// The settings every sheet of one run shares.
#[derive(Clone, Debug)]
struct PublishOutput {
    /// `pdf`, `svg` or `png`.
    to: &'static str,
    /// Files go here; without one each sheet comes back as base64 `data`.
    folder: Option<std::path::PathBuf>,
    /// File name pattern: `{drawing}`, `{sheet}` and `{n}`, with `{{` and `}}` for braces.
    name: String,
    /// PDF settings (`layers`, `offLayers`, `text`).
    pdf: serde_json::Map<String, Value>,
    copies: u64,
    /// One PDF with a page per sheet, instead of a file per sheet.
    combine: bool,
    /// Text added to every sheet.
    stamp: Option<Stamp>,
    /// `action: "status"`: check the sheets and name the files, write nothing.
    status: bool,
}

pub(super) fn unknown_key(cmd: &str, v: &serde_json::Map<String, Value>, allowed: &[&str], what: &str) -> Result<()> {
    match v.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(bad(cmd, format!("unknown {what} `{k}` (expected one of: {})", allowed.join(", ")))),
        None => Ok(()),
    }
}

/// A sheet's page setup override: checked here, so a bad value fails the run before any file is
/// written, and returned as plot options.
pub(super) fn setup_override(cmd: &str, v: &Value) -> Result<serde_json::Map<String, Value>> {
    let o = v.as_object().ok_or_else(|| bad(cmd, "`pageSetup` must be an object"))?;
    unknown_key(cmd, o, SETUP_KEYS, "page setup setting")?;
    let mut out = serde_json::Map::new();
    for key in ["landscape", "fit", "lineweights", "plotTransparency"] {
        if let Some(x) = o.get(key) {
            let b = x.as_bool().ok_or_else(|| bad(cmd, format!("`pageSetup.{key}` must be true or false")))?;
            out.insert(key.into(), json!(b));
        }
    }
    if let Some(x) = o.get("paper") {
        let name = x
            .as_str()
            .filter(|n| cadcraft_render::paper_size(n).is_some())
            .ok_or_else(|| bad(cmd, "`pageSetup.paper` must be a paper name such as \"A3\" or \"Letter\""))?;
        out.insert("paper".into(), json!(name));
    }
    let side = |key: &str| -> Result<Option<f64>> {
        match o.get(key) {
            None => Ok(None),
            Some(x) => x
                .as_f64()
                .filter(|w| w.is_finite() && *w > 0.0 && *w <= MAX_PAPER_MM)
                .map(Some)
                .ok_or_else(|| bad(cmd, format!("`pageSetup.{key}` must be a paper size in millimetres up to {MAX_PAPER_MM}"))),
        }
    };
    match (side("width")?, side("height")?) {
        (Some(w), Some(h)) => {
            out.insert("width".into(), json!(w));
            out.insert("height".into(), json!(h));
        }
        (None, None) => {}
        _ => return Err(bad(cmd, "`pageSetup.width` and `pageSetup.height` go together")),
    }
    if let Some(x) = o.get("scale") {
        let k = super::layout::parse_scale(x)
            .ok_or_else(|| bad(cmd, "`pageSetup.scale` must be positive: paper units per drawing unit, or a ratio such as \"1:50\""))?;
        out.insert("scale".into(), json!(k));
    }
    if let Some(x) = o.get("margins") {
        let m: Option<Vec<f64>> = x
            .as_array()
            .filter(|a| a.len() == 4)
            .and_then(|a| a.iter().map(|m| m.as_f64().filter(|m| m.is_finite() && (0.0..=MAX_MARGIN_MM).contains(m))).collect());
        let m = m.ok_or_else(|| {
            bad(cmd, format!("`pageSetup.margins` must be four millimetre values from 0 to {MAX_MARGIN_MM}: left, bottom, right, top"))
        })?;
        out.insert("margins".into(), json!(m));
    }
    if let Some(x) = o.get("plotStyleTable") {
        let t =
            x.as_str().filter(|t| t.chars().count() <= 260).ok_or_else(|| bad(cmd, "`pageSetup.plotStyleTable` must be a plot style table name"))?;
        out.insert("plotStyleTable".into(), json!(t));
    }
    Ok(out)
}

/// The `sheets` list: `[{drawing?: index | path, layout, pageSetup?}]`.
fn sheet_list(cmd: &str, s: &Session, v: &Value) -> Result<Vec<SheetRequest>> {
    let a = v.as_array().ok_or_else(|| bad(cmd, "`sheets` must be a list of {drawing?, layout, pageSetup?}"))?;
    if a.len() > MAX_SHEETS {
        return Err(bad(cmd, format!("at most {MAX_SHEETS} sheets can be published at once")));
    }
    a.iter()
        .enumerate()
        .map(|(i, x)| {
            let at = i + 1;
            let o = x.as_object().ok_or_else(|| bad(cmd, format!("sheet {at} must be an object: {{drawing?, layout, pageSetup?}}")))?;
            unknown_key(cmd, o, &["drawing", "layout", "pageSetup"], "sheet setting")?;
            let source = match o.get("drawing") {
                None | Some(Value::Null) => Source::Open(s.active),
                Some(Value::String(path)) if !path.trim().is_empty() => Source::File(path.clone()),
                Some(d) => {
                    let i = d.as_u64().and_then(|i| usize::try_from(i).ok());
                    Source::Open(i.ok_or_else(|| bad(cmd, format!("sheet {at}: `drawing` must be an open drawing's index or a file path")))?)
                }
            };
            let layout = o
                .get("layout")
                .and_then(Value::as_str)
                .filter(|l| !l.trim().is_empty())
                .ok_or_else(|| bad(cmd, format!("sheet {at}: `layout` (a layout name or \"Model\") is required")))?;
            let setup = o.get("pageSetup").filter(|v| !v.is_null()).map(|v| setup_override(cmd, v)).transpose()?;
            Ok(SheetRequest { source, layout: layout.to_string(), setup })
        })
        .collect()
}

/// The older form's sheets: `layouts` of the current drawing (checked up front, as before), or
/// Model if it has objects and then every layout in tab order.
fn legacy_sheets(cmd: &str, s: &Session, layouts: Option<&Value>) -> Result<Vec<SheetRequest>> {
    let d = s.doc()?;
    let names: Vec<String> = match layouts {
        Some(v) => {
            let a = v.as_array().ok_or_else(|| bad(cmd, "`layouts` must be a list of layout names"))?;
            a.iter()
                .map(|n| {
                    let n = n.as_str().ok_or_else(|| bad(cmd, "`layouts` must be a list of layout names"))?;
                    if n.eq_ignore_ascii_case("model") {
                        return Ok("Model".to_string());
                    }
                    d.layout(n).map(|l| l.name.clone()).ok_or_else(|| bad(cmd, format!("no layout `{n}`")))
                })
                .collect::<Result<_>>()?
        }
        None => {
            let mut layouts: Vec<_> = d.layouts.iter().collect();
            layouts.sort_by_key(|l| l.tab_order);
            let model = (!d.model.is_empty()).then(|| "Model".to_string());
            model.into_iter().chain(layouts.iter().map(|l| l.name.clone())).collect()
        }
    };
    Ok(names.into_iter().map(|layout| SheetRequest { source: Source::Open(s.active), layout, setup: None }).collect())
}

/// Check a file name pattern: known tokens only, and nothing that leaves the folder.
pub(super) fn check_name(cmd: &str, pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() || pattern.chars().count() > MAX_NAME {
        return Err(bad(cmd, format!("`name` must be a file name pattern of 1 to {MAX_NAME} characters")));
    }
    if pattern.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(bad(cmd, "`name` can't contain / \\ : * ? \" < > | or control characters"));
    }
    if pattern.trim() == "." || pattern.trim() == ".." {
        return Err(bad(cmd, "`name` must be a file name"));
    }
    render_name(pattern, "", "", 1).map(|_| ()).map_err(|e| bad(cmd, e))
}

/// A file name from its pattern (without the extension). `{drawing}` and `{sheet}` are made safe
/// for a file name; `{n}` is the sheet's place in the run, from 1.
fn render_name(pattern: &str, drawing: &str, sheet: &str, n: usize) -> std::result::Result<String, String> {
    let (drawing, sheet, n) = (file_part(drawing), file_part(sheet), n.to_string());
    fill_tokens("`name`", pattern, &[("drawing", &drawing), ("sheet", &sheet), ("n", &n)])
}

/// Read PUBLISH's parameters, both forms. Everything is checked before any sheet is plotted.
fn publish_request(s: &Session, p: &Value) -> Result<(Vec<SheetRequest>, PublishOutput)> {
    let cmd = "publish";
    let empty = serde_json::Map::new();
    let o = match p {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(bad(cmd, "parameters must be an object")),
    };
    unknown_key(cmd, o, PUBLISH_KEYS, "parameter")?;
    let legacy = o.contains_key("path") || o.contains_key("layouts");
    if legacy && ["sheets", "folder", "name"].iter().any(|k| o.contains_key(*k)) {
        return Err(bad(cmd, "use `sheets`, `folder` and `name`, or the older `path` and `layouts`, not both"));
    }
    let sheets = match o.get("sheets") {
        Some(v) => sheet_list(cmd, s, v)?,
        None => legacy_sheets(cmd, s, o.get("layouts"))?,
    };
    if sheets.is_empty() {
        return Err(bad(cmd, "there are no sheets to publish"));
    }
    if sheets.len() > MAX_SHEETS {
        return Err(bad(cmd, format!("at most {MAX_SHEETS} sheets can be published at once")));
    }
    let to = match o.get("to") {
        None | Some(Value::Null) => "pdf",
        Some(Value::String(t)) if t.eq_ignore_ascii_case("pdf") => "pdf",
        Some(Value::String(t)) if t.eq_ignore_ascii_case("svg") => "svg",
        Some(Value::String(t)) if t.eq_ignore_ascii_case("png") => "png",
        Some(_) => return Err(bad(cmd, "`to` must be \"pdf\", \"svg\" or \"png\"")),
    };
    let (folder, name) = if legacy {
        match str_param(p, "path") {
            Some(path) => {
                let pb = std::path::Path::new(path);
                // The older form names files `NAME-<sheet>.pdf`; braces in NAME stay braces.
                let stem = drawing_stem(path).replace('{', "{{").replace('}', "}}");
                (Some(pb.parent().map(std::path::Path::to_path_buf).unwrap_or_default()), format!("{stem}-{{sheet}}"))
            }
            None if o.get("path").is_some_and(|v| !v.is_null()) => return Err(bad(cmd, "`path` must be a file path")),
            None => (None, DEFAULT_NAME.to_string()),
        }
    } else {
        let folder = match o.get("folder") {
            None | Some(Value::Null) => None,
            Some(Value::String(f)) if !f.trim().is_empty() => Some(std::path::PathBuf::from(f)),
            Some(_) => return Err(bad(cmd, "`folder` must be a folder path")),
        };
        let name = match o.get("name") {
            None | Some(Value::Null) => DEFAULT_NAME.to_string(),
            Some(Value::String(n)) => {
                check_name(cmd, n)?;
                n.clone()
            }
            Some(_) => return Err(bad(cmd, "`name` must be a file name pattern such as \"{drawing}-{sheet}\"")),
        };
        (folder, name)
    };
    let pdf = match o.get("pdf") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(m)) => {
            unknown_key(cmd, m, PDF_KEYS, "pdf setting")?;
            for key in ["layers", "offLayers"] {
                if m.get(key).is_some_and(|v| !v.is_boolean()) {
                    return Err(bad(cmd, format!("`pdf.{key}` must be true or false")));
                }
            }
            if m.get("text").is_some_and(|t| !t.as_str().is_some_and(|t| t.eq_ignore_ascii_case("text") || t.eq_ignore_ascii_case("geometry"))) {
                return Err(bad(cmd, "`pdf.text` must be \"text\" or \"geometry\""));
            }
            m.clone()
        }
        Some(_) => return Err(bad(cmd, "`pdf` must be an object: {layers?, offLayers?, text?}")),
    };
    let copies = match o.get("copies") {
        None | Some(Value::Null) => 1,
        Some(c) => c
            .as_u64()
            .filter(|c| (1..=MAX_COPIES).contains(c))
            .ok_or_else(|| bad(cmd, format!("`copies` must be a whole number from 1 to {MAX_COPIES}")))?,
    };
    // Files are written once; copies are for printers and plotters, which publishing can't send to yet.
    if copies > 1 {
        return Err(bad(cmd, "more than one copy is not available yet: publishing writes files, once each"));
    }
    let combine = match o.get("combine") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`combine` must be true or false")),
    };
    if combine && to != "pdf" {
        return Err(bad(cmd, "only PDFs can be combined: use `to: \"pdf\"`, or one file per sheet for SVG and PNG"));
    }
    if combine && legacy {
        return Err(bad(cmd, "`combine` goes with `sheets`, not the older `path` and `layouts`"));
    }
    // A combined file is named after its first sheet's drawing unless a pattern is given.
    let name = if combine && o.get("name").is_none_or(Value::is_null) { DEFAULT_COMBINED_NAME.to_string() } else { name };
    let status = match o.get("action") {
        None | Some(Value::Null) => false,
        Some(Value::String(a)) if a.eq_ignore_ascii_case("publish") => false,
        Some(Value::String(a)) if a.eq_ignore_ascii_case("status") => true,
        Some(_) => return Err(bad(cmd, "`action` must be \"publish\" or \"status\"")),
    };
    let stamp = match o.get("stamp") {
        Some(v) => stamp_settings(cmd, v)?,
        None => None,
    };
    Ok((sheets, PublishOutput { to, folder, name, pdf, copies, combine, stamp, status }))
}

/// One sheet ready to plot: its drawing, space and plot options.
struct Prepared<'a> {
    /// The drawing as plotted: the snapshot, or a copy with the stamp.
    d: Cow<'a, Drawing>,
    space: Space,
    /// The layout's own name (`Model` for model space).
    layout: String,
    /// The drawing's title, or the file's path.
    title: String,
    opts: Value,
}

/// Drawing files read for PUBLISH without opening them, most recent last: (path, size,
/// modified time, drawing). A file is read again when its size or time changes.
type DetachedCache = Vec<(String, u64, Option<std::time::SystemTime>, Arc<Drawing>)>;
static DETACHED: std::sync::Mutex<DetachedCache> = std::sync::Mutex::new(Vec::new());
/// Drawings kept: enough for a list's status checks to read each file once, without holding
/// many big drawings (by count, and by objects in all; the most recent is always kept).
const DETACHED_KEPT: usize = 16;
const DETACHED_MAX_OBJECTS: usize = 2_000_000;

/// Forget the drawings read for publishing (the Publish Sheets window closed).
pub fn clear_publish_cache() {
    DETACHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// A drawing file read without opening it, from the cache when the file hasn't changed. The
/// Publish Sheets window checks its sheets often; a big file is read once, not on every check.
pub(crate) fn detached(cmd: &str, path: &str) -> Result<Arc<Drawing>> {
    let meta = std::fs::metadata(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if !meta.is_file() {
        return Err(bad(cmd, format!("{path}: not a drawing file")));
    }
    let (len, modified) = (meta.len(), meta.modified().ok());
    {
        let mut cache = DETACHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(i) = cache.iter().position(|(p, l, m, _)| p == path && *l == len && *m == modified) {
            let hit = cache.remove(i);
            let d = hit.3.clone();
            cache.push(hit);
            return Ok(d);
        }
    }
    let d = Arc::new(super::file::read_detached(cmd, path)?);
    let mut cache = DETACHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(p, _, _, _)| p != path);
    cache.push((path.to_string(), len, modified, d.clone()));
    let objects = |c: &DetachedCache| c.iter().map(|(_, _, _, d)| d.entity_count()).fold(0usize, usize::saturating_add);
    while cache.len() > 1 && (cache.len() > DETACHED_KEPT || objects(&cache) > DETACHED_MAX_OBJECTS) {
        cache.remove(0);
    }
    Ok(d)
}

/// The drawing files a run's sheets name, each read once; a file that can't be read keeps its
/// error.
type SheetFiles = BTreeMap<String, std::result::Result<Arc<Drawing>, String>>;

fn read_sheet_files(sheets: &[SheetRequest]) -> SheetFiles {
    let mut files = BTreeMap::new();
    for sheet in sheets {
        if let Source::File(path) = &sheet.source
            && !files.contains_key(path)
        {
            files.insert(path.clone(), detached("publish", path).map_err(|e| error_text(&e)));
        }
    }
    files
}

/// Find a sheet's drawing and layout and build its plot options.
/// Find a sheet's drawing and layout and build its plot options. With `now` (a run, not a
/// status check), the stamp is added for the `n`th sheet.
fn prepare<'a>(
    job: &'a PublishJob,
    files: &'a SheetFiles,
    sheet: &SheetRequest,
    stamp_at: Option<(usize, &(String, String))>,
) -> Result<Prepared<'a>> {
    let cmd = "publish";
    let out = &job.out;
    let (d, title) = match &sheet.source {
        Source::Open(i) => {
            let (title, d) = job.docs.get(*i).ok_or_else(|| bad(cmd, format!("there is no open drawing {i}")))?;
            (&**d, title.clone())
        }
        Source::File(path) => match files.get(path) {
            Some(Ok(d)) => (&**d, path.clone()),
            Some(Err(e)) => return Err(bad(cmd, e.clone())),
            None => return Err(bad(cmd, format!("{path}: the file could not be read"))),
        },
    };
    let (space, layout) = if sheet.layout.eq_ignore_ascii_case("model") {
        if d.model.is_empty() {
            return Err(bad(cmd, "model space is empty: there is nothing to plot"));
        }
        (Space::Model, "Model".to_string())
    } else {
        let l = d.layout(&sheet.layout).ok_or_else(|| bad(cmd, format!("no layout `{}`", sheet.layout)))?;
        if l.entities.is_empty() {
            return Err(bad(cmd, format!("layout `{}` is empty: there is nothing to plot", l.name)));
        }
        (Space::Paper(l.name.clone()), l.name.clone())
    };
    let mut opts = sheet.setup.clone().unwrap_or_default();
    for (k, v) in &out.pdf {
        opts.insert(k.clone(), v.clone());
    }
    opts.insert("output".into(), json!(out.to));
    opts.insert("title".into(), json!(format!("{} - {layout}", drawing_stem(&title))));
    // PLOTTRANSPARENCYOVERRIDE as it was when the run began, else the sheet's own setting.
    match job.transparency.or_else(|| opts.get("plotTransparency").and_then(Value::as_bool)) {
        Some(t) => opts.insert("plotTransparency".into(), json!(t)),
        None => opts.remove("plotTransparency"),
    };
    let d = match (&out.stamp, stamp_at) {
        (Some(stamp), Some((n, (date, time)))) => {
            let text = stamp_text(&stamp.text, &drawing_stem(&title), &layout, n, (date, time)).map_err(|e| bad(cmd, e))?;
            Cow::Owned(stamped(d, &space, sheet.setup.as_ref(), stamp, text))
        }
        _ => Cow::Borrowed(d),
    };
    Ok(Prepared { d, space, layout, title, opts: Value::Object(opts) })
}

/// The file name for a sheet (or the combined file): the pattern, then " (2)", " (3)"… when an
/// earlier file of the run took the name.
fn unique_file(out: &PublishOutput, used: &mut BTreeSet<String>, title: &str, layout: &str, n: usize) -> Result<String> {
    let stem = render_name(&out.name, &drawing_stem(title), layout, n).map_err(|e| bad("publish", e))?;
    let mut file = format!("{stem}.{}", out.to);
    let mut k = 2;
    while used.contains(&file.to_lowercase()) {
        file = format!("{stem} ({k}).{}", out.to);
        k += 1;
    }
    used.insert(file.to_lowercase());
    Ok(file)
}

/// Write `bytes` as `file` in the run's folder, or return them as base64 `data`; adds `path` or
/// `data` to `r`.
fn deliver_file(out: &PublishOutput, file: &str, bytes: &[u8], r: &mut Value) -> Result<()> {
    match &out.folder {
        Some(folder) => {
            let path = folder.join(file).to_string_lossy().to_string();
            super::layout::write_plot_file("publish", &path, bytes)?;
            r["path"] = json!(path);
        }
        None => r["data"] = json!(base64_encode(bytes)),
    }
    Ok(())
}

/// An error's message without the "invalid parameters for `publish`" frame.
fn error_text(e: &EngineError) -> String {
    match e {
        EngineError::BadParams { msg, .. } => msg.clone(),
        e => e.to_string(),
    }
}

/// How a sheet will publish: `ready` (its own page setup, or one changed for this run),
/// `default` (it plots with a default page setup, or ignores part of its own) or `error` (it
/// will fail), with the reason.
fn sheet_status(sh: &Prepared, sheet: &SheetRequest) -> (&'static str, String) {
    if sheet.setup.is_some() {
        return ("ready", "Page setup changed for this run".into());
    }
    let Space::Paper(name) = &sh.space else {
        return ("default", "Model space has no page setup: it is fitted to the default paper".into());
    };
    let Some(page) = sh.d.layout(name).map(|l| &l.page) else {
        return ("error", format!("no layout `{name}`"));
    };
    if *page == cadcraft_doc::PageSetup::default() {
        return ("default", "Uses the default page setup".into());
    }
    let device = page.device.trim();
    if !device.is_empty() && !device.eq_ignore_ascii_case("none") && super::plotters::find_device(device).is_none() {
        return ("default", format!("Plotter \"{device}\" isn't set up on this computer: the sheet is published with its paper size"));
    }
    ("ready", String::new())
}

/// A drawing a run publishes from: an open drawing's title and contents when the run began.
type Docs = Vec<(String, Arc<Drawing>)>;

/// A PUBLISH run, checked and ready: the sheets, the output settings and a snapshot of the open
/// drawings (shared, not copied). It needs no session, so the plotting can run on a worker
/// thread while the drawings stay editable; edits made meanwhile aren't in this run.
pub struct PublishJob {
    sheets: Vec<SheetRequest>,
    out: PublishOutput,
    docs: Docs,
    /// PLOTTRANSPARENCYOVERRIDE when the run began: `Some` forces plot transparency on or off.
    transparency: Option<bool>,
}

impl PublishJob {
    /// Check `publish`'s parameters (as the command takes them) against the session and take the
    /// snapshot. Bad parameters are refused here, before anything is plotted.
    pub fn new(s: &Session, p: &Value) -> Result<PublishJob> {
        let (sheets, out) = publish_request(s, p)?;
        let docs = s.docs.iter().map(|d| (d.title.clone(), d.doc.clone())).collect();
        let transparency = super::super::plotters::plot_transparency(s, &Value::Null);
        Ok(PublishJob { sheets, out, docs, transparency })
    }

    /// How many sheets the run publishes.
    pub fn len(&self) -> usize {
        self.sheets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// The request was `action: "status"`.
    pub fn is_status(&self) -> bool {
        self.out.status
    }

    fn label(&self, sheet: &SheetRequest) -> String {
        match &sheet.source {
            Source::Open(i) => self.docs.get(*i).map_or_else(|| format!("drawing {i}"), |d| d.0.clone()),
            Source::File(path) => path.clone(),
        }
    }

    /// `action: "status"`: what each sheet will do and the files it will make; plots and writes
    /// nothing.
    pub fn status(&self) -> Value {
        let out = &self.out;
        let sheets = &self.sheets;
        let unavailable = if io().and_then(|h| h.plot).is_none() {
            Some("plotting is not available in this build")
        } else if out.combine && plot_book().is_none() {
            Some("combining sheets into one PDF is not available in this build")
        } else {
            None
        };
        let files = read_sheet_files(sheets);
        let mut used = BTreeSet::new();
        let mut results = Vec::with_capacity(sheets.len());
        let mut counts = BTreeMap::from([("ready", 0usize), ("default", 0), ("error", 0)]);
        let mut first: Option<(String, String)> = None;
        let mut page = 0usize;
        for (i, sheet) in sheets.iter().enumerate() {
            let mut r = json!({ "drawing": self.label(sheet), "layout": sheet.layout });
            let (status, reason) = match (unavailable, prepare(self, &files, sheet, None)) {
                (Some(why), _) => ("error", why.to_string()),
                (None, Err(e)) => ("error", error_text(&e)),
                (None, Ok(sh)) => {
                    r["layout"] = json!(sh.layout);
                    if out.combine {
                        page += 1;
                        r["page"] = json!(page);
                        first.get_or_insert_with(|| (sh.title.clone(), sh.layout.clone()));
                    } else if let Ok(file) = unique_file(out, &mut used, &sh.title, &sh.layout, i + 1) {
                        r["file"] = json!(file);
                    }
                    sheet_status(&sh, sheet)
                }
            };
            r["status"] = json!(status);
            if !reason.is_empty() {
                r["reason"] = json!(reason);
            }
            if let Some(n) = counts.get_mut(status) {
                *n += 1;
            }
            results.push(r);
        }
        let count = |k: &str| counts.get(k).copied().unwrap_or(0);
        let mut r = json!({ "action": "status", "sheets": results, "ready": count("ready"), "default": count("default"), "errors": count("error") });
        if let Some((title, layout)) = first
            && let Ok(file) = unique_file(out, &mut used, &title, &layout, 1)
        {
            for sheet in r["sheets"].as_array_mut().into_iter().flatten().filter(|x| x["status"] != "error") {
                sheet["file"] = json!(file);
            }
            r["combined"] = json!({ "file": file, "pages": page });
        }
        r
    }

    /// Plot and write the sheets. `progress(done, total)` is called before each sheet and once at
    /// the end; when it returns false the run stops and the sheets not reached are reported as
    /// cancelled (a combined PDF isn't written). Returns `publish`'s result.
    pub fn run(&self, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<Value> {
        let cmd = "publish";
        let (sheets, out) = (&self.sheets, &self.out);
        let total = sheets.len();
        let plot = io().and_then(|h| h.plot).ok_or_else(|| bad(cmd, "plotting is not available in this build"))?;
        let book = if out.combine {
            Some(plot_book().ok_or_else(|| bad(cmd, "combining sheets into one PDF is not available in this build"))?)
        } else {
            None
        };
        // The folder is made if it doesn't exist yet, as a save dialog's new folder would be.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(folder) = &out.folder {
            std::fs::create_dir_all(folder).map_err(|e| bad(cmd, format!("{}: {e}", folder.display())))?;
        }
        let now = utc_now();
        let files = read_sheet_files(sheets);
        let mut used = BTreeSet::new();
        let mut results = Vec::with_capacity(total);
        let mut failures = Vec::new();
        let mut fail = |sheet: &SheetRequest, error: String, results: &mut Vec<Value>| {
            let drawing = self.label(sheet);
            failures.push(format!("{} ({drawing}): {error}", sheet.layout));
            results.push(json!({ "drawing": drawing, "layout": sheet.layout, "ok": false, "error": error }));
        };
        let mut cancelled = false;
        let mut combined = None;
        match book {
            None => {
                for (i, sheet) in sheets.iter().enumerate() {
                    if cancelled || !progress(i, total) {
                        cancelled = true;
                        fail(sheet, "cancelled".into(), &mut results);
                        continue;
                    }
                    let r = prepare(self, &files, sheet, Some((i + 1, &now))).and_then(|sh| {
                        let bytes = plot(&sh.d, &sh.space, &sh.opts).map_err(|e| bad(cmd, e))?;
                        let file = unique_file(out, &mut used, &sh.title, &sh.layout, i + 1)?;
                        let mut r = json!({ "drawing": sh.title, "layout": sh.layout, "ok": true, "file": file, "bytes": bytes.len(), "pages": 1 });
                        deliver_file(out, &file, &bytes, &mut r)?;
                        Ok(r)
                    });
                    match r {
                        Ok(r) => results.push(r),
                        Err(e) => fail(sheet, error_text(&e), &mut results),
                    }
                }
            }
            Some(book) => {
                // One PDF, a page per sheet that could be found; the others are reported.
                let mut prepared = Vec::new();
                for (i, sheet) in sheets.iter().enumerate() {
                    if !progress(i, total) {
                        return Err(bad(cmd, "publishing was cancelled"));
                    }
                    match prepare(self, &files, sheet, Some((i + 1, &now))) {
                        Ok(sh) => {
                            results.push(json!({ "drawing": sh.title, "layout": sh.layout, "ok": true, "page": prepared.len() + 1 }));
                            prepared.push(sh);
                        }
                        Err(e) => fail(sheet, error_text(&e), &mut results),
                    }
                }
                if let Some(first) = prepared.first() {
                    let (title, layout) = (first.title.clone(), first.layout.clone());
                    let parts: Vec<(&Drawing, Space, Value)> = prepared.iter().map(|sh| (&*sh.d, sh.space.clone(), sh.opts.clone())).collect();
                    let mut file_opts = Value::Object(out.pdf.clone());
                    file_opts["title"] = json!(drawing_stem(&title));
                    let bytes = book(&parts, &file_opts).map_err(|e| bad(cmd, format!("the combined PDF could not be made: {e}")))?;
                    let file = unique_file(out, &mut used, &title, &layout, 1)?;
                    let mut r = json!({ "file": file, "bytes": bytes.len(), "pages": parts.len() });
                    deliver_file(out, &file, &bytes, &mut r)?;
                    for sheet in results.iter_mut().filter(|r| r["ok"] == true) {
                        sheet["file"] = json!(file);
                    }
                    combined = Some(r);
                }
            }
        }
        progress(total, total);
        let published = results.len() - failures.len();
        if published == 0 {
            let shown: Vec<&str> = failures.iter().take(3).map(String::as_str).collect();
            let more = failures.len().saturating_sub(shown.len());
            let tail = if more > 0 { format!("; and {more} more") } else { String::new() };
            return Err(bad(cmd, format!("no sheet was published: {}{tail}", shown.join("; "))));
        }
        let paths: Vec<&str> = match &combined {
            Some(c) => c["path"].as_str().into_iter().collect(),
            None => results.iter().filter_map(|r| r["path"].as_str()).collect(),
        };
        let mut message = if paths.is_empty() {
            format!("Published {published} sheet(s)")
        } else {
            format!("Published {published} sheet(s): {}", paths.join(", "))
        };
        if !failures.is_empty() {
            message.push_str(&format!("; {} failed: {}", failures.len(), failures.join("; ")));
        }
        let mut r = json!({ "sheets": results, "published": published, "failed": failures.len(), "copies": out.copies, "cancelled": cancelled, "message": message });
        if let Some(c) = combined {
            r["combined"] = c;
        }
        // Where the files went, for the report's Reveal button.
        if let Some(folder) = &out.folder {
            r["folder"] = json!(folder.to_string_lossy());
        }
        Ok(r)
    }
}

pub(super) fn run_publish(s: &mut Session, p: &Value) -> Result<Value> {
    let job = PublishJob::new(s, p)?;
    if job.is_status() {
        return Ok(job.status());
    }
    job.run(&mut |_, _| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_filled_and_bad_patterns_refused() {
        let v = [("drawing", "Plan"), ("sheet", "A1")];
        assert_eq!(fill_tokens("x", "{drawing}-{sheet} {{v2}}", &v).as_deref(), Ok("Plan-A1 {v2}"));
        for bad in ["{drawing", "{", "a}", "{who}", "{sheet}{"] {
            assert!(fill_tokens("x", bad, &v).is_err(), "{bad}");
        }
        assert!(fill_tokens("x", "{drawing", &v).is_err_and(|e| e.contains("without its }")));
        // File names make the values safe; the stamp keeps them as they are.
        assert_eq!(render_name("{sheet}", "", "PV/03", 1).as_deref(), Ok("PV_03"));
        assert_eq!(stamp_text("{sheet} {n}", "", "PV/03", 2, ("", "")).as_deref(), Ok("PV/03 2"));
    }

    #[test]
    fn utc_dates_are_right() {
        assert_eq!(utc_at(0), ("1970-01-01".to_string(), "00:00 UTC".to_string()));
        // 2000-02-29 (a leap day) at 12:34.
        assert_eq!(utc_at(951_827_640), ("2000-02-29".to_string(), "12:34 UTC".to_string()));
        assert_eq!(utc_at(1_792_026_000).0, "2026-10-15");
        // Far future: no overflow, still a date.
        assert!(utc_at(u64::MAX).0.len() >= 10);
    }
}
