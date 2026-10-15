//! The picture of an embedded OLE object (OLE2FRAME), when it is a bitmap.
//!
//! The entity's data is a short header followed by a compound file (Microsoft's published
//! [MS-CFB]) holding the object's streams. A static picture keeps itself in `CONTENTS`; other
//! objects keep the picture they are shown with in a presentation stream ([MS-OLEDS]). The
//! picture is a bitmap, or an enhanced metafile ([MS-EMF]) which may do nothing but draw one
//! bitmap: those two are read here. A metafile that draws lines and text (a pasted worksheet,
//! say) is not, and neither is the object itself: only its picture.
//!
//! All of it is hostile input: every offset and length is checked, and every chain of sectors
//! and list of records is followed for a bounded number of steps.

use cadcraft_doc::Raster;

use crate::{IoError, Result, raster};

/// Largest OLE object looked into (bytes).
pub const MAX_OLE_BYTES: usize = raster::MAX_IMAGE_BYTES;
/// How far into the data the compound file may start.
const MAX_HEADER: usize = 4096;
/// Most directory entries of a compound file looked at.
const MAX_ENTRIES: usize = 4096;
/// Most metafile records looked at.
const MAX_RECORDS: usize = 1 << 20;

const CFB_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Sector numbers from here up are markers (free, end of chain, …), not sectors.
const MAX_SECTOR: u32 = 0xFFFF_FFFA;

fn bad(e: impl std::fmt::Display) -> IoError {
    IoError::Format(format!("OLE object: {e}"))
}

fn not_a_bitmap() -> IoError {
    bad("its picture is a drawing, not a bitmap, and can't be shown yet")
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*b.get(at..at.checked_add(2)?)?.first_chunk()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*b.get(at..at.checked_add(4)?)?.first_chunk()?))
}
fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(*b.get(at..at.checked_add(4)?)?.first_chunk()?))
}
fn usize_at(b: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32_at(b, at)?).ok()
}

/// The picture of the OLE object whose entity data is `data`.
pub fn picture(data: &[u8]) -> Result<Raster> {
    if data.len() > MAX_OLE_BYTES {
        return Err(bad(format!("it is larger than {} MB", MAX_OLE_BYTES >> 20)));
    }
    let head = data.get(..MAX_HEADER.min(data.len())).unwrap_or_default();
    let start = head.windows(CFB_SIGNATURE.len()).position(|w| w == CFB_SIGNATURE).ok_or_else(|| bad("no object data"))?;
    let file = Cfb::open(data.get(start..).unwrap_or_default()).ok_or_else(|| bad("the object data is damaged"))?;
    if let Some(contents) = file.stream("CONTENTS") {
        return static_picture(&contents);
    }
    match file.stream("\u{2}OlePres000") {
        Some(presentation) => presentation_picture(&presentation),
        None => Err(bad("it has no picture")),
    }
}

/// A compound file: sectors chained through an allocation table, with a directory of streams.
struct Cfb<'a> {
    data: &'a [u8],
    sector: usize,
    mini_sector: usize,
    /// Streams shorter than this live in the mini stream.
    mini_cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    /// Name, first sector and size of each stream.
    streams: Vec<(String, u32, u64)>,
}

