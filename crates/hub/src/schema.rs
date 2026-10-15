//! The document knowledge the hub validates against (spec §3.6 phase A: `DocSchema`): which
//! kinds exist, where entities live, the document-level entry lists that sync per entry with
//! their order — and, for CADCraft, the **unit-bearing value ranges** (§18.4 row 10): every
//! length, angle and scale that rides a batch must be finite and inside a sensible range for its
//! unit, and the unit-system document scalars must sit in their real enums (`LUNITS` 1–5,
//! `LUPREC` 0–8, `AUNITS` 0–4, `AUPREC` 0–8, `INSUNITS` 0–24, `MEASUREMENT` 0–1).

use cadcraft_doc::{Block, Constraint, Drawing, Entity, Layer, Layout, Linetype, Parameter};
use craft_collab::adapter::ValidationCtx;
use craft_collab::change::{Change, EntryId};
use craft_collab::limits::Limits;
use craft_collab::value::Value;
use serde_json::Value as Json;

use crate::props;

/// The largest magnitude a unit-bearing number may carry (drawing units run from Angstroms to
/// parsecs per `INSUNITS`; anything beyond this is garbage, not geometry).
const MAX_MAGNITUDE: f64 = 1.0e18;

/// The cadcraft document schema.
#[derive(Debug, Default)]
pub struct CadSchema {
    limits: Limits,
}

impl craft_collab::adapter::DocSchema for CadSchema {
    fn validate(&self, change: &Change, ctx: &ValidationCtx<'_>) -> Result<(), String> {
        match change {
            Change::Create { kind, parent, props, .. } => {
                if !crate::KINDS.contains(&kind.as_ref()) {
                    return Err(format!("unknown kind '{kind}'"));
                }
                if *parent != crate::ROOT {
                    let p = ctx.replica.node(*parent);
                    if p.is_some() {
                        return Err(format!("a '{}' accepts no children; CADCraft entities live in model space", kind));
                    }
                    return Err(format!("unknown parent {parent:?}"));
                }
                for (p, v) in props {
                    if p.as_ref() != "parent" && p.as_ref() != "position" && !props::NODE_PROPS.contains(&p.as_ref()) {
                        return Err(format!("unknown property '{p}'"));
                    }
                    check_entity_value(p.as_ref(), &v.to_json())?;
                }
                Ok(())
            }
            Change::Set { prop, value, .. } => {
                if prop.as_ref() == "parent" || prop.as_ref() == "position" {
                    return Ok(());
                }
                if !props::NODE_PROPS.contains(&prop.as_ref()) {
                    return Err(format!("unknown property '{prop}'"));
                }
                check_entity_value(prop.as_ref(), &value.to_json())
            }
            Change::Delete { .. } => Ok(()),
            Change::SetDoc { prop, value } => {
                if is_order_prop(prop) {
                    return Ok(());
                }
                let Some((min, max)) = props::doc_prop_range(prop) else {
                    return Err(format!("unknown document property '{prop}' (value {value:?})"));
                };
                let v = value.to_json().as_f64().ok_or_else(|| "the unit scalars are numbers".to_string())?;
                if !v.is_finite() || v < min || v > max {
                    return Err(format!("'{prop}' must be a finite number in {min}..={max}, got {v}"));
                }
                Ok(())
            }
            Change::Put { entry, value } => {
                let key = entry.0.as_ref();
                let json = value.to_json();
                match entry_kind(key) {
                    Some("constraint") => check_constraint(&json),
                    Some("param") => check_param(&json),
                    Some("layer") => check_layer(&json),
                    Some("block") | Some("layout") => check_container(key, &json, self.limits.max_nodes),
                    _ => Ok(()),
                }
            }
            Change::Remove { entry } => {
                if entry_kind(&entry.0).is_some() {
                    Ok(())
                } else {
                    Err(format!("unknown entry kind in '{entry:?}'"))
                }
            }
        }
    }

