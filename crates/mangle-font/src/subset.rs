//! A TrueType subsetter: a smaller font holding only the glyphs asked for.
//!
//! The charter's fourth text-editing law for typed characters says a new glyph the run's
//! own font does not have must come from somewhere, embedded as a subset (GOAL.md §4.4).
//! This is the somewhere's first half: given a TrueType program and the Unicode code
//! points it must keep, build a smaller, valid TrueType program holding those glyphs and
//! nothing else. The second half — the `CIDFontType2` dictionary, the `ToUnicode`, the
//! resource surgery that installs it — lives where fonts meet pages, not here.
//!
//! ## Scope, stated rather than discovered later
//!
//! Only the basic multilingual plane: a code point above U+FFFF is refused, because the
//! rebuilt `cmap` is format 4. Only TrueType outlines (`glyf`): an `OTTO` program is
//! refused rather than misread. Composite glyphs close over their components, so an `é`
//! keeps its `e` and its accent; anything else keeps exactly what it names.
//!
//! Hinting programs (`prep`, `fpgm`, `cvt `) are copied verbatim rather than rebuilt or
//! dropped: rebuilding them is a second subsetter, and dropping them would leave glyph
//! instructions calling functions that no longer exist. The `maxp` maxima are kept as
//! upper bounds rather than recomputed, for the same reason a larger box is still a box.
//! The `post` table is written fresh as 3.0 (no names): names are metadata, and
//! `ToUnicode` — built beside the embedding, not here — is what carries the meaning.
//!
//! Every read is bounds-checked and every number from the file is validated: a damaged
//! program is an error naming what was wrong, never a panic and never a truncated font
//! that claims success.

use std::collections::{BTreeMap, BTreeSet};

/// The glyph `code` names in this TrueType program, through its Unicode `cmap`.
///
/// The same lookup the subsetter keeps glyphs by, exposed because an embedder needs it
/// too: choosing which glyphs to keep, and later which CIDs they become, both start
/// from the character asked for rather than from a glyph number nobody outside the font
/// knows.
pub fn glyph_for_unicode(data: &[u8], code: u32) -> Result<Option<u32>, String> {
    Sfnt::read(data)?.cmap_glyph(code)
}

/// Keep this many glyphs' worth of table directory sane: a real font holds under thirty
/// tables, so anything past this is damage rather than a font.
const MAX_TABLES: usize = 256;

/// How many `cmap` subtables to consider before giving up: real fonts hold a handful.
const MAX_CMAP_TABLES: usize = 32;

/// A code point the rebuilt `cmap` cannot name.
const MAX_BMP: u32 = 0xFFFF;

/// The TrueType `sfnt` versions this reads: 1.0 and the old Mac `true` tag, both of which
/// name `glyf` outlines.
const VERSION_TRUETYPE: u32 = 0x0001_0000;
const VERSION_TRUE: u32 = 0x7472_7565;

/// What a correct file sums to, with the adjustment zeroed: the checksum of checksums.
const CHECKSUM_MAGIC: u32 = 0xB1B0_AFBA;

