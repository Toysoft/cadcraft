//! What a table cell holds beyond its text: a block (AutoCAD's TINSERT) and the cell's own
//! borders, drawn over its cell style's.

use std::collections::{BTreeMap, HashSet};

use cadcraft_geom::{Bounds2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::dynblock::{ParameterKind, dynamic_block};
use crate::{Attrib, Block, CellBorder, Drawing, Entity, EntityKind, Handle, Insert, Lineweight, entity_bounds};

/// The most attribute values and custom properties a cell block keeps (hostile input).
pub const MAX_CELL_BLOCK_VALUES: usize = 256;

/// A block shown in a table cell. The cell refers to the block definition, so editing the block
/// changes every cell that shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CellBlock {
    /// The block's name.
    pub name: String,
    /// Scale of the block in the cell; `None`: fitted into the cell inside its margins
    /// (AutoCAD's AutoFit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Rotation in radians.
    pub rotation: f64,
    /// Attribute values by tag (tags match in any case); missing tags show their default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub attribs: BTreeMap<String, String>,
    /// Dynamic block custom properties by name, as the Properties palette names them
    /// (`Visibility1` → a visibility state).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, String>,
}

impl CellBlock {
    /// A copy with every value in range (hostile input from files and JSON).
    pub fn sanitized(mut self) -> CellBlock {
        self.name = self.name.trim().chars().take(255).collect();
        self.scale = self.scale.filter(|s| s.is_finite() && *s > 0.0).map(|s| s.min(1e9));
        if !self.rotation.is_finite() {
            self.rotation = 0.0;
        }
        let clean = |m: BTreeMap<String, String>| -> BTreeMap<String, String> {
            m.into_iter()
                .filter(|(k, _)| !k.trim().is_empty())
                .take(MAX_CELL_BLOCK_VALUES)
                .map(|(k, v)| (k.trim().chars().take(255).collect(), v.chars().take(4096).collect()))
                .collect()
        };
        self.attribs = clean(self.attribs);
        self.props = clean(self.props);
        self
    }

    /// The value given for attribute `tag` (any case).
    pub fn attrib(&self, tag: &str) -> Option<&str> {
        self.attribs.iter().find(|(k, _)| k.eq_ignore_ascii_case(tag)).map(|(_, v)| v.as_str())
    }

    /// The value given for custom property `name` (any case).
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A cell's own borders; `None` edges follow the cell style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CellEdges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<CellBorder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<CellBorder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<CellBorder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<CellBorder>,
}

impl CellEdges {
    /// Whether no edge is overridden.
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.right.is_none() && self.bottom.is_none() && self.left.is_none()
    }
}

/// A cell's own look over its cell style's: alignment, margins and text rotation. `None`
/// values follow the cell style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CellLook {
    /// MTEXT attachment, 1 top left … 9 bottom right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<u8>,
    /// Margin between the cell's sides and its content, along the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_h: Option<f64>,
    /// Margin across the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_v: Option<f64>,
    /// Text rotation in degrees: 0, 90 (reading upwards), 180 or 270.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<u16>,
}

impl CellLook {
    /// Whether nothing is overridden.
    pub fn is_empty(&self) -> bool {
        *self == CellLook::default()
    }

    /// A copy with every value in range (hostile input from files and JSON): alignments
    /// 1..=9, finite non-negative margins, quarter-turn rotations.
    pub fn sanitized(self) -> CellLook {
        let margin = |m: Option<f64>| m.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(1e9));
        CellLook {
            align: self.align.filter(|a| (1..=9).contains(a)),
            margin_h: margin(self.margin_h),
            margin_v: margin(self.margin_v),
            rotation: self.rotation.filter(|r| [0, 90, 180, 270].contains(r)),
        }
    }
}

/// How heavy a border looks, to pick between two borders on one edge: an invisible border is
/// lightest, a logical lineweight counts as the default 0.25 mm.
pub fn border_weight(b: &CellBorder) -> i32 {
    if !b.visible {
        return -1;
    }
    match b.lineweight {
        Lineweight::Mm100(v) => i32::from(v),
        Lineweight::ByLayer | Lineweight::ByBlock | Lineweight::Default => 25,
    }
}