    fn recompute(&self, _touched: &[craft_collab::ids::NodeId]) -> Vec<Change> {
        // CADCraft's constraint re-solve is the engine's post-command hook today, not a property
        // pass the document runs on its own — a command the author runs (or that runs after
        // their edit), exactly the shape the pilot documented for figcraft's auto-layout. So
        // there is no author-only recompute to compose here (spec §5.7's map decides: "commands,
        // not derived state"); the session wiring will run the same `cadcraft-constraints::solve`
        // the engine does, on the author's side alone, after its batches land. When a derived
        // property lands in the model, its recompute belongs here and nowhere else.
        Vec::new()
    }

    fn limits(&self) -> &Limits {
        &self.limits
    }
}

/// Check one entity property value: finite, in magnitude, and per-property sane for its unit.
fn check_entity_value(prop: &str, json: &Json) -> Result<(), String> {
    if props::json_has_non_finite(json) {
        return Err(format!("'{prop}' carries a non-finite number (NaN or infinity)"));
    }
    check_magnitude(json)?;
    match prop {
        "ltscale" => {
            let v = json.as_f64().ok_or("'ltscale' is a number")?;
            if v <= 0.0 || v > 1.0e6 {
                return Err(format!("'ltscale' must be in 0 < v ≤ 1e6, got {v}"));
            }
        }
        "thickness" => {
            let v = json.as_f64().ok_or("'thickness' is a number")?;
            if v.abs() > 1.0e9 {
                return Err(format!("'thickness' must be within ±1e9 drawing units, got {v}"));
            }
        }
        "kind" => check_kind_payload(json)?,
        _ => {}
    }
    Ok(())
}

/// Every number inside `json` stays inside the magnitude a real drawing unit can carry.
fn check_magnitude(json: &Json) -> Result<(), String> {
    match json {
        Json::Number(n) => {
            if let Some(f) = n.as_f64()
                && f.abs() > MAX_MAGNITUDE
            {
                return Err(format!("a unit-bearing value exceeds ±{MAX_MAGNITUDE}: {f}"));
            }
            Ok(())
        }
        Json::Array(a) => a.iter().try_for_each(check_magnitude),
        Json::Object(o) => o.values().try_for_each(check_magnitude),
        _ => Ok(()),
    }
}

/// The per-kind unit rules: radii positive, text heights positive, oblique angles inside
/// AutoCAD's ±85°, ellipse ratios positive. The payload's own type tag routes them.
fn check_kind_payload(json: &Json) -> Result<(), String> {
    let Some(kind) = json.get("type").and_then(Json::as_str) else {
        return Err("the kind payload carries no type tag".into());
    };
    let positive = |v: Option<&Json>, what: &str| -> Result<(), String> {
        let v = v.and_then(Json::as_f64).ok_or_else(|| format!("'{what}' is a number"))?;
        if v <= 0.0 {
            return Err(format!("'{what}' must be positive, got {v}"));
        }
        Ok(())
    };
    match kind {
        "circle" | "arc" => positive(json.get("radius"), "radius")?,
        "ellipse" => positive(json.get("ratio"), "ratio")?,
        "text" | "mText" => {
            positive(json.get("height"), "height")?;
            if kind == "text" {
                positive(json.get("widthFactor"), "widthFactor")?;
                let oblique = json.get("oblique").and_then(Json::as_f64).ok_or("'oblique' is a number")?;
                if !(-85.0..=85.0).contains(&oblique) {
                    return Err(format!("'oblique' must be within ±85 degrees, got {oblique}"));
                }
            }
        }
        "attDef" => {
            let text = json.get("text").ok_or("an attribute definition carries its text object")?;
            positive(text.get("height"), "height")?;
            positive(text.get("widthFactor"), "widthFactor")?;
        }
        _ => {}
    }
    Ok(())
}

/// A constraint entry: a real id, bounded names and expressions, and references to real handles.
/// The expression itself is parsed by `cadcraft-constraints` at solve time (the engine's own
/// behavior); here it only has to be a bounded, balanced, control-free string.
fn check_constraint(json: &Json) -> Result<(), String> {
    let id = json.get("id").and_then(Json::as_u64).ok_or("a constraint carries a whole-number id")?;
    if id == 0 {
        return Err("constraint ids start at 1".into());
    }
    let refs = json.get("refs").and_then(Json::as_array).ok_or("a constraint carries its refs array")?;
    if refs.is_empty() {
        return Err("a constraint references at least one object".into());
    }
    for r in refs {
        let h = r.get("handle").and_then(Json::as_u64).ok_or("a constraint ref carries a whole-number handle")?;
        if h == 0 {
            return Err("a constraint ref points at a real object (handle 0 is the root)".into());
        }
    }
    check_bounded_text(json.get("name"), "name", 255)?;
    check_bounded_text(json.get("expr"), "expr", 256)?;
    check_balanced(json.get("expr"))
}

