//! PDF 1.7 vector paths → editable CAD entities. Written from ISO 32000-1 (4.3–4.6,
//! 8.3–8.5, 9.4) and the public lopdf API. No renderer, OCR or proprietary converter.
//!
//! Page coordinates are millimetres, with the rotated CropBox lower-left at (0,0).
//! Strokes become polylines and exact cubic splines; fills become editable outlines.
//! Form XObjects, graphics-state transforms and polygon clipping are respected.
use std::collections::BTreeMap;
use std::sync::Arc;

use cadcraft_color::{Color, Rgb};
use cadcraft_doc::{Common, Drawing, EntityKind, Line, Lineweight, LwPolyline, Space, Text};
use cadcraft_geom::{Bounds2, Mat3, PolyVertex, Spline, Vec2, Vec3};
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, Stream, content::Content};
use serde_json::{Value, json};

use crate::{IoError, Result};
mod cmap;

pub const MAX_BYTES: usize = 64 << 20;
const MAX_DECODED: usize = 64 << 20;
const MAX_STREAM: usize = 16 << 20;
const MAX_OPS: usize = 500_000;
const MAX_ENTITIES: usize = 200_000;
const MAX_DEPTH: usize = 64;
const MAX_CLIP_EDGES: usize = 2048;
const MAX_CLIP_WORK: usize = 20_000_000;
const MM_PER_POINT: f64 = 25.4 / 72.0;

fn error(message: impl std::fmt::Display) -> IoError {
    IoError::Format(format!("PDF: {message}"))
}

pub struct Imported {
    pub drawing: Drawing,
    pub page: u32,
    pub pages: usize,
    pub width_mm: f64,
    pub height_mm: f64,
    pub warnings: Vec<String>,
}
impl Imported {
    pub fn info(&self) -> Value {
        json!({"page":self.page,"pages":self.pages,"widthMm":self.width_mm,
            "heightMm":self.height_mm,"objects":self.drawing.model.len(),"warnings":self.warnings})
    }
}

/// Import one page (1-based). The entire page is validated before the caller changes a drawing.
pub fn read(bytes: &[u8], page: u32) -> Result<Imported> {
    read_with(bytes, page, true)
}
pub fn read_with(bytes: &[u8], page: u32, outline_text: bool) -> Result<Imported> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_inner(bytes, page, outline_text)))
        .map_err(|_| error("the PDF reader could not process this file"))?
}
fn read_inner(bytes: &[u8], page: u32, outline_text: bool) -> Result<Imported> {
    let doc = load(bytes)?;
    read_document(&doc, page, outline_text).map(|(imported, _, _)| imported)
}
fn load(bytes: &[u8]) -> Result<Document> {
    if bytes.len() > MAX_BYTES {
        return Err(error("file exceeds 64 MiB"));
    }
    let doc = std::panic::catch_unwind(|| Document::load_mem_with_options(bytes, LoadOptions::with_max_decompressed_size(MAX_STREAM)))
        .map_err(|_| error("the PDF reader could not read this file"))?
        .map_err(error)?;
    if doc.is_encrypted() && doc.encryption_state.is_none() {
        return Err(error("password-protected PDFs are not supported; save an unencrypted copy"));
    }
    if doc.objects.len() > 500_000 {
        return Err(error("too many PDF objects"));
    }
    let pages = doc.get_pages();
    if pages.len() > 10_000 {
        return Err(error("more than 10000 pages"));
    }
    Ok(doc)
}
fn read_document(doc: &Document, page: u32, outline_text: bool) -> Result<(Imported, usize, usize)> {
    if page == 0 {
        return Err(error("page numbers start at 1"));
    }
    let pages = doc.get_pages();
    let id = *pages.get(&page).ok_or_else(|| error(format!("page {page} does not exist ({} pages)", pages.len())))?;
    let media = box_value(inherited(doc, id, b"MediaBox")?.ok_or_else(|| error("page has no MediaBox"))?)?;
    let crop = inherited(doc, id, b"CropBox")?.map(box_value).transpose()?.unwrap_or(media);
    let crop = Bounds2 {
        min: Vec2::new(crop.min.x.max(media.min.x), crop.min.y.max(media.min.y)),
        max: Vec2::new(crop.max.x.min(media.max.x), crop.max.y.min(media.max.y)),
    };
    if crop.width() <= 0.0 || crop.height() <= 0.0 {
        return Err(error("empty page CropBox"));
    }
    let user_unit = doc.get_dictionary(id).map_err(error)?.get(b"UserUnit").ok().map(number).transpose()?.unwrap_or(1.0);
    if !(1e-6..=75_000.0).contains(&user_unit) {
        return Err(error("invalid UserUnit"));
    }
    let rotation = inherited(doc, id, b"Rotate")?.map(number).transpose()?.unwrap_or(0.0);
    if rotation.fract() != 0.0 || rotation.abs() > 1e6 || rotation % 90.0 != 0.0 {
        return Err(error("page rotation must be a multiple of 90 degrees"));
    }
    let rotation = (rotation as i32).rem_euclid(360);
    let (w, h) = (crop.width(), crop.height());
    let turn = match rotation {
        90 => Mat3 { a: 0.0, b: -1.0, c: 1.0, d: 0.0, e: 0.0, f: w },
        180 => Mat3 { a: -1.0, b: 0.0, c: 0.0, d: -1.0, e: w, f: h },
        270 => Mat3 { a: 0.0, b: 1.0, c: -1.0, d: 0.0, e: h, f: 0.0 },
        _ => Mat3::IDENTITY,
    };
    let k = MM_PER_POINT * user_unit;
    let matrix = Mat3::translate(-crop.min).then(turn).then(Mat3::scale(k, k));
    let (width_mm, height_mm) = if rotation == 90 || rotation == 270 { (h * k, w * k) } else { (w * k, h * k) };
    let resources =
        inherited(doc, id, b"Resources")?.map(|o| resolve(doc, o)).transpose()?.and_then(|o| o.as_dict().ok()).cloned().unwrap_or_default();
    let page_dict = doc.get_dictionary(id).map_err(error)?;
    let mut reader = Reader::new(doc, page);
    reader.outline_text = outline_text;
    let content = reader.contents(page_dict.get(b"Contents").ok())?;
    let state =
        State { matrix, clips: vec![Arc::new(Clip::rect(Bounds2 { min: Vec2::ZERO, max: Vec2::new(width_mm, height_mm) }))], ..State::default() };
    reader.operations(&content, &resources, state, 0, &mut Vec::new())?;
    if reader.drawing.model.is_empty() {
        return Err(error("this page has no supported vector geometry or text (a scanned image needs tracing/OCR)"));
    }
    Ok((
        Imported { drawing: reader.drawing, page, pages: pages.len(), width_mm, height_mm, warnings: reader.warnings },
        reader.ops,
        reader.geometry_points,
    ))
}

