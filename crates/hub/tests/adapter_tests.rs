//! The adapter's ported convergence suite (CraftHub spec §3.6 phase B: "with its entire
//! convergence tests ported as a literal suite — the family's most valuable testing asset").
//!
//! Every test works over the *real* cadcraft drawing, driven through its public API, and the
//! guarantee under test is the family's own: `apply(diff(a, b), a) == b`, with draw-order
//! positions, for real edits — plus the two-client convergence of the generic state when both
//! sides edit, and CADCraft's own row-10 requirement: unit-bearing values that differ only in
//! the last bits must not fight (§18.4), proven against the real constraint solver.

// Tests may unwrap: a failing assertion is the point of a test (the family's clippy rule).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cadcraft_constraints as cs;
use cadcraft_doc::{Circle, Common, Constraint, ConstraintKind, Drawing, EntityKind, GeomRef, Handle, Space};
use cadcraft_hub::{CadcraftHub, ROOT};
use craft_collab::adapter::HubAdapter;
use craft_collab::ids::NodeId as HubId;

fn circle_at(radius: f64) -> EntityKind {
    EntityKind::Circle(Circle { center: cadcraft_doc::geom::Vec3::ZERO, radius })
}

fn mtext(insert_x: f64) -> EntityKind {
    EntityKind::MText(cadcraft_doc::MText {
        insert: cadcraft_doc::geom::Vec3::new(insert_x, 0.0, 0.0),
        height: 2.5,
        width: 0.0,
        attach: 1,
        rotation: 0.0,
        style: "Standard".into(),
        contents: "Notes".into(),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
    })
}

fn sample() -> Drawing {
    let mut doc = Drawing::new_metric();
    let walls = Common { layer: "Walls".into(), ..Common::default() };
    doc.add(&Space::Model, Common::default(), circle_at(30.0)).unwrap();
    doc.add(
        &Space::Model,
        walls,
        EntityKind::Line(cadcraft_doc::Line { a: cadcraft_doc::geom::Vec3::ZERO, b: cadcraft_doc::geom::Vec3::new(100.0, 0.0, 0.0) }),
    )
    .unwrap();
    doc.add(
        &Space::Model,
        Common::default(),
        EntityKind::LwPolyline(cadcraft_doc::LwPolyline { vertices: Vec::new(), closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }),
    )
    .unwrap();
    doc.add(&Space::Model, Common { layer: "Notes".into(), ..Common::default() }, mtext(10.0)).unwrap();
    doc
}

fn entity_by_layer(doc: &Drawing, layer: &str) -> Handle {
    doc.model.iter().find(|e| e.common.layer == layer).map(|e| e.handle).unwrap()
}

fn circle_handle(doc: &Drawing) -> Handle {
    doc.model.iter().find(|e| matches!(e.kind, EntityKind::Circle(_))).map(|e| e.handle).unwrap()
}

fn radius_of(doc: &Drawing, h: Handle) -> f64 {
    match &doc.model.get(h).unwrap().kind {
        EntityKind::Circle(c) => c.radius,
        _ => panic!("not a circle"),
    }
}

fn assert_same_state(hub: &CadcraftHub, a: &Drawing, b: &Drawing, what: &str) {
    let (va, vb) = (hub.view(a), hub.view(b));
    assert_eq!(va.nodes.len(), vb.nodes.len(), "{what}: entity count");
    for (na, nb) in va.nodes.iter().zip(vb.nodes.iter()) {
        assert_eq!(na.id, nb.id, "{what}: id");
        assert_eq!(na.kind, nb.kind, "{what}: kind of {na:?}");
        assert_eq!(na.parent, nb.parent, "{what}: parent of {na:?}");
        assert_eq!(na.position, nb.position, "{what}: draw order of {na:?}");
        assert_eq!(na.props, nb.props, "{what}: props of {na:?}");
    }
    assert_eq!(va.entries, vb.entries, "{what}: entries");
    assert_eq!(va.doc, vb.doc, "{what}: doc props");
}

