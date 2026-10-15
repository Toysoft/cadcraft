//! # cadcraft-hub — CADCraft's CraftHub client path
//!
//! The [`HubAdapter`] that maps CADCraft's drawing database ([`Drawing`], the DXF/DWG-shaped
//! model: entities in a copy-on-write store, symbol tables, blocks, layouts, parametric
//! constraints) onto `craft-collab`'s generalized object/property model (the CraftHub
//! specification §3.6 phase B, §18.4 row 10).
//!
//! What belongs here, per the family rule (§3.6: "adapters for an app ship with the app"):
//!
//! * [`props`] — the synchronized property table (the per-kind payload plus every common entity
//!   field, with the compile-time guard) and the canonical ten-digit precision every unit-bearing
//!   value syncs at (§18.4 row 10: units and precision — the drawing's own tolerance story).
//! * [`CadcraftHub`] — the adapter: `view`/`apply`/`diff` over the real drawing, the draw-order
//!   position keys (the state a session keeps beside the document), and the presence projection.
//! * [`commands`] — the unified `hub.*` command catalog (same names in every Craft app, §18.3)
//!   with the v2 `collab.*` compatibility mapping.
//! * [`deeplink`] — `cadcraft://join?...` links (§18.5).
//! * [`schema`] — the document knowledge the hub server needs to validate batches: which kinds
//!   exist, where entities live, the per-entry document lists and their orders, and the
//!   unit-bearing value ranges.
//!
//! ## How the drawing maps (honestly)
//!
//! * **Model space is the tree.** The generic root *is* model space; entities are its children,
//!   ordered by their draw order (fractional keys, one per position in the order). This is where
//!   concurrent drafting happens, and it is the whole node tree — CAD has no deeper nesting: a
//!   block reference displays a definition's entities but does not contain them.
//! * **The object id is the database handle**, verbatim (`NodeId(handle)`), exactly like
//!   figcraft's `NodeId`s: a shared legacy file has the same handles everywhere (space 0), and
//!   the session wiring re-seeds the local `handseed` into the client's granted id space before
//!   accepting local edits, so live-created handles embed the client space and never collide
//!   (the core's §5.1 discipline; one `handseed` write in the follow-up wiring, never here —
//!   `view` does not mutate the document). Handle 0 is the core's root marker and never a real
//!   entity, so entities carrying it are skipped.
//! * **Symbol tables, blocks, layouts, constraints and parameters sync as document-level
//!   entries** (one id per record, one order property per list) — the v2 keyed-list discipline.
//!   Blocks and layouts carry their entity content inside the entry value as one
//!   last-writer-wins blob: paper-space content and block definitions are usually owned by one
//!   author at a time, and per-entity sync there is the natural follow-up (documented in the PR).
//! * **Units are document scalars**: `INSUNITS`, `MEASUREMENT`, `LUNITS`/`LUPREC`,
//!   `AUNITS`/`AUPREC`, `LTSCALE` — the system every collaborator must agree on, validated
//!   against their real ranges.
//!
//! What deliberately does **not** live here: networking, sessions, the UI, and the constraint
//! solver. Re-solving after an edit is the engine's post-command hook today (a command, not a
//! property recompute — see [`crate::schema`]'s honest note); the session wiring (a v3 `NetLink`
//! installed by the engine behind the app's `crafthub` feature flag) lands as the follow-up once
//! this contract is reviewed. Nothing here changes any user-visible behavior.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod commands;
pub mod deeplink;
pub mod presence;
pub mod props;
pub mod schema;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cadcraft_doc::{Drawing, Entity, EntityKind, Handle};
use craft_collab::adapter::{ApplyReport, DocumentView, HubAdapter, UiState, ViewNode};
use craft_collab::change::Change;
use craft_collab::frac;
use craft_collab::ids::NodeId as HubId;
use craft_collab::value::Value;
use props::handle_of;

/// The generic root: model space (space 0, counter 0 — the core's root marker, never a handle).
pub const ROOT: HubId = HubId(0);