/// A user parameter entry: a bounded name and expression.
fn check_param(json: &Json) -> Result<(), String> {
    check_bounded_text(json.get("name"), "name", 255)?;
    check_bounded_text(json.get("expr"), "expr", 256)?;
    check_balanced(json.get("expr"))
}

/// A layer entry: a bounded name and a transparency inside the 0–90 percent range.
fn check_layer(json: &Json) -> Result<(), String> {
    check_bounded_text(json.get("name"), "name", 255)?;
    if let Some(t) = json.get("transparency").and_then(Json::as_u64)
        && t > 90
    {
        return Err(format!("layer transparency is 0–90 percent, got {t}"));
    }
    Ok(())
}

/// A block or layout entry: a bounded name and an entity list inside the node budget (the
/// entities ride inside the entry value — the documented v1 decision).
fn check_container(key: &str, json: &Json, max_nodes: usize) -> Result<(), String> {
    check_bounded_text(json.get("name"), "name", 255)?;
    if let Some(entities) = json.get("entities").and_then(Json::as_array)
        && entities.len() > max_nodes
    {
        return Err(format!("'{key}' carries {} entities, over the node budget {max_nodes}", entities.len()));
    }
    Ok(())
}

fn check_bounded_text(value: Option<&Json>, what: &str, max: usize) -> Result<(), String> {
    let Some(s) = value.and_then(Json::as_str) else {
        return Err(format!("'{what}' is a string"));
    };
    if s.len() > max {
        return Err(format!("'{what}' is longer than {max} characters"));
    }
    if s.chars().any(char::is_control) {
        return Err(format!("'{what}' carries control characters"));
    }
    Ok(())
}

/// Parentheses and string quotes balance (a malformed expression is refused before it can
/// confuse anyone's solver; a well-formed one still gets its real parse at solve time).
fn check_balanced(value: Option<&Json>) -> Result<(), String> {
    let Some(s) = value.and_then(Json::as_str) else { return Ok(()) };
    let mut depth = 0i64;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err("the expression closes a parenthesis that never opened".into());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("the expression leaves a parenthesis open".into());
    }
    Ok(())
}

/// Which list an entry id belongs to (v2's keyed-list string forms, §2.3).
fn entry_kind(entry: &str) -> Option<&'static str> {
    for (prefix, kind) in [
        ("Layer:", "layer"),
        ("Linetype:", "linetype"),
        ("TextStyle:", "textStyle"),
        ("DimStyle:", "dimStyle"),
        ("MLeaderStyle:", "mleaderStyle"),
        ("TableStyle:", "tableStyle"),
        ("MLineStyle:", "mlineStyle"),
        ("Block:", "block"),
        ("Layout:", "layout"),
        ("Constraint:", "constraint"),
        ("Param:", "param"),
    ] {
        if entry.starts_with(prefix) {
            return Some(kind);
        }
    }
    None
}

/// The record key inside a wire entry id: the prefix names the list, the rest is the record's
/// own key (its name — the lists key their records by the bare name, so upsert/remove compare
/// against this stripped form, never against the whole wire id). Unknown forms fall back to the
/// whole string; the caller routed through `entry_kind` first, so the fallback only guards
/// hostile keys.
fn record_key(key: &str) -> &str {
    key.split_once(':').map_or(key, |(_, rest)| rest)
}

/// Whether `prop` is one of the per-list order properties.
fn is_order_prop(prop: &str) -> bool {
    matches!(
        prop,
        "layerOrder"
            | "linetypeOrder"
            | "textStyleOrder"
            | "dimStyleOrder"
            | "mleaderStyleOrder"
            | "tableStyleOrder"
            | "mlineStyleOrder"
            | "blockOrder"
            | "layoutOrder"
            | "constraintOrder"
            | "paramOrder"
    )
}

