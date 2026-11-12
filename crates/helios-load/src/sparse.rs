//! A raw file read without its sensor data: what its embedded previews
//! and metadata need, nothing more.
//!
//! A raw is tens of megabytes, nearly all of them the mosaic; its previews
//! and EXIF are a few. A glance, a thumbnail or an offline preview reads
//! only those: [`read`] walks the file's structure with seeks (the TIFF
//! IFDs of a NEF, ARW, DNG, CR2…, a RAF's header and raw container, a
//! CR3's boxes, through rawler's own readers), finds where the sensor data
//! lies and returns a buffer of the file's full length in which those
//! "holes" are zeros and every other byte is the file's. rawler, asked for
//! a preview or the metadata on that buffer, reads what it reads on the
//! whole file: the same pixels, the same tags.
//!
//! It works over any `Read + Seek`: a local file, or a connector's ranged
//! fetch. Reads go through 64 KiB blocks, so the many small reads of an IFD
//! walk cost a few block reads. A layout it does not know (a CRW, an
//! RW2…), or one whose metadata would lie inside sensor data, is read
//! whole, as before.
//!
//! The idea of keeping everything but the bulk sample data, and the first
//! bytes of that data (where coding headers sit), follows `LightCraft`'s
//! `raw/src/sparse.rs` (MIT OR Apache-2.0); the code here is Helios's own.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};

use rawler::formats::bmff::Bmff;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD};

/// Bytes kept at the start of a skipped block: the coding headers a
/// decoder may look at when it opens the file.
const HEAD: u64 = 64 * 1024;
/// Image data smaller than this is always read (thumbnails, small
/// previews, tables).
const BULK: u64 = 256 * 1024;
/// The read block.
const BLOCK: u64 = 64 * 1024;
/// Bytes between two blocks of sensor data that join them (alignment).
const PADDING: u64 = 16;
/// IFDs walked at most (a loop of offsets ends here).
const MAX_IFDS: usize = 1024;

/// A `Read + Seek` that counts what it reads: the bytes and the calls.
pub struct Counting<R> {
    inner: R,
    /// Bytes read.
    pub bytes: u64,
    /// `read` calls that returned bytes.
    pub reads: u64,
}

impl<R> Counting<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: 0,
            reads: 0,
        }
    }
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        if n > 0 {
            self.reads += 1;
        }
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A reader served from [`BLOCK`]-sized blocks read once each.
struct Blocks<'a, R> {
    inner: &'a mut R,
    len: u64,
    pos: u64,
    cache: BTreeMap<u64, Vec<u8>>,
}

impl<'a, R: Read + Seek> Blocks<'a, R> {
    fn new(inner: &'a mut R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(Self {
            inner,
            len,
            pos: 0,
            cache: BTreeMap::new(),
        })
    }

    fn block(&mut self, index: u64) -> io::Result<&[u8]> {
        if !self.cache.contains_key(&index) {
            let start = index * BLOCK;
            let end = (start + BLOCK).min(self.len);
            let mut data = vec![0u8; usize::try_from(end - start).unwrap_or(0)];
            self.inner.seek(SeekFrom::Start(start))?;
            self.inner.read_exact(&mut data)?;
            self.cache.insert(index, data);
        }
        Ok(&self.cache[&index])
    }

    /// The file's bytes at `[start, start + n)` (shorter at its end).
    fn bytes(&mut self, start: u64, n: usize) -> io::Result<Vec<u8>> {
        self.seek(SeekFrom::Start(start))?;
        let mut out = vec![0u8; n];
        let mut got = 0;
        while got < n {
            let k = self.read(&mut out[got..])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        out.truncate(got);
        Ok(out)
    }

    /// A buffer of the file's length: every byte outside `holes` read
    /// (in one read per run of blocks not read yet), the holes zeros.
    fn fill(self, holes: &[(u64, u64)]) -> io::Result<Vec<u8>> {
        let Blocks {
            inner,
            len,
            cache: blocks,
            ..
        } = self;
        let mut out = vec![0u8; usize::try_from(len).map_err(io::Error::other)?];
        let count = len.div_ceil(BLOCK);
        let in_hole = |i: u64| {
            let (a, b) = (i * BLOCK, ((i + 1) * BLOCK).min(len));
            holes.iter().any(|&(s, e)| s <= a && b <= e)
        };
        let mut i = 0;
        while i < count {
            if blocks.contains_key(&i) || in_hole(i) {
                i += 1;
                continue;
            }
            let first = i;
            while i < count && !blocks.contains_key(&i) && !in_hole(i) {
                i += 1;
            }
            let (a, b) = (first * BLOCK, (i * BLOCK).min(len));
            inner.seek(SeekFrom::Start(a))?;
            inner.read_exact(&mut out[a as usize..b as usize])?;
        }
        for (index, data) in blocks {
            let a = (index * BLOCK) as usize;
            out[a..a + data.len()].copy_from_slice(&data);
        }
        Ok(out)
    }
}