/// The family's core promise, kept: applying the diff of two drawings to the first makes it the
/// second — field edits, creations, deletions, draw-order moves, unit changes, entries.
#[test]
fn apply_diff_roundtrip() {
    let hub = CadcraftHub::new();
    let a = sample();
    let mut b = a.clone();

    // Field edits: the common properties and the kind payload.
    let circle = circle_handle(&b);
    b.modify_entity(circle, |e| {
        e.common.layer = "Dimensions".into();
        e.common.ltscale = 2.5;
        e.kind = circle_at(31.5);
    })
    .unwrap();

    // Creation, deletion, draw-order move. The store's order edits return `bool` (whether the
    // move happened), not `Result` — assert they did, which is the stronger claim anyway.
    let added = b
        .add(
            &Space::Model,
            Common::default(),
            EntityKind::Arc(cadcraft_doc::Arc { center: cadcraft_doc::geom::Vec3::ZERO, radius: 12.5, start: 0.0, end: 1.0 }),
        )
        .unwrap();
    b.remove_entity(entity_by_layer(&b, "Notes")).unwrap();
    assert!(b.model.bring_to_front(entity_by_layer(&b, "Walls")));
    assert!(b.model.move_next_to(added, circle, false));

    // Document level: the unit system.
    b.header.set_i64("INSUNITS", 6);
    b.header.set_i64("LUPREC", 6);

    // Entries: a new layer, a swapped order, a user parameter.
    b.layers.push(cadcraft_doc::Layer::new("Dims"));
    b.layers.swap(0, 1);
    b.parametric.parameters.push(cadcraft_doc::Parameter { name: "width".into(), expr: "120".into(), description: String::new() });

    let changes = hub.diff(&a, &b);
    assert!(!changes.is_empty());

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "roundtrip");
    // The roundtrip is stable: diffing again finds nothing.
    assert!(hub.diff(&applied, &b).is_empty(), "the applied state diffs clean against the target");
    // And the draw order really moved. "Front" in the store's draw order is *drawn on top* — the
    // end of the handle list (and the arc's `move_next_to` put it before the circle, so Walls is
    // the last handle, not the first).
    let order = applied.model.handles();
    assert_eq!(order.last().copied(), Some(entity_by_layer(&applied, "Walls")), "bring-to-front landed (drawn on top)");
}

/// An insertion in the middle of the draw order costs one position key — the others keep theirs.
#[test]
fn insertion_in_the_middle_costs_one_key() {
    let hub = CadcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    let middle = b.add(&Space::Model, Common::default(), circle_at(5.0)).unwrap();
    // `bool`, not `Result` (see the roundtrip test): assert the move happened.
    assert!(b.model.move_next_to(middle, entity_by_layer(&b, "Walls"), false));

    let changes = hub.diff(&a, &b);
    let positions = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Set { prop, .. } if prop.as_ref() == "position")).count();
    let creates = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Create { .. })).count();
    assert_eq!(creates, 1, "one created entity");
    assert_eq!(positions, 1, "exactly one position write: the inserted entity");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "insert");
    // And the new entity really sits under the Walls line. The store's `move_next_to(…, above:
    // false)` inserts immediately *before* the anchor in the list — entities later in the list
    // draw on top, so "under the anchor" is one slot ahead of it in handle order.
    let order = applied.model.handles();
    let at = order.iter().position(|h| *h == middle).unwrap();
    let walls = order.iter().position(|h| *h == entity_by_layer(&applied, "Walls")).unwrap();
    assert_eq!(at + 1, walls, "the inserted entity sits directly under the anchor");
}

/// Two clients editing different properties of the same entity converge (the server orders the
/// batches; last-writer-wins per property, and neither side loses the other's write).
#[test]
fn two_clients_converge() {
    let hub = CadcraftHub::new();
    let base = sample();

    let mut local = base.clone();
    let mut remote = base.clone();
    let circle_l = circle_handle(&local);
    let circle_r = circle_handle(&remote);
    local.modify_entity(circle_l, |e| e.common.layer = "Local layer".into()).unwrap();
    remote.modify_entity(circle_r, |e| e.common.ltscale = 3.0).unwrap();

    let local_batch = hub.diff(&base, &local);
    let remote_batch = hub.diff(&base, &remote);
    assert_eq!(local_batch.len(), 1);
    assert_eq!(remote_batch.len(), 1);

    // Both sides receive both batches (the server echoes each to everyone).
    hub.apply(&mut local, &remote_batch);
    hub.apply(&mut remote, &local_batch);
    assert_same_state(&hub, &local, &remote, "convergence");
}

/// Deleting an entity is one change, and deleting a deleted entity does nothing.
#[test]
fn delete_is_one_change_and_tolerant() {
    let hub = CadcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    b.remove_entity(circle_handle(&b)).unwrap();

    let changes = hub.diff(&a, &b);
    let deletes = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Delete { .. })).count();
    assert_eq!(deletes, 1, "one Delete for the entity");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "delete");
    // Tolerant re-apply: deleting a deleted entity does nothing (skipped, not an error).
    let report = hub.apply(&mut applied, &changes);
    assert_eq!(report.applied, 0);
}

