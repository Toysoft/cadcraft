//! Publish lists: a Publish Sheets run saved to a `.ccpub` file, CADCraft's own JSON format. It
//! names the sheets (drawing, layout, whether it is ticked, a page setup changed for the run)
//! and the output settings. Drawing paths are stored relative to the list's folder, so a folder
//! of drawings and its list can move together; a drawing that is gone is reported as not found.
//!
//! The file is untrusted: its size, the number of sheets and every field are checked, and
//! anything unknown is refused rather than ignored.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};

use super::MAX_SHEETS;
use super::publish::{check_name, setup_override, stamp_settings, unknown_key};
use crate::cmd::bad;
use crate::{Result, Session};

/// What the file says it is.
const KIND: &str = "publish-list";
/// The format version this build writes and reads.
const VERSION: u64 = 1;
/// Largest list file read, in bytes.
const MAX_LIST_BYTES: u64 = 1 << 20;
/// Most sheets in one list (more than one run publishes, so a list can hold unticked ones).
const MAX_LIST_SHEETS: usize = 4 * MAX_SHEETS;
/// Longest drawing path and layout name, in characters.
const MAX_PATH: usize = 4096;
const MAX_LAYOUT: usize = 255;

const FILE_KEYS: &[&str] = &["cadcraft", "version", "sheets", "output"];
const SHEET_KEYS: &[&str] = &["drawing", "layout", "include", "pageSetup"];
const OUTPUT_KEYS: &[&str] = &["to", "combine", "folder", "name", "layers", "stamp"];

pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "publishlist";
    let o = p.as_object().ok_or_else(|| bad(cmd, "parameters must be an object: {action, path, …}"))?;
    let path = o.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).ok_or_else(|| bad(cmd, "`path` (a file path) is required"))?;
    match o.get("action").and_then(Value::as_str) {
        Some("read") => {
            unknown_key(cmd, o, &["action", "path"], "parameter")?;
            read(cmd, path)
        }
        Some("write") => {
            unknown_key(cmd, o, &["action", "path", "sheets", "output"], "parameter")?;
            write(cmd, path, o.get("sheets").unwrap_or(&Value::Null), o.get("output"))
        }
        Some("drawing") => {
            unknown_key(cmd, o, &["action", "path"], "parameter")?;
            drawing(cmd, s, path)
        }
        _ => Err(bad(cmd, "`action` must be \"read\", \"write\" or \"drawing\"")),
    }
}

/// A file path from a list: a non-empty string without control characters.
fn path_text(cmd: &str, v: Option<&Value>, what: &str) -> Result<String> {
    let t = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("{what} must be a file path")))?;
    if t.trim().is_empty() || t.chars().count() > MAX_PATH || t.chars().any(char::is_control) {
        return Err(bad(cmd, format!("{what} must be a file path of 1 to {MAX_PATH} characters")));
    }
    Ok(t.to_string())
}

/// One list entry, checked. The drawing path is returned as written.
fn sheet_entry(cmd: &str, at: usize, v: &Value) -> Result<(String, String, bool, Option<Map<String, Value>>)> {
    let o = v.as_object().ok_or_else(|| bad(cmd, format!("sheet {at} must be an object: {{drawing, layout, include?, pageSetup?}}")))?;
    unknown_key(cmd, o, SHEET_KEYS, "sheet setting")?;
    let drawing = path_text(cmd, o.get("drawing"), &format!("sheet {at}: `drawing`"))?;
    let layout = o
        .get("layout")
        .and_then(Value::as_str)
        .filter(|l| !l.trim().is_empty() && l.chars().count() <= MAX_LAYOUT && !l.chars().any(char::is_control))
        .ok_or_else(|| bad(cmd, format!("sheet {at}: `layout` must be a layout name (or \"Model\") of 1 to {MAX_LAYOUT} characters")))?;
    let include = match o.get("include") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, format!("sheet {at}: `include` must be true or false"))),
    };
    let setup = o.get("pageSetup").filter(|v| !v.is_null()).map(|v| setup_override(cmd, v)).transpose()?;
    Ok((drawing, layout.to_string(), include, setup))
}