impl<'a> Cfb<'a> {
    fn open(data: &'a [u8]) -> Option<Cfb<'a>> {
        if !data.starts_with(&CFB_SIGNATURE) {
            return None;
        }
        let sector = match u16_at(data, 30)? {
            9 => 512,
            12 => 4096,
            _ => return None,
        };
        let mini_sector = match u16_at(data, 32)? {
            6 => 64,
            _ => return None,
        };
        let mut file = Cfb {
            data,
            sector,
            mini_sector,
            mini_cutoff: u64::from(u32_at(data, 56)?),
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            streams: Vec::new(),
        };
        // The sectors holding the allocation table: 109 listed in the header, the rest in a
        // chain of sectors whose last entry names the next one.
        let fat_sectors = usize_at(data, 44)?;
        let mut listed: Vec<u32> = (0..109).filter_map(|i| u32_at(data, 76 + i * 4)).collect();
        let mut next = u32_at(data, 68)?;
        for _ in 0..file.sector_count() {
            if next >= MAX_SECTOR || listed.len() >= fat_sectors {
                break;
            }
            let more = file.sector(next)?;
            let (entries, link) = more.split_at(more.len().checked_sub(4)?);
            listed.extend(entries.as_chunks::<4>().0.iter().map(|e| u32::from_le_bytes(*e)));
            next = u32_at(link, 0)?;
        }
        listed.truncate(fat_sectors.min(file.sector_count()));
        for s in listed.into_iter().filter(|s| *s < MAX_SECTOR) {
            file.fat.extend(file.sector(s)?.as_chunks::<4>().0.iter().map(|e| u32::from_le_bytes(*e)));
        }

        let directory = file.chain_bytes(u32_at(data, 48)?, u64::MAX)?;
        let mut root = None;
        for entry in directory.as_chunks::<128>().0.iter().take(MAX_ENTRIES) {
            // The name is UTF-16 with its length in bytes, the closing zero included.
            let name_bytes = usize::from(u16_at(entry, 64)?);
            let Some(name) = entry.get(..name_bytes.saturating_sub(2).min(64)) else { continue };
            let units: Vec<u16> = name.as_chunks::<2>().0.iter().map(|u| u16::from_le_bytes(*u)).collect();
            let (first, size) = (u32_at(entry, 116)?, u64::from(u32_at(entry, 120)?));
            match entry.get(66)? {
                2 => file.streams.push((String::from_utf16_lossy(&units), first, size)),
                5 => root = Some((first, size)),
                _ => {}
            }
        }
        // Small streams are cut from the mini stream, which is the root entry's own stream.
        if let Some((first, size)) = root {
            file.mini_stream = file.chain_bytes(first, size).unwrap_or_default();
            let table = file.chain_bytes(u32_at(data, 60)?, u64::MAX).unwrap_or_default();
            file.mini_fat = table.as_chunks::<4>().0.iter().map(|e| u32::from_le_bytes(*e)).collect();
        }
        Some(file)
    }

    fn sector_count(&self) -> usize {
        self.data.len() / self.sector
    }

    /// Sector `n` (the header takes the place of one sector before the first).
    fn sector(&self, n: u32) -> Option<&'a [u8]> {
        let start = usize::try_from(n).ok()?.checked_add(1)?.checked_mul(self.sector)?;
        self.data.get(start..start.checked_add(self.sector)?)
    }

    /// The sectors chained from `first`, joined and cut to `size` bytes.
    fn chain_bytes(&self, first: u32, size: u64) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut next = first;
        // A chain can't be longer than the file has sectors: a longer one runs in a circle.
        for _ in 0..=self.sector_count() {
            if next >= MAX_SECTOR || u64::try_from(out.len()).ok()? >= size {
                break;
            }
            out.extend_from_slice(self.sector(next)?);
            next = *self.fat.get(usize::try_from(next).ok()?)?;
        }
        out.truncate(usize::try_from(size).unwrap_or(usize::MAX));
        Some(out)
    }

    /// The same through the mini stream.
    fn mini_chain_bytes(&self, first: u32, size: u64) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut next = first;
        for _ in 0..=self.mini_fat.len() {
            if next >= MAX_SECTOR || u64::try_from(out.len()).ok()? >= size {
                break;
            }
            let start = usize::try_from(next).ok()?.checked_mul(self.mini_sector)?;
            out.extend_from_slice(self.mini_stream.get(start..start.checked_add(self.mini_sector)?)?);
            next = *self.mini_fat.get(usize::try_from(next).ok()?)?;
        }
        out.truncate(usize::try_from(size).unwrap_or(usize::MAX));
        Some(out)
    }

    /// The contents of the stream called `name`, when it is there and whole.
    fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let (_, first, size) = self.streams.iter().find(|(n, ..)| n == name)?;
        let bytes = if *size < self.mini_cutoff { self.mini_chain_bytes(*first, *size) } else { self.chain_bytes(*first, *size) }?;
        (u64::try_from(bytes.len()).ok()? == *size && !bytes.is_empty()).then_some(bytes)
    }
}

/// The picture a static picture object keeps as its contents: a bitmap file, or a metafile
/// after its header's length.
fn static_picture(contents: &[u8]) -> Result<Raster> {
    if contents.starts_with(b"BM") {
        return opaque(raster::decode(contents)?);
    }
    let metafile = [0, 4].into_iter().filter_map(|at| contents.get(at..)).find(|m| is_emf(m)).ok_or_else(not_a_bitmap)?;
    emf_bitmap(metafile)
}

const CF_DIB: u32 = 8;
const CF_ENHMETAFILE: u32 = 14;

