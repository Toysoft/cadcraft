//! XCLIP: clip one block or xref reference without changing its definition.

use cadcraft_doc::ClipBoundary;
use cadcraft_geom::Vec2;
use serde_json::{Value, json};

use super::*;

const MAX_POINTS: usize = cadcraft_render::clip::MAX_CLIP_POINTS;

pub(super) fn spec() -> CommandSpec {
    CommandSpec::new("xclip", "Clip Reference", run)
        .menu(&["Modify", "Clip", "Xref"])
        .params("{handle: block or xref reference, option?: new|on|off|delete|query (default new when a boundary is given), rectangle?: [[x,y],[x,y]], points?: [[x,y],...] (3–4096 polygon vertices)}")
        .interactive(|_| Ok(Box::new(XclipM::default())))
}

/// Boundary vertices supplied in world coordinates, with a rectangular pair expanded before
/// applying the inverse reference transform. A rectangle in WCS may be a polygon in block space.
fn world_boundary(p: &Value) -> Result<Vec<Vec2>> {
    if let Some(rect) = p.get("rectangle") {
        let a = rect.as_array().ok_or_else(|| bad("xclip", "`rectangle` needs two points"))?;
        let (Some(lo), Some(hi)) = (a.first().and_then(point_value), a.get(1).and_then(point_value)) else {
            return Err(bad("xclip", "`rectangle` needs two finite points"));
        };
        if a.len() != 2 || (lo.x - hi.x).abs() <= 1e-12 || (lo.y - hi.y).abs() <= 1e-12 {
            return Err(bad("xclip", "the clipping rectangle must have positive width and height"));
        }
        return Ok(vec![
            Vec2::new(lo.x.min(hi.x), lo.y.min(hi.y)),
            Vec2::new(lo.x.max(hi.x), lo.y.min(hi.y)),
            Vec2::new(lo.x.max(hi.x), lo.y.max(hi.y)),
            Vec2::new(lo.x.min(hi.x), lo.y.max(hi.y)),
        ]);
    }
    let pts = p.get("points").and_then(Value::as_array).ok_or_else(|| bad("xclip", "`points` or `rectangle` is required"))?;
    if !(3..=MAX_POINTS).contains(&pts.len()) {
        return Err(bad("xclip", format!("the boundary needs 3–{MAX_POINTS} points")));
    }
    pts.iter().map(|v| point_value(v).ok_or_else(|| bad("xclip", "every boundary point must be finite [x,y]"))).collect()
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = crate::cmd::targets(s, p)?;
    let [h] = hs.as_slice() else { return Err(bad("xclip", "select one block or xref reference")) };
    let (ins, base) = {
        let d = s.doc()?;
        let e = d.entity(*h).ok_or_else(|| bad("xclip", "the reference no longer exists"))?;
        let EntityKind::Insert(ins) = &e.kind else { return Err(bad("xclip", "the selected object is not a block or xref reference")) };
        let b = d.block(&ins.block).ok_or_else(|| bad("xclip", "the referenced block is missing"))?;
        (ins.clone(), b.base.xy())
    };
    let option = str_param(p, "option").unwrap_or(if p.get("points").is_some() || p.get("rectangle").is_some() { "new" } else { "query" });
    let next = match option.to_ascii_lowercase().as_str() {
        "new" => {
            let world = world_boundary(p)?;
            if cadcraft_render::clip::PolyClip::new(&world).is_none() {
                return Err(bad("xclip", "the clipping boundary is degenerate"));
            }
            let inv = ins.transform(base).inverse().ok_or_else(|| bad("xclip", "the reference has a zero scale"))?;
            let local = world.into_iter().map(|v| inv.apply(v)).collect();
            Some(ClipBoundary { points: local, enabled: true, inverted: false, deleted: false })
        }
        "on" | "off" => {
            let mut clip = ins.clip.filter(|c| !c.deleted).ok_or_else(|| bad("xclip", "this reference has no clipping boundary"))?;
            clip.enabled = option.eq_ignore_ascii_case("on");
            Some(clip)
        }
        "delete" => Some(ClipBoundary { points: Vec::new(), enabled: false, inverted: false, deleted: true }),
        "query" => {
            let live = ins.clip.as_ref().filter(|c| !c.deleted);
            let boundary = live.map(|c| c.points.iter().map(|v| ins.transform(base).apply(*v)).collect::<Vec<_>>());
            return Ok(
                json!({ "handle": h.hex(), "boundary": boundary, "enabled": live.is_some_and(|c| c.enabled), "inverted": live.is_some_and(|c| c.inverted) }),
            );
        }
        _ => return Err(bad("xclip", format!("unknown option \"{option}\""))),
    };
    s.doc_mut()?.modify_entity(*h, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.clip = next;
        }
    })?;
    Ok(json!({ "handle": h.hex(), "option": option.to_ascii_lowercase() }))
}

