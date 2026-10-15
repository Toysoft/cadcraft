//! The picture of an embedded OLE object (#956): a bitmap kept as the object's contents, in a
//! metafile that only draws it, or in a presentation stream; what is refused; and damaged data.

use cadcraft_io::ole::picture;

fn le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// A 24-bit bitmap (header and pixels, no file header) of `w` x `h` pixels, the pixel in column
/// `x` of row `y` from the top being `[x * 10, y * 10, 200]`.
fn dib(w: u32, h: u32) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [40, w, h] {
        d.extend(le(v));
    }
    d.extend([1, 0, 24, 0]);
    d.extend([0; 24]);
    let pad = (4 - (w * 3) % 4) % 4;
    // Rows from the bottom, blue first.
    for y in (0..h).rev() {
        for x in 0..w {
            d.extend([200, (y * 10) as u8, (x * 10) as u8]);
        }
        d.extend(std::iter::repeat_n(0, pad as usize));
    }
    d
}

/// `dib` as a bitmap file.
fn bmp(dib: &[u8]) -> Vec<u8> {
    let mut f = b"BM".to_vec();
    f.extend(le(dib.len() as u32 + 14));
    f.extend([0; 4]);
    f.extend(le(54));
    f.extend(dib);
    f
}

fn record(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend(le(kind));
    r.extend(le(payload.len() as u32 + 8));
    r.extend(payload);
    r
}

/// The record drawing bitmap `dib` (whose header is 40 bytes) from source rectangle `src` to a
/// destination of `dst` size.
fn stretch(dib: &[u8], src: [i32; 4], dst: [i32; 2], operation: u32) -> Vec<u8> {
    let mut p = vec![0; 16];
    for v in [0, 0, src[0], src[1], src[2], src[3]] {
        p.extend(v.to_le_bytes());
    }
    for v in [80, 40, 120, dib.len() as u32 - 40, 0, operation] {
        p.extend(le(v));
    }
    for v in dst {
        p.extend(v.to_le_bytes());
    }
    p.extend(dib);
    record(81, &p)
}

/// A metafile whose picture is `w` x `h` device units, with `records` between header and end.
fn emf(w: i32, h: i32, records: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0; 80];
    header[8..12].copy_from_slice(&(w - 1).to_le_bytes());
    header[12..16].copy_from_slice(&(h - 1).to_le_bytes());
    header[32..36].copy_from_slice(b" EMF");
    let mut m = record(1, &header);
    for r in records {
        m.extend(r);
    }
    m.extend(record(14, &[0; 12]));
    m
}

const SRCCOPY: u32 = 0x00CC_0020;

/// A metafile drawing all of a `w` x `h` bitmap, among the records a pasted picture has.
fn picture_emf(w: u32, h: u32) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    let mut blank = vec![0; 92];
    blank[32..36].copy_from_slice(&le(0x00AA_0029));
    emf(
        wi,
        hi,
        &[
            record(70, &[0; 8]),
            record(33, &[]),
            record(21, &le(4)),
            stretch(&dib(w, h), [0, 0, wi, hi], [wi, hi], SRCCOPY),
            record(34, &le(1)),
            record(76, &blank),
        ],
    )
}