/// The object kinds: the real [`EntityKind`] discriminants, under their serde (camelCase) names.
pub const KINDS: &[&str] = &[
    "line",
    "point",
    "circle",
    "arc",
    "ellipse",
    "lwPolyline",
    "polyline3d",
    "spline",
    "helix",
    "ray",
    "xLine",
    "text",
    "mText",
    "attDef",
    "insert",
    "dimension",
    "mLine",
    "leader",
    "mLeader",
    "hatch",
    "solid",
    "trace",
    "face3d",
    "region",
    "viewport",
    "image",
    "wipeout",
    "table",
    "tolerance",
    "unknown",
];

/// The kinds that accept children: none — a CAD entity owns no sub-entities (block references
/// display a definition's entities without containing them). The list exists so the schema table
/// says so explicitly instead of by omission.
pub const CONTAINERS: &[&str] = &[];

/// The adapter: document binding, draw-order keys and presence projection in one value.
///
/// The position keys are adapter state, exactly as the spec prescribes (§2.3: "existing
/// documents take keys by their current order at first sync"). They sit behind a mutex so the
/// adapter stays shareable; a session owns one adapter per open drawing and the family's document
/// discipline is single-threaded per session, so contention is not a concern — correctness is.
pub struct CadcraftHub {
    schema: schema::CadSchema,
    positions: Mutex<BTreeMap<Handle, String>>,
}

impl CadcraftHub {
    /// A fresh adapter with the default limits and an empty key map.
    #[must_use]
    pub fn new() -> Self {
        Self { schema: schema::CadSchema::default(), positions: Mutex::new(BTreeMap::new()) }
    }

    /// The draw-order key of `handle`, if one has been assigned.
    #[must_use]
    pub fn position_key(&self, handle: Handle) -> Option<String> {
        self.positions.lock().unwrap_or_else(|e| e.into_inner()).get(&handle).cloned()
    }

