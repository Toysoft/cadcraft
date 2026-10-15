//! Raster images on the canvas. Each loaded image is an egui texture drawn as a mesh over its
//! rectangle, under the drawing's lines (the drawing itself goes through the GPU callback, so
//! images are not yet interleaved with it in draw order).

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use cadcraft_doc::Raster;
use cadcraft_geom::{Bounds2, Vec2};
use cadcraft_render::{DImage, DisplayList, Kind};
use egui::{Color32, Mesh, Rect, Shape, TextureHandle, TextureOptions, epaint::Vertex, pos2};

use super::Xf;

/// The longest texture side uploaded, whatever the device allows (memory).
const MAX_TEXTURE_SIDE: usize = 8192;

/// Uploaded textures by the address of the pixels they were made from; an entry goes when its
/// pixels do.
#[derive(Clone, Default)]
struct Textures(HashMap<usize, (Weak<Raster>, TextureHandle)>);

/// The pixels of `raster` as an egui image, keeping only every `step`-th pixel of every
/// `step`-th row (all of them for a step of 0 or 1).
fn decimated(raster: &Raster, step: usize) -> Option<egui::ColorImage> {
    let width = usize::try_from(raster.width()).ok()?;
    let height = usize::try_from(raster.height()).ok()?;
    if step <= 1 {
        return Some(egui::ColorImage::from_rgba_unmultiplied([width, height], raster.rgba()));
    }
    let mut kept = Vec::new();
    for row in raster.rgba().chunks_exact(width.checked_mul(4)?).step_by(step) {
        for pixel in row.as_chunks::<4>().0.iter().step_by(step) {
            kept.extend_from_slice(pixel);
        }
    }
    let size = [width.div_ceil(step), height.div_ceil(step)];
    (kept.len() == size[0] * size[1] * 4).then(|| egui::ColorImage::from_rgba_unmultiplied(size, &kept))
}

/// Where the uploaded textures are kept between frames.
fn textures_id() -> egui::Id {
    egui::Id::new("cadcraft-raster-textures")
}

/// The texture of `raster`: the one uploaded earlier for these pixels, else a new upload.
fn texture(ctx: &egui::Context, raster: &Arc<Raster>) -> Option<TextureHandle> {
    let key = Arc::as_ptr(raster) as usize;
    let uploaded = ctx.data_mut(|d| {
        let textures = &mut d.get_temp_mut_or_default::<Textures>(textures_id()).0;
        // Textures whose pixels are gone (drawing closed, image undone) go too.
        textures.retain(|_, (pixels, _)| pixels.strong_count() > 0);
        textures.get(&key).map(|(_, texture)| texture.clone())
    });
    if uploaded.is_some() {
        return uploaded;
    }
    let texture = upload(ctx, raster, key)?;
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(textures_id()).0.insert(key, (Arc::downgrade(raster), texture.clone())));
    Some(texture)
}

/// Upload `raster`, thinned so its longer side fits a texture.
fn upload(ctx: &egui::Context, raster: &Raster, key: usize) -> Option<TextureHandle> {
    let longest_allowed = ctx.input(|i| i.max_texture_side).clamp(1, MAX_TEXTURE_SIDE);
    let longest = usize::try_from(raster.width().max(raster.height())).ok()?;
    let step = longest.div_ceil(longest_allowed);
    Some(ctx.load_texture(format!("raster-{key:x}"), decimated(raster, step)?, TextureOptions::LINEAR))
}

/// `outline` as a rectangle when it is one with sides along the axes.
fn axis_rect(outline: &[Vec2]) -> Option<Bounds2> {
    if outline.len() != 4 {
        return None;
    }
    let bounds = Bounds2::from_points(outline.iter().copied());
    // Four points are such a rectangle when each lies on a corner of their bounding box.
    let close = (bounds.width() + bounds.height()) * 1e-9;
    let on_either = |v: f64, low: f64, high: f64| (v - low).abs() <= close || (v - high).abs() <= close;
    let on_a_corner = |q: &Vec2| on_either(q.x, bounds.min.x, bounds.max.x) && on_either(q.y, bounds.min.y, bounds.max.y);
    outline.iter().all(on_a_corner).then_some(bounds)
}