/// The picture in a presentation stream: a clipboard format, the target device, how the
/// picture is to be shown, its size, then the data.
fn presentation_picture(stream: &[u8]) -> Result<Raster> {
    let damaged = || bad("its picture is damaged");
    // A standard clipboard format follows a marker of -1 or -2; a registered one is a name.
    let format = match u32_at(stream, 0).ok_or_else(damaged)? {
        0xFFFF_FFFF | 0xFFFF_FFFE => u32_at(stream, 4).ok_or_else(damaged)?,
        _ => return Err(not_a_bitmap()),
    };
    // The target device's size counts its own four bytes.
    let device = usize_at(stream, 8).ok_or_else(damaged)?.max(4);
    // Aspect, index, flags, reserved, width and height, then the data's size.
    let size_at = 8usize.checked_add(device).and_then(|at| at.checked_add(24)).ok_or_else(damaged)?;
    let size = usize_at(stream, size_at).ok_or_else(damaged)?;
    let data = stream.get(size_at.saturating_add(4)..).and_then(|d| d.get(..size.min(d.len()))).ok_or_else(damaged)?;
    match format {
        CF_DIB => dib_raster(data, None),
        CF_ENHMETAFILE if is_emf(data) => emf_bitmap(data),
        _ => Err(not_a_bitmap()),
    }
}

const EMR_HEADER: u32 = 1;
const EMR_EOF: u32 = 14;
const EMR_BITBLT: u32 = 76;
const EMR_STRETCHDIBITS: u32 = 81;
/// Records that draw nothing and leave where things are drawn alone: comments, saving and
/// restoring the state, clipping, drawing modes and colours, making and choosing pens, brushes
/// and palettes.
const EMR_PASSIVE: [u32; 21] = [EMR_HEADER, EMR_EOF, 70, 33, 34, 98, 26, 29, 30, 75, 18, 20, 21, 24, 25, 37, 38, 39, 40, 48, 49];
/// Raster operations: copy the source, and leave the destination as it is.
const SRCCOPY: u32 = 0x00CC_0020;
const NO_OPERATION: u32 = 0x00AA_0029;

fn is_emf(m: &[u8]) -> bool {
    u32_at(m, 0) == Some(EMR_HEADER) && m.get(40..44) == Some(b" EMF")
}

/// The bitmap of a metafile that draws one bitmap over its whole picture and nothing else.
fn emf_bitmap(emf: &[u8]) -> Result<Raster> {
    let damaged = || bad("its picture is damaged");
    let (left, top, right, bottom) = (i32_at(emf, 8), i32_at(emf, 12), i32_at(emf, 16), i32_at(emf, 20));
    let (Some(left), Some(top), Some(right), Some(bottom)) = (left, top, right, bottom) else { return Err(damaged()) };
    // The picture's size in device units (its bounds include both edges).
    let (width, height) = (i64::from(right) - i64::from(left) + 1, i64::from(bottom) - i64::from(top) + 1);
    let mut bitmap = None;
    let mut at = 0usize;
    for _ in 0..MAX_RECORDS {
        let (Some(kind), Some(len)) = (u32_at(emf, at), usize_at(emf, at.saturating_add(4))) else { break };
        let record = at.checked_add(len).and_then(|end| emf.get(at..end)).filter(|_| len >= 8).ok_or_else(damaged)?;
        match kind {
            EMR_STRETCHDIBITS if bitmap.is_none() => bitmap = Some(record),
            // A bit-block transfer without a source that leaves the destination alone.
            EMR_BITBLT if u32_at(record, 40) == Some(NO_OPERATION) && u32_at(record, 88) == Some(0) => {}
            k if EMR_PASSIVE.contains(&k) => {}
            _ => return Err(not_a_bitmap()),
        }
        if kind == EMR_EOF {
            break;
        }
        at = at.saturating_add(len);
    }
    let record = bitmap.ok_or_else(not_a_bitmap)?;
    // Destination corner, source rectangle, where the bitmap's header and pixels are in the
    // record, the colour table's kind, the raster operation and the destination's size.
    let field = |at: usize| i32_at(record, at).ok_or_else(damaged);
    let (src_x, src_y, src_w, src_h) = (field(32)?, field(36)?, field(40)?, field(44)?);
    let (header_at, header_len, bits_at, bits_len) = (field(48)?, field(52)?, field(56)?, field(60)?);
    let (usage, operation, dst_w, dst_h) = (field(64)?, u32_at(record, 68).ok_or_else(damaged)?, field(72)?, field(76)?);
    // The colours must be given as such (not as palette indices), the source copied as it is,
    // and the bitmap must cover the picture (not be one part of a drawing).
    let covers = |drawn: i32, whole: i64| i64::from(drawn).abs().saturating_mul(10) >= whole.saturating_mul(9);
    if usage != 0 || operation != SRCCOPY || !covers(dst_w, width) || !covers(dst_h, height) {
        return Err(not_a_bitmap());
    }
    let part = |at: i32, len: i32| {
        let (at, len) = (usize::try_from(at).ok()?, usize::try_from(len).ok()?);
        record.get(at..at.checked_add(len)?)
    };
    let (Some(header), Some(bits)) = (part(header_at, header_len), part(bits_at, bits_len)) else { return Err(damaged()) };
    let dib = [header, bits].concat();
    let whole = dib_raster(&dib, Some(header.len()))?;
    let shown = cropped(&whole, &dib, [src_x, src_y, src_w, src_h]).ok_or_else(damaged)?;
    // A negative size draws the bitmap mirrored.
    Ok(mirrored(shown, dst_w < 0, dst_h < 0))
}