/// Subset `data`, keeping the glyphs for `unicodes` and nothing else.
///
/// Glyph zero (`.notdef`) is always kept: a font whose first glyph is anything else is a
/// font no reader accepts. The output is deterministic — same bytes in, same bytes out —
/// because every table is walked in sorted order and nothing here reads a clock, a map
/// with a random order, or any other font on the machine.
pub fn subset_truetype(data: &[u8], unicodes: &[u32]) -> Result<Vec<u8>, String> {
    let font = Sfnt::read(data)?;
    if font.version != VERSION_TRUETYPE && font.version != VERSION_TRUE {
        return Err("not a TrueType font: no glyf outlines to subset".to_string());
    }
    let head = font.table(b"head").ok_or("no head table")?;
    if head.len() < 54 {
        return Err("the head table is too short to hold its own fields".to_string());
    }
    let loca_format = i16at(head, 50).ok_or("the head table ends before indexToLocFormat")?;
    if loca_format != 0 && loca_format != 1 {
        return Err("an indexToLocFormat that is neither short nor long".to_string());
    }
    if i16at(head, 52).unwrap_or(1) != 0 {
        return Err("a glyphDataFormat that is not zero".to_string());
    }
    let maxp = font.table(b"maxp").ok_or("no maxp table")?;
    if maxp.len() < 6 {
        return Err("the maxp table is too short to hold its glyph count".to_string());
    }
    let num_glyphs = u32::from(u16at(maxp, 4).ok_or("the maxp table ends before numGlyphs")?);
    // The request, as (character, glyph) pairs in sorted order: the same request always
    // builds the same font, because the new numbering is assigned in this order.
    let pairs = fix_cmap_glyphs(&font, unicodes)?;
    // Glyph zero first and everything sorted: the new numbering is assigned in this
    // order, so the same request always builds the same font.
    let mut kept = BTreeSet::new();
    kept.insert(0u32);
    let mut work: Vec<u32> = pairs.iter().map(|(_, gid)| *gid).collect();
    let loca = font.table(b"loca").ok_or("no loca table")?;
    let glyf = font.table(b"glyf").ok_or("no glyf table")?;
    while let Some(gid) = work.pop() {
        if gid >= num_glyphs {
            return Err(format!(
                "a glyph number {gid} past the font's {num_glyphs} glyphs"
            ));
        }
        if !kept.insert(gid) {
            continue;
        }
        // A composite glyph names its components, which must be kept too: an `é` that
        // kept its `e` but lost its accent would draw as the wrong letter rather than
        // refuse, which is the plausible-wrong-shape class this project refuses to ship.
        for component in composite_components(glyf, loca, loca_format, gid, num_glyphs)? {
            if !kept.contains(&component) {
                work.push(component);
            }
        }
    }
    let new_id: BTreeMap<u32, u32> = kept
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new as u32))
        .collect();
    let count = kept.len() as u32;

    // The `cmap` is rebuilt from the request, not copied: the old table names glyphs this
    // font no longer has, and a `cmap` that names a missing glyph is a file that lies.
    // Non-BMP pairs cannot be named by the format-4 table below, and are refused rather
    // than silently dropped.
    let mut kept_pairs: Vec<(u32, u32)> = Vec::new();
    for (code, gid) in pairs {
        if code > MAX_BMP {
            return Err(format!(
                "U+{code:04X} is outside the basic multilingual plane this subsetter writes"
            ));
        }
        if let Some(new) = new_id.get(&gid) {
            kept_pairs.push((code, *new));
        }
    }

    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    // Optional programs and data, copied rather than rebuilt: dropping `fpgm` would leave
    // glyph instructions calling missing functions, and rebuilding it is a second project.
    for tag in [b"fpgm", b"prep", b"cvt ", b"name", b"OS/2"] {
        if let Some(bytes) = font.table(tag) {
            tables.push((*tag, bytes.to_vec()));
        }
    }
    tables.push((*b"cmap", build_cmap4(&kept_pairs)?));
    let (new_glyf, new_loca) = build_glyf_loca(glyf, loca, loca_format, &kept, &new_id)?;
    tables.push((*b"glyf", new_glyf));
    tables.push((*b"head", patch_head(head)?));
    let hhea = font.table(b"hhea").ok_or("no hhea table")?;
    let hmtx = font.table(b"hmtx").ok_or("no hmtx table")?;
    let num_h_metrics = u16at(hhea, 34).ok_or("the hhea table ends before numberOfHMetrics")?;
    tables.push((*b"hhea", patch_hhea(hhea, count)?));
    tables.push((
        *b"hmtx",
        build_hmtx(hmtx, u32::from(num_h_metrics), num_glyphs, &kept)?,
    ));
    tables.push((*b"loca", new_loca));
    tables.push((*b"maxp", patch_maxp(maxp, count)?));
    tables.push((*b"post", build_post30()));
    assemble(font.version, &mut tables)
}

/// The glyphs `unicodes` name in this font, as (character, glyph) pairs in sorted order.
///
/// A character the font has no glyph for is an error naming the character: skipping it
/// would build a font that cannot show what was asked for and say nothing about it.
fn fix_cmap_glyphs(font: &Sfnt<'_>, unicodes: &[u32]) -> Result<Vec<(u32, u32)>, String> {
    let mut out = Vec::new();
    for code in BTreeSet::from_iter(unicodes.iter().copied()) {
        match font.cmap_glyph(code)? {
            Some(gid) => out.push((code, gid)),
            None => {
                return Err(format!(
                    "the font has no glyph for U+{code:04X}, so there is nothing to keep"
                ));
            }
        }
    }
    Ok(out)
}