/// Upsert one document-level entry; wrong shapes are errors, never panics. The wire id is the
/// prefixed form (`Layer:Walls`); the record's natural key is the bare name, so the upsert
/// compares against the stripped form (see [`record_key`]).
pub fn put_entry(doc: &mut Drawing, entry: &EntryId, value: &Value) -> Result<(), String> {
    let json = value.to_json();
    let key = entry.0.as_ref();
    match entry_kind(key) {
        Some("layer") => {
            let v: Layer = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.layers, |l| l.name.clone(), v, &record_key(key).to_owned())
        }
        Some("linetype") => {
            let v: Linetype = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.linetypes, |l| l.name.clone(), v, &record_key(key).to_owned())
        }
        Some("textStyle") => {
            let v: cadcraft_doc::TextStyle = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.text_styles, |s| s.name.clone(), v, &record_key(key).to_owned())
        }
        Some("dimStyle") => {
            let v: cadcraft_doc::DimStyle = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.dim_styles, |s| s.name.clone(), v, &record_key(key).to_owned())
        }
        Some("mleaderStyle") => {
            let v: cadcraft_doc::MLeaderStyle = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.mleader_styles, |s| s.name.clone(), v, &record_key(key).to_owned())
        }
        Some("tableStyle") => {
            let v: cadcraft_doc::TableStyle = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.table_styles, |s| s.name.clone(), v, &record_key(key).to_owned())
        }
        Some("mlineStyle") => {
            let v: cadcraft_doc::MLineStyle = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.mline_styles, |s| s.name.clone(), v, &record_key(key).to_owned())
        }
        Some("param") => {
            let v: Parameter = serde_json::from_value(json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.parametric.parameters, |p| p.name.clone(), v, &record_key(key).to_owned())
        }
        Some("constraint") => {
            let v: Constraint = serde_json::from_value(json).map_err(|e| e.to_string())?;
            let n: u32 = key.strip_prefix("Constraint:").and_then(|t| t.parse().ok()).ok_or_else(|| format!("malformed constraint id '{key}'"))?;
            upsert(doc, |d| &mut d.constraints, |c| c.id, v, &n)
        }
        Some("layout") => {
            let v: Layout = layout_from_json(&json).map_err(|e| e.to_string())?;
            upsert(doc, |d| &mut d.layouts, |l| l.name.clone(), v, &record_key(key).to_owned())
        }
        Some("block") => {
            let v: Block = block_from_json(&json).map_err(|e| e.to_string())?;
            doc.blocks.insert(record_key(key).to_string(), std::sync::Arc::new(v));
            Ok(())
        }
        _ => Err(format!("unknown entry kind in '{key}'")),
    }
}

/// Remove one document-level entry (absent entries are fine — the tolerant v2 rule). The wire
/// id is prefixed; the records key by their bare names, so the retain compares against the
/// stripped form.
pub fn remove_entry(doc: &mut Drawing, entry: &EntryId) -> Result<(), String> {
    let key = entry.0.as_ref();
    match entry_kind(key) {
        Some("layer") => {
            let name = record_key(key);
            doc.layers.retain(|l| l.name != name);
            Ok(())
        }
        Some("linetype") => {
            let name = record_key(key);
            doc.linetypes.retain(|l| l.name != name);
            Ok(())
        }
        Some("textStyle") => {
            let name = record_key(key);
            doc.text_styles.retain(|s| s.name != name);
            Ok(())
        }
        Some("dimStyle") => {
            let name = record_key(key);
            doc.dim_styles.retain(|s| s.name != name);
            Ok(())
        }
        Some("mleaderStyle") => {
            let name = record_key(key);
            doc.mleader_styles.retain(|s| s.name != name);
            Ok(())
        }
        Some("tableStyle") => {
            let name = record_key(key);
            doc.table_styles.retain(|s| s.name != name);
            Ok(())
        }
        Some("mlineStyle") => {
            let name = record_key(key);
            doc.mline_styles.retain(|s| s.name != name);
            Ok(())
        }
        Some("param") => {
            let name = record_key(key);
            doc.parametric.parameters.retain(|p| p.name != name);
            Ok(())
        }
        Some("constraint") => {
            let n: u32 = key.strip_prefix("Constraint:").and_then(|t| t.parse().ok()).ok_or_else(|| format!("malformed constraint id '{key}'"))?;
            doc.constraints.retain(|c| c.id != n);
            Ok(())
        }
        Some("layout") => {
            let name = record_key(key);
            doc.layouts.retain(|l| l.name != name);
            Ok(())
        }
        Some("block") => {
            doc.blocks.remove(record_key(key));
            Ok(())
        }
        _ => Err(format!("unknown entry kind in '{key}'")),
    }
}