/// Document-level lists sync per entry with their order (v2's keyed-list discipline).
#[test]
fn document_entries_sync_with_order() {
    let hub = CadcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    let mut dims = cadcraft_doc::Layer::new("Dims");
    dims.color = cadcraft_doc::color::Color::Index(3);
    b.layers.push(dims);
    b.layers.swap(0, 1);

    let changes = hub.diff(&a, &b);
    let puts = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Put { .. })).count();
    let orders = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::SetDoc { prop, .. } if prop.as_ref() == "layerOrder")).count();
    assert_eq!(puts, 1, "one Put for the new layer");
    assert_eq!(orders, 1, "one order property for the swap");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "layers");
    assert_eq!(applied.layers[0].name, "Defpoints", "the order swap landed");
}

/// The schema table: kinds and parents validate, and — CADCraft's row-10 addition — unit-bearing
/// values outside their ranges are refused with a reason.
#[test]
fn schema_validates_kinds_parents_and_units() {
    use craft_collab::adapter::DocSchema;
    use craft_collab::change::Change;
    use craft_collab::replica::Replica;
    use craft_collab::value::Value;
    use std::sync::Arc;

    let schema = cadcraft_hub::schema::CadSchema::default();
    let replica = Replica::default();
    let ctx = craft_collab::adapter::ValidationCtx { replica: &replica, author_space: 1, limits: &craft_collab::Limits::default() };

    // A circle in model space (the root): accepted.
    let circle = Change::Create {
        id: craft_collab::ids::NodeId::compose(1, 1),
        kind: Arc::from("circle"),
        parent: ROOT,
        position: "a0".into(),
        props: vec![(
            Arc::from("kind"),
            Value::Json(serde_json::json!({ "type": "circle", "center": { "x": 0.0, "y": 0.0, "z": 0.0 }, "radius": 25.0 })),
        )],
    };
    assert!(schema.validate(&circle, &ctx).is_ok());

    // An unknown kind: refused with a reason.
    let unknown = Change::Create {
        id: craft_collab::ids::NodeId::compose(1, 2),
        kind: Arc::from("hologram"),
        parent: ROOT,
        position: "a1".into(),
        props: vec![],
    };
    assert!(schema.validate(&unknown, &ctx).is_err());

    // An entity under another entity: CAD accepts no children; entities live in model space.
    let parented = Change::Create {
        id: craft_collab::ids::NodeId::compose(1, 3),
        kind: Arc::from("line"),
        parent: HubId(1),
        position: "a2".into(),
        props: vec![],
    };
    assert!(schema.validate(&parented, &ctx).is_err());

    // Unit-bearing values: a negative radius never rides a batch.
    let bad_radius = Change::Set {
        id: craft_collab::ids::NodeId::compose(1, 4),
        prop: Arc::from("kind"),
        value: Value::Json(serde_json::json!({ "type": "circle", "center": { "x": 0.0, "y": 0.0, "z": 0.0 }, "radius": -5.0 })),
    };
    assert!(schema.validate(&bad_radius, &ctx).is_err());

    // Zero text height, either.
    let bad_height = Change::Set {
        id: craft_collab::ids::NodeId::compose(1, 5),
        prop: Arc::from("kind"),
        value: Value::Json(
            serde_json::json!({ "type": "text", "insert": { "x": 0.0, "y": 0.0, "z": 0.0 }, "height": 0.0, "value": "", "widthFactor": 1.0, "style": "Standard", "halign": "left", "valign": "baseline" }),
        ),
    };
    assert!(schema.validate(&bad_height, &ctx).is_err());

    // Linetype scale has a real range (0, 1e6].
    let set = |prop: &str, v: f64| Change::Set {
        id: craft_collab::ids::NodeId::compose(1, 6),
        prop: Arc::from(prop),
        value: Value::Json(serde_json::json!(v)),
    };
    assert!(schema.validate(&set("ltscale", 2.0), &ctx).is_ok());
    assert!(schema.validate(&set("ltscale", 0.0), &ctx).is_err());
    assert!(schema.validate(&set("ltscale", 1.0e7), &ctx).is_err());

    // The unit-system document scalars sit in their real enums.
    let doc = |prop: &str, v: i64| Change::SetDoc { prop: Arc::from(prop), value: Value::Json(serde_json::json!(v)) };
    assert!(schema.validate(&doc("insunits", 4), &ctx).is_ok());
    assert!(schema.validate(&doc("insunits", 99), &ctx).is_err());
    assert!(schema.validate(&doc("lunits", 9), &ctx).is_err());
    assert!(schema.validate(&doc("luprec", 8), &ctx).is_ok());
    assert!(schema.validate(&doc("auprec", 9), &ctx).is_err());
    assert!(schema.validate(&doc("measurement", 2), &ctx).is_err());
}