/// One TrueType program, as its table directory and the slices behind it.
struct Sfnt<'a> {
    version: u32,
    tables: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> Sfnt<'a> {
    fn read(data: &'a [u8]) -> Result<Self, String> {
        let version = u32at(data, 0).ok_or("fewer than four bytes")?;
        let num_tables = u16at(data, 4).ok_or("no table count")? as usize;
        if num_tables > MAX_TABLES {
            return Err("a table count no font has".to_string());
        }
        let mut tables = Vec::with_capacity(num_tables);
        for i in 0..num_tables {
            let at = 12 + i * 16;
            let tag: [u8; 4] = data
                .get(at..at + 4)
                .and_then(|s| <[u8; 4]>::try_from(s).ok())
                .ok_or("the table directory ends mid-record")?;
            let offset = u32at(data, at + 8).ok_or("the table directory ends mid-record")? as usize;
            let length =
                u32at(data, at + 12).ok_or("the table directory ends mid-record")? as usize;
            let bytes = data
                .get(offset..offset.saturating_add(length))
                .filter(|s| s.len() == length)
                .ok_or("a table that runs past the end of the file")?;
            tables.push((tag, bytes));
        }
        Ok(Self { version, tables })
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        self.tables
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, bytes)| *bytes)
    }

    /// The glyph `code` names, through the first usable `cmap` subtable.
    ///
    /// Tried in Unicode order — full Unicode, then BMP, then Mac Roman — and in formats 4
    /// and 0 only: a symbolic font's private-use mapping is the author's own numbering
    /// rather than Unicode, so it is skipped rather than misread as text.
    fn cmap_glyph(&self, code: u32) -> Result<Option<u32>, String> {
        let cmap = self.table(b"cmap").ok_or("no cmap table")?;
        if cmap.len() < 4 {
            return Err("the cmap table is too short to hold its own header".to_string());
        }
        let num_tables = u16at(cmap, 2).ok_or("the cmap table ends mid-header")? as usize;
        if num_tables > MAX_CMAP_TABLES {
            return Err("a cmap table count no font has".to_string());
        }
        let mut candidates: Vec<(u16, u16, usize)> = Vec::new();
        for i in 0..num_tables {
            let at = 4 + i * 8;
            let platform = u16at(cmap, at).ok_or("the cmap ends mid-record")?;
            let encoding = u16at(cmap, at + 2).ok_or("the cmap ends mid-record")?;
            let offset = u32at(cmap, at + 4).ok_or("the cmap ends mid-record")? as usize;
            candidates.push((platform, encoding, offset));
        }
        for (platform, encoding) in [(3u16, 10u16), (0, 3), (3, 1), (1, 0)] {
            for (_, _, offset) in candidates
                .iter()
                .filter(|(p, e, _)| *p == platform && *e == encoding)
            {
                let sub = cmap.get(*offset..).ok_or("a cmap offset past the table")?;
                let format = u16at(sub, 0).ok_or("a cmap subtable with no format")?;
                match format {
                    0 => return Ok(map_format0(sub, code)),
                    4 => return map_format4(sub, code),
                    _ => {}
                }
            }
        }
        Err("no Unicode cmap subtable this reads".to_string())
    }
}

/// The glyph `code` names in a format-0 `cmap`: 256 bytes, one glyph each.
fn map_format0(sub: &[u8], code: u32) -> Option<u32> {
    if code > 255 {
        return None;
    }
    let gid = *sub.get(6 + code as usize)?;
    if gid == 0 { None } else { Some(u32::from(gid)) }
}

/// The glyph `code` names in a format-4 `cmap`.
///
/// The classic segmented mapping, with its two traps laid out plainly: `idDelta` is a
/// *signed* adjustment applied modulo 65536, and `idRangeOffset` is measured from the
/// address of its own word rather than from the table. A segment that ends before it
/// starts is damage and is skipped; the terminator maps nowhere because its delta lands
/// on zero, which is skipped like any other missing glyph.
fn map_format4(sub: &[u8], code: u32) -> Result<Option<u32>, String> {
    if code > MAX_BMP || sub.len() < 16 {
        return Ok(None);
    }
    let code = code as u16;
    let seg_count = u16at(sub, 6).ok_or("a format-4 table with no segment count")? as usize / 2;
    let ends_at = 14usize;
    let starts_at = ends_at + seg_count * 2 + 2;
    let deltas_at = starts_at + seg_count * 2;
    let ranges_at = deltas_at + seg_count * 2;
    for s in 0..seg_count {
        let end = u16at(sub, ends_at + s * 2).ok_or("a format-4 segment past the table")?;
        let start = u16at(sub, starts_at + s * 2).ok_or("a format-4 segment past the table")?;
        if end < start {
            continue;
        }
        if code < start || code > end {
            continue;
        }
        let delta =
            u16at(sub, deltas_at + s * 2).ok_or("a format-4 segment past the table")? as i16;
        let range_offset =
            u16at(sub, ranges_at + s * 2).ok_or("a format-4 segment past the table")? as usize;
        if range_offset == 0 {
            // Signed and modulo the code space, in wrapping arithmetic: a plain addition
            // overflows for a negative delta, and an overflow is a panic in a debug
            // build rather than a wrong glyph.
            let gid = u32::from(code)
                .wrapping_add(0x1_0000)
                .wrapping_add(i32::from(delta) as u32)
                & 0xFFFF;
            return Ok(if gid == 0 { None } else { Some(gid) });
        }
        // From the range word's own address, plus two bytes per step into the segment.
        let position = ranges_at + s * 2 + range_offset + 2 * (u32::from(code - start)) as usize;
        let raw = u16at(sub, position).ok_or("a format-4 glyph index past the table")?;
        if raw == 0 {
            return Ok(None);
        }
        let gid = u32::from(raw)
            .wrapping_add(0x1_0000)
            .wrapping_add(i32::from(delta) as u32)
            & 0xFFFF;
        return Ok(if gid == 0 { None } else { Some(gid) });
    }
    Ok(None)
}