    /// The kind string of an entity (the discriminant; the payload syncs inside the `kind`
    /// property).
    #[must_use]
    pub fn kind_name(kind: &EntityKind) -> &'static str {
        match kind {
            EntityKind::Line(_) => "line",
            EntityKind::Point(_) => "point",
            EntityKind::Circle(_) => "circle",
            EntityKind::Arc(_) => "arc",
            EntityKind::Ellipse(_) => "ellipse",
            EntityKind::LwPolyline(_) => "lwPolyline",
            EntityKind::Polyline3d(_) => "polyline3d",
            EntityKind::Spline(_) => "spline",
            EntityKind::Helix(_) => "helix",
            EntityKind::Ray(_) => "ray",
            EntityKind::XLine(_) => "xLine",
            EntityKind::Text(_) => "text",
            EntityKind::MText(_) => "mText",
            EntityKind::AttDef(_) => "attDef",
            EntityKind::Insert(_) => "insert",
            EntityKind::Dimension(_) => "dimension",
            EntityKind::MLine(_) => "mLine",
            EntityKind::Leader(_) => "leader",
            EntityKind::MLeader(_) => "mLeader",
            EntityKind::Hatch(_) => "hatch",
            EntityKind::Solid(_) => "solid",
            EntityKind::Trace(_) => "trace",
            EntityKind::Face3d(_) => "face3d",
            EntityKind::Region(_) => "region",
            EntityKind::Viewport(_) => "viewport",
            EntityKind::Image(_) => "image",
            EntityKind::Wipeout(_) => "wipeout",
            EntityKind::Table(_) => "table",
            EntityKind::Tolerance(_) => "tolerance",
            EntityKind::Unknown(_) => "unknown",
        }
    }

    /// A default payload for a kind string, for a `Create` that arrives without its `kind`
    /// property (the diff always sends one, so this only feeds schema round-trips and tests).
    /// Kinds whose payload has no sensible empty form return `None` — the change is then
    /// skipped, never faked.
    #[must_use]
    pub fn default_kind(name: &str) -> Option<EntityKind> {
        // Via cadcraft-doc's re-export (`pub use cadcraft_geom as geom`) — the adapter depends on
        // the doc model, not on the geom crate directly.
        use cadcraft_doc::geom::Vec3;
        let z = Vec3::ZERO;
        let kind = match name {
            "line" => EntityKind::Line(cadcraft_doc::Line { a: z, b: z }),
            "point" => EntityKind::Point(cadcraft_doc::Point { p: z, angle: 0.0 }),
            "circle" => EntityKind::Circle(cadcraft_doc::Circle { center: z, radius: 1.0 }),
            "arc" => EntityKind::Arc(cadcraft_doc::Arc { center: z, radius: 1.0, start: 0.0, end: std::f64::consts::FRAC_PI_2 }),
            "ellipse" => EntityKind::Ellipse(cadcraft_doc::Ellipse {
                center: z,
                major: Vec3::new(1.0, 0.0, 0.0),
                ratio: 1.0,
                start: 0.0,
                end: std::f64::consts::TAU,
            }),
            "lwPolyline" => EntityKind::LwPolyline(cadcraft_doc::LwPolyline {
                vertices: Vec::new(),
                closed: false,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
            "polyline3d" => EntityKind::Polyline3d(cadcraft_doc::Polyline3d { points: Vec::new(), closed: false }),
            "ray" | "xLine" => EntityKind::Ray(cadcraft_doc::RayLine { base: z, dir: Vec3::new(1.0, 0.0, 0.0) }),
            "text" => EntityKind::Text(cadcraft_doc::Text {
                insert: z,
                align_pt: None,
                height: 1.0,
                value: String::new(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
            "mText" => EntityKind::MText(cadcraft_doc::MText {
                insert: z,
                height: 1.0,
                width: 0.0,
                attach: 1,
                rotation: 0.0,
                style: "Standard".into(),
                contents: String::new(),
                line_spacing: 1.0,
                line_spacing_exact: false,
                columns: None,
            }),
            "solid" | "trace" => EntityKind::Solid(cadcraft_doc::Solid { corners: [z; 4] }),
            "face3d" => EntityKind::Face3d(cadcraft_doc::Face3d { corners: [z; 4], hidden_edges: 0 }),
            "wipeout" => EntityKind::Wipeout(cadcraft_doc::Wipeout { boundary: Vec::new() }),
            _ => return None,
        };
        Some(kind)
    }

    /// The entity handles of a document's model space in draw order.
    fn children(doc: &Drawing) -> Vec<Handle> {
        doc.model.handles()
    }

    /// Sort model space by `(key, handle)` when the order changed (a no-op check first, so an
    /// unchanged order keeps the store's chunk sharing — the snapshot discipline).
    fn reorder_model(doc: &mut Drawing, keys: &BTreeMap<Handle, String>) {
        let rank = |h: &Handle| (keys.get(h).cloned().unwrap_or_default(), *h);
        let current = doc.model.handles();
        let mut wanted = current.clone();
        wanted.sort_by_key(rank);
        if current != wanted {
            doc.model.sort_by_key(|e| rank(&e.handle));
        }
    }

    /// Assign draw-order keys to every entity that has none, by current order (the first-sync
    /// rule, §2.3).
    fn first_sync_keys(&self, doc: &Drawing) {
        let mut positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
        let missing: Vec<Handle> = doc.model.handles().into_iter().filter(|h| !positions.contains_key(h)).collect();
        if missing.is_empty() {
            return;
        }
        if let Ok(keys) = frac::n_between(None, None, missing.len()) {
            for (h, key) in missing.into_iter().zip(keys) {
                positions.insert(h, key);
            }
        }
    }

    /// Record `key` for `handle`.
    fn set_key(&self, handle: Handle, key: String) {
        self.positions.lock().unwrap_or_else(|e| e.into_inner()).insert(handle, key);
    }

    /// Drop the key of a handle (before the drawing forgets it).
    fn drop_key(&self, handle: Handle) {
        self.positions.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle);
    }

    /// Diff the model-space draw order between two drawings, assigning fresh keys to the moved
    /// and inserted entities while the longest rising run of old keys keeps its keys (the v2
    /// discipline: only the entities that must move get new keys, so an insertion in the middle
    /// costs one key, not a renumbering). Emits `Set(position)` changes into `out`.
    fn diff_order(&self, before: &Drawing, after: &Drawing, out: &mut Vec<Change>) {
        let old = Self::children(before);
        let new = Self::children(after);
        if old == new {
            return;
        }
        let mut positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
        // First-sync rule: entities of the before-order without keys take them by order.
        let unkeyed: Vec<Handle> = old.iter().copied().filter(|h| !positions.contains_key(h)).collect();
        if !unkeyed.is_empty()
            && let Ok(fresh) = frac::n_between(None, None, unkeyed.len())
        {
            for (h, key) in unkeyed.into_iter().zip(fresh) {
                positions.insert(h, key);
            }
        }
        // The longest strictly-rising run of keys over the new order, counting only entities
        // that already have keys AND already belonged to model space (an entity new to the space
        // always takes a fresh key).
        let has_key = |h: &Handle| positions.contains_key(h);
        let old_member = |h: &Handle| old.contains(h);
        let n = new.len();
        let mut len = vec![0usize; n];
        let mut prev = vec![usize::MAX; n];
        let mut best_end = None;
        let mut best_len = 0usize;
        for i in 0..n {
            if has_key(&new[i]) && old_member(&new[i]) {
                len[i] = 1;
                for j in 0..i {
                    if has_key(&new[j])
                        && old_member(&new[j])
                        && positions.get(&new[j]).is_some_and(|a| positions.get(&new[i]).is_some_and(|b| a < b))
                        && len[j] + 1 > len[i]
                    {
                        len[i] = len[j] + 1;
                        prev[i] = j;
                    }
                }
                if len[i] > best_len {
                    best_len = len[i];
                    best_end = Some(i);
                }
            }
        }
        let mut kept = vec![false; n];
        let mut walk = best_end;
        while let Some(i) = walk {
            kept[i] = true;
            walk = if prev[i] == usize::MAX { None } else { Some(prev[i]) };
        }
        // Walk the new order; assign fresh keys to runs of not-kept entities between the kept
        // neighbours (the key of the kept entity before the run and the one after it).
        fn flush(
            pending: &mut Vec<Handle>,
            prev_key: Option<&str>,
            next_key: Option<&str>,
            positions: &mut BTreeMap<Handle, String>,
            out: &mut Vec<Change>,
        ) {
            if pending.is_empty() {
                return;
            }
            let count = pending.len();
            if let Ok(fresh) = frac::n_between(prev_key, next_key, count) {
                for (h, key) in pending.drain(..).zip(fresh) {
                    positions.insert(h, key.clone());
                    out.push(Change::Set { id: props::hub_id(h), prop: Arc::from("position"), value: Value::Json(serde_json::Value::String(key)) });
                }
            }
        }
        let mut last_kept_key: Option<String> = None;
        let mut pending: Vec<Handle> = Vec::new();
        for (i, h) in new.iter().enumerate() {
            if kept[i] {
                let next_key = positions.get(h).cloned();
                flush(&mut pending, last_kept_key.as_deref(), next_key.as_deref(), &mut positions, out);
                last_kept_key = positions.get(h).cloned();
            } else {
                pending.push(*h);
            }
        }
        flush(&mut pending, last_kept_key.as_deref(), None, &mut positions, out);
    }
}

impl Default for CadcraftHub {
    fn default() -> Self {
        Self::new()
    }
}

impl HubAdapter for CadcraftHub {
    type Doc = Drawing;

    fn view(&self, doc: &Drawing) -> DocumentView {
        self.first_sync_keys(doc);
        let positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
        let mut nodes = Vec::new();
        for entity in doc.model.iter() {
            // Handle 0 is the core's root marker; a real drawing never carries it.
            if entity.handle.0 == 0 {
                continue;
            }
            let props = props::NODE_PROPS.iter().filter_map(|p| props::field_value(entity, p).map(|v| (Arc::from(*p), v))).collect();
            nodes.push(ViewNode {
                id: props::hub_id(entity.handle),
                kind: Arc::from(Self::kind_name(&entity.kind)),
                parent: ROOT,
                position: positions.get(&entity.handle).cloned().unwrap_or_default(),
                props,
            });
        }
        let entries = schema::entry_values(doc);
        let doc_props = props::DOC_PROPS.iter().filter_map(|p| props::doc_prop_value(doc, p).map(|v| ((*p).to_owned(), v))).collect();
        DocumentView { nodes, entries, doc: doc_props }
    }

    fn apply(&self, doc: &mut Drawing, changes: &[Change]) -> ApplyReport {
        let mut report = ApplyReport::default();
        for change in changes {
            match change {
                Change::Create { id, kind, parent, position, props } => {
                    let handle = handle_of(*id);
                    if handle.0 == 0 || doc.model.contains(handle) {
                        report.skipped += 1;
                        continue;
                    }
                    // Model space is the only parent (a CAD entity accepts no children); a
                    // hostile create that claims another one has nowhere to live here.
                    if *parent != ROOT {
                        report.skipped += 1;
                        continue;
                    }
                    // The full payload rides the `kind` property; the discriminant string is for
                    // validation and display.
                    let entity_kind = props
                        .iter()
                        .find(|(p, _)| p.as_ref() == "kind")
                        .and_then(|(_, v)| serde_json::from_value::<EntityKind>(v.to_json()).ok())
                        .or_else(|| Self::default_kind(kind));
                    let Some(entity_kind) = entity_kind else {
                        report.skipped += 1;
                        continue;
                    };
                    let mut entity = Entity::new(handle, entity_kind);
                    for (p, v) in props {
                        if p.as_ref() == "position" || p.as_ref() == "parent" {
                            continue;
                        }
                        match props::set_field(&mut entity, p, v) {
                            Some(Ok(())) => report.applied += 1,
                            Some(Err(_)) | None => report.skipped += 1,
                        }
                    }
                    self.set_key(handle, position.clone());
                    let keys = self.positions.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    doc.model.push(entity);
                    // Local handle allocation stays above every received id (the §5.1 discipline).
                    doc.bump_handseed(handle);
                    Self::reorder_model(doc, &keys);
                    report.applied += 1;
                }
                Change::Set { id, prop, value } => {
                    let handle = handle_of(*id);
                    if prop.as_ref() == "position" {
                        if let Value::Json(serde_json::Value::String(key)) = value {
                            self.set_key(handle, key.clone());
                            let keys = self.positions.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            Self::reorder_model(doc, &keys);
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    } else if prop.as_ref() == "parent" {
                        // Everything lives in model space; there is no other parent to move to.
                        report.skipped += 1;
                    } else {
                        // The copy-on-write modify: one chunk is cloned, the rest stay shared.
                        let mut outcome: Option<Option<Result<(), String>>> = None;
                        doc.model.modify(handle, |e| {
                            outcome = Some(props::set_field(e, prop, value));
                        });
                        match outcome {
                            Some(Some(Ok(()))) => report.applied += 1,
                            Some(Some(Err(_))) | Some(None) | None => report.skipped += 1,
                        }
                    }
                }
                Change::Delete { id } => {
                    let handle = handle_of(*id);
                    if doc.model.contains(handle) {
                        self.drop_key(handle);
                        if doc.remove_entity(handle).is_some() {
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    }
                    // Deleting a deleted entity does nothing (v2 tolerance, kept). Constraints
                    // referencing it stay for the app's own purge (cadcraft-constraints'
                    // `purge_invalid`), exactly as a local delete leaves them.
                }
                Change::SetDoc { prop, value } => {
                    let routed = props::set_doc_prop(doc, prop, value).or_else(|| schema::set_doc_order(doc, prop, value));
                    match routed {
                        Some(Ok(())) => report.applied += 1,
                        Some(Err(_)) | None => report.skipped += 1,
                    }
                }
                Change::Put { entry, value } => match schema::put_entry(doc, entry, value) {
                    Ok(()) => report.applied += 1,
                    Err(_) => report.skipped += 1,
                },
                Change::Remove { entry } => match schema::remove_entry(doc, entry) {
                    Ok(()) => report.applied += 1,
                    Err(_) => report.skipped += 1,
                },
            }
        }
        report
    }

    fn diff(&self, before: &Drawing, after: &Drawing) -> Vec<Change> {
        let mut sets = Vec::new();
        let mut creates = Vec::new();

        if !before.model.same_as(&after.model) {
            for entity in after.model.iter() {
                let handle = entity.handle;
                if handle.0 == 0 {
                    continue;
                }
                match before.model.get(handle) {
                    // Arc-identical entities are skipped — the copy-on-write store shares what
                    // didn't change, so the walk is edit-sized.
                    Some(before_arc) if Arc::ptr_eq(before_arc, entity) => {}
                    Some(_) => {
                        if let Some(b) = before.model.get(handle) {
                            for prop in props::NODE_PROPS {
                                if props::field_differs(b, entity, prop).unwrap_or(false)
                                    && let Some(v) = props::field_value(entity, prop)
                                {
                                    sets.push(Change::Set { id: props::hub_id(handle), prop: Arc::from(*prop), value: v });
                                }
                            }
                        }
                    }
                    None => {
                        // A creation: the whole entity (the order pass below assigns its key).
                        let props_vec = props::NODE_PROPS.iter().filter_map(|p| props::field_value(entity, p).map(|v| (Arc::from(*p), v))).collect();
                        creates.push(Change::Create {
                            id: props::hub_id(handle),
                            kind: Arc::from(Self::kind_name(&entity.kind)),
                            parent: ROOT,
                            position: String::new(),
                            props: props_vec,
                        });
                    }
                }
            }
        }

        // Deletions: one Delete per entity (flat model space — there are no subtrees).
        let deletes: Vec<Change> = before
            .model
            .handles()
            .into_iter()
            .filter(|h| h.0 != 0 && !after.model.contains(*h))
            .map(|h| Change::Delete { id: props::hub_id(h) })
            .collect();

        // Draw order (the only parent is the root, so one order pass).
        let mut order_changes = Vec::new();
        self.diff_order(before, after, &mut order_changes);

        // The creations' keys, now that the order pass has assigned them.
        let positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
        for create in &mut creates {
            if let Change::Create { id, position, .. } = create {
                *position = positions.get(&handle_of(*id)).cloned().unwrap_or_default();
            }
        }

        // Document-level scalars and entries.
        let mut doc_level = Vec::new();
        for prop in props::DOC_PROPS {
            if props::doc_prop_value(before, prop) != props::doc_prop_value(after, prop)
                && let Some(v) = props::doc_prop_value(after, prop)
            {
                doc_level.push(Change::SetDoc { prop: Arc::from(*prop), value: v });
            }
        }
        doc_level.extend(schema::diff_entries(before, after));

        // Batch order (§2.3): field writes, creations, order, deletes, doc level.
        let mut batch = sets;
        batch.append(&mut creates);
        batch.append(&mut order_changes);
        batch.extend(deletes);
        batch.append(&mut doc_level);
        batch
    }

    fn presence(&self, doc: &Drawing, ui: &UiState) -> Option<craft_collab::presence::PresenceMsg> {
        let page = ui.page.or_else(|| Some(active_tab(doc)))?;
        Some(craft_collab::presence::PresenceMsg {
            cursor: ui.cursor,
            page: Some(page),
            selection: ui.selection.clone(),
            viewport: ui.viewport,
            // The text editor's caret projection needs the editor state the follow-up wiring
            // owns; nothing fakes one here.
            editing_text: ui.editing_text,
            playhead: None,    // CADCraft has no timeline
            track_range: None, // CADCraft has no tracks
            generating: None,  // CADCraft runs no generation jobs
        })
    }

    fn schema(&self) -> &dyn craft_collab::adapter::DocSchema {
        &self.schema
    }
}

/// The active tab as a number for the presence row: 0 is model space, the current layout's
/// `tab_order` otherwise (`CTAB` names it; best effort — presence is ephemeral, and layouts
/// rename without moving tabs).
fn active_tab(doc: &Drawing) -> u64 {
    let tab = doc.header.str("CTAB", "Model");
    if tab == "Model" { 0 } else { doc.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(&tab)).map_or(0, |l| u64::from(l.tab_order)) }
}