impl<R: Read + Seek> Read for Blocks<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let (index, at) = (self.pos / BLOCK, (self.pos % BLOCK) as usize);
        let block = self.block(index)?;
        let n = buf.len().min(block.len() - at);
        buf[..n].copy_from_slice(&block[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for Blocks<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos =
            to.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before 0"))?;
        Ok(self.pos)
    }
}

/// The file in `reader`, its sensor data left out (zeros) when its layout
/// is known: what a preview or the metadata read gives the same as on the
/// whole file. A layout not known, or one whose metadata sits inside its
/// sensor data, is read whole (each block once).
///
/// # Errors
/// The reader fails.
pub fn read<R: Read + Seek>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut blocks = Blocks::new(reader)?;
    // rawler's readers panic on some malformed structures: a panic is a
    // layout not read in part.
    let holes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| holes(&mut blocks)))
        .unwrap_or(Ok(None))?;
    blocks.fill(&holes.unwrap_or_default())
}

/// The sensor data of the file: `[start, end)` ranges, sorted. `None`
/// for a layout not known, or one whose metadata sits inside sensor data.
fn holes<R: Read + Seek>(r: &mut Blocks<R>) -> io::Result<Option<Vec<(u64, u64)>>> {
    let len = r.len;
    let head = r.bytes(0, 16)?;
    if head.len() < 16 {
        return Ok(None);
    }
    let mut holes = if head.starts_with(b"FUJIFILMCCD-RAW") {
        raf(r)?
    } else if &head[4..8] == b"ftyp" {
        cr3(r)?
    } else if head.starts_with(b"II") || head.starts_with(b"MM") {
        tiff(r)?
    } else {
        None
    };
    if let Some(h) = holes.as_mut() {
        for span in h.iter_mut() {
            span.1 = span.1.min(len);
        }
        h.retain(|&(a, b)| a < b);
        h.sort_unstable();
        // Tiles side by side, or a padding byte apart, are one hole: a
        // block across two is not read.
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(h.len());
        for &(a, b) in h.iter() {
            match merged.last_mut() {
                Some(last) if a <= last.1 + PADDING => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        *h = merged;
    }
    Ok(holes)
}

/// The hole of a block of sensor data at `offset`, `size` bytes: all but
/// its first [`HEAD`] bytes.
fn hole(offset: u64, size: u64) -> (u64, u64) {
    (offset.saturating_add(HEAD), offset.saturating_add(size))
}

/// Whether the bytes at `offset` start a lossy (baseline, extended or
/// progressive) JPEG stream: a preview, not a lossless mosaic.
fn lossy_jpeg_at<R: Read + Seek>(r: &mut Blocks<R>, offset: u64) -> io::Result<bool> {
    let mut at = offset;
    let soi = r.bytes(at, 2)?;
    if soi != [0xFF, 0xD8] {
        return Ok(false);
    }
    at += 2;
    for _ in 0..64 {
        let m = r.bytes(at, 4)?;
        if m.len() < 4 || m[0] != 0xFF {
            return Ok(false);
        }
        match m[1] {
            0xC0..=0xC2 => return Ok(true),
            0xFF => at += 1,
            0x01 | 0xD0..=0xD7 => at += 2,
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xD8..=0xDA | 0x00 => {
                return Ok(false);
            }
            _ => at += 2 + u64::from(u16::from_be_bytes([m[2], m[3]])),
        }
    }
    Ok(false)
}

/// Every IFD of a TIFF tree: its chain and their sub-IFDs, depth first.
fn all_ifds(roots: &[IFD]) -> Vec<&IFD> {
    let mut out = Vec::new();
    let mut stack: Vec<&IFD> = roots.iter().rev().collect();
    while let Some(ifd) = stack.pop() {
        if out.len() >= MAX_IFDS {
            break;
        }
        out.push(ifd);
        stack.extend(ifd.chain.iter().rev());
        for subs in ifd.sub.values() {
            stack.extend(subs.iter().rev());
        }
    }
    out
}

/// An IFD's image chunks, `(offset, size)`: its strips or its tiles.
fn chunks(ifd: &IFD) -> Vec<(u64, u64)> {
    let pair = |o: u16, c: u16| match (ifd.entries.get(&o), ifd.entries.get(&c)) {
        (Some(o), Some(c)) if o.value.count() == c.value.count() => Some(
            (0..o.value.count())
                .map(|i| (o.value.force_u64(i), c.value.force_u64(i)))
                .collect(),
        ),
        _ => None,
    };
    // StripOffsets, StripByteCounts; TileOffsets, TileByteCounts.
    pair(0x111, 0x117)
        .or_else(|| pair(0x144, 0x145))
        .unwrap_or_default()
}

/// Where an IFD's table and its values out of line lie, absolute.
fn metadata(ifd: &IFD) -> Vec<(u64, u64)> {
    let base = u64::from(ifd.base);
    let table = base + u64::from(ifd.offset);
    let mut out = vec![(table, table + 2 + 12 * ifd.entries.len() as u64 + 4)];
    for entry in ifd.entries.values() {
        if let Some(at) = entry.offset() {
            let size = entry.value.byte_size() as u64;
            if size > 4 {
                let at = base + at as u64;
                out.push((at, at + size));
            }
        }
    }
    out
}

/// The sensor data of the images of a TIFF tree: each image whose chunks
/// hold [`BULK`] bytes or more, unless it is a preview (a reduced
/// resolution image, an RGB or YCbCr one, or one lossy JPEG stream). `None` when an IFD or a
/// value sits inside one.
fn tiff_holes<R: Read + Seek>(
    r: &mut Blocks<R>,
    roots: &[IFD],
) -> io::Result<Option<Vec<(u64, u64)>>> {
    let ifds = all_ifds(roots);
    let mut holes = Vec::new();
    for ifd in &ifds {
        let reduced = ifd
            .entries
            .get(&0xFE)
            .is_some_and(|e| e.value.force_u32(0) & 1 == 1);
        // RGB or YCbCr and not said to be the full image: a picture (a
        // CR2's small RGB image), not a mosaic (a lossless ARW's tiles
        // are said YCbCr, in the full-resolution image).
        let full = ifd
            .entries
            .get(&0xFE)
            .is_some_and(|e| e.value.force_u32(0) == 0);
        let picture = !full
            && ifd
                .entries
                .get(&0x106)
                .is_some_and(|e| matches!(e.value.force_u16(0), 2 | 6));
        if reduced || picture {
            continue;
        }
        let parts = chunks(ifd);
        let total = parts.iter().fold(0u64, |n, c| n.saturating_add(c.1));
        if total < BULK {
            continue;
        }
        if parts.len() == 1 && lossy_jpeg_at(r, parts[0].0)? {
            continue;
        }
        for (i, &(offset, size)) in parts.iter().enumerate() {
            holes.push(if i == 0 {
                hole(offset, size)
            } else {
                (offset, offset.saturating_add(size))
            });
        }
    }
    let inside = ifds
        .iter()
        .copied()
        .flat_map(metadata)
        .any(|(a, b)| holes.iter().any(|&(s, e)| a < e && s < b));
    Ok((!inside).then_some(holes))
}

fn tiff<R: Read + Seek>(r: &mut Blocks<R>) -> io::Result<Option<Vec<(u64, u64)>>> {
    r.seek(SeekFrom::Start(0))?;
    let Ok(tiff) = GenericTiffReader::new(r, 0, 0, None, &[]) else {
        return Ok(None);
    };
    let roots = tiff.file().chain.clone();
    tiff_holes(r, &roots)
}

/// A RAF: its header points at the embedded JPEG (with the EXIF), a
/// record directory and the raw container, a TIFF whose raw IFD lists the
/// CFA strip, or the CFA itself.
fn raf<R: Read + Seek>(r: &mut Blocks<R>) -> io::Result<Option<Vec<(u64, u64)>>> {
    let head = r.bytes(0, 108)?;
    if head.len() < 108 {
        return Ok(None);
    }
    let be = |at: usize| {
        u64::from(u32::from_be_bytes([
            head[at],
            head[at + 1],
            head[at + 2],
            head[at + 3],
        ]))
    };
    let (start, size) = (be(100), be(104));
    if start == 0 || size < BULK || start.saturating_add(size) > r.len {
        return Ok(None);
    }
    let base = u32::try_from(start).map_err(io::Error::other)?;
    // The raw container's TIFF and its raw IFD (0xF000), as rawler reads
    // them; a container that is no TIFF is the CFA itself.
    let parsed = IFD::new_root_with_correction(r, 0, base, 0, 10, &[0xF000]);
    let Ok(root) = parsed else {
        return Ok(Some(vec![hole(start, size)]));
    };
    let roots = [root];
    let ifds = all_ifds(&roots);
    let strip = ifds.iter().find_map(|ifd| {
        let o = ifd.entries.get(&0xF007)?.value.force_u64(0);
        let n = ifd.entries.get(&0xF008)?.value.force_u64(0);
        Some((start.saturating_add(o), n))
    });
    let Some((offset, n)) = strip.filter(|&(_, n)| n >= BULK) else {
        return Ok(None);
    };
    let holes = vec![hole(offset, n)];
    let inside = ifds
        .iter()
        .copied()
        .flat_map(metadata)
        .any(|(a, b)| holes.iter().any(|&(s, e)| a < e && s < b));
    Ok((!inside).then_some(holes))
}

/// A CR3 (ISO base media): every box but `mdat` is read; in `mdat`, each
/// track sample of [`BULK`] bytes or more that is not a JPEG is sensor
/// data.
fn cr3<R: Read + Seek>(r: &mut Blocks<R>) -> io::Result<Option<Vec<(u64, u64)>>> {
    r.seek(SeekFrom::Start(0))?;
    let Ok(bmff) = Bmff::new(&mut *r) else {
        return Ok(None);
    };
    if !bmff.compatible_brand("crx ") {
        return Ok(None);
    }
    let mut holes = Vec::new();
    for trak in &bmff.filebox.moov.traks {
        let stbl = &trak.mdia.minf.stbl;
        if stbl.co64.is_none() || stbl.stsc.entries.is_empty() {
            continue;
        }
        for sample in 1..=stbl.stsz.sample_count {
            let Some((offset, size)) = stbl.get_sample_offset(sample) else {
                continue;
            };
            let (offset, size) = (offset as u64, size as u64);
            if size < BULK || r.bytes(offset, 2)? == [0xFF, 0xD8] {
                continue;
            }
            holes.push(hole(offset, size));
        }
    }
    Ok(Some(holes))
}