/// The component glyphs of a composite `glyf` entry: an empty list for a simple glyph.
///
/// Only the component numbers are read — the arguments and scales are the glyf rebuild's
/// business — so this walks each record's header and skips what follows it by the flags.
fn composite_components(
    glyf: &[u8],
    loca: &[u8],
    loca_format: i16,
    gid: u32,
    num_glyphs: u32,
) -> Result<Vec<u32>, String> {
    let (start, end) = loca_range(loca, loca_format, gid, num_glyphs, glyf.len())?;
    if start == end {
        return Ok(Vec::new());
    }
    let bytes = glyf
        .get(start..end)
        .ok_or("a glyph that runs past the glyf table")?;
    if bytes.len() < 10 {
        return Err("a glyph too short to hold its own header".to_string());
    }
    if i16at(bytes, 0).ok_or("a glyph cut before its contour count")? >= 0 {
        return Ok(Vec::new());
    }
    // Component record flags: which argument form, which scale form, whether another
    // record follows, and whether instructions trail the last one.
    const ARGS_ARE_WORDS: u16 = 0x0001;
    const WE_HAVE_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_X_Y_SCALE: u16 = 0x0040;
    const WE_HAVE_TWO_BY_TWO: u16 = 0x0080;
    let mut out = Vec::new();
    let mut at = 10usize;
    loop {
        let flags = u16at(bytes, at).ok_or("a composite glyph cut mid-record")?;
        let component = u16at(bytes, at + 2).ok_or("a composite glyph cut mid-record")?;
        out.push(u32::from(component));
        at += 4;
        at += if flags & ARGS_ARE_WORDS != 0 { 4 } else { 2 };
        if flags & WE_HAVE_SCALE != 0 {
            at += 2;
        } else if flags & WE_HAVE_X_Y_SCALE != 0 {
            at += 4;
        } else if flags & WE_HAVE_TWO_BY_TWO != 0 {
            at += 8;
        }
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
        if at + 4 > bytes.len() {
            return Err("a composite glyph cut between records".to_string());
        }
    }
    Ok(out)
}

/// The byte range of one glyph: entry `gid` to entry `gid + 1` of the `loca`.
fn loca_range(
    loca: &[u8],
    loca_format: i16,
    gid: u32,
    num_glyphs: u32,
    glyf_len: usize,
) -> Result<(usize, usize), String> {
    if gid >= num_glyphs {
        return Err(format!(
            "a glyph number {gid} past the font's {num_glyphs} glyphs"
        ));
    }
    let entry = |i: u32| -> Result<usize, String> {
        if loca_format == 0 {
            u16at(loca, (i as usize) * 2)
                .map(|v| (v as usize) * 2)
                .ok_or_else(|| "a loca entry past the table".to_string())
        } else {
            u32at(loca, (i as usize) * 4)
                .map(|v| v as usize)
                .ok_or_else(|| "a loca entry past the table".to_string())
        }
    };
    let start = entry(gid)?;
    let end = entry(gid + 1)?;
    if start > end || end > glyf_len {
        return Err("a loca range outside the glyf table".to_string());
    }
    Ok((start, end))
}