/// Reorder one document-level list from an order property's value. The value carries the same
/// wire keys as the entry ids (`Layer:Walls`), so the reorder compares in that form; entries the
/// array misses keep their relative order at the end — lenient, like the client replay.
pub fn set_doc_order(doc: &mut Drawing, prop: &str, value: &Value) -> Option<Result<(), String>> {
    let json = value.to_json();
    let order: Vec<String> = match serde_json::from_value(json) {
        Ok(v) => v,
        Err(e) => return Some(Err(e.to_string())),
    };
    match prop {
        "layerOrder" => Some(reorder(&mut doc.layers, |l| format!("Layer:{}", l.name), &order)),
        "linetypeOrder" => Some(reorder(&mut doc.linetypes, |l| format!("Linetype:{}", l.name), &order)),
        "textStyleOrder" => Some(reorder(&mut doc.text_styles, |s| format!("TextStyle:{}", s.name), &order)),
        "dimStyleOrder" => Some(reorder(&mut doc.dim_styles, |s| format!("DimStyle:{}", s.name), &order)),
        "mleaderStyleOrder" => Some(reorder(&mut doc.mleader_styles, |s| format!("MLeaderStyle:{}", s.name), &order)),
        "tableStyleOrder" => Some(reorder(&mut doc.table_styles, |s| format!("TableStyle:{}", s.name), &order)),
        "mlineStyleOrder" => Some(reorder(&mut doc.mline_styles, |s| format!("MLineStyle:{}", s.name), &order)),
        "layoutOrder" => Some(reorder(&mut doc.layouts, |l| format!("Layout:{}", l.name), &order)),
        "constraintOrder" => {
            let ids: Result<Vec<u32>, _> = order
                .iter()
                .map(|k| k.strip_prefix("Constraint:").and_then(|t| t.parse().ok()).ok_or_else(|| format!("malformed constraint id '{k}'")))
                .collect();
            let ids = match ids {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            Some(reorder(&mut doc.constraints, |c| c.id, &ids))
        }
        "paramOrder" => Some(reorder(&mut doc.parametric.parameters, |p| format!("Param:{}", p.name), &order)),
        "blockOrder" => Some(Ok(())), // the block map is name-ordered by construction; the order property is the wire record of the names
        _ => None,
    }
}

/// Reorder `list` so the entries whose keys appear in `order` come first in that order.
fn reorder<T, K: PartialEq + Clone>(list: &mut Vec<T>, key: fn(&T) -> K, order: &[K]) -> Result<(), String> {
    let mut decorated: Vec<(usize, T)> = std::mem::take(list).into_iter().map(|item| (order.len() + 1, item)).collect();
    for (slot, item) in decorated.iter_mut() {
        let k = key(item);
        if let Some(i) = order.iter().position(|o| *o == k) {
            *slot = i;
        }
    }
    decorated.sort_by_key(|(slot, _)| *slot);
    *list = decorated.into_iter().map(|(_, item)| item).collect();
    Ok(())
}

/// Insert-or-replace by key: replace the entry with the same key, or append.
fn upsert<T, K>(doc: &mut Drawing, get: fn(&mut Drawing) -> &mut Vec<T>, key: fn(&T) -> K, item: T, want: &K) -> Result<(), String>
where
    K: PartialEq,
{
    let list = get(doc);
    for slot in list.iter_mut() {
        if key(slot) == *want {
            *slot = item;
            return Ok(());
        }
    }
    list.push(item);
    Ok(())
}

/// A layout's wire value: the record plus its paper-space entities (the serde form skips the
/// store, so the entities ride as an array — the documented v1 "whole container" decision).
fn layout_value(layout: &Layout) -> Option<Json> {
    let mut json = serde_json::to_value(layout).ok()?;
    let entities: Vec<Json> = layout.entities.iter().filter_map(|e| serde_json::to_value(e.as_ref()).ok()).collect();
    json["entities"] = Json::Array(entities);
    Some(props::canonical(json))
}

/// A block's wire value, same shape decision.
fn block_value(block: &Block) -> Option<Json> {
    let mut json = serde_json::to_value(block).ok()?;
    let entities: Vec<Json> = block.entities.iter().filter_map(|e| serde_json::to_value(e.as_ref()).ok()).collect();
    json["entities"] = Json::Array(entities);
    Some(props::canonical(json))
}

/// Rebuild a layout from its wire value (its entities re-enter a fresh store).
fn layout_from_json(json: &Json) -> Result<Layout, String> {
    let mut layout: Layout = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
    layout.entities = store_from_json(json)?;
    Ok(layout)
}

/// Rebuild a block from its wire value.
fn block_from_json(json: &Json) -> Result<Block, String> {
    let mut block: Block = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
    block.entities = store_from_json(json)?;
    Ok(block)
}

/// The entities of a container value, pushed into a fresh store in their wire order.
fn store_from_json(json: &Json) -> Result<cadcraft_doc::EntityStore, String> {
    let mut store = cadcraft_doc::EntityStore::new();
    let Some(entities) = json.get("entities").and_then(Json::as_array) else { return Ok(store) };
    for e in entities {
        let entity: Entity = serde_json::from_value(e.clone()).map_err(|err| err.to_string())?;
        store.push(entity);
    }
    Ok(store)
}

/// The document-level entries of `doc` in order, as `(entry id, value)` pairs for the view.
pub fn entry_values(doc: &Drawing) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut push = |key: String, json: Option<Json>| {
        if let Some(v) = json {
            out.push((key, Value::Json(v)));
        }
    };
    for l in &doc.layers {
        push(format!("Layer:{}", l.name), serde_json::to_value(l).ok().map(props::canonical));
    }
    for l in &doc.linetypes {
        push(format!("Linetype:{}", l.name), serde_json::to_value(l).ok().map(props::canonical));
    }
    for s in &doc.text_styles {
        push(format!("TextStyle:{}", s.name), serde_json::to_value(s).ok().map(props::canonical));
    }
    for s in &doc.dim_styles {
        push(format!("DimStyle:{}", s.name), serde_json::to_value(s).ok().map(props::canonical));
    }
    for s in &doc.mleader_styles {
        push(format!("MLeaderStyle:{}", s.name), serde_json::to_value(s).ok().map(props::canonical));
    }
    for s in &doc.table_styles {
        push(format!("TableStyle:{}", s.name), serde_json::to_value(s).ok().map(props::canonical));
    }
    for s in &doc.mline_styles {
        push(format!("MLineStyle:{}", s.name), serde_json::to_value(s).ok().map(props::canonical));
    }
    for b in doc.blocks.values() {
        push(format!("Block:{}", b.name), block_value(b));
    }
    for l in &doc.layouts {
        push(format!("Layout:{}", l.name), layout_value(l));
    }
    for c in &doc.constraints {
        push(format!("Constraint:{}", c.id), serde_json::to_value(c).ok().map(props::canonical));
    }
    for p in &doc.parametric.parameters {
        push(format!("Param:{}", p.name), serde_json::to_value(p).ok().map(props::canonical));
    }
    out
}