/// Draw the images of `list`.
pub(super) fn draw(p: &egui::Painter, xf: &Xf, list: &DisplayList) {
    if list.images.is_empty() {
        return;
    }
    let vis = xf.world_bounds();
    for prim in &list.prims {
        if let Kind::Image { index } = prim.kind
            && let Some(img) = list.images.get(index as usize)
            && Bounds2::from_points(img.corners()).intersects(&vis)
        {
            draw_one(p, xf, img, prim.alpha());
        }
    }
}

/// Draw one image as a textured mesh: its rectangle, or the outline it is clipped to.
fn draw_one(p: &egui::Painter, xf: &Xf, img: &DImage, alpha: u8) {
    let Some(tex) = texture(p.ctx(), &img.raster) else { return };
    let cut = cut(img, xf, p.clip_rect());
    let color = Color32::from_white_alpha((img.opacity(alpha) * 255.0) as u8);
    let (w, h) = (f64::from(img.raster.width()), f64::from(img.raster.height()));
    let mut mesh = Mesh::with_texture(tex.id());
    for q in triangles(img, cut.outline) {
        // Where in the picture this point is, as a fraction of its width and height.
        let Some(px) = img.pixel_of(q) else { return };
        let uv = pos2((px.x / w).clamp(0.0, 1.0) as f32, (px.y / h).clamp(0.0, 1.0) as f32);
        mesh.indices.push(mesh.vertices.len() as u32);
        mesh.vertices.push(Vertex { pos: xf.to_screen(q), uv, color });
    }
    if !mesh.indices.is_empty() && cut.rect.is_positive() {
        p.with_clip_rect(cut.rect).add(Shape::mesh(mesh));
    }
}

/// How an image's clip outlines are applied on the canvas, which can clip a painter to a
/// rectangle but has no general clip.
struct Cut<'a> {
    /// The screen rectangle the painter is clipped to: the canvas, narrowed by every outline
    /// that is a rectangle along the axes (viewports, an upright image's own boundary).
    rect: Rect,
    /// The first outline of another shape, which the mesh itself is cut to. Further ones are
    /// not applied on screen.
    outline: Option<&'a [Vec2]>,
}

fn cut<'a>(img: &'a DImage, xf: &Xf, canvas: Rect) -> Cut<'a> {
    let mut cut = Cut { rect: canvas, outline: None };
    for outline in &img.clips {
        match axis_rect(outline) {
            Some(b) => cut.rect = cut.rect.intersect(Rect::from_two_pos(xf.to_screen(b.min), xf.to_screen(b.max))),
            None if cut.outline.is_none() => cut.outline = Some(outline),
            None => {}
        }
    }
    cut
}