/// The rebuilt `glyf` and `loca`: every kept glyph in its new order, each starting where
/// the format's alignment says it must.
fn build_glyf_loca(
    glyf: &[u8],
    loca: &[u8],
    loca_format: i16,
    kept: &BTreeSet<u32>,
    new_id: &BTreeMap<u32, u32>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let num_glyphs = kept.len();
    // The old count is needed for the last entry's range: `loca` holds one entry past
    // the last glyph, and the kept set alone cannot say how many that was.
    let old_count = loca_count(loca, loca_format)?;
    let mut out_glyf: Vec<u8> = Vec::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(num_glyphs + 1);
    let align = if loca_format == 0 { 2usize } else { 4usize };
    for old in kept {
        while out_glyf.len() % align != 0 {
            out_glyf.push(0);
        }
        offsets.push(out_glyf.len() as u32);
        let (start, end) = loca_range(loca, loca_format, *old, old_count, glyf.len())?;
        let bytes = glyf
            .get(start..end)
            .ok_or("a glyph that runs past the glyf table")?;
        if bytes.len() >= 2 && i16at(bytes, 0).unwrap_or(0) < 0 {
            out_glyf.extend_from_slice(&remap_composite(bytes, new_id)?);
        } else {
            out_glyf.extend_from_slice(bytes);
        }
    }
    while out_glyf.len() % align != 0 {
        out_glyf.push(0);
    }
    offsets.push(out_glyf.len() as u32);
    let mut out_loca = Vec::with_capacity((num_glyphs + 1) * if loca_format == 0 { 2 } else { 4 });
    for offset in offsets {
        if loca_format == 0 {
            if offset / 2 > u32::from(u16::MAX) {
                return Err("a short loca cannot address this much outline data".to_string());
            }
            out_loca.extend_from_slice(&((offset / 2) as u16).to_be_bytes());
        } else {
            out_loca.extend_from_slice(&offset.to_be_bytes());
        }
    }
    Ok((out_glyf, out_loca))
}

/// How many glyphs the old `loca` describes: one entry past the last.
fn loca_count(loca: &[u8], loca_format: i16) -> Result<u32, String> {
    let entries = if loca_format == 0 {
        loca.len() / 2
    } else {
        loca.len() / 4
    };
    if entries == 0 {
        return Err("a loca table with no entries".to_string());
    }
    Ok(entries as u32 - 1)
}

/// One composite glyph with its component numbers rewritten to the new order.
///
/// The header, the arguments and the scales come across byte-for-byte: only the two
/// bytes naming each component change, because they are the only bytes that name a
/// glyph at all.
fn remap_composite(bytes: &[u8], new_id: &BTreeMap<u32, u32>) -> Result<Vec<u8>, String> {
    const ARGS_ARE_WORDS: u16 = 0x0001;
    const WE_HAVE_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_X_Y_SCALE: u16 = 0x0040;
    const WE_HAVE_TWO_BY_TWO: u16 = 0x0080;
    const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
    let mut out = bytes.to_vec();
    let mut at = 10usize;
    loop {
        let flags = u16at(bytes, at).ok_or("a composite glyph cut mid-record")?;
        let component = u16at(bytes, at + 2).ok_or("a composite glyph cut mid-record")?;
        let Some(new) = new_id.get(&u32::from(component)) else {
            return Err("a composite component that was not kept".to_string());
        };
        let bytes_new = (*new as u16).to_be_bytes();
        if let Some(slot) = out.get_mut(at + 2..at + 4) {
            slot.copy_from_slice(&bytes_new);
        } else {
            return Err("a composite glyph cut mid-record".to_string());
        }
        at += 4;
        at += if flags & ARGS_ARE_WORDS != 0 { 4 } else { 2 };
        if flags & WE_HAVE_SCALE != 0 {
            at += 2;
        } else if flags & WE_HAVE_X_Y_SCALE != 0 {
            at += 4;
        } else if flags & WE_HAVE_TWO_BY_TWO != 0 {
            at += 8;
        }
        if flags & MORE_COMPONENTS == 0 {
            // Instructions trail the last record, and come across whole: they belong to
            // the composite, not to any component, so there is nothing in them to remap.
            if flags & WE_HAVE_INSTRUCTIONS != 0 {
                let count = u16at(bytes, at).ok_or("cut instructions")? as usize;
                if bytes.get(at..at + 2 + count).is_none() {
                    return Err("cut instructions".to_string());
                }
            }
            break;
        }
        if at + 4 > bytes.len() {
            return Err("a composite glyph cut between records".to_string());
        }
    }
    Ok(out)
}

/// The rebuilt horizontal metrics: every kept glyph with its own advance and bearing.
///
/// Written all-long rather than packed: the trailing run of equal advances a packed
/// table shares is two bytes a glyph, and correctness here is worth more than density.
fn build_hmtx(
    hmtx: &[u8],
    num_h_metrics: u32,
    num_glyphs: u32,
    kept: &BTreeSet<u32>,
) -> Result<Vec<u8>, String> {
    let metric = |gid: u32| -> Result<(u16, i16), String> {
        let long = gid.min(num_h_metrics.saturating_sub(1));
        let advance = u16at(hmtx, (long as usize) * 4).ok_or("hmtx ends mid-metric")?;
        let bearing = if gid < num_h_metrics {
            i16at(hmtx, (gid as usize) * 4 + 2).ok_or("hmtx ends mid-metric")?
        } else {
            let at = (num_h_metrics as usize) * 4 + ((gid - num_h_metrics) as usize) * 2;
            i16at(hmtx, at).ok_or("hmtx ends mid-bearing")?
        };
        Ok((advance, bearing))
    };
    let mut out = Vec::with_capacity(kept.len() * 4);
    for old in kept {
        if *old >= num_glyphs {
            return Err(format!(
                "a glyph number {old} past the font's {num_glyphs} glyphs"
            ));
        }
        let (advance, bearing) = metric(*old)?;
        out.extend_from_slice(&advance.to_be_bytes());
        out.extend_from_slice(&bearing.to_be_bytes());
    }
    Ok(out)
}

