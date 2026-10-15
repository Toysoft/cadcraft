//! Raster images in PDF plots: an IMAGE whose file is found is placed as an image XObject
//! (with a soft mask for its transparency) instead of its crossed placeholder frame; one whose
//! file is missing keeps the placeholder.

use cadcraft_doc::{Common, Drawing, EntityKind, Image, Space, Viewport};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_io::{PdfOptions, pdf};

/// A drawing with one 4 × 2 pixel image at (10, 20), one drawing unit per pixel.
fn with_image(path: &str) -> Drawing {
    let mut d = Drawing::new_metric();
    let img = Image { insert: Vec3::new(10.0, 20.0, 0.0), size: Vec2::new(4.0, 2.0), path: path.into(), ..Image::default() };
    d.add(&Space::Model, Common::default(), EntityKind::Image(img)).unwrap();
    d
}

/// A 4 × 2 PNG in a fresh folder; the folder is returned.
fn png_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadcraft-pdf-image-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = image::RgbaImage::new(4, 2);
    png.put_pixel(0, 0, image::Rgba([255, 0, 0, 128]));
    png.save(dir.join("logo.png")).unwrap();
    dir
}

fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|b| if b.is_ascii() { char::from(*b) } else { '?' }).collect()
}

#[test]
fn found_image_is_placed_instead_of_its_placeholder() {
    let dir = png_dir("model");
    // A path from another machine: found by its file name in the drawing's folder.
    let d = with_image("C:\\Projetos\\Logos\\logo.png");
    let o = PdfOptions { image_dir: Some(dir.to_string_lossy().into_owned()), ..PdfOptions::default() };
    let t = text(&pdf(&d, &Space::Model, &o).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(t.contains("/Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceRGB"), "image XObject");
    assert!(t.contains("/ColorSpace /DeviceGray"), "soft mask for the transparent pixel");
    assert!(t.contains("/XObject << /Im0 "), "page resources");
    let content = &t[t.find("stream\nq\n").unwrap()..];
    assert!(content.contains(" cm\n/Im0 Do\nQ"), "drawn");
    // Frame and diagonals are not drawn: no path ops after the plot-area clip.
    let after_clip = &content[content.find(" re W n\n").unwrap() + 8..];
    assert!(!after_clip.contains(" m\n"), "placeholder left out: {after_clip}");
}

#[test]
fn missing_image_keeps_its_placeholder() {
    let d = with_image("nowhere/missing.png");
    let o = PdfOptions { image_dir: Some(std::env::temp_dir().to_string_lossy().into_owned()), ..PdfOptions::default() };
    let t = text(&pdf(&d, &Space::Model, &o).unwrap());
    assert!(!t.contains("/Subtype /Image"));
    assert!(!t.contains("/XObject"));
    let content = &t[t.find("stream\nq\n").unwrap()..];
    assert_eq!(content.matches(" m\n").count(), 3, "frame and two diagonals");
}

#[test]
fn image_seen_through_a_viewport_is_placed_and_clipped_by_it() {
    let dir = png_dir("viewport");
    let mut d = with_image("logo.png");
    let paper = Space::Paper("Layout1".into());
    let vp = Viewport {
        center: Vec3::new(150.0, 100.0, 0.0),
        width: 200.0,
        height: 150.0,
        view_center: Vec2::new(12.0, 21.0),
        view_height: 10.0,
        id: 2,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
        clip: None,
        on: true,
    };
    d.add(&paper, Common::default(), EntityKind::Viewport(vp)).unwrap();
    let o = PdfOptions { image_dir: Some(dir.to_string_lossy().into_owned()), ..PdfOptions::default() };
    let t = text(&pdf(&d, &paper, &o).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    let content = &t[t.find("stream\nq\n").unwrap()..];
    let at = content.find(" cm\n/Im0 Do\nQ").expect("image drawn");
    // The viewport outline clips it: a four-corner path closed into the clip before `cm`.
    let before = &content[..at];
    let q = before.rfind("q\n").unwrap();
    assert_eq!(before[q..].matches(" l\n").count(), 3, "viewport outline: {}", &before[q..]);
    assert!(before[q..].contains("h W n\n"));
}