#[derive(Default)]
enum Stage {
    #[default]
    Select,
    Option,
    BoundaryType,
    RectFirst,
    RectSecond,
    Polygon,
}

#[derive(Default)]
struct XclipM {
    sel: SelectPhase,
    stage: Stage,
    handle: Option<Handle>,
    points: Vec<Vec2>,
}

impl XclipM {
    fn selected(&mut self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        self.handle = hs.iter().copied().find(|h| s.doc().ok().and_then(|d| d.entity(*h)).is_some_and(|e| matches!(e.kind, EntityKind::Insert(_))));
        let Some(h) = self.handle else { return Err(bad("xclip", "select a block or xref reference")) };
        s.set_selection(vec![h]);
        self.stage = Stage::Option;
        Ok(Step::Continue)
    }

    fn apply(&self, s: &mut Session, option: &str, boundary: Option<Value>) -> Result<Step> {
        let Some(h) = self.handle else { return Err(bad("xclip", "select a reference")) };
        let mut p = json!({ "handle": h.hex(), "option": option });
        if let Some(v) = boundary {
            p[if option == "new-rectangle" { "rectangle" } else { "points" }] = v;
            p["option"] = json!("new");
        }
        run(s, &p)?;
        s.echo(format!("XCLIP: {option}."));
        Ok(Step::Done)
    }
}

impl Interactive for XclipM {
    fn name(&self) -> &'static str {
        "XCLIP"
    }

    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        self.sel.single = true;
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return self.selected(s, &hs);
        }
        Ok(Step::Continue)
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => Prompt::new("Select block or xref reference", Accept::SELECT),
            Stage::Option => Prompt::new("Enter clipping option", Accept::TEXT).kw(&["New", "ON", "OFF", "Delete"]).default("New"),
            Stage::BoundaryType => Prompt::new("Specify clipping boundary", Accept::TEXT).kw(&["Rectangular", "Polygonal"]).default("Rectangular"),
            Stage::RectFirst => Prompt::new("Specify first corner", Accept::POINT),
            Stage::RectSecond => Prompt::new("Specify opposite corner", Accept::POINT).base_opt(self.points.first().copied()),
            Stage::Polygon => Prompt::new(
                if self.points.is_empty() { "Specify first boundary point" } else { "Specify next boundary point or press Enter to close" },
                Accept::POINT,
            )
            .kw(&["Undo"])
            .base_opt(self.points.last().copied()),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.stage {
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.selected(s, &hs),
            },
            Stage::Option => match i {
                Input::Keyword(k) if k.eq_ignore_ascii_case("new") => {
                    self.stage = Stage::BoundaryType;
                    Ok(Step::Continue)
                }
                Input::Enter => {
                    self.stage = Stage::BoundaryType;
                    Ok(Step::Continue)
                }
                Input::Keyword(k) if k.eq_ignore_ascii_case("on") => self.apply(s, "on", None),
                Input::Keyword(k) if k.eq_ignore_ascii_case("off") => self.apply(s, "off", None),
                Input::Keyword(k) if k.eq_ignore_ascii_case("delete") => self.apply(s, "delete", None),
                _ => Err(bad("xclip", "choose New, ON, OFF or Delete")),
            },
            Stage::BoundaryType => match i {
                Input::Keyword(k) if k.eq_ignore_ascii_case("polygonal") => {
                    self.points.clear();
                    self.stage = Stage::Polygon;
                    Ok(Step::Continue)
                }
                Input::Keyword(k) if k.eq_ignore_ascii_case("rectangular") => {
                    self.points.clear();
                    self.stage = Stage::RectFirst;
                    Ok(Step::Continue)
                }
                Input::Enter => {
                    self.points.clear();
                    self.stage = Stage::RectFirst;
                    Ok(Step::Continue)
                }
                _ => Err(bad("xclip", "choose Rectangular or Polygonal")),
            },
            Stage::RectFirst => match i {
                Input::Point(p) => {
                    self.points.push(p);
                    self.stage = Stage::RectSecond;
                    Ok(Step::Continue)
                }
                _ => Err(bad("xclip", "specify the first corner")),
            },
            Stage::RectSecond => match i {
                Input::Point(p) => self.apply(s, "new-rectangle", Some(json!([self.points.first(), p]))),
                _ => Err(bad("xclip", "specify the opposite corner")),
            },
            Stage::Polygon => match i {
                Input::Point(p) if self.points.len() < MAX_POINTS => {
                    self.points.push(p);
                    Ok(Step::Continue)
                }
                Input::Keyword(k) if k.eq_ignore_ascii_case("undo") => {
                    self.points.pop();
                    Ok(Step::Continue)
                }
                Input::Enter if self.points.len() >= 3 => self.apply(s, "new", Some(json!(self.points))),
                Input::Enter => Err(bad("xclip", "a polygon needs at least three points")),
                _ => Err(bad("xclip", format!("the boundary allows at most {MAX_POINTS} points"))),
            },
        }
    }
}