/// The `head` with a zeroed checksum adjustment, set for real once the file is assembled.
fn patch_head(head: &[u8]) -> Result<Vec<u8>, String> {
    if head.len() < 54 {
        return Err("the head table is too short".to_string());
    }
    let mut out = head.to_vec();
    if let Some(slot) = out.get_mut(8..12) {
        slot.copy_from_slice(&0u32.to_be_bytes());
    } else {
        return Err("the head table is too short".to_string());
    }
    Ok(out)
}

/// The `hhea` with its long-metric count set to the kept glyphs: all-long metrics below.
fn patch_hhea(hhea: &[u8], count: u32) -> Result<Vec<u8>, String> {
    if hhea.len() < 36 || count > u32::from(u16::MAX) {
        return Err("the hhea table is too short".to_string());
    }
    let mut out = hhea.to_vec();
    if let Some(slot) = out.get_mut(34..36) {
        slot.copy_from_slice(&(count as u16).to_be_bytes());
    } else {
        return Err("the hhea table is too short".to_string());
    }
    Ok(out)
}

/// The `maxp` with its glyph count set, and everything else kept as upper bounds.
fn patch_maxp(maxp: &[u8], count: u32) -> Result<Vec<u8>, String> {
    if maxp.len() < 6 || count > u32::from(u16::MAX) {
        return Err("the maxp table is too short".to_string());
    }
    let mut out = maxp.to_vec();
    if let Some(slot) = out.get_mut(4..6) {
        slot.copy_from_slice(&(count as u16).to_be_bytes());
    } else {
        return Err("the maxp table is too short".to_string());
    }
    Ok(out)
}