/// Lightweight previews of every page, loading the PDF only once. Unsupported pages
/// keep their page number and error so later vector pages can still be selected.
pub struct PagePreview {
    pub info: Value,
    pub png: Option<Vec<u8>>,
}
pub fn page_previews(bytes: &[u8], outline_text: bool) -> Result<Vec<PagePreview>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = load(bytes)?;
        let pages = doc.get_pages();
        if pages.len() > 512 {
            return Err(error("page overview is limited to 512 pages"));
        }
        let (mut ops, mut points) = (0usize, 0usize);
        let mut previews = Vec::new();
        for page in pages.keys().copied() {
            match read_document(&doc, page, outline_text) {
                Ok((imported, page_ops, page_points)) => {
                    ops = ops.saturating_add(page_ops);
                    points = points.saturating_add(page_points);
                    if ops > 5_000_000 || points > 10_000_000 {
                        return Err(error("page overview exceeds the PDF analysis budget"));
                    }
                    let png = crate::png(&imported.drawing, &Space::Model, 240, 160)?;
                    previews.push(PagePreview { info: imported.info(), png: Some(png) });
                }
                Err(e) => previews.push(PagePreview { info: json!({"page":page,"error":e.to_string()}), png: None }),
            }
        }
        Ok(previews)
    }))
    .map_err(|_| error("the PDF reader could not analyze this file"))?
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> Result<&'a Object> {
    doc.dereference(object).map(|(_, o)| o).map_err(error)
}
fn inherited<'a>(doc: &'a Document, mut id: ObjectId, key: &[u8]) -> Result<Option<&'a Object>> {
    let mut visited = Vec::new();
    for _ in 0..MAX_DEPTH {
        if visited.contains(&id) {
            return Err(error("cyclic page inheritance"));
        }
        visited.push(id);
        let dict = doc.get_dictionary(id).map_err(error)?;
        if let Ok(o) = dict.get(key) {
            return resolve(doc, o).map(Some);
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(next) => id = next,
            Err(_) => return Ok(None),
        }
    }
    Err(error("page inheritance is too deep"))
}
fn number(o: &Object) -> Result<f64> {
    let n = match o {
        Object::Integer(n) => *n as f64,
        Object::Real(n) => f64::from(*n),
        _ => return Err(error("expected numeric operand")),
    };
    if !n.is_finite() || n.abs() > 1e12 {
        return Err(error("coordinate or parameter is not finite or exceeds 1e12"));
    }
    Ok(n)
}
fn numbers<const N: usize>(ops: &[Object]) -> Result<[f64; N]> {
    if ops.len() != N {
        return Err(error("wrong number of operands"));
    }
    let mut out = [0.0; N];
    for (slot, operand) in out.iter_mut().zip(ops) {
        *slot = number(operand)?;
    }
    Ok(out)
}
fn box_value(o: &Object) -> Result<Bounds2> {
    let a = o.as_array().map_err(error)?;
    let [x0, y0, x1, y1] = numbers(a)?;
    if x0 >= x1 || y0 >= y1 {
        return Err(error("invalid page/form box"));
    }
    Ok(Bounds2 { min: Vec2::new(x0, y0), max: Vec2::new(x1, y1) })
}
fn matrix_value(o: &Object) -> Result<Mat3> {
    let [a, b, c, d, e, f] = numbers(o.as_array().map_err(error)?)?;
    Ok(Mat3 { a, b, c, d, e, f })
}
fn checked_point(p: Vec2) -> Result<Vec2> {
    if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e12 { Err(error("transformed coordinate exceeds ±1e12")) } else { Ok(p) }
}
fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