/// The output settings, checked.
fn output(cmd: &str, v: Option<&Value>) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(out) };
    let o = v.as_object().ok_or_else(|| bad(cmd, "`output` must be an object: {to?, combine?, folder?, name?, layers?, stamp?}"))?;
    unknown_key(cmd, o, OUTPUT_KEYS, "output setting")?;
    if let Some(to) = o.get("to") {
        let to = to.as_str().map(str::to_ascii_lowercase).filter(|t| ["pdf", "svg", "png"].contains(&t.as_str()));
        out.insert("to".into(), json!(to.ok_or_else(|| bad(cmd, "`output.to` must be \"pdf\", \"svg\" or \"png\""))?));
    }
    for key in ["combine", "layers"] {
        if let Some(b) = o.get(key) {
            out.insert(key.into(), json!(b.as_bool().ok_or_else(|| bad(cmd, format!("`output.{key}` must be true or false")))?));
        }
    }
    if let Some(f) = o.get("folder") {
        out.insert("folder".into(), json!(path_text(cmd, Some(f), "`output.folder`")?));
    }
    if let Some(stamp) = o.get("stamp").filter(|v| !v.is_null()) {
        stamp_settings(cmd, stamp)?;
        out.insert("stamp".into(), stamp.clone());
    }
    if let Some(n) = o.get("name") {
        let n = n.as_str().ok_or_else(|| bad(cmd, "`output.name` must be a file name pattern"))?;
        check_name(cmd, n)?;
        out.insert("name".into(), json!(n));
    }
    Ok(out)
}

/// `path` without `.` and with `..` taken against the part before it (no file system access).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Above the root is the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// A drawing path as written in a list, resolved against the list's folder.
fn resolve(base: &Path, written: &str) -> PathBuf {
    let p = Path::new(written);
    normalize(&if p.is_absolute() { p.to_path_buf() } else { base.join(p) })
}