/// Diff the document-level lists between two documents: per-entry `Put`/`Remove` plus one
/// order property per list whose order changed (v2's discipline: entries plus their order). The
/// entry ids and the order arrays both carry the wire key form (`Layer:Walls`) — the same
/// strings the view reports and `put_entry`/`remove_entry` route by.
pub fn diff_entries(before: &Drawing, after: &Drawing) -> Vec<Change> {
    let mut out = Vec::new();
    diff_list(before, after, |d| &d.layers, |l| format!("Layer:{}", l.name), "layerOrder", &mut out);
    diff_list(before, after, |d| &d.linetypes, |l| format!("Linetype:{}", l.name), "linetypeOrder", &mut out);
    diff_list(before, after, |d| &d.text_styles, |s| format!("TextStyle:{}", s.name), "textStyleOrder", &mut out);
    diff_list(before, after, |d| &d.dim_styles, |s| format!("DimStyle:{}", s.name), "dimStyleOrder", &mut out);
    diff_list(before, after, |d| &d.mleader_styles, |s| format!("MLeaderStyle:{}", s.name), "mleaderStyleOrder", &mut out);
    diff_list(before, after, |d| &d.table_styles, |s| format!("TableStyle:{}", s.name), "tableStyleOrder", &mut out);
    diff_list(before, after, |d| &d.mline_styles, |s| format!("MLineStyle:{}", s.name), "mlineStyleOrder", &mut out);
    diff_list(before, after, |d| &d.parametric.parameters, |p| format!("Param:{}", p.name), "paramOrder", &mut out);
    diff_list(before, after, |d| &d.constraints, |c| format!("Constraint:{}", c.id), "constraintOrder", &mut out);
    diff_blocks(before, after, &mut out);
    diff_list(before, after, |d| &d.layouts, |l| format!("Layout:{}", l.name), "layoutOrder", &mut out);
    out
}

