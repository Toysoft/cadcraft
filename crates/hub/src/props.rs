//! The synchronized property table (spec §2.3: "every field the file saves is a property").
//!
//! CADCraft's [`Entity`] carries its common properties ([`Common`]: layer, color, linetype, …)
//! beside the per-kind payload ([`EntityKind`]). Both sync: the common fields one property each,
//! the payload as one value under `kind` — the figcraft decision ("big or list-valued fields ride
//! as one last-writer-wins value"), applied to the CAD payload. The tree position is not a
//! property: the adapter owns the draw-order keys and the drawing owns the store (spec §2.3).
//!
//! # Units and precision (§18.4 row 10)
//!
//! CAD quantities are unit-bearing (drawing units per `INSUNITS`, degrees for angles) and the
//! file stores full `f64`s. Two collaborators dragging the same grip to "the same" place produce
//! values that differ in the last bits, and a naive sync would turn that noise into a write war.
//! So every unit-bearing number syncs at a **canonical ten significant digits**:
//!
//! * tighter than the display precision (`LUPREC` maxes at eight decimals) and than
//!   AUTOCONSTRAIN's default 0.05-unit tolerance — no real CAD precision is lost;
//! * looser than `f64` noise, so equal-within-epsilon writes canonicalize to *the same bytes*
//!   and simply do not fight (tested in the adapter suite);
//! * applied symmetrically in the view, the diff and the comparison, so the roundtrip guarantee
//!   `apply(diff(a, b), a) == b` holds bit-exactly at the canonical layer.
//!
//! Non-finite numbers never enter the drawing through a batch: [`set_field`] refuses them, and
//! the schema refuses them upstream (see [`crate::schema`]).

use cadcraft_doc::{Common, Entity, EntityKind, Handle};
use craft_collab::Value;
use serde_json::Value as Json;

/// How many significant digits a unit-bearing value syncs at (see the module docs).
pub const SYNC_DIGITS: usize = 10;

/// The synchronized entity properties: the per-kind payload plus every common field.
pub const NODE_PROPS: &[&str] = &[
    "kind",
    "layer",
    "color",
    "linetype",
    "lineweight",
    "ltscale",
    "transparency",
    "visible",
    "thickness",
    "extrusion",
    "plotStyle",
    "annoScales",
    "xdata",
];

/// Round one `f64` to [`SYNC_DIGITS`] significant digits (the canonical sync value).
///
/// Idempotent: canonicalizing an already-canonical value returns it unchanged. Zero keeps its
/// sign (which `==` ignores); non-finite values are returned as they are — they are refused
/// elsewhere, never silently mangled.
#[must_use]
pub fn canonical_f64(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let text = format!("{:.*e}", SYNC_DIGITS - 1, x);
    text.parse().unwrap_or(x)
}

/// The canonical wire form of a serialized field: every float rounded to [`SYNC_DIGITS`]
/// significant digits, recursively through arrays and objects.
#[must_use]
pub fn canonical(json: Json) -> Json {
    match json {
        Json::Number(n) => match n.as_f64() {
            // Integers (counts, handles, flags) stay exact; only unit-bearing floats round.
            Some(f) if !n.is_i64() && !n.is_u64() => {
                let c = canonical_f64(f);
                serde_json::Number::from_f64(c).map(Json::Number).unwrap_or(Json::Null)
            }
            _ => Json::Number(n),
        },
        Json::Array(a) => Json::Array(a.into_iter().map(canonical).collect()),
        Json::Object(o) => Json::Object(o.into_iter().map(|(k, v)| (k, canonical(v))).collect()),
        other => other,
    }
}

/// The value of property `prop` of `entity`, canonically rounded; `None` when not a property.
pub fn field_value(entity: &Entity, prop: &str) -> Option<Value> {
    let raw = match prop {
        "kind" => serde_json::to_value(entity.kind.clone()).ok()?,
        "layer" => Json::String(entity.common.layer.clone()),
        "color" => serde_json::to_value(entity.common.color).ok()?,
        "linetype" => Json::String(entity.common.linetype.clone()),
        "lineweight" => serde_json::to_value(entity.common.lineweight).ok()?,
        "transparency" => serde_json::to_value(entity.common.transparency).ok()?,
        "plotStyle" => Json::String(entity.common.plot_style.clone()),
        "annoScales" => serde_json::to_value(entity.common.anno_scales.clone()).ok()?,
        "xdata" => serde_json::to_value(entity.common.xdata.clone()).ok()?,
        // Unit-bearing scalars round-trip through the canonicalizer.
        "ltscale" => Json::Number(serde_json::Number::from_f64(entity.common.ltscale)?),
        "thickness" => Json::Number(serde_json::Number::from_f64(entity.common.thickness)?),
        "visible" => Json::Bool(entity.common.visible),
        "extrusion" => serde_json::to_value(entity.common.extrusion).ok()?,
        _ => return None,
    };
    Some(Value::Json(canonical(raw)))
}