/// A format-4 `cmap` over `pairs` of (character, new glyph number).
///
/// Segments group runs where both sides step by one, with the delta carried rather than
/// a table: a run of ordinary Latin is one segment, and the terminator every format-4
/// table ends with closes it. Characters must arrive sorted and deduplicated, which the
/// caller guarantees with the same set it numbered the glyphs from.
fn build_cmap4(pairs: &[(u32, u32)]) -> Result<Vec<u8>, String> {
    // Runs of (char, gid) stepping together; every other character stands alone.
    let mut segments: Vec<(u32, u32)> = Vec::new();
    let mut run_start: Option<(u32, u32)> = None;
    let mut previous: Option<(u32, u32)> = None;
    for (char, gid) in pairs.iter().copied() {
        match (run_start, previous) {
            (Some(_), Some((pc, pg))) if char == pc + 1 && gid == pg + 1 => {
                previous = Some((char, gid));
            }
            _ => {
                if let Some((sc, _)) = run_start {
                    let (pc, _) = previous.unwrap_or((sc, 0));
                    segments.push((sc, pc));
                }
                run_start = Some((char, gid));
                previous = Some((char, gid));
            }
        }
    }
    if let Some((sc, _)) = run_start {
        let (pc, _) = previous.unwrap_or((sc, 0));
        segments.push((sc, pc));
    }
    // Deltas, reread from the pairs: a run's delta is its glyph minus its character,
    // modulo the code space, which is what makes the run one segment rather than many.
    let delta_of = |start: u32| -> Result<i16, String> {
        let gid = pairs
            .iter()
            .find(|(c, _)| *c == start)
            .map(|(_, g)| *g)
            .ok_or("an internal segment error")?;
        Ok(((i64::from(gid) - i64::from(start)) & 0xFFFF) as u16 as i16)
    };
    let seg_count = segments.len() + 1;
    // Two bytes per segment in the count field below: past half the u16 range the count
    // itself no longer fits, and a truncating cast would write a table that lies about
    // how many segments it holds.
    let seg_count_x2 = (seg_count as u32)
        .checked_mul(2)
        .filter(|v| u16::try_from(*v).is_ok())
        .ok_or("too many cmap segments")? as u16;
    // The subtable's own binary-search parameters, over its segment count.
    let entries: u32 = seg_count as u32;
    let log: u32 = entries.ilog2();
    let unit: u32 = 1 << log;
    let search_range: u16 = (unit * 2) as u16;
    let entry_selector: u16 = log as u16;
    let range_shift: u16 = (entries * 2 - unit * 2) as u16;
    let length = 16usize + seg_count * 8;
    if length > usize::from(u16::MAX) {
        return Err("a cmap subtable larger than its length field".to_string());
    }
    let mut out = Vec::with_capacity(length + 8);
    out.extend_from_slice(&[0, 4]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&seg_count_x2.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    for (_, end) in segments.iter().copied() {
        out.extend_from_slice(&(end as u16).to_be_bytes());
    }
    out.extend_from_slice(&0xFFFFu16.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    for (start, _) in segments.iter().copied() {
        out.extend_from_slice(&(start as u16).to_be_bytes());
    }
    out.extend_from_slice(&0xFFFFu16.to_be_bytes());
    for (start, _) in segments.iter().copied() {
        out.extend_from_slice(&(delta_of(start)? as u16).to_be_bytes());
    }
    out.extend_from_slice(&1u16.to_be_bytes());
    for _ in &segments {
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    // The header names one Unicode BMP table: platform 3, encoding 1, which is what
    // the content is.
    let mut header = Vec::with_capacity(12 + out.len());
    header.extend_from_slice(&[0, 0]);
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&[0, 3, 0, 1]);
    header.extend_from_slice(&12u32.to_be_bytes());
    header.extend_from_slice(&out);
    Ok(header)
}

/// A `post` 3.0: a version and nothing else, because names are metadata.
fn build_post30() -> Vec<u8> {
    let mut out = vec![0u8; 32];
    if let Some(slot) = out.get_mut(0..4) {
        slot.copy_from_slice(&0x0003_0000u32.to_be_bytes());
    }
    out
}

/// The file: a sorted directory, checksummed tables, and a checksum-adjusted `head`.
fn assemble(version: u32, tables: &mut [([u8; 4], Vec<u8>)]) -> Result<Vec<u8>, String> {
    tables.sort_by_key(|(tag, _)| *tag);
    let count = tables.len();
    if count > MAX_TABLES {
        return Err("too many tables".to_string());
    }
    let log: u32 = (count as u32).ilog2();
    let unit: u32 = 1 << log;
    let search_range: u16 = (unit * 16) as u16;
    let entry_selector: u16 = log as u16;
    let range_shift: u16 = (count as u32 * 16 - unit * 16) as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&(count as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    let offset = 12 + count * 16;
    let mut directory: Vec<u8> = Vec::new();
    let mut bodies: Vec<u8> = Vec::new();
    for (tag, bytes) in tables.iter() {
        while bodies.len() % 4 != 0 {
            bodies.push(0);
        }
        let start = offset + bodies.len();
        directory.extend_from_slice(tag);
        directory.extend_from_slice(&checksum(bytes).to_be_bytes());
        directory.extend_from_slice(&(start as u32).to_be_bytes());
        directory.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        bodies.extend_from_slice(bytes);
    }
    out.extend_from_slice(&directory);
    out.extend_from_slice(&bodies);
    // The adjustment goes into the `head` copy inside the file, then the file is whole:
    // zero the field, sum the file, and store what the sum falls short of the magic by.
    let head_index = tables
        .iter()
        .position(|(tag, _)| tag == b"head")
        .ok_or("an internal head error")?;
    let head_at = 12 + head_index * 16;
    let head_offset = out
        .get(head_at + 8..head_at + 12)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_be_bytes)
        .ok_or("an internal head error")? as usize;
    let total: u32 = out
        .chunks(4)
        .map(|c| {
            let mut word = [0u8; 4];
            for (slot, byte) in word.iter_mut().zip(c.iter()) {
                *slot = *byte;
            }
            u32::from_be_bytes(word)
        })
        .fold(0u32, |a, b| a.wrapping_add(b));
    let adjustment = CHECKSUM_MAGIC.wrapping_sub(total);
    if let Some(slot) = out.get_mut(head_offset + 8..head_offset + 12) {
        slot.copy_from_slice(&adjustment.to_be_bytes());
    } else {
        return Err("an internal head error".to_string());
    }
    Ok(out)
}

/// A table's checksum: big-endian words, padded, wrapping.
fn checksum(bytes: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut chunks = bytes.chunks_exact(4);
    for word in &mut chunks {
        let mut value = 0u32;
        for byte in word {
            value = (value << 8) | u32::from(*byte);
        }
        sum = sum.wrapping_add(value);
    }
    // A ragged tail pads with zeros on the right, which is what the checksum means by
    // a partial word.
    let mut value = 0u32;
    let mut shift = 24u32;
    for byte in chunks.remainder() {
        value |= u32::from(*byte) << shift;
        shift = shift.saturating_sub(8);
    }
    sum = sum.wrapping_add(value);
    sum
}

fn u16at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_be_bytes)
}