#[derive(Clone)]
struct TextState {
    matrix: Mat3,
    line: Mat3,
    font: Vec<u8>,
    size: f64,
    char_space: f64,
    word_space: f64,
    horizontal: f64,
    leading: f64,
    rise: f64,
    mode: i32,
}
impl Default for TextState {
    fn default() -> Self {
        Self {
            matrix: Mat3::IDENTITY,
            line: Mat3::IDENTITY,
            font: Vec::new(),
            size: 12.0,
            char_space: 0.0,
            word_space: 0.0,
            horizontal: 1.0,
            leading: 0.0,
            rise: 0.0,
            mode: 0,
        }
    }
}
#[derive(Clone)]
struct State {
    matrix: Mat3,
    stroke: Color,
    fill: Color,
    width: f64,
    clips: Vec<Arc<Clip>>,
    text: TextState,
}
impl Default for State {
    fn default() -> Self {
        Self { matrix: Mat3::IDENTITY, stroke: Color::WHITE, fill: Color::WHITE, width: 1.0, clips: Vec::new(), text: TextState::default() }
    }
}
#[derive(Clone)]
enum Segment {
    Line(Vec2),
    Cubic(Vec2, Vec2, Vec2),
}
#[derive(Clone)]
struct Subpath {
    start: Vec2,
    segments: Vec<Segment>,
    closed: bool,
}
impl Subpath {
    fn end(&self) -> Vec2 {
        self.segments
            .last()
            .map(|s| match s {
                Segment::Line(p) | Segment::Cubic(_, _, p) => *p,
            })
            .unwrap_or(self.start)
    }
    fn polygon(&self) -> Result<Vec<Vec2>> {
        let mut out = vec![self.start];
        let mut p = self.start;
        for s in &self.segments {
            match s {
                Segment::Line(q) => out.push(*q),
                Segment::Cubic(a, b, q) => flatten(p, *a, *b, *q, 0, &mut out, MAX_CLIP_EDGES)?,
            }
            if out.len() > MAX_CLIP_EDGES {
                return Err(error("clipping path has too many vertices"));
            }
            p = match s {
                Segment::Line(p) | Segment::Cubic(_, _, p) => *p,
            };
        }
        if out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        Ok(out)
    }
}
struct Clip {
    polygons: Vec<Vec<Vec2>>,
    even_odd: bool,
    bounds: Bounds2,
    rectangle: bool,
}
impl Clip {
    fn rect(b: Bounds2) -> Self {
        Self {
            polygons: vec![vec![b.min, Vec2::new(b.max.x, b.min.y), b.max, Vec2::new(b.min.x, b.max.y)]],
            even_odd: false,
            bounds: b,
            rectangle: true,
        }
    }
    fn contains(&self, p: Vec2) -> bool {
        if p.x < self.bounds.min.x - 1e-8 || p.y < self.bounds.min.y - 1e-8 || p.x > self.bounds.max.x + 1e-8 || p.y > self.bounds.max.y + 1e-8 {
            return false;
        }
        if self.rectangle {
            return true;
        }
        let mut winding = 0i32;
        for polygon in &self.polygons {
            for (a, b) in edges(polygon) {
                let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                if cross.abs() < 1e-8
                    && p.x >= a.x.min(b.x) - 1e-8
                    && p.x <= a.x.max(b.x) + 1e-8
                    && p.y >= a.y.min(b.y) - 1e-8
                    && p.y <= a.y.max(b.y) + 1e-8
                {
                    return true;
                }
                if a.y <= p.y && b.y > p.y && cross > 0.0 {
                    winding += 1;
                }
                if a.y > p.y && b.y <= p.y && cross < 0.0 {
                    winding -= 1;
                }
            }
        }
        if self.even_odd { winding % 2 != 0 } else { winding != 0 }
    }
}
fn edges(p: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    p.iter().copied().zip(p.iter().copied().cycle().skip(1)).take(p.len())
}
fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}
fn flatten(p: Vec2, a: Vec2, b: Vec2, q: Vec2, depth: usize, out: &mut Vec<Vec2>, limit: usize) -> Result<()> {
    if out.len() >= limit {
        return Err(error("flattened curve has too many vertices"));
    }
    let chord = q - p;
    let len = chord.len();
    let deviation = if len > 1e-12 { cross(a - p, chord).abs().max(cross(b - p, chord).abs()) / len } else { (a - p).len().max((b - p).len()) };
    if deviation <= 0.005 || depth >= 12 {
        out.push(q);
        return Ok(());
    }
    let pa = (p + a) * 0.5;
    let ab = (a + b) * 0.5;
    let bq = (b + q) * 0.5;
    let u = (pa + ab) * 0.5;
    let v = (ab + bq) * 0.5;
    let mid = (u + v) * 0.5;
    flatten(p, pa, u, mid, depth + 1, out, limit)?;
    flatten(mid, v, bq, q, depth + 1, out, limit)
}