/// A compound file holding `streams`: the short ones in the mini stream, the others in sectors.
fn cfb(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    const END: u32 = 0xFFFF_FFFE;
    const NONE: u32 = 0xFFFF_FFFF;
    let sectors = |len: usize| len.div_ceil(512).max(1) as u32;
    // Sector 0 is the allocation table and sector 1 the directory; then the mini table, the
    // mini stream, and the long streams.
    let mut fat = vec![0xFFFF_FFFD, END];
    let mut body: Vec<u8> = Vec::new();
    let mut put = |fat: &mut Vec<u32>, bytes: &[u8]| -> u32 {
        let first = fat.len() as u32;
        let n = sectors(bytes.len());
        for i in 0..n {
            fat.push(if i + 1 == n { END } else { first + i + 1 });
        }
        let end = body.len() + n as usize * 512;
        body.extend(bytes);
        body.resize(end, 0);
        first
    };
    let (mut mini, mut mini_fat, mut entries) = (Vec::new(), Vec::new(), Vec::new());
    for (name, bytes) in streams {
        let first = if bytes.len() < 4096 {
            let first = mini_fat.len() as u32;
            let n = bytes.len().div_ceil(64).max(1) as u32;
            for i in 0..n {
                mini_fat.push(if i + 1 == n { END } else { first + i + 1 });
            }
            mini.extend(bytes);
            mini.resize(mini.len().div_ceil(64) * 64, 0);
            first
        } else {
            put(&mut fat, bytes)
        };
        entries.push((name.to_string(), 2u8, first, bytes.len() as u32));
    }
    let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|v| le(*v)).collect();
    let mini_fat_at = put(&mut fat, &mini_fat_bytes);
    let mini_at = put(&mut fat, &mini);
    entries.insert(0, ("Root Entry".to_string(), 5, mini_at, mini.len() as u32));
    assert!(fat.len() <= 128 && entries.len() <= 4, "one table sector and one directory sector");

    let mut directory = Vec::new();
    for (name, kind, first, size) in &entries {
        let mut e = vec![0u8; 128];
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        e[..units.len()].copy_from_slice(&units);
        e[64..66].copy_from_slice(&(units.len() as u16 + 2).to_le_bytes());
        e[66] = *kind;
        for at in [68, 72, 76] {
            e[at..at + 4].copy_from_slice(&le(NONE));
        }
        e[116..120].copy_from_slice(&le(*first));
        e[120..124].copy_from_slice(&le(*size));
        directory.extend(e);
    }
    directory.resize(512, 0);

    let mut file = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    file.resize(512, 0);
    file[24..32].copy_from_slice(&[0x3E, 0, 3, 0, 0xFE, 0xFF, 9, 0]);
    file[32] = 6;
    for (at, v) in [(44, 1), (48, 1), (56, 4096), (60, mini_fat_at), (64, 1), (68, END), (72, 0), (76, 0)] {
        file[at..at + 4].copy_from_slice(&le(v));
    }
    for at in (80..512).step_by(4) {
        file[at..at + 4].copy_from_slice(&le(NONE));
    }
    let mut table: Vec<u8> = fat.iter().flat_map(|v| le(*v)).collect();
    table.resize(512, 0xFF);
    file.extend(table);
    file.extend(directory);
    file.extend(body);
    file
}

/// The entity data of an OLE object: its own header, then the compound file.
fn ole(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut d = vec![0x80, 0x55];
    d.resize(128, 0);
    d.extend(cfb(streams));
    d
}

/// An OLE object whose contents are `contents`, as a static picture keeps them.
fn static_ole(contents: Vec<u8>) -> Vec<u8> {
    ole(&[("\u{1}Ole", vec![1; 20]), ("CONTENTS", contents)])
}

/// A static metafile picture: the metafile after its header's length.
fn emf_ole(emf: &[u8]) -> Vec<u8> {
    static_ole([le(108).as_slice(), emf].concat())
}

fn message(data: &[u8]) -> String {
    picture(data).unwrap_err().to_string()
}

#[test]
fn a_bitmap_kept_as_the_contents_is_read() {
    // Long enough to be kept in sectors of its own.
    let r = picture(&static_ole(bmp(&dib(40, 36)))).unwrap();
    assert_eq!((r.width(), r.height()), (40, 36));
    assert_eq!(r.pixel(0, 0), Some([0, 0, 200, 255]));
    assert_eq!(r.pixel(3, 2), Some([30, 20, 200, 255]));
    assert_eq!(r.pixel(39, 35), Some([134, 94, 200, 255]));
    // And a small one, from the mini stream.
    let r = picture(&static_ole(bmp(&dib(3, 2)))).unwrap();
    assert_eq!((r.width(), r.height(), r.pixel(2, 1)), (3, 2, Some([20, 10, 200, 255])));
}

#[test]
fn a_metafile_that_only_draws_a_bitmap_gives_the_bitmap() {
    for (w, h) in [(3, 2), (40, 36)] {
        let r = picture(&emf_ole(&picture_emf(w, h))).unwrap();
        assert_eq!((r.width(), r.height()), (w, h));
        assert_eq!(r.pixel(w - 1, h - 1), Some([((w - 1) * 10) as u8, ((h - 1) * 10) as u8, 200, 255]));
        assert_eq!(r.pixel(0, 0), Some([0, 0, 200, 255]));
    }
    // The metafile may also start the contents.
    assert!(picture(&static_ole(picture_emf(3, 2))).is_ok());
}