/// World points, three per triangle, covering what is drawn of `img`: the area inside
/// `outline`, or the whole rectangle.
fn triangles(img: &DImage, outline: Option<&[Vec2]>) -> Vec<Vec2> {
    match outline {
        Some(o) => {
            let mut t = cadcraft_render::triangulate_evenodd(&[o.to_vec()]);
            t.truncate(t.len() - t.len() % 3);
            t
        }
        None => {
            let [a, b, c, d] = img.corners();
            vec![a, b, c, a, c, d]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{Common, Drawing, EntityKind, Image, Space};
    use cadcraft_geom::Vec3;
    use cadcraft_render::{Options, build};
    use egui::epaint::ClippedShape;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// `w` x `h` pixels, each with its column in red and its row in green.
    fn raster(w: u32, h: u32) -> Arc<Raster> {
        let px: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, 0, 255])).collect();
        Arc::new(Raster::new(w, h, px).unwrap())
    }

    /// A drawing with a 10 x 10 unit image of 2 x 2 pixels at the origin, changed by `f`.
    fn list(f: impl FnOnce(&mut Image)) -> DisplayList {
        let mut i = Image { u: Vec3::new(5.0, 0.0, 0.0), v: Vec3::new(0.0, 5.0, 0.0), size: v(2.0, 2.0), path: "a.png".into(), ..Image::default() };
        f(&mut i);
        let mut d = Drawing::new_imperial();
        d.add(&Space::Model, Common::default(), EntityKind::Image(i)).unwrap();
        d.rasters.insert("a.png", raster(2, 2));
        build(&d, &Space::Model, &Options::default())
    }

    /// A 100 x 100 point canvas showing the 10 x 10 square, and what `draw` painted on it.
    fn painted(list: &DisplayList) -> (egui::Context, Vec<ClippedShape>) {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        let xf = Xf { rect, center: v(5.0, 5.0), scale: 10.0 };
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::background()).with_clip_rect(rect);
            draw(&painter, &xf, list);
        });
        // No renderer takes the uploads in a test.
        out.textures_delta.clear();
        let shapes = std::mem::take(&mut out.shapes);
        (ctx, shapes)
    }

    fn meshes(shapes: &[ClippedShape]) -> Vec<(Rect, &Mesh)> {
        shapes.iter().filter_map(|s| if let Shape::Mesh(m) = &s.shape { Some((s.clip_rect, &**m)) } else { None }).collect()
    }

    #[test]
    fn a_rectangle_along_the_axes_is_recognised() {
        assert!(axis_rect(&[v(0.0, 0.0), v(4.0, 0.0), v(4.0, 2.0), v(0.0, 2.0)]).is_some());
        assert!(axis_rect(&[v(4.0, 2.0), v(0.0, 2.0), v(0.0, 0.0), v(4.0, 0.0)]).is_some());
        assert!(axis_rect(&[v(0.0, 0.0), v(4.0, 1.0), v(4.0, 2.0), v(0.0, 2.0)]).is_none(), "a slanted side");
        assert!(axis_rect(&[v(0.0, 0.0), v(4.0, 0.0), v(0.0, 2.0)]).is_none(), "a triangle");
        assert!(axis_rect(&[]).is_none());
        assert!(axis_rect(&[v(f64::NAN, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)]).is_none());
    }

    #[test]
    fn large_images_are_thinned_to_fit_a_texture() {
        let r = raster(10, 6);
        assert_eq!(decimated(&r, 1).map(|i| i.size), Some([10, 6]));
        assert_eq!(decimated(&r, 0).map(|i| i.size), Some([10, 6]));
        let half = decimated(&r, 2).unwrap();
        assert_eq!(half.size, [5, 3]);
        // Every second pixel of every second row.
        assert_eq!(half.pixels.get(6).map(|c| (c.r(), c.g())), Some((2, 2)));
        assert_eq!(decimated(&r, 4).map(|i| i.size), Some([3, 2]));
        assert_eq!(decimated(&r, 1000).map(|i| i.size), Some([1, 1]));
    }

    #[test]
    fn an_image_is_one_textured_rectangle() {
        let l = list(|_| {});
        let (_ctx, shapes) = painted(&l);
        let m = meshes(&shapes);
        assert_eq!(m.len(), 1);
        let (_, mesh) = m[0];
        assert_ne!(mesh.texture_id, egui::TextureId::default());
        assert_eq!(mesh.indices, [0, 1, 2, 3, 4, 5], "two triangles");
        // On screen and in the texture: top left, top right, bottom right, then top left, bottom
        // right, bottom left of the picture.
        let (tl, tr, br, bl) = (pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 100.0), pos2(0.0, 100.0));
        let uv = |p: egui::Pos2| pos2(p.x / 100.0, p.y / 100.0);
        let got: Vec<(egui::Pos2, egui::Pos2)> = mesh.vertices.iter().map(|v| (v.pos, v.uv)).collect();
        assert_eq!(got, [tl, tr, br, tl, br, bl].map(|p| (p, uv(p))));
        assert!(mesh.vertices.iter().all(|v| v.color == Color32::WHITE));
    }

    #[test]
    fn fade_dims_the_image_and_nothing_is_drawn_when_fully_faded_or_out_of_view() {
        let (_c, shapes) = painted(&list(|i| i.fade = 50));
        assert_eq!(meshes(&shapes)[0].1.vertices[0].color, Color32::from_white_alpha(127));
        // Far from the view: skipped.
        let (_c, shapes) = painted(&list(|i| i.insert = Vec3::new(1000.0, 1000.0, 0.0)));
        assert!(meshes(&shapes).is_empty());
        // No images at all.
        let (_c, shapes) = painted(&DisplayList::default());
        assert!(shapes.is_empty());
    }

    #[test]
    fn an_upright_clip_boundary_clips_the_painter_and_another_shapes_the_mesh() {
        // The left pixel column: a rectangle along the axes.
        let (_c, shapes) = painted(&list(|i| {
            i.clip = vec![v(-0.5, -0.5), v(0.5, 1.5)];
            i.clipping = true;
        }));
        let m = meshes(&shapes);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 100.0)));
        assert_eq!(m[0].1.vertices.len(), 6, "the whole rectangle, cut by the clip");

        // A triangle over the top-left half: the mesh is the triangle, with matching texture points.
        let (_c, shapes) = painted(&list(|i| {
            i.clip = vec![v(-0.5, -0.5), v(1.5, -0.5), v(-0.5, 1.5)];
            i.clipping = true;
        }));
        let m = meshes(&shapes);
        let mesh = m[0].1;
        assert!(mesh.vertices.len() >= 3 && mesh.indices.len().is_multiple_of(3) && mesh.indices.len() == mesh.vertices.len());
        for vert in &mesh.vertices {
            assert!((vert.pos.x / 100.0 - vert.uv.x).abs() < 1e-4 && (vert.pos.y / 100.0 - vert.uv.y).abs() < 1e-4, "{vert:?}");
            assert!(vert.pos.x + vert.pos.y <= 100.0 + 1e-3, "inside the triangle: {vert:?}");
        }
    }

    #[test]
    fn a_texture_is_uploaded_once_and_dropped_with_its_pixels() {
        let ctx = egui::Context::default();
        let r = raster(4, 4);
        let count = |ctx: &egui::Context| ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(textures_id()).0.len());
        let a = texture(&ctx, &r).unwrap();
        let b = texture(&ctx, &r).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(count(&ctx), 1);
        // Other pixels get their own texture.
        let other = raster(4, 4);
        assert_ne!(texture(&ctx, &other).unwrap().id(), a.id());
        assert_eq!(count(&ctx), 2);
        // The pixels gone (drawing closed, image undone): the texture goes on the next draw.
        drop(r);
        let _ = texture(&ctx, &other);
        assert_eq!(count(&ctx), 1);
    }

    /// A known limit, kept visible here: on screen images are painted first, so they lie under
    /// lines drawn before them. (Pictures and plots keep the draw order; see the renderer's
    /// `draw_order_is_kept_in_pictures`.) When the canvas learns to interleave them, this test
    /// changes with it.
    #[test]
    fn on_screen_images_lie_under_the_lines_whatever_the_draw_order() {
        use cadcraft_doc::Line;
        let line = EntityKind::Line(Line { a: Vec3::new(0.0, 5.0, 0.0), b: Vec3::new(10.0, 5.0, 0.0) });
        let image = EntityKind::Image(Image {
            u: Vec3::new(5.0, 0.0, 0.0),
            v: Vec3::new(0.0, 5.0, 0.0),
            size: v(2.0, 2.0),
            path: "a.png".into(),
            ..Image::default()
        });
        for order in [[line.clone(), image.clone()], [image, line]] {
            let mut d = Drawing::new_imperial();
            for k in order {
                d.add(&Space::Model, Common::default(), k).unwrap();
            }
            d.rasters.insert("a.png", raster(2, 2));
            let l = build(&d, &Space::Model, &Options::default());
            let ctx = egui::Context::default();
            let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
            let xf = Xf { rect, center: v(5.0, 5.0), scale: 10.0 };
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
                let painter = ui.ctx().layer_painter(egui::LayerId::background()).with_clip_rect(rect);
                crate::canvas::draw_list(&painter, &xf, &l, cadcraft_color::Rgb(0, 0, 0), false);
            });
            out.textures_delta.clear();
            let kinds: Vec<bool> = out.shapes.iter().map(|s| matches!(s.shape, Shape::Mesh(_))).collect();
            assert!(kinds.len() >= 2, "the image and the lines: {kinds:?}");
            assert_eq!(kinds.iter().position(|m| *m), Some(0), "the image comes first: {kinds:?}");
            assert_eq!(kinds.iter().filter(|m| **m).count(), 1);
        }
    }

    #[test]
    fn degenerate_images_do_not_panic() {
        for f in [
            (|i: &mut Image| i.u = Vec3::ZERO) as fn(&mut Image),
            |i| i.v = i.u,
            |i| i.u = Vec3::new(1e300, 0.0, 0.0),
            |i| i.insert = Vec3::new(-1e300, 1e300, 0.0),
            |i| {
                i.clip = vec![v(0.0, 0.0), v(0.0, 0.0), v(0.0, 0.0)];
                i.clipping = true;
            },
            |i| {
                i.clip = vec![v(f64::NAN, 0.0), v(1.0, f64::INFINITY), v(-1e300, 1e300)];
                i.clipping = true;
            },
        ] {
            let _ = painted(&list(f));
        }
    }
}