struct FontData {
    encoding: OwnedEncoding,
    widths: BTreeMap<u32, f64>,
    outline: Option<PdfFont>,
}
#[derive(Clone)]
struct PdfFont {
    face: cadcraft_fonts::ttf::Face,
    cid_map: Option<Vec<u8>>,
}
impl PdfFont {
    fn glyph(&self, code: u32) -> Result<Arc<[Vec<Vec2>]>> {
        let gid = if let Some(map) = &self.cid_map {
            let offset = usize::try_from(code).ok().and_then(|n| n.checked_mul(2)).ok_or_else(|| error("invalid CID code"))?;
            let a = *map.get(offset).ok_or_else(|| error("CIDToGIDMap is too short"))?;
            let b = *map.get(offset + 1).ok_or_else(|| error("CIDToGIDMap is too short"))?;
            u16::from_be_bytes([a, b])
        } else {
            u16::try_from(code).map_err(error)?
        };
        Ok(cadcraft_fonts::ttf::glyph_contours(&self.face, gid))
    }
}
enum OwnedEncoding {
    Single(Vec<String>),
    Simple(Vec<u8>),
    Unicode(lopdf::Encoding<'static>),
    Mapping(cmap::Mapping),
}
impl OwnedEncoding {
    fn new(e: lopdf::Encoding<'_>) -> Result<Self> {
        match e {
            lopdf::Encoding::UnicodeMapEncoding(map) => Ok(Self::Unicode(lopdf::Encoding::UnicodeMapEncoding(map))),
            lopdf::Encoding::SimpleEncoding(name) => Ok(Self::Simple(name.to_vec())),
            e => {
                let mut table = Vec::with_capacity(256);
                for n in 0..=255u8 {
                    table.push(e.bytes_to_string(&[n]).map_err(error)?);
                }
                Ok(Self::Single(table))
            }
        }
    }
    fn decode(&self, bytes: &[u8]) -> Result<String> {
        let mut out = String::new();
        match self {
            Self::Single(table) => {
                for n in bytes {
                    if let Some(text) = table.get(usize::from(*n)) {
                        out.push_str(text);
                    }
                }
            }
            Self::Simple(name) => return lopdf::Encoding::SimpleEncoding(name).bytes_to_string(bytes).map_err(error),
            Self::Mapping(map) => {
                let mut code = 0u32;
                let mut len = 0u8;
                for b in bytes {
                    code = code.wrapping_mul(256).wrapping_add(u32::from(*b));
                    len += 1;
                    if let Some(value) = map.get(&(code, len)) {
                        out.push_str(value);
                        code = 0;
                        len = 0;
                    } else if len == 4 {
                        out.push('\u{fffd}');
                        code = 0;
                        len = 0;
                    }
                    if out.len() > 1 << 20 {
                        return Err(error("decoded text is too long"));
                    }
                }
                if len > 0 {
                    out.push('\u{fffd}');
                }
            }
            Self::Unicode(lopdf::Encoding::UnicodeMapEncoding(map)) => {
                let mut code = 0u32;
                let mut len = 0u8;
                for n in bytes {
                    code = code.wrapping_mul(256).wrapping_add(u32::from(*n));
                    len += 1;
                    if let Some(chars) = map.get(code, len) {
                        if chars.len() > 256 {
                            return Err(error("one glyph maps to too much text"));
                        }
                        out.push_str(&String::from_utf16_lossy(&chars));
                        code = 0;
                        len = 0;
                    } else if len == 4 {
                        out.push('\u{fffd}');
                        code = 0;
                        len = 0;
                    }
                    if out.len() > 1 << 20 {
                        return Err(error("decoded text is too long"));
                    }
                }
                if len > 0 {
                    out.push('\u{fffd}');
                }
            }
            Self::Unicode(_) => return Err(error("invalid cached font encoding")),
        }
        Ok(out)
    }
}
struct Reader<'a> {
    doc: &'a Document,
    drawing: Drawing,
    layer: String,
    warnings: Vec<String>,
    decoded: usize,
    ops: usize,
    clip_work: usize,
    fonts: BTreeMap<usize, FontData>,
    font_work: usize,
    outline_text: bool,
    geometry_points: usize,
    font_memory: usize,
}
impl<'a> Reader<'a> {
    fn new(doc: &'a Document, page: u32) -> Self {
        let mut drawing = Drawing::new_metric();
        let layer = format!("PDF_PAGE_{page}");
        drawing.layers.push(cadcraft_doc::Layer::new(&layer));
        Self {
            doc,
            drawing,
            layer,
            warnings: Vec::new(),
            decoded: 0,
            ops: 0,
            clip_work: 0,
            fonts: BTreeMap::new(),
            font_work: 0,
            outline_text: true,
            geometry_points: 0,
            font_memory: 0,
        }
    }
    fn warn(&mut self, s: &str) {
        if !self.warnings.iter().any(|w| w == s) {
            self.warnings.push(s.into());
        }
    }
    fn stream(&mut self, stream: &Stream) -> Result<Vec<u8>> {
        let remaining = MAX_DECODED.saturating_sub(self.decoded).min(MAX_STREAM);
        let bytes = if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(remaining).map_err(error)?
        } else {
            if stream.content.len() > remaining {
                return Err(error("decoded content exceeds the import limit"));
            }
            stream.content.clone()
        };
        self.decoded = self.decoded.saturating_add(bytes.len());
        Ok(bytes)
    }
    fn contents(&mut self, o: Option<&Object>) -> Result<Vec<u8>> {
        let Some(o) = o else { return Ok(Vec::new()) };
        let o = resolve(self.doc, o)?;
        let mut out = Vec::new();
        let streams: Vec<&Object> = match o {
            Object::Array(a) => a.iter().collect(),
            _ => vec![o],
        };
        if streams.len() > 10_000 {
            return Err(error("too many page content streams"));
        }
        for obj in streams {
            let stream = resolve(self.doc, obj)?.as_stream().map_err(error)?;
            let part = self.stream(stream)?;
            out.extend(part);
            out.push(b'\n');
        }
        Ok(out)
    }
    fn add(&mut self, kind: EntityKind, color: Color, width: f64) -> Result<()> {
        let points = match &kind {
            EntityKind::LwPolyline(p) => p.vertices.len(),
            EntityKind::Spline(s) => s.control.len(),
            _ => 2,
        };
        self.geometry_points = self.geometry_points.saturating_add(points);
        if self.geometry_points > 2_000_000 {
            return Err(error("more than 2000000 imported geometry vertices"));
        }
        if self.drawing.model.len() >= MAX_ENTITIES {
            return Err(error("more than 200000 imported entities"));
        }
        let common = Common {
            layer: self.layer.clone(),
            color,
            lineweight: Lineweight::Mm100((width.clamp(0.0, 2.11) * 100.0).round() as u16),
            ..Common::default()
        };
        self.drawing.add(&Space::Model, common, kind).map(|_| ()).map_err(error)
    }
    fn resource<'b>(&self, res: &'b Dictionary, category: &[u8], name: &[u8]) -> Result<Option<&'b Object>>
    where
        'a: 'b,
    {
        let Ok(category) = res.get(category) else { return Ok(None) };
        let dict = resolve(self.doc, category)?.as_dict().map_err(error)?;
        dict.get(name).ok().map(|o| resolve(self.doc, o)).transpose()
    }
    fn operations(&mut self, bytes: &[u8], res: &Dictionary, mut state: State, depth: usize, active: &mut Vec<ObjectId>) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(error("Form XObjects are nested too deeply"));
        }
        let content = Content::decode_strict(bytes).map_err(error)?;
        self.ops = self.ops.saturating_add(content.operations.len());
        if self.ops > MAX_OPS {
            return Err(error("more than 500000 drawing operations"));
        }
        let mut saved = Vec::new();
        let mut paths: Vec<Subpath> = Vec::new();
        let mut clip_rule = None;
        let mut path_segments = 0usize;
        let mut compatibility = 0usize;
        for op in content.operations {
            let p = &op.operands;
            match op.operator.as_str() {
                "q" => {
                    if saved.len() >= MAX_DEPTH {
                        return Err(error("graphics-state stack is too deep"));
                    }
                    saved.push(state.clone());
                }
                "Q" => state = saved.pop().ok_or_else(|| error("unbalanced graphics-state restore"))?,
                "cm" => {
                    let [a, b, c, d, e, f] = numbers(p)?;
                    state.matrix = Mat3 { a, b, c, d, e, f }.then(state.matrix);
                }
                "w" => {
                    let [w] = numbers(p)?;
                    if w < 0.0 {
                        return Err(error("negative line width"));
                    }
                    state.width = w;
                }
                "G" | "g" | "RG" | "rg" | "K" | "k" => {
                    let c = color(p)?;
                    if op.operator.as_bytes().first().is_some_and(u8::is_ascii_uppercase) {
                        state.stroke = c;
                    } else {
                        state.fill = c;
                    }
                }
                "m" => {
                    let [x, y] = numbers(p)?;
                    paths.push(Subpath { start: checked_point(state.matrix.apply(Vec2::new(x, y)))?, segments: Vec::new(), closed: false });
                }
                "l" => {
                    let [x, y] = numbers(p)?;
                    let q = checked_point(state.matrix.apply(Vec2::new(x, y)))?;
                    current(&mut paths)?.segments.push(Segment::Line(q));
                    current(&mut paths)?.closed = false;
                    path_segments += 1;
                }
                "c" | "v" | "y" => {
                    let q = current(&mut paths)?.end();
                    let (a, b, end) = match op.operator.as_str() {
                        "c" => {
                            let [a, b, c, d, e, f] = numbers(p)?;
                            (Vec2::new(a, b), Vec2::new(c, d), Vec2::new(e, f))
                        }
                        "v" => {
                            let [c, d, e, f] = numbers(p)?;
                            (Vec2::ZERO, Vec2::new(c, d), Vec2::new(e, f))
                        }
                        _ => {
                            let [a, b, e, f] = numbers(p)?;
                            (Vec2::new(a, b), Vec2::new(e, f), Vec2::new(e, f))
                        }
                    };
                    let a = if op.operator == "v" { q } else { checked_point(state.matrix.apply(a))? };
                    let b = checked_point(state.matrix.apply(b))?;
                    let end = checked_point(state.matrix.apply(end))?;
                    current(&mut paths)?.segments.push(Segment::Cubic(a, b, end));
                    current(&mut paths)?.closed = false;
                    path_segments += 1;
                }
                "h" => {
                    let path = current(&mut paths)?;
                    if path.end() != path.start {
                        path.segments.push(Segment::Line(path.start));
                        path_segments += 1;
                    }
                    path.closed = true;
                }
                "re" => {
                    let [x, y, w, h] = numbers(p)?;
                    let points = [Vec2::new(x, y), Vec2::new(x + w, y), Vec2::new(x + w, y + h), Vec2::new(x, y + h)]
                        .map(|q| checked_point(state.matrix.apply(q)));
                    let [a, b, c, d] = points;
                    let a = a?;
                    paths.push(Subpath {
                        start: a,
                        segments: vec![Segment::Line(b?), Segment::Line(c?), Segment::Line(d?), Segment::Line(a)],
                        closed: true,
                    });
                    path_segments += 4;
                }
                "W" => clip_rule = Some(false),
                "W*" => clip_rule = Some(true),
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    let stroke = matches!(op.operator.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*");
                    let fill = matches!(op.operator.as_str(), "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    if matches!(op.operator.as_str(), "s" | "b" | "b*")
                        && let Some(last) = paths.last_mut()
                    {
                        last.closed = true;
                    }
                    if stroke || fill {
                        self.paint(&paths, &state, stroke, fill)?;
                    }
                    if let Some(even_odd) = clip_rule.take() {
                        self.clip(&paths, even_odd, &mut state)?;
                    }
                    paths.clear();
                    path_segments = 0;
                }
                "Do" => {
                    let name = p.first().ok_or_else(|| error("missing XObject name"))?.as_name().map_err(error)?;
                    let category = res.get(b"XObject").map_err(error)?;
                    let dict = resolve(self.doc, category)?.as_dict().map_err(error)?;
                    let object = dict.get(name).map_err(error)?;
                    let id = object.as_reference().ok();
                    let stream = resolve(self.doc, object)?.as_stream().map_err(error)?;
                    match stream.dict.get(b"Subtype").and_then(Object::as_name).map_err(error)? {
                        b"Image" => self.warn("Raster images are skipped; this import converts vector paths and text only."),
                        b"Form" => {
                            if let Some(id) = id {
                                if active.contains(&id) {
                                    return Err(error("cyclic Form XObject"));
                                }
                                active.push(id);
                            }
                            let mut child = state.clone();
                            if let Ok(m) = stream.dict.get(b"Matrix") {
                                child.matrix = matrix_value(resolve(self.doc, m)?)?.then(child.matrix);
                            }
                            if let Ok(b) = stream.dict.get(b"BBox") {
                                let bounds = box_value(resolve(self.doc, b)?)?;
                                let corners = [bounds.min, Vec2::new(bounds.max.x, bounds.min.y), bounds.max, Vec2::new(bounds.min.x, bounds.max.y)];
                                let mut path = Subpath { start: checked_point(child.matrix.apply(corners[0]))?, segments: Vec::new(), closed: true };
                                for p in corners.iter().skip(1) {
                                    path.segments.push(Segment::Line(checked_point(child.matrix.apply(*p))?));
                                }
                                self.clip(&[path], false, &mut child)?;
                            }
                            let child_res = stream
                                .dict
                                .get(b"Resources")
                                .ok()
                                .map(|o| resolve(self.doc, o))
                                .transpose()?
                                .map(|o| o.as_dict().map_err(error))
                                .transpose()?
                                .unwrap_or(res);
                            let decoded = self.stream(stream)?;
                            self.operations(&decoded, child_res, child, depth + 1, active)?;
                            if id.is_some() {
                                active.pop();
                            }
                        }
                        _ => self.warn("Unsupported XObjects are skipped."),
                    }
                }
                "BT" => {
                    state.text.matrix = Mat3::IDENTITY;
                    state.text.line = Mat3::IDENTITY;
                }
                "Tf" => {
                    if p.len() != 2 {
                        return Err(error("invalid font selection"));
                    }
                    state.text.font = p.first().ok_or_else(|| error("missing font"))?.as_name().map_err(error)?.to_vec();
                    state.text.size = number(p.get(1).ok_or_else(|| error("missing font size"))?)?;
                }
                "Tm" => {
                    let [a, b, c, d, e, f] = numbers(p)?;
                    state.text.matrix = Mat3 { a, b, c, d, e, f };
                    state.text.line = state.text.matrix;
                }
                "Td" | "TD" => {
                    let [x, y] = numbers(p)?;
                    if op.operator == "TD" {
                        state.text.leading = -y;
                    }
                    text_move(&mut state.text, x, y);
                }
                "T*" => {
                    let leading = state.text.leading;
                    text_move(&mut state.text, 0.0, -leading);
                }
                "Tc" | "Tw" | "Tz" | "TL" | "Ts" | "Tr" => {
                    let [v] = numbers(p)?;
                    match op.operator.as_str() {
                        "Tc" => state.text.char_space = v,
                        "Tw" => state.text.word_space = v,
                        "Tz" => state.text.horizontal = v / 100.0,
                        "TL" => state.text.leading = v,
                        "Ts" => state.text.rise = v,
                        _ => state.text.mode = v as i32,
                    };
                }
                "Tj" => {
                    let s = p.first().ok_or_else(|| error("missing text"))?.as_str().map_err(error)?;
                    self.text(s, res, &mut state)?;
                }
                "TJ" => {
                    let a = p.first().ok_or_else(|| error("missing text array"))?.as_array().map_err(error)?;
                    if a.len() > 100_000 {
                        return Err(error("text array is too large"));
                    }
                    for o in a {
                        if let Ok(s) = o.as_str() {
                            self.text(s, res, &mut state)?;
                        } else {
                            let dx = -number(o)? * state.text.size * state.text.horizontal / 1000.0;
                            state.text.matrix = Mat3::translate(Vec2::new(dx, 0.0)).then(state.text.matrix);
                        }
                    }
                }
                "'" | "\"" => {
                    let lead = state.text.leading;
                    text_move(&mut state.text, 0.0, -lead);
                    if op.operator == "\"" {
                        if p.len() != 3 {
                            return Err(error("invalid text spacing"));
                        }
                        state.text.word_space = number(p.first().ok_or_else(|| error("missing spacing"))?)?;
                        state.text.char_space = number(p.get(1).ok_or_else(|| error("missing spacing"))?)?;
                    }
                    let s = p.last().ok_or_else(|| error("missing text"))?.as_str().map_err(error)?;
                    self.text(s, res, &mut state)?;
                }
                "gs" => self.warn("PDF transparency, soft masks and special graphics-state effects are not reproduced."),
                "sh" => self.warn("Shadings and gradients are skipped."),
                "BI" | "ID" | "EI" => self.warn("Inline raster images are skipped."),
                "CS" | "cs" | "SC" | "SCN" | "sc" | "scn" => self.warn("Special colour spaces and patterns are simplified to the current colour."),
                "d" => self.warn("Dashed strokes become continuous editable paths."),
                // Painting-independent metadata, text markers, line caps and joins.
                "ET" | "j" | "J" | "M" | "ri" | "i" | "BMC" | "BDC" | "EMC" | "MP" | "DP" => {}
                "BX" => {
                    compatibility += 1;
                    if compatibility > MAX_DEPTH {
                        return Err(error("compatibility sections are too deep"));
                    }
                }
                "EX" => {
                    compatibility = compatibility.checked_sub(1).ok_or_else(|| error("unbalanced compatibility section"))?;
                }
                _ if compatibility > 0 => {
                    self.warn("Some unsupported PDF compatibility operators were skipped; compare the result with the original page.")
                }
                _ => return Err(error(format!("unsupported or damaged content operator {}", op.operator))),
            }
            if paths.len() > 10_000 || path_segments > 100_000 {
                return Err(error("one PDF path is too large"));
            }
        }
        Ok(())
    }
    fn clip(&mut self, paths: &[Subpath], even_odd: bool, state: &mut State) -> Result<()> {
        if state.clips.len() >= MAX_DEPTH {
            return Err(error("too many clipping paths"));
        }
        let mut polygons = Vec::new();
        let mut vertices = 0usize;
        for path in paths {
            let polygon = path.polygon()?;
            vertices = vertices.saturating_add(polygon.len());
            if vertices > MAX_CLIP_EDGES {
                return Err(error("clipping path has more than 2048 edges"));
            }
            polygons.push(polygon);
        }
        if polygons.iter().map(Vec::len).sum::<usize>() > MAX_CLIP_EDGES {
            return Err(error("clipping path has more than 2048 edges"));
        }
        let mut bounds = Bounds2::EMPTY;
        for p in polygons.iter().flatten() {
            bounds.add(*p);
        }
        let rectangle = polygons.len() == 1 && polygons.first().is_some_and(|p| p.len() == 4 && edges(p).all(|(a, b)| a.x == b.x || a.y == b.y));
        state.clips.push(Arc::new(Clip { polygons, even_odd, bounds, rectangle }));
        Ok(())
    }
    fn inside(&self, p: Vec2, state: &State) -> bool {
        state.clips.iter().all(|c| c.contains(p))
    }
    fn line(&mut self, a: Vec2, b: Vec2, state: &State, color: Color, width: f64) -> Result<()> {
        if (b - a).len() < 1e-12 {
            return Ok(());
        }
        let mut cuts = vec![0.0, 1.0];
        for clip in &state.clips {
            if clip.rectangle && clip.contains(a) && clip.contains(b) {
                continue;
            }
            for polygon in &clip.polygons {
                for (c, d) in edges(polygon) {
                    self.clip_work = self.clip_work.saturating_add(1);
                    if self.clip_work > MAX_CLIP_WORK {
                        return Err(error("clipping is too complex for this import"));
                    }
                    let den = cross(b - a, d - c);
                    if den.abs() > 1e-15 {
                        let t = cross(c - a, d - c) / den;
                        let u = cross(c - a, b - a) / den;
                        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                            cuts.push(t);
                        }
                    }
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
        for pair in cuts.windows(2) {
            let Some((&lo, &hi)) = pair.first().zip(pair.get(1)) else { continue };
            if hi - lo > 1e-10 && self.inside(a + (b - a) * ((lo + hi) * 0.5), state) {
                self.add(EntityKind::Line(Line { a: v3(a + (b - a) * lo), b: v3(a + (b - a) * hi) }), color, width)?;
            }
        }
        Ok(())
    }
    fn paint(&mut self, paths: &[Subpath], state: &State, stroke: bool, fill: bool) -> Result<()> {
        if fill {
            self.warn("Filled areas are imported as editable boundary outlines, not solid hatches.");
        }
        let color = if stroke { state.stroke } else { state.fill };
        let width = if stroke { state.width * state.matrix.scale_factor() } else { 0.0 };
        for path in paths {
            let all_inside = state.clips.iter().all(|c| c.rectangle)
                && self.inside(path.start, state)
                && path.segments.iter().all(|s| match s {
                    Segment::Line(p) => self.inside(*p, state),
                    Segment::Cubic(a, b, p) => self.inside(*a, state) && self.inside(*b, state) && self.inside(*p, state),
                });
            if all_inside && path.segments.iter().all(|s| matches!(s, Segment::Line(_))) && !path.segments.is_empty() {
                let mut vertices = vec![PolyVertex::new(path.start)];
                for s in &path.segments {
                    if let Segment::Line(p) = s {
                        vertices.push(PolyVertex::new(*p));
                    }
                }
                self.add(
                    EntityKind::LwPolyline(LwPolyline { vertices, closed: path.closed || fill, const_width: 0.0, elevation: 0.0, plinegen: false }),
                    color,
                    width,
                )?;
                continue;
            }
            let mut p = path.start;
            for s in &path.segments {
                match s {
                    Segment::Line(q) => self.line(p, *q, state, color, width)?,
                    Segment::Cubic(a, b, q) if all_inside => {
                        self.add(EntityKind::Spline(Spline::from_control(vec![p, *a, *b, *q], 3)), color, width)?
                    }
                    Segment::Cubic(a, b, q) => {
                        let mut pts = vec![p];
                        flatten(p, *a, *b, *q, 0, &mut pts, 100_000)?;
                        for pair in pts.windows(2) {
                            if let Some((a, b)) = pair.first().zip(pair.get(1)) {
                                self.line(*a, *b, state, color, width)?;
                            }
                        }
                    }
                }
                p = match s {
                    Segment::Line(p) | Segment::Cubic(_, _, p) => *p,
                };
            }
            if path.closed || fill {
                self.line(p, path.start, state, color, width)?;
            }
        }
        Ok(())
    }
    fn text(&mut self, bytes: &[u8], resources: &Dictionary, state: &mut State) -> Result<()> {
        if bytes.len() > 65536 {
            return Err(error("one text string is too long"));
        }
        let font = self.resource(resources, b"Font", &state.text.font)?.ok_or_else(|| error("text font is missing"))?.as_dict().map_err(error)?;
        let subtype = font.get(b"Subtype").and_then(Object::as_name).unwrap_or_default();
        if subtype == b"Type3" {
            self.warn("Type3 font glyphs are skipped.");
            return Ok(());
        }
        let composite = subtype == b"Type0";
        if composite && font.get(b"Encoding").and_then(Object::as_name).is_ok_and(|n| n.ends_with(b"-V")) {
            self.warn("Vertical PDF text is skipped.");
            return Ok(());
        }
        let key = font as *const Dictionary as usize;
        if !self.fonts.contains_key(&key) {
            if self.fonts.len() >= 512 {
                return Err(error("more than 512 text fonts"));
            }
            let encoding = if let Ok(unicode) = font.get(b"ToUnicode") {
                let stream = resolve(self.doc, unicode)?.as_stream().map_err(error)?;
                let bytes = self.stream(stream)?;
                OwnedEncoding::Mapping(cmap::parse(&bytes)?)
            } else {
                OwnedEncoding::new(font.get_font_encoding_with_limit(self.doc, 1 << 20).map_err(error)?)?
            };
            let outline = if self.outline_text && composite { self.embedded_font(font)? } else { None };
            let cost = match &encoding {
                OwnedEncoding::Mapping(map) => map.values().map(|s| s.len().saturating_add(64)).sum(),
                OwnedEncoding::Single(table) => table.iter().map(|s| s.len().saturating_add(32)).sum(),
                _ => 1 << 20,
            };
            self.font_memory = self.font_memory.saturating_add(cost);
            if self.font_memory > 32 << 20 {
                return Err(error("font mapping cache exceeds 32 MiB"));
            }
            self.fonts.insert(key, FontData { encoding, widths: BTreeMap::new(), outline });
        }
        let value = self.fonts.get(&key).ok_or_else(|| error("font cache is unavailable"))?.encoding.decode(bytes)?;
        let mut advance = 0.0;
        let codes: Vec<u32> = if composite {
            bytes.chunks(2).map(|s| u32::from(s.first().copied().unwrap_or(0)) * 256 + u32::from(s.get(1).copied().unwrap_or(0))).collect()
        } else {
            bytes.iter().map(|b| u32::from(*b)).collect()
        };
        let outlines = self.fonts.get(&key).and_then(|f| f.outline.clone());
        let text_matrix = state.text.matrix.then(state.matrix);
        for code in codes {
            let cached = self.fonts.get(&key).and_then(|f| f.widths.get(&code)).copied();
            let width = match cached {
                Some(w) => w,
                None => {
                    let w = font_width(self.doc, font, code, composite, &mut self.font_work)?;
                    if let Some(font) = self.fonts.get_mut(&key) {
                        font.widths.insert(code, w);
                    }
                    w
                }
            };
            if let Some(font) = &outlines
                && state.text.mode != 3
                && state.text.mode < 4
            {
                let contours = font.glyph(code)?;
                let mut paths = Vec::new();
                let mut points = 0usize;
                for contour in contours.iter() {
                    points = points.saturating_add(contour.len());
                    if points > 100_000 {
                        return Err(error("glyph outline is too complex"));
                    }
                    let transform = |p: Vec2| {
                        checked_point(text_matrix.apply(Vec2::new(
                            p.x * state.text.size * state.text.horizontal / 1000.0 + advance,
                            p.y * state.text.size / 1000.0 + state.text.rise,
                        )))
                    };
                    if let Some(first) = contour.first() {
                        let mut path = Subpath { start: transform(*first)?, segments: Vec::new(), closed: true };
                        for p in contour.iter().skip(1) {
                            path.segments.push(Segment::Line(transform(*p)?));
                        }
                        paths.push(path);
                    }
                }
                self.paint(&paths, state, matches!(state.text.mode, 1 | 2), state.text.mode != 1)?;
            }
            advance +=
                (width / 1000.0 * state.text.size + state.text.char_space + if code == 32 && !composite { state.text.word_space } else { 0.0 })
                    * state.text.horizontal;
        }
        let base = checked_point(text_matrix.apply(Vec2::new(0.0, state.text.rise)))?;
        let ex = text_matrix.apply_vec(Vec2::new(state.text.size * state.text.horizontal, 0.0));
        let ey = text_matrix.apply_vec(Vec2::new(0.0, state.text.size));
        let height = ey.len();
        if !(0.0..=1e9).contains(&height) || !advance.is_finite() {
            return Err(error("invalid text transform"));
        }
        if state.text.mode >= 4 {
            self.warn("Text clipping is not reproduced; clipped glyphs are skipped.");
        }
        if outlines.is_none() && state.text.mode != 3 && state.text.mode < 4 && height > 1e-10 && !value.trim().is_empty() {
            if self.outline_text {
                self.warn("This font has no supported embedded TrueType outlines; its text uses the drawing's Standard font instead.");
            }
            let end = checked_point(text_matrix.apply(Vec2::new(advance, state.text.rise)))?;
            if self.inside(base, state) && self.inside(end, state) {
                if cross(ex, ey) < 0.0 {
                    self.warn("Mirrored text is skipped.");
                } else {
                    self.warn("PDF text uses the drawing's Standard font; original font metrics may differ.");
                    self.add(
                        EntityKind::Text(Text {
                            insert: v3(base),
                            align_pt: None,
                            height,
                            value,
                            rotation: ex.y.atan2(ex.x),
                            width_factor: (ex.len() / height).clamp(1e-6, 1e6),
                            oblique: 0.0,
                            style: "Standard".into(),
                            halign: Default::default(),
                            valign: Default::default(),
                        }),
                        state.fill,
                        0.0,
                    )?;
                }
            }
        }
        state.text.matrix = Mat3::translate(Vec2::new(advance, 0.0)).then(state.text.matrix);
        Ok(())
    }
    fn embedded_font(&mut self, font: &Dictionary) -> Result<Option<PdfFont>> {
        let Some(descendant) = font.get(b"DescendantFonts").ok().and_then(|o| o.as_array().ok()).and_then(|a| a.first()) else { return Ok(None) };
        let font = resolve(self.doc, descendant)?.as_dict().map_err(error)?;
        if font.get(b"Subtype").and_then(Object::as_name).unwrap_or_default() != b"CIDFontType2" {
            return Ok(None);
        }
        let Some(descriptor) = font.get(b"FontDescriptor").ok() else {
            return Ok(None);
        };
        let descriptor = resolve(self.doc, descriptor)?.as_dict().map_err(error)?;
        let Some(file) = descriptor.get(b"FontFile2").ok() else { return Ok(None) };
        let stream = resolve(self.doc, file)?.as_stream().map_err(error)?;
        let bytes = self.stream(stream)?;
        let Some(face) = cadcraft_fonts::ttf::Face::new(Arc::new(bytes), 0) else { return Err(error("invalid embedded TrueType font")) };
        let cid_map = match font.get(b"CIDToGIDMap") {
            Ok(o) => match resolve(self.doc, o)? {
                Object::Stream(stream) => Some(self.stream(stream)?),
                Object::Name(name) if name == b"Identity" => None,
                _ => return Err(error("unsupported CIDToGIDMap")),
            },
            Err(_) => None,
        };
        Ok(Some(PdfFont { face, cid_map }))
    }
}
fn current(paths: &mut [Subpath]) -> Result<&mut Subpath> {
    paths.last_mut().ok_or_else(|| error("path segment without a move-to"))
}
fn text_move(state: &mut TextState, x: f64, y: f64) {
    state.line = Mat3::translate(Vec2::new(x, y)).then(state.line);
    state.matrix = state.line;
}
fn color(operands: &[Object]) -> Result<Color> {
    let byte = |n: f64| (n.clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgb = match operands.len() {
        1 => {
            let [g] = numbers(operands)?;
            Rgb(byte(g), byte(g), byte(g))
        }
        3 => {
            let [r, g, b] = numbers(operands)?;
            Rgb(byte(r), byte(g), byte(b))
        }
        4 => {
            let [c, m, y, k] = numbers(operands)?;
            Rgb(byte(1.0 - (c + k).min(1.0)), byte(1.0 - (m + k).min(1.0)), byte(1.0 - (y + k).min(1.0)))
        }
        _ => return Err(error("invalid colour operands")),
    };
    Ok(if rgb == Rgb(0, 0, 0) || rgb == Rgb(255, 255, 255) { Color::WHITE } else { Color::True(rgb) })
}
fn font_width(doc: &Document, font: &Dictionary, code: u32, composite: bool, work: &mut usize) -> Result<f64> {
    if !composite {
        let first = font.get(b"FirstChar").and_then(Object::as_i64).unwrap_or(0);
        if let Ok(widths) = font.get(b"Widths") {
            let widths = resolve(doc, widths)?.as_array().map_err(error)?;
            if let Some(i) = i64::from(code).checked_sub(first).and_then(|i| usize::try_from(i).ok())
                && let Some(w) = widths.get(i)
            {
                return number(w);
            }
        }
        // PDF standard fonts have implicit metrics; Courier is exactly 600 units.
        return Ok(if font.get(b"BaseFont").and_then(Object::as_name).is_ok_and(|n| n.starts_with(b"Courier")) { 600.0 } else { 500.0 });
    }
    let descendant = font.get(b"DescendantFonts").map_err(error)?.as_array().map_err(error)?.first().ok_or_else(|| error("missing CID font"))?;
    let font = resolve(doc, descendant)?.as_dict().map_err(error)?;
    if let Ok(w) = font.get(b"W") {
        let widths = resolve(doc, w)?.as_array().map_err(error)?;
        if widths.len() > 100_000 {
            return Err(error("CID width table is too large"));
        }
        let mut i = 0;
        while let Some(start) = widths.get(i) {
            *work = work.saturating_add(1);
            if *work > MAX_CLIP_WORK {
                return Err(error("text width tables are too complex"));
            }
            let start = start.as_i64().map_err(error)?;
            i += 1;
            let entry = widths.get(i).ok_or_else(|| error("invalid CID widths"))?;
            i += 1;
            if let Object::Array(values) = entry {
                if let Some(index) = i64::from(code).checked_sub(start).and_then(|n| usize::try_from(n).ok())
                    && let Some(w) = values.get(index)
                {
                    return number(w);
                }
            } else {
                let end = entry.as_i64().map_err(error)?;
                let width = widths.get(i).ok_or_else(|| error("missing CID width"))?;
                i += 1;
                if (start..=end).contains(&i64::from(code)) {
                    return number(width);
                }
            }
        }
    }
    font.get(b"DW").ok().map(number).transpose().map(|v| v.unwrap_or(1000.0))
}