#[test]
fn a_negative_destination_mirrors_and_a_source_rectangle_crops() {
    let d = dib(4, 3);
    // Drawn right to left and bottom to top.
    let r = picture(&emf_ole(&emf(4, 3, &[stretch(&d, [0, 0, 4, 3], [-4, -3], SRCCOPY)]))).unwrap();
    assert_eq!((r.pixel(0, 0), r.pixel(3, 2)), (Some([30, 20, 200, 255]), Some([0, 0, 200, 255])));
    let r = picture(&emf_ole(&emf(4, 3, &[stretch(&d, [0, 0, 4, 3], [-4, 3], SRCCOPY)]))).unwrap();
    assert_eq!((r.pixel(0, 0), r.pixel(3, 2)), (Some([30, 0, 200, 255]), Some([0, 20, 200, 255])));
    // Columns 1..3 of the two lowest rows (the rows of this bitmap count from the bottom).
    let r = picture(&emf_ole(&emf(2, 2, &[stretch(&d, [1, 0, 2, 2], [2, 2], SRCCOPY)]))).unwrap();
    assert_eq!((r.width(), r.height()), (2, 2));
    assert_eq!((r.pixel(0, 0), r.pixel(1, 1)), (Some([10, 10, 200, 255]), Some([20, 20, 200, 255])));
    // A source rectangle outside the bitmap is refused.
    for src in [[3, 0, 2, 2], [0, 2, 2, 2], [-1, 0, 2, 2], [0, 0, -2, 2], [0, 0, i32::MAX, i32::MAX]] {
        assert!(picture(&emf_ole(&emf(2, 2, &[stretch(&d, src, [2, 2], SRCCOPY)]))).is_err(), "{src:?}");
    }
}

/// A presentation stream: clipboard format, no target device, then the data.
fn presentation(format: u32, data: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    for v in [0xFFFF_FFFF, format, 4, 1, 0xFFFF_FFFF, 2, 0, 100, 100, data.len() as u32] {
        s.extend(le(v));
    }
    s.extend(data);
    s
}

#[test]
fn the_picture_an_object_is_shown_with_is_read_from_its_presentation() {
    let object = |p: Vec<u8>| ole(&[("Package", vec![7; 5000]), ("\u{2}OlePres000", p)]);
    let r = picture(&object(presentation(8, &dib(5, 4)))).unwrap();
    assert_eq!((r.width(), r.height(), r.pixel(4, 3)), (5, 4, Some([40, 30, 200, 255])));
    let r = picture(&object(presentation(14, &picture_emf(5, 4)))).unwrap();
    assert_eq!((r.width(), r.height(), r.pixel(4, 3)), (5, 4, Some([40, 30, 200, 255])));
    // Another format, a named format, and data that is not what the format says.
    assert!(message(&object(presentation(3, &[0; 64]))).contains("not a bitmap"));
    assert!(message(&object(presentation(14, &[1; 64]))).contains("not a bitmap"));
    let mut named = presentation(8, &dib(5, 4));
    named[..4].copy_from_slice(&le(6));
    assert!(message(&object(named)).contains("not a bitmap"));
    assert!(picture(&object(presentation(8, &[40, 0, 0]))).is_err());
    assert!(picture(&object(vec![0xFF; 7])).is_err());
}

#[test]
fn a_fourth_byte_per_pixel_does_not_make_the_picture_transparent() {
    // 32 bits per pixel with every fourth byte zero.
    let mut d = Vec::new();
    for v in [40, 2, 1] {
        d.extend(le(v));
    }
    d.extend([1, 0, 32, 0]);
    d.extend([0; 24]);
    d.extend([10, 20, 30, 0, 40, 50, 60, 0]);
    let r = picture(&static_ole(bmp(&d))).unwrap();
    assert_eq!((r.pixel(0, 0), r.pixel(1, 0), r.opaque()), (Some([30, 20, 10, 255]), Some([60, 50, 40, 255]), true));
}