/// Write property `prop` of `entity`; `None` when `prop` is not a property.
///
/// A value of the wrong shape — or a non-finite number anywhere inside it — is an error, not a
/// panic: shipped code never crashes, and NaN never enters a drawing through a batch.
pub fn set_field(entity: &mut Entity, prop: &str, value: &Value) -> Option<Result<(), String>> {
    let json = value.to_json();
    if json_has_non_finite(&json) {
        return Some(Err("the value carries a non-finite number".into()));
    }
    match prop {
        "kind" => Some(serde_json::from_value::<EntityKind>(json).map(|v| entity.kind = v).map_err(|e| e.to_string())),
        "layer" => Some(serde_json::from_value::<String>(json).map(|v| entity.common.layer = v).map_err(|e| e.to_string())),
        "color" => Some(serde_json::from_value(json).map(|v| entity.common.color = v).map_err(|e| e.to_string())),
        "linetype" => Some(serde_json::from_value::<String>(json).map(|v| entity.common.linetype = v).map_err(|e| e.to_string())),
        "lineweight" => Some(serde_json::from_value(json).map(|v| entity.common.lineweight = v).map_err(|e| e.to_string())),
        "ltscale" => Some(read_f64(&json).map(|v| entity.common.ltscale = v).map_err(|e| e.to_string())),
        "transparency" => Some(serde_json::from_value(json).map(|v| entity.common.transparency = v).map_err(|e| e.to_string())),
        "visible" => Some(serde_json::from_value::<bool>(json).map(|v| entity.common.visible = v).map_err(|e| e.to_string())),
        "thickness" => Some(read_f64(&json).map(|v| entity.common.thickness = v).map_err(|e| e.to_string())),
        "extrusion" => Some(serde_json::from_value(json).map(|v| entity.common.extrusion = v).map_err(|e| e.to_string())),
        "plotStyle" => Some(serde_json::from_value::<String>(json).map(|v| entity.common.plot_style = v).map_err(|e| e.to_string())),
        "annoScales" => Some(serde_json::from_value(json).map(|v| entity.common.anno_scales = v).map_err(|e| e.to_string())),
        "xdata" => Some(serde_json::from_value(json).map(|v| entity.common.xdata = v).map_err(|e| e.to_string())),
        _ => None,
    }
}

/// Whether property `prop` differs between `a` and `b`, at the canonical precision; `None` when
/// not a property. Sub-epsilon differences are *not* differences — that is the whole point.
pub fn field_differs(a: &Entity, b: &Entity, prop: &str) -> Option<bool> {
    match prop {
        "kind" => Some(canonical(serde_json::to_value(&a.kind).ok()?) != canonical(serde_json::to_value(&b.kind).ok()?)),
        "layer" => Some(a.common.layer != b.common.layer),
        "color" => Some(a.common.color != b.common.color),
        "linetype" => Some(a.common.linetype != b.common.linetype),
        "lineweight" => Some(a.common.lineweight != b.common.lineweight),
        "ltscale" => Some(canonical_f64(a.common.ltscale) != canonical_f64(b.common.ltscale)),
        "transparency" => Some(a.common.transparency != b.common.transparency),
        "visible" => Some(a.common.visible != b.common.visible),
        "thickness" => Some(canonical_f64(a.common.thickness) != canonical_f64(b.common.thickness)),
        "extrusion" => Some(a.common.extrusion != b.common.extrusion),
        "plotStyle" => Some(a.common.plot_style != b.common.plot_style),
        "annoScales" => Some(a.common.anno_scales != b.common.anno_scales),
        "xdata" => Some(a.common.xdata != b.common.xdata),
        _ => None,
    }
}

/// The object id an entity's handle maps to (the drawing's handles are the file's ids, exactly
/// like figcraft's `NodeId`s: legacy files live in space 0 and first-sync like any legacy file).
#[must_use]
pub fn hub_id(h: Handle) -> craft_collab::ids::NodeId {
    craft_collab::ids::NodeId(h.0)
}

/// The entity handle an object id maps back to.
#[must_use]
pub fn handle_of(id: craft_collab::ids::NodeId) -> Handle {
    Handle(id.0)
}