/// The border drawn on an edge two cells share when both give one: the heavier wins, the
/// second on a tie (so a later override replaces an earlier one).
pub fn heavier(a: CellBorder, b: CellBorder) -> CellBorder {
    if border_weight(&a) > border_weight(&b) { a } else { b }
}

/// Which objects of a dynamic block a cell block's visibility state shows: the objects the
/// visibility parameter controls and, of those, the ones the chosen state shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellVisibility {
    pub controlled: HashSet<Handle>,
    pub shown: HashSet<Handle>,
}

impl CellVisibility {
    /// Whether object `e` of the block is drawn: a controlled object when its state shows it
    /// (whatever its own visibility in the definition), any other when it is visible.
    pub fn shows(&self, e: &Entity) -> bool {
        if self.controlled.contains(&e.handle) { self.shown.contains(&e.handle) } else { e.common.visible }
    }
}

/// The visibility state a cell block asks for, read from its dynamic block: the property named
/// as the visibility parameter (`Visibility1`), or `Visibility` for the block's first one.
/// Plain blocks and unknown states show the block as defined.
pub fn cell_visibility(d: &Drawing, cb: &CellBlock) -> CellVisibility {
    if cb.props.is_empty() {
        return CellVisibility::default();
    }
    let Some(m) = dynamic_block(d, &cb.name) else { return CellVisibility::default() };
    for p in &m.parameters {
        let ParameterKind::Visibility { label, controlled, states, .. } = &p.kind else { continue };
        let Some(want) = cb.prop(label).or_else(|| cb.prop("Visibility")) else { continue };
        let Some(state) = states.iter().find(|s| s.name.eq_ignore_ascii_case(want.trim())) else { continue };
        return CellVisibility { controlled: controlled.iter().copied().collect(), shown: state.visible.iter().copied().collect() };
    }
    CellVisibility::default()
}

/// A cell block placed in its cell: the block reference it draws as and the objects it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedCellBlock {
    pub insert: Insert,
    pub visibility: CellVisibility,
}

/// The attributes reference `ins` of `blk` shows: one per non-constant definition, placed by
/// the reference, with the cell's value for its tag or the definition's default.
fn cell_attribs(blk: &Block, ins: &Insert, cb: &CellBlock) -> Vec<Attrib> {
    let m = ins.transform(blk.base.xy());
    blk.entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::AttDef(ad) if !ad.constant => Some(ad),
            _ => None,
        })
        .map(|ad| {
            let mut k = EntityKind::Text(ad.text.clone());
            k.transform(&m);
            let mut text = if let EntityKind::Text(t) = k { t } else { ad.text.clone() };
            text.value = cb.attrib(&ad.tag).map(str::to_string).unwrap_or_else(|| ad.text.value.clone());
            Attrib {
                tag: ad.tag.clone(),
                text,
                invisible: ad.invisible,
                constant: false,
                prompt: String::new(),
                props: Default::default(),
                multiline: ad.multiline.as_ref().map(|ml| ml.transformed(&m)),
                modes: ad.modes.for_reference(),
            }
        })
        .collect()
}

/// The extents of reference `ins` drawing the objects `vis` shows, with its visible attributes.
fn shown_bounds(d: &Drawing, blk: &Block, ins: &Insert, vis: &CellVisibility) -> Bounds2 {
    let m = ins.transform(blk.base.xy());
    let mut b = Bounds2::EMPTY;
    for be in blk.entities.iter() {
        if matches!(be.kind, EntityKind::AttDef(_)) || !vis.shows(be) {
            continue;
        }
        if be.common.layer != "0" && d.layer(&be.common.layer).is_some_and(|l| !l.visible()) {
            continue;
        }
        let inner = entity_bounds(d, be, 1);
        if !inner.is_empty() {
            b = b.union(&Bounds2::from_points(inner.corners().map(|c| m.apply(c))));
        }
    }
    for a in ins.attribs.iter().filter(|a| !a.invisible && !a.text.value.trim().is_empty()) {
        let e = Entity::new(Handle(0), EntityKind::Text(a.text.clone()));
        b = b.union(&entity_bounds(d, &e, 1));
    }
    b
}

