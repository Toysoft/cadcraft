//! IMPORTPDF shares the format reader and drawing merge with OPEN/IMPORT. Hosts install
//! a reader hook just as they install DXF/DWG I/O; programmatic calls never open dialogs.
use super::*;
use cadcraft_doc::Drawing;
use cadcraft_geom::Vec2;
use serde_json::{Value, json};
use std::sync::OnceLock;

pub type ReaderHook = fn(&[u8], u32, bool, bool) -> std::result::Result<(Drawing, Value), String>;
static READER: OnceLock<ReaderHook> = OnceLock::new();
pub type PreviewHook = fn(&[u8], bool) -> std::result::Result<Value, String>;
static PREVIEWER: OnceLock<PreviewHook> = OnceLock::new();
pub fn set_reader(reader: ReaderHook) {
    let _ = READER.set(reader);
}
pub fn set_previewer(previewer: PreviewHook) {
    let _ = PREVIEWER.set(previewer);
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("importpdf","Import Vector PDF...",run)
        .menu(&["File","Import Vector PDF..."]).alias(&["-importpdf", "pdfimport", "-pdfimport"])
        .params("{path | data: base64, name?, page?: 1-based (default 1), units?: mm|m|points (default mm), scale?: positive factor (default 1), at?: [x,y], rotation?: degrees, text?: outlines|editable (default outlines), preview?: bool, thumbnail?: bool, pages?: bool} → {page,pages,widthMm,heightMm,objects,warnings,handles?}; preview:true,pages:true returns all page previews without modifying the drawing")]
}
fn positive(p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && (1e-9..=1e9).contains(n))
            .ok_or_else(|| bad("importpdf", format!("{key} must be a positive number between 1e-9 and 1e9"))),
    }
}
fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let reader = READER.get().ok_or_else(|| bad("importpdf", "PDF import is not available in this build"))?;
    let page = match p.get("page") {
        None => 1,
        Some(v) => v.as_u64().filter(|n| (1..=10_000).contains(n)).ok_or_else(|| bad("importpdf", "page must be an integer from 1 to 10000"))? as u32,
    };
    let units = match str_param(p, "units").unwrap_or("mm") {
        "mm" => 1.0,
        "m" => 0.001,
        "points" => 72.0 / 25.4,
        _ => return Err(bad("importpdf", "units must be mm, m or points")),
    };
    let scale = positive(p, "scale", 1.0)? * units;
    let at = match p.get("at") {
        None => Vec2::ZERO,
        Some(v) => point_value(v)
            .filter(|p| p.is_finite() && p.x.abs().max(p.y.abs()) <= 1e12)
            .ok_or_else(|| bad("importpdf", "at must be a finite point [x,y] within ±1e12"))?,
    };
    let rotation = match p.get("rotation") {
        None => 0.0,
        Some(v) => v
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e6)
            .ok_or_else(|| bad("importpdf", "rotation must be a finite angle within ±1000000 degrees"))?,
    };
    let preview = match p.get("preview") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => return Err(bad("importpdf", "preview must be true or false")),
    };
    let (bytes, name) = source(p)?;
    let thumbnail = match p.get("thumbnail") {
        None => false,
        Some(Value::Bool(v)) => *v,
        _ => return Err(bad("importpdf", "thumbnail must be true or false")),
    };
    let outlines = match str_param(p, "text").unwrap_or("outlines") {
        "outlines" => true,
        "editable" => false,
        _ => return Err(bad("importpdf", "text must be outlines or editable")),
    };
    match p.get("pages") {
        Some(Value::Bool(true)) if preview => {
            let reader = PREVIEWER.get().ok_or_else(|| bad("importpdf", "PDF page overview is not available in this build"))?;
            return reader(&bytes, outlines).map_err(|e| bad("importpdf", e));
        }
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => return Err(bad("importpdf", "pages:true requires preview:true")),
        _ => return Err(bad("importpdf", "pages must be true or false")),
    }
    let (mut drawing, mut info) = reader(&bytes, page, thumbnail && preview, outlines).map_err(|e| bad("importpdf", e))?;
    if rotation != 0.0 {
        let m = cadcraft_geom::Mat3::rotate(rotation.to_radians());
        for handle in drawing.model.handles() {
            drawing.model.modify(handle, |entity| entity.kind.transform(&m));
        }
    }
    // Validate the final placement before modifying the destination drawing.
    for entity in drawing.model.iter() {
        let b = cadcraft_doc::entity_bounds(&drawing, entity, 0);
        for q in [b.min, b.max] {
            let q = q * scale + at;
            if !q.is_finite() || q.x.abs().max(q.y.abs()) > 1e12 {
                return Err(bad("importpdf", "placed PDF geometry exceeds ±1e12"));
            }
        }
    }
    info["units"] = json!(str_param(p, "units").unwrap_or("mm"));
    info["scale"] = json!(scale / units);
    if preview {
        return Ok(info);
    }
    for entity in drawing.model.iter() {
        if s.doc()?.layer(&entity.common.layer).is_some_and(|l| l.locked) {
            return Err(bad("importpdf", format!("destination layer {} is locked", entity.common.layer)));
        }
    }
    let r = super::exchange::import_drawing(s, &drawing, at, scale, &name)?;
    for (key, value) in r.as_object().into_iter().flatten() {
        info[key] = value.clone();
    }
    Ok(info)
}
fn source(p: &Value) -> Result<(Vec<u8>, String)> {
    const MAX: usize = 64 << 20;
    if let Some(path) = str_param(p, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::io::Read;
            let file = std::fs::File::open(path).map_err(|e| bad("importpdf", format!("{path}: {e}")))?;
            if file.metadata().map_err(|e| bad("importpdf", e.to_string()))?.len() > MAX as u64 {
                return Err(bad("importpdf", "PDF exceeds 64 MiB"));
            }
            let mut bytes = Vec::new();
            file.take(MAX as u64 + 1).read_to_end(&mut bytes).map_err(|e| bad("importpdf", e.to_string()))?;
            if bytes.len() > MAX {
                return Err(bad("importpdf", "PDF exceeds 64 MiB"));
            }
            return Ok((bytes, path.into()));
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(bad("importpdf", "paths are unavailable in the browser; pass base64 data"));
        }
    }
    let data = str_param(p, "data").ok_or_else(|| bad("importpdf", "path or base64 data is required"))?;
    if data.len() > MAX / 3 * 4 + 8 {
        return Err(bad("importpdf", "PDF exceeds 64 MiB"));
    }
    let bytes = super::file::base64_decode(data).ok_or_else(|| bad("importpdf", "invalid base64"))?;
    if bytes.len() > MAX {
        return Err(bad("importpdf", "PDF exceeds 64 MiB"));
    }
    Ok((bytes, str_param(p, "name").unwrap_or("import.pdf").into()))
}