/// `target` relative to the folder `base`, with `/` between parts, when both are absolute and on
/// the same root; otherwise `target` as it is.
fn relative(base: &Path, target: &Path) -> String {
    let (base, target) = (normalize(base), normalize(target));
    let root = |p: &Path| p.components().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).count();
    if !base.is_absolute() || !target.is_absolute() || base.components().take(root(&base)).ne(target.components().take(root(&target))) {
        return target.to_string_lossy().to_string();
    }
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), b.len().saturating_sub(common)).collect();
    parts.extend(t.iter().skip(common).map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

fn list_folder(path: &str) -> PathBuf {
    Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Read a list. Each sheet's drawing comes back as an absolute path, with whether it was found.
fn read(cmd: &str, path: &str) -> Result<Value> {
    let meta = std::fs::metadata(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if meta.len() > MAX_LIST_BYTES {
        return Err(bad(cmd, format!("{path}: a publish list is at most {} KB", MAX_LIST_BYTES / 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if bytes.len() as u64 > MAX_LIST_BYTES {
        return Err(bad(cmd, format!("{path}: a publish list is at most {} KB", MAX_LIST_BYTES / 1024)));
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| bad(cmd, format!("{path}: not a publish list ({e})")))?;
    let o = v.as_object().ok_or_else(|| bad(cmd, format!("{path}: not a publish list")))?;
    if o.get("cadcraft").and_then(Value::as_str) != Some(KIND) {
        return Err(bad(cmd, format!("{path}: not a publish list")));
    }
    match o.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(n) if n > VERSION => return Err(bad(cmd, format!("{path}: made by a newer CADCraft (list version {n})"))),
        _ => return Err(bad(cmd, format!("{path}: `version` must be {VERSION}"))),
    }
    unknown_key(cmd, o, FILE_KEYS, "list setting")?;
    let entries = o.get("sheets").and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("{path}: `sheets` must be a list")))?;
    if entries.len() > MAX_LIST_SHEETS {
        return Err(bad(cmd, format!("{path}: a publish list holds at most {MAX_LIST_SHEETS} sheets")));
    }
    let base = list_folder(path);
    let mut sheets = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let (drawing, layout, include, setup) = sheet_entry(cmd, i + 1, e)?;
        let full = resolve(&base, &drawing);
        let mut r = json!({ "drawing": full.to_string_lossy(), "layout": layout, "include": include, "found": full.is_file() });
        if let Some(setup) = setup {
            r["pageSetup"] = Value::Object(setup);
        }
        sheets.push(r);
    }
    let mut out = output(cmd, o.get("output"))?;
    if let Some(folder) = out.get("folder").and_then(Value::as_str).map(|f| resolve(&base, f).to_string_lossy().to_string()) {
        out.insert("folder".into(), json!(folder));
    }
    Ok(json!({ "path": path, "sheets": sheets, "output": out }))
}

/// Write a list. Drawing paths (and the output folder) are stored relative to the list's folder.
fn write(cmd: &str, path: &str, sheets: &Value, out: Option<&Value>) -> Result<Value> {
    let entries = sheets.as_array().ok_or_else(|| bad(cmd, "`sheets` must be a list of {drawing, layout, include?, pageSetup?}"))?;
    if entries.len() > MAX_LIST_SHEETS {
        return Err(bad(cmd, format!("a publish list holds at most {MAX_LIST_SHEETS} sheets")));
    }
    let base = list_folder(path);
    let cwd = std::env::current_dir().unwrap_or_default();
    let base_abs = if base.is_absolute() { base } else { cwd.join(&base) };
    let mut written = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let (drawing, layout, include, setup) = sheet_entry(cmd, i + 1, e)?;
        // A relative path given here is relative to the working folder, like any other path.
        let full = if Path::new(&drawing).is_absolute() { PathBuf::from(&drawing) } else { cwd.join(&drawing) };
        let mut r = json!({ "drawing": relative(&base_abs, &full), "layout": layout, "include": include });
        if let Some(setup) = setup {
            r["pageSetup"] = Value::Object(setup);
        }
        written.push(r);
    }
    let mut output = output(cmd, out)?;
    if let Some(folder) = output.get("folder").and_then(Value::as_str).map(|f| relative(&base_abs, &cwd.join(f))) {
        output.insert("folder".into(), json!(folder));
    }
    let file = json!({ "cadcraft": KIND, "version": VERSION, "sheets": written, "output": output });
    let text = serde_json::to_string_pretty(&file).map_err(|e| bad(cmd, e.to_string()))?;
    super::super::layout::write_plot_file(cmd, path, text.as_bytes())?;
    Ok(json!({ "path": path, "sheets": entries.len() }))
}

/// A drawing file's sheets, read without opening it: model space when it has objects, then the
/// layouts in tab order, each with its page setup.
fn drawing(cmd: &str, _s: &Session, path: &str) -> Result<Value> {
    let d = super::publish::detached(cmd, path)?;
    let mut sheets = Vec::new();
    if !d.model.is_empty() {
        sheets.push(json!({ "layout": "Model", "model": true }));
    }
    let mut layouts: Vec<_> = d.layouts.iter().collect();
    layouts.sort_by_key(|l| l.tab_order);
    for l in layouts {
        sheets.push(json!({ "layout": l.name, "model": false, "page": serde_json::to_value(&l.page).unwrap_or(Value::Null) }));
    }
    let title = Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string());
    Ok(json!({ "path": path, "title": title, "sheets": sheets }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_stored_relative_to_the_list() {
        let base = Path::new("/work/sets");
        assert_eq!(relative(base, Path::new("/work/sets/a.dxf")), "a.dxf");
        assert_eq!(relative(base, Path::new("/work/sets/plans/b.dwg")), "plans/b.dwg");
        assert_eq!(relative(base, Path::new("/work/other/c.dxf")), "../other/c.dxf");
        assert_eq!(relative(base, Path::new("/elsewhere/d.dxf")), "../../elsewhere/d.dxf");
        assert_eq!(relative(base, Path::new("/work/sets/./x/../e.dxf")), "e.dxf");
        assert_eq!(relative(base, Path::new("relative.dxf")), "relative.dxf", "a relative drawing path stays as it is");
        assert_eq!(resolve(base, "../other/c.dxf"), PathBuf::from("/work/other/c.dxf"));
        assert_eq!(resolve(base, "/abs/f.dxf"), PathBuf::from("/abs/f.dxf"));
        assert_eq!(resolve(base, "../../../../../g.dxf"), PathBuf::from("/g.dxf"), "`..` stops at the root");
    }
}