/// Place cell block `cb` in the cell spanning `min` … `max` (world coordinates), inside margins
/// `margin` (horizontal, vertical), aligned by `attach` (MTEXT numbering, 1 top left … 9 bottom
/// right): fitted into the cell unless it has its own scale. `None` when the block is missing.
pub fn place_cell_block(d: &Drawing, cb: &CellBlock, vis: CellVisibility, min: Vec2, max: Vec2, margin: Vec2, attach: u8) -> Option<PlacedCellBlock> {
    let blk = d.block(&cb.name)?;
    let rotation = if cb.rotation.is_finite() { cb.rotation } else { 0.0 };
    let mut ins = Insert {
        block: blk.name.clone(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
    };
    ins.attribs = cell_attribs(blk, &ins, cb);
    let unit = shown_bounds(d, blk, &ins, &vis);
    let avail = Vec2::new((max.x - min.x - 2.0 * margin.x).max(0.0), (max.y - min.y - 2.0 * margin.y).max(0.0));
    let fit = || {
        let k = |a: f64, b: f64| if b > 1e-12 { a / b } else { f64::INFINITY };
        let s = k(avail.x, unit.width()).min(k(avail.y, unit.height()));
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    };
    let scale = if unit.is_empty() { cb.scale.unwrap_or(1.0) } else { cb.scale.unwrap_or_else(fit) };
    let scale = if scale.is_finite() && scale > 0.0 { scale.clamp(1e-9, 1e9) } else { 1.0 };
    let (lo, hi) = if unit.is_empty() { (Vec2::ZERO, Vec2::ZERO) } else { (unit.min * scale, unit.max * scale) };
    let attach = attach.clamp(1, 9);
    let x = match (attach - 1) % 3 {
        0 => min.x + margin.x - lo.x,
        1 => (min.x + max.x) / 2.0 - (lo.x + hi.x) / 2.0,
        _ => max.x - margin.x - hi.x,
    };
    let y = match (attach - 1) / 3 {
        0 => max.y - margin.y - hi.y,
        1 => (min.y + max.y) / 2.0 - (lo.y + hi.y) / 2.0,
        _ => min.y + margin.y - lo.y,
    };
    ins.scale = Vec3::new(scale, scale, scale);
    // The bounds were measured with the reference at the origin: they scale about it and move
    // with the insertion point.
    ins.insert = Vec3::new(x, y, 0.0);
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    ins.attribs = cell_attribs(blk, &ins, cb);
    Some(PlacedCellBlock { insert: ins, visibility: vis })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_cell_block_is_sanitized() {
        let mut attribs = BTreeMap::new();
        for i in 0..1000 {
            attribs.insert(format!("T{i}"), "x".repeat(10_000));
        }
        attribs.insert("  ".into(), "dropped".into());
        let b = CellBlock { name: "  B ".into(), scale: Some(f64::NAN), rotation: f64::INFINITY, attribs, props: BTreeMap::new() }.sanitized();
        assert_eq!(b.name, "B");
        assert_eq!((b.scale, b.rotation), (None, 0.0));
        assert_eq!(b.attribs.len(), MAX_CELL_BLOCK_VALUES);
        assert!(b.attribs.values().all(|v| v.chars().count() <= 4096));
    }

    #[test]
    fn heavier_border_wins() {
        let thin = CellBorder { lineweight: Lineweight::Mm100(18), ..CellBorder::default() };
        let main = CellBorder { lineweight: Lineweight::Mm100(50), ..CellBorder::default() };
        let off = CellBorder { visible: false, lineweight: Lineweight::Mm100(200), ..CellBorder::default() };
        assert_eq!(heavier(thin, main), main);
        assert_eq!(heavier(main, thin), main);
        assert_eq!(heavier(off, thin), thin);
        assert_eq!(heavier(CellBorder::default(), thin), CellBorder::default());
    }
}