#[test]
fn drawings_and_parts_of_drawings_are_refused() {
    let d = dib(4, 4);
    let whole = stretch(&d, [0, 0, 4, 4], [4, 4], SRCCOPY);
    let line = record(54, &[0; 8]);
    let refused = |records: &[Vec<u8>]| message(&emf_ole(&emf(4, 4, records)));
    assert!(refused(std::slice::from_ref(&line)).contains("not a bitmap"), "lines only");
    assert!(refused(&[whole.clone(), line]).contains("not a bitmap"), "a bitmap with lines over it");
    assert!(refused(&[whole.clone(), whole.clone()]).contains("not a bitmap"), "two bitmaps");
    assert!(refused(&[]).contains("not a bitmap"), "nothing drawn");
    // A bitmap over a corner of the picture, one combined with what is under it, one whose
    // colours are palette indices.
    assert!(message(&emf_ole(&emf(40, 40, &[stretch(&d, [0, 0, 4, 4], [4, 4], SRCCOPY)]))).contains("not a bitmap"));
    assert!(refused(&[stretch(&d, [0, 0, 4, 4], [4, 4], 0x0066_0046)]).contains("not a bitmap"));
    let mut indexed = whole;
    indexed[64..68].copy_from_slice(&le(1));
    assert!(refused(&[indexed]).contains("not a bitmap"));
    // A transfer that draws: a fill.
    let mut fill = vec![0; 92];
    fill[32..36].copy_from_slice(&le(0x00F0_0021));
    assert!(refused(&[stretch(&d, [0, 0, 4, 4], [4, 4], SRCCOPY), record(76, &fill)]).contains("not a bitmap"));
}

#[test]
fn objects_without_a_readable_picture_are_refused() {
    assert!(message(&[]).contains("no object data"));
    assert!(message(&[0x80, 0x55, 0, 0]).contains("no object data"));
    assert!(message(&ole(&[("\u{1}Ole", vec![1; 20])])).contains("no picture"));
    assert!(message(&static_ole(b"hello, world".to_vec())).contains("not a bitmap"));
    assert!(picture(&static_ole(b"BM".to_vec())).is_err());
    // The compound file too far into the data to be the object's.
    let mut late = vec![0; 8192];
    late.extend(cfb(&[("CONTENTS", bmp(&dib(3, 2)))]));
    assert!(message(&late).contains("no object data"));
}

/// Damaged data is refused or read, never a crash, a hang or a huge allocation.
#[test]
fn hostile_ole_does_not_panic() {
    let samples = [
        static_ole(bmp(&dib(40, 36))),
        emf_ole(&picture_emf(3, 2)),
        emf_ole(&picture_emf(40, 36)),
        ole(&[("Package", vec![7; 5000]), ("\u{2}OlePres000", presentation(8, &dib(5, 4)))]),
    ];
    for good in &samples {
        assert!(picture(good).is_ok());
        // Cut short anywhere.
        for len in (0..good.len()).step_by(7) {
            let _ = picture(&good[..len]);
        }
        // Any byte of the headers, tables and records set to an extreme.
        for at in (0..good.len()).step_by(if good.len() > 4000 { 3 } else { 1 }) {
            for value in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
                let mut bad = good.clone();
                bad[at] = value;
                let _ = picture(&bad);
            }
        }
        // Every 32-bit field of the compound file's header, table and directory, and of the
        // first sectors after them, set to extremes.
        for at in (128..good.len().min(128 + 512 * 6)).step_by(4) {
            for value in [0u32, 1, 2, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFA, 0xFFFF_FFFE, 0xFFFF_FFFF] {
                let mut bad = good.clone();
                bad[at..at + 4].copy_from_slice(&le(value));
                let _ = picture(&bad);
            }
        }
    }
    // A table whose every sector points at itself, and one that points at the first sector.
    for link in [None, Some(0u32)] {
        let mut circular = static_ole(bmp(&dib(40, 36)));
        for i in 0..128u32 {
            let at = 128 + 512 + i as usize * 4;
            circular[at..at + 4].copy_from_slice(&le(link.unwrap_or(i)));
        }
        let _ = picture(&circular);
    }
    // A bitmap claiming a size no file this short can hold.
    let mut huge = dib(4, 4);
    huge[4..8].copy_from_slice(&le(60_000));
    huge[8..12].copy_from_slice(&le(60_000));
    assert!(picture(&static_ole(bmp(&huge))).is_err());
    assert!(picture(&emf_ole(&emf(4, 4, &[stretch(&huge, [0, 0, 4, 4], [4, 4], SRCCOPY)]))).is_err());
    // Noise.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for len in [1usize, 64, 511, 512, 513, 2048, 9000] {
        let mut noise = static_ole(bmp(&dib(40, 36)));
        for b in noise.iter_mut().skip(136).take(len) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *b = seed as u8;
        }
        let _ = picture(&noise);
    }
}