/// A device-independent bitmap (header, colour table, pixels) as pixels. `bits_at` is where the
/// pixels start when the header does not say so by its own length.
fn dib_raster(dib: &[u8], bits_at: Option<usize>) -> Result<Raster> {
    let damaged = || bad("its picture is damaged");
    // A bitmap file is the same after 14 bytes: "BM", the file's length, and where the pixels are.
    let header_len = usize_at(dib, 0).ok_or_else(damaged)?;
    let bits_at = match bits_at {
        Some(at) => at,
        None => {
            // After the header: three colour masks when the header says so and has no room for
            // them, then the colour table.
            let (depth, compression) = (u16_at(dib, 14).ok_or_else(damaged)?, u32_at(dib, 16).ok_or_else(damaged)?);
            let masks = if header_len == 40 && compression == 3 { 12 } else { 0 };
            let colours = match (usize_at(dib, 32).ok_or_else(damaged)?, depth) {
                (0, 1..=8) => 1usize << depth,
                (n, _) => n.min(256),
            };
            header_len.checked_add(masks).and_then(|at| at.checked_add(colours * 4)).ok_or_else(damaged)?
        }
    };
    let len = u32::try_from(dib.len().checked_add(14).ok_or_else(damaged)?).map_err(|_| damaged())?;
    let offset = u32::try_from(bits_at.checked_add(14).ok_or_else(damaged)?).map_err(|_| damaged())?;
    let mut file = Vec::with_capacity(dib.len().saturating_add(14));
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&len.to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&offset.to_le_bytes());
    file.extend_from_slice(dib);
    opaque(raster::decode(&file)?)
}

/// `r` without transparency: these pictures are drawn as they are, whatever a fourth byte per
/// pixel holds.
fn opaque(r: Raster) -> Result<Raster> {
    if r.opaque() {
        return Ok(r);
    }
    let mut rgba = r.rgba().to_vec();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[3] = u8::MAX;
    }
    Raster::new(r.width(), r.height(), rgba).ok_or_else(|| bad("its picture is empty"))
}

/// The part of `whole` a metafile draws: the source rectangle `[x, y, width, height]` of the
/// bitmap `dib`, whose rows count from the bottom unless its height is negative.
fn cropped(whole: &Raster, dib: &[u8], [x, y, width, height]: [i32; 4]) -> Option<Raster> {
    let (x, y, width, height) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?, u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    if (x, y, width, height) == (0, 0, whole.width(), whole.height()) {
        return Some(whole.clone());
    }
    let right = x.checked_add(width).filter(|r| *r <= whole.width())?;
    let rows_end = y.checked_add(height).filter(|b| *b <= whole.height())?;
    let top = if i32_at(dib, 8)? < 0 { y } else { whole.height() - rows_end };
    let row_bytes = usize::try_from(whole.width()).ok()?.checked_mul(4)?;
    let (from, to) = (usize::try_from(x).ok()?.checked_mul(4)?, usize::try_from(right).ok()?.checked_mul(4)?);
    let mut rgba = Vec::new();
    for row in whole.rgba().chunks_exact(row_bytes).skip(usize::try_from(top).ok()?).take(usize::try_from(height).ok()?) {
        rgba.extend_from_slice(row.get(from..to)?);
    }
    Raster::new(width, height, rgba)
}

/// `r` with its columns and its rows in the opposite order, as asked.
fn mirrored(r: Raster, columns: bool, rows: bool) -> Raster {
    let Ok(row_bytes) = usize::try_from(r.width()).map(|w| w.saturating_mul(4)) else { return r };
    if !(columns || rows) || row_bytes == 0 {
        return r;
    }
    let mut lines: Vec<Vec<u8>> = r.rgba().chunks_exact(row_bytes).map(<[u8]>::to_vec).collect();
    if columns {
        for line in &mut lines {
            let mut pixels: Vec<[u8; 4]> = line.as_chunks::<4>().0.to_vec();
            pixels.reverse();
            *line = pixels.concat();
        }
    }
    if rows {
        lines.reverse();
    }
    Raster::new(r.width(), r.height(), lines.concat()).unwrap_or(r)
}