/// The document-level properties: the unit system the whole drawing must agree on (§18.4 row 10).
/// `INSUNITS` picks the drawing unit (1–24, the DXF insertion-unit table), `MEASUREMENT` the
/// metric/imperial template, `LUNITS`/`LUPREC` format lengths, `AUNITS`/`AUPREC` angles, `LTSCALE`
/// scales linetypes. The handle seed deliberately does *not* sync: handle allocation is per
/// client (the session wiring re-seeds `handseed` into the client's id space before accepting
/// local edits — see the adapter docs in [`crate`]).
pub const DOC_PROPS: &[&str] = &["insunits", "measurement", "lunits", "luprec", "aunits", "auprec", "ltscale"];

/// The valid ranges of the unit-bearing document scalars (the schema enforces these; listed here
/// so the property table and the validation share one source). `(min, max)` inclusive.
#[must_use]
pub fn doc_prop_range(prop: &str) -> Option<(f64, f64)> {
    match prop {
        "insunits" => Some((0.0, 24.0)),
        "measurement" => Some((0.0, 1.0)),
        "lunits" => Some((1.0, 5.0)),
        "luprec" => Some((0.0, 8.0)),
        "aunits" => Some((0.0, 4.0)),
        "auprec" => Some((0.0, 8.0)),
        "ltscale" => Some((f64::MIN_POSITIVE, 1.0e6)),
        _ => None,
    }
}

/// The value of a document-level unit scalar, as a header variable read.
#[must_use]
pub fn doc_prop_value(doc: &cadcraft_doc::Drawing, prop: &str) -> Option<Value> {
    let v = match prop {
        "insunits" => Json::Number(serde_json::Number::from(doc.header.i64("INSUNITS", 1))),
        "measurement" => Json::Number(serde_json::Number::from(doc.header.i64("MEASUREMENT", 1))),
        "lunits" => Json::Number(serde_json::Number::from(doc.header.i64("LUNITS", 2))),
        "luprec" => Json::Number(serde_json::Number::from(doc.header.i64("LUPREC", 4))),
        "aunits" => Json::Number(serde_json::Number::from(doc.header.i64("AUNITS", 0))),
        "auprec" => Json::Number(serde_json::Number::from(doc.header.i64("AUPREC", 0))),
        "ltscale" => Json::Number(serde_json::Number::from_f64(doc.header.f64("LTSCALE", 1.0))?),
        _ => return None,
    };
    Some(Value::Json(v))
}

/// Write a document-level unit scalar; `None` when `prop` is not one of the synced scalars.
pub fn set_doc_prop(doc: &mut cadcraft_doc::Drawing, prop: &str, value: &Value) -> Option<Result<(), String>> {
    let json = value.to_json();
    match prop {
        "insunits" => Some(write_header_int(doc, "INSUNITS", &json)),
        "measurement" => Some(write_header_int(doc, "MEASUREMENT", &json)),
        "lunits" => Some(write_header_int(doc, "LUNITS", &json)),
        "luprec" => Some(write_header_int(doc, "LUPREC", &json)),
        "aunits" => Some(write_header_int(doc, "AUNITS", &json)),
        "auprec" => Some(write_header_int(doc, "AUPREC", &json)),
        "ltscale" => Some(read_f64(&json).map(|v| doc.header.set_f64("LTSCALE", v)).map_err(|e| e.to_string())),
        _ => None,
    }
}

fn write_header_int(doc: &mut cadcraft_doc::Drawing, name: &str, json: &Json) -> Result<(), String> {
    let v = read_i64(json)?;
    doc.header.set_i64(name, v);
    Ok(())
}

fn read_f64(json: &Json) -> Result<f64, String> {
    let f = json.as_f64().ok_or("expected a number")?;
    if !f.is_finite() {
        return Err("expected a finite number".into());
    }
    Ok(f)
}

fn read_i64(json: &Json) -> Result<i64, String> {
    json.as_i64().ok_or_else(|| "expected a whole number".into())
}

/// Whether any number inside `json` is a non-finite float (NaN or an infinity).
#[must_use]
pub fn json_has_non_finite(json: &Json) -> bool {
    match json {
        Json::Number(n) => n.as_f64().is_some_and(|f| !f.is_finite()),
        Json::Array(a) => a.iter().any(json_has_non_finite),
        Json::Object(o) => o.values().any(json_has_non_finite),
        _ => false,
    }
}

/// Fails to compile when [`Common`] gains a field that isn't listed as a property above — the
/// figcraft guard, applied to the struct that will grow: a new saved field must decide how it
/// syncs. ([`Entity`]'s own guard is the `kind` property plus the two non-properties: `handle`,
/// the object id, and `common`, whose fields are guarded here.)
#[allow(dead_code)]
fn every_common_field_is_a_property(c: &Common) {
    let Common {
        layer: _,
        color: _,
        linetype: _,
        lineweight: _,
        ltscale: _,
        transparency: _,
        visible: _,
        thickness: _,
        extrusion: _,
        plot_style: _,
        anno_scales: _,
        xdata: _,
    } = c;
}