/// Diff one keyed list: value `Put`s for new or changed entries, `Remove`s for gone ones, and
/// one `SetDoc` when the order changed.
fn diff_list<T, F, K>(before: &Drawing, after: &Drawing, list: F, key: fn(&T) -> K, order_prop: &str, out: &mut Vec<Change>)
where
    T: PartialEq + serde::Serialize,
    F: Fn(&Drawing) -> &Vec<T>,
    K: std::fmt::Display,
{
    let (b, a) = (list(before), list(after));
    let key_of = |item: &T| key(item).to_string();
    let b_keys: Vec<String> = b.iter().map(&key_of).collect();
    let a_keys: Vec<String> = a.iter().map(&key_of).collect();
    if b_keys != a_keys {
        out.push(Change::SetDoc { prop: order_prop.into(), value: Value::Json(serde_json::json!(a_keys)) });
    }
    for item in a {
        let k = key_of(item);
        let same = b_keys.contains(&k) && b.iter().any(|bi| key_of(bi) == k && bi == item);
        if !same && let Some(v) = serde_json::to_value(item).ok().map(props::canonical).map(Value::Json) {
            out.push(Change::Put { entry: EntryId(k.into()), value: v });
        }
    }
    for item in b {
        let k = key_of(item);
        if !a_keys.contains(&k) {
            out.push(Change::Remove { entry: EntryId(k.into()) });
        }
    }
}

/// Diff the block map: value `Put`s for new or changed blocks (Arc-shared ones skip the
/// serialization entirely), `Remove`s for gone ones, and one order property when names changed
/// (in the same wire key form as every other list).
fn diff_blocks(before: &Drawing, after: &Drawing, out: &mut Vec<Change>) {
    let b_names: Vec<String> = before.blocks.keys().map(|n| format!("Block:{n}")).collect();
    let a_names: Vec<String> = after.blocks.keys().map(|n| format!("Block:{n}")).collect();
    if b_names != a_names {
        out.push(Change::SetDoc { prop: "blockOrder".into(), value: Value::Json(serde_json::json!(a_names)) });
    }
    for (name, after_block) in &after.blocks {
        let shared = before.blocks.get(name).is_some_and(|b| std::sync::Arc::ptr_eq(b, after_block));
        let same = shared || before.blocks.get(name).is_some_and(|b| block_value(b) == block_value(after_block));
        if !same && let Some(v) = block_value(after_block) {
            out.push(Change::Put { entry: EntryId(format!("Block:{name}").into()), value: Value::Json(v) });
        }
    }
    for name in before.blocks.keys() {
        if !after.blocks.contains_key(name) {
            out.push(Change::Remove { entry: EntryId(format!("Block:{name}").into()) });
        }
    }
}