fn u32at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_be_bytes)
}

fn i16at(bytes: &[u8], at: usize) -> Option<i16> {
    u16at(bytes, at).map(|v| v as i16)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index what they just built; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::{map_format4, subset_truetype};

    /// A format-4 subtable by hand: one delta segment, one range segment, one terminator.
    ///
    /// Hand-built rather than generated so the reader is checked against numbers chosen to
    /// catch its two classic mistakes: `idDelta` read unsigned (A would map to 0xFFC4
    /// rather than 5), and `idRangeOffset` measured from the table rather than from its
    /// own word (0x30 would read wherever that lands rather than 7).
    fn fixture() -> Vec<u8> {
        let mut sub = Vec::new();
        sub.extend_from_slice(&[0, 4]); // format
        sub.extend_from_slice(&[0, 40]); // length: 16 + 3 segments * 8
        sub.extend_from_slice(&[0, 0]); // language
        sub.extend_from_slice(&[0, 6]); // segCountX2
        sub.extend_from_slice(&[0, 4]); // searchRange
        sub.extend_from_slice(&[0, 1]); // entrySelector
        sub.extend_from_slice(&[0, 2]); // rangeShift
        sub.extend_from_slice(&[0x00, 0x43, 0x00, 0x31, 0xFF, 0xFF]); // endCount
        sub.extend_from_slice(&[0, 0]); // reservedPad
        sub.extend_from_slice(&[0x00, 0x41, 0x00, 0x30, 0xFF, 0xFF]); // startCount
        sub.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x00, 0x00, 0x01]); // idDelta
        sub.extend_from_slice(&[0x00, 0x00, 0x00, 0x04, 0x00, 0x00]); // idRangeOffset
        sub.extend_from_slice(&[0x00, 0x07, 0x00, 0x08]); // glyphIdArray
        sub
    }

    #[test]
    fn a_delta_segment_maps_signed() {
        let sub = fixture();
        // A is 0x41; delta is -60 signed, so A maps to 5 rather than to 0xFFC4.
        assert_eq!(map_format4(&sub, 0x41), Ok(Some(5)));
        assert_eq!(map_format4(&sub, 0x43), Ok(Some(7)));
        assert_eq!(map_format4(&sub, 0x44), Ok(None));
    }

    #[test]
    fn a_range_offset_is_measured_from_its_own_word() {
        let sub = fixture();
        // The range word sits at offset 28 and the array at 32: the value 4 bridges them.
        assert_eq!(map_format4(&sub, 0x30), Ok(Some(7)));
        assert_eq!(map_format4(&sub, 0x31), Ok(Some(8)));
    }

    #[test]
    fn a_built_cmap_reads_back_what_was_put_in() {
        let pairs = [(0x41u32, 5u32), (0x42, 6), (0x44, 9), (0xE9, 12)];
        let table = super::build_cmap4(&pairs).expect("three segments build");
        let sub = &table[12..];
        for (char, gid) in pairs {
            assert_eq!(map_format4(sub, char), Ok(Some(gid)), "U+{char:04X}");
        }
        assert_eq!(map_format4(sub, 0x43), Ok(None));
    }

    #[test]
    fn a_cff_program_is_refused_rather_than_misread() {
        let mut font = vec![0u8; 12];
        font[0..4].copy_from_slice(b"OTTO");
        let err = subset_truetype(&font, &[0x41]).expect_err("no glyf to subset");
        assert!(err.contains("TrueType"), "{err}");
    }

    #[test]
    fn an_empty_request_keeps_only_notdef() {
        let data = liberation();
        let out = subset_truetype(&data, &[]).expect("nothing to keep is still a font");
        let face = ttf_parser::Face::parse(&out, 0).expect("a font");
        assert_eq!(face.number_of_glyphs(), 1);
    }

    #[test]
    fn a_missing_glyph_names_its_character() {
        let data = liberation();
        let err = subset_truetype(&data, &[0x378]).expect_err("U+0378 is unassigned");
        assert!(err.contains("U+0378"), "{err}");
    }

    #[test]
    fn a_non_bmp_character_is_refused() {
        let data = liberation();
        let err = subset_truetype(&data, &[0x1F600]).expect_err("no format for astral planes");
        assert!(err.contains("U+1F600"), "{err}");
    }

    fn liberation() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/LiberationSans-Regular.ttf");
        std::fs::read(&path).expect("the bundled face is checked in")
    }
}