/// CADCraft's row-10 tolerance test (§18.4): equal-value-within-epsilon writes do not fight,
/// and concurrent edits to a constraint resolve within tolerance — against the real solver.
///
/// Two collaborators drag the same circle to "the same" radius, differing by 1e-9 drawing units.
/// Both diffs canonicalize the radius to the same ten-significant-digit value, so the two
/// batches are *identical bytes*: there is nothing to fight over, no echo storm, no oscillation.
/// After the exchange (and after each side's solver run), the constraint holds on both sides and
/// the geometry agrees within tolerance.
#[test]
fn equal_value_within_epsilon_writes_do_not_fight() {
    let hub = CadcraftHub::new();
    let mut base = Drawing::new_metric();
    let circle = base.add(&Space::Model, Common::default(), circle_at(30.0)).unwrap();
    // A real radius constraint, added through the real API (it solves and pins the geometry).
    cs::add(
        &mut base,
        Constraint { id: 0, kind: ConstraintKind::Radius, refs: vec![GeomRef::whole(circle)], name: String::new(), expr: String::new() },
        &cs::SolveOptions::default(),
    )
    .unwrap();
    assert!(cs::all_satisfied(&base));
    let pinned: f64 = radius_of(&base, circle);

    // Two clients concurrently set the radius to values that differ by 1e-9 units, and edit the
    // constraint's expression to the matching value.
    let target = (pinned - 5.0).max(0.0);
    let mut local = base.clone();
    let mut remote = base.clone();
    local.modify_entity(circle, |e| e.kind = circle_at(target)).unwrap();
    remote.modify_entity(circle, |e| e.kind = circle_at(target + 1.0e-9)).unwrap();
    for d in [&mut local, &mut remote] {
        d.constraints[0].expr = target.to_string();
    }

    let local_batch = hub.diff(&base, &local);
    let remote_batch = hub.diff(&base, &remote);
    assert_eq!(local_batch.len(), 2, "one geometry write and one constraint write");
    assert_eq!(remote_batch.len(), 2);
    // The epsilon never reaches the wire: both batches canonicalize to the same bytes.
    assert_eq!(local_batch, remote_batch, "equal-within-epsilon writes produce identical batches");

    // The exchange, in the server's order on both sides: converged, and no residue.
    for d in [&mut local, &mut remote] {
        hub.apply(d, &local_batch);
        hub.apply(d, &remote_batch);
    }
    assert_same_state(&hub, &local, &remote, "epsilon convergence");
    assert!(hub.diff(&local, &remote).is_empty(), "no follow-up writes: nothing fought");

    // The solver agrees on both sides: the constraint resolves within tolerance.
    cs::solve(&mut local, &cs::SolveOptions::default()).unwrap();
    cs::solve(&mut remote, &cs::SolveOptions::default()).unwrap();
    assert!(cs::all_satisfied(&local));
    assert!(cs::all_satisfied(&remote));
    let (rl, rr) = (radius_of(&local, circle), radius_of(&remote, circle));
    assert!((rl - rr).abs() < 1.0e-9, "the resolved radii agree within 1e-9: {rl} vs {rr}");
    let residual = cs::residuals(&local).into_iter().find(|(id, _)| *id == local.constraints[0].id).and_then(|(_, r)| r).unwrap_or(0.0);
    assert!(residual.abs() <= local.parametric.settings.distance_tolerance, "the residual {residual} is within the drawing's tolerance");
}

/// The command catalog resolves the legacy names onto their `hub.*` twins (§18.10 compatibility).
#[test]
fn command_catalog_resolves_legacy_names() {
    assert_eq!(cadcraft_hub::commands::resolve("collab.share").map(|c| c.name), Some("hub.share"));
    assert_eq!(cadcraft_hub::commands::resolve("hub.peers").map(|c| c.name), Some("hub.peers"));
    assert!(cadcraft_hub::commands::resolve("hub.teleport").is_none());
}
