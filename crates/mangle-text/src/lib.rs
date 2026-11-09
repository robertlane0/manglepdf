//! Text decoding, reading-order extraction and search.
//!
//! Decodes glyph runs to Unicode strings via font `/Encoding` tables, the Adobe Glyph List,
//! and `/ToUnicode` CMaps; orders runs into reading order using either the tagged structure
//! tree (`/StructTreeRoot` `MCID`s) when present or recursive XY-cut column/baseline
//! geometry; joins runs into lines and words; and searches the extracted text.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

// ---- decoding one run ----

/// One run of glyphs, as the interpreter recorded it.
///
/// The facts a run's bytes need to be read as text: where it was drawn (which resource table its
/// names resolve against), what it named its font, the string's own bytes, and where those bytes'
/// glyphs sat. The last one is not decoration — it is the only thing that says whether two runs
/// were one word or two.
#[derive(Debug, Clone)]
pub struct Run<'a> {
    /// The form XObject it was drawn in, if any.
    pub form: Option<&'a str>,
    /// The font as the stream named it — `/F3`, which is a resource name and not a family.
    pub font: Option<&'a str>,
    /// The string's own bytes, before any encoding is applied.
    pub text: &'a [u8],
    /// Each glyph's origin, in the same order as the character codes in `text`.
    ///
    /// The codes and the origins are two lists of the same length, and a decoder that read one
    /// without the other would be reading a page it could not lay out. Owned rather than borrowed
    /// because the origins come out of the interpreter's placement matrices, which are not laid
    /// out the way a caller happens to want them.
    pub origins: Vec<(f64, f64)>,
    /// The type size **as the reader sees it**, which is what a gap between runs is measured
    /// against.
    ///
    /// Not the number in the `Tf`. A file may set the font at one point and scale the whole run
    /// with a `cm` — which is ordinary in design exports — and then the `Tf` says `1` while the
    /// glyphs are twelve points tall. A gap measured against the stream's number is a gap measured
    /// against nothing: the threshold came out at a quarter of a point and every space in the page
    /// looked like a word break. The caller has the transformation in force, so the caller
    /// computes this: `size` scaled by how far the `cm` stretches.
    pub size: f64,
}

/// What one run's bytes turned out to be.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// The text. Empty when the run could not be read at all.
    pub text: String,
    /// How many character codes had no glyph in the font's encoding.
    ///
    /// Counted rather than dropped silently, because a page that loses half its letters this way
    /// is a page a user cannot read and has to be told about. `0` is the ordinary answer.
    pub unmapped: usize,
    /// Where the text starts and ends, in page space: the first and last glyph that was read.
    ///
    /// `None` when nothing was read, because a run with no text has no position either. These are
    /// what [`text_of`] joins runs with, and the reason a run carries its origins at all.
    pub extent: Option<((f64, f64), (f64, f64))>,
    /// The type size, kept so the caller can measure a gap in the units the page used.
    pub size: f64,
}

/// Read one run's bytes as text.
///
/// The path is the one the specification describes: the bytes are character *codes*, the font's
/// `/Encoding` says which glyph each code selects, and the glyph's name says which Unicode scalar
/// it is through the Adobe Glyph List. `fi` is one glyph and one code point (U+FB01), which is why
/// a ligature in a file is one character in the text and not two — and why searching for "office"
/// has to know about it.
///
/// `None` when the run has no readable font: a composite font's codes are CIDs and its `/Encoding`
/// is a CMap, so the byte is not a glyph selector and this has nothing to say. That is a real
/// finding about the file and a caller has to surface it rather than return an empty string and
/// call it a page with no text on it.
///
/// A gap between two glyphs of the run wider than a tenth of the type size reads as a word space
/// and is written into the text, because a TeX file has no space glyphs at all: its words are
/// separated by `TJ` adjustments, and without this they run together. The text is then no longer
/// one character per code, which is what [`decode_glyphs`] is for.
#[must_use]
pub fn decode(
    resources: &mangle_content::Resources,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    run: &Run<'_>,
) -> Option<Decoded> {
    decode_with(resources, resolve, run, true)
}

/// [`decode`] without the inferred word spaces: one character for each code the font maps.
///
/// What an editor reads, because a string retyped over a run is cut across the run's glyphs by
/// count, and a space that is only a gap between two glyphs has no glyph to take its share.
#[must_use]
pub fn decode_glyphs(
    resources: &mangle_content::Resources,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    run: &Run<'_>,
) -> Option<Decoded> {
    decode_with(resources, resolve, run, false)
}

/// The one reading behind [`decode`] and [`decode_glyphs`]; `spaces` says whether to infer word
/// spaces from the glyph positions.
fn decode_with(
    resources: &mangle_content::Resources,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    run: &Run<'_>,
    spaces: bool,
) -> Option<Decoded> {
    // A form carries its own table, and a form with none inherits the page's — which is what an
    // absent entry means here, not "use nothing".
    let table = match run.form {
        Some(name) => resources
            .form_resources(name)
            .unwrap_or_else(|| std::sync::Arc::new(resources.clone())),
        None => std::sync::Arc::new(resources.clone()),
    };
    let name = run.font?;
    let font = table.fonts.get(name)?;
    let widths = table.font_widths(name);
    let resolved = resolve(font).unwrap_or_else(|| font.clone());
    let dict = resolved.as_dict()?;
    let to_unicode = mangle_font::ToUnicode::from_font_dict(dict, resolve);
    if dict
        .get("Subtype")
        .and_then(|o| o.as_name())
        .is_some_and(|n| n == b"Type0")
    {
        let cmap = to_unicode?;
        let mut out = Decoded {
            size: run.size,
            ..Decoded::default()
        };
        let codes: Vec<u32> = run
            .text
            .chunks_exact(2)
            .filter_map(|pair| {
                let hi = *pair.first()?;
                let lo = *pair.get(1)?;
                Some(u32::from(u16::from_be_bytes([hi, lo])))
            })
            .collect();
        let base_tc = estimate_base_char_spacing(&codes, &run.origins, widths, run.size, 1000);
        let mut prev: Option<((f64, f64), u32)> = None;
        for (code, origin) in codes.into_iter().zip(run.origins.iter().copied()) {
            match cmap.get(code) {
                Some(s) if !s.is_empty() => {
                    if spaces
                        && let Some((prev_origin, prev_code)) = prev
                        && should_insert_in_run_space(
                            prev_origin,
                            prev_code,
                            origin,
                            widths,
                            run.size,
                            1000,
                            base_tc,
                            &out.text,
                            s,
                        )
                    {
                        out.text.push(' ');
                    }
                    out.extent = Some(match out.extent {
                        Some((first, _)) => (first, origin),
                        None => (origin, origin),
                    });
                    out.text.push_str(s);
                    prev = Some((origin, code));
                }
                _ => out.unmapped += 1,
            }
        }
        return Some(out);
    }
    let encoding =
        mangle_font::Encoding::from_font_dict(dict, mangle_font::EncodingBase::Standard, resolve);
    if to_unicode.is_none() && encoding.is_none() {
        return None;
    }
    let mut out = Decoded {
        size: run.size,
        ..Decoded::default()
    };
    let codes: Vec<u32> = run.text.iter().map(|b| u32::from(*b)).collect();
    let base_tc = estimate_base_char_spacing(&codes, &run.origins, widths, run.size, 500);
    let mut prev: Option<((f64, f64), u32)> = None;
    for (code, origin) in codes.into_iter().zip(run.origins.iter().copied()) {
        if let Some(s) = to_unicode.as_ref().and_then(|cmap| cmap.get(code))
            && !s.is_empty()
        {
            if spaces
                && let Some((prev_origin, prev_code)) = prev
                && should_insert_in_run_space(
                    prev_origin,
                    prev_code,
                    origin,
                    widths,
                    run.size,
                    500,
                    base_tc,
                    &out.text,
                    s,
                )
            {
                out.text.push(' ');
            }
            out.extent = Some(match out.extent {
                Some((first, _)) => (first, origin),
                None => (origin, origin),
            });
            out.text.push_str(s);
            prev = Some((origin, code));
            continue;
        }
        let Some(glyph) = encoding.as_ref().and_then(|enc| enc.glyph_for(code)) else {
            out.unmapped += 1;
            continue;
        };
        match mangle_font::agl(glyph).and_then(char::from_u32) {
            Some(c) => {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                if spaces
                    && let Some((prev_origin, prev_code)) = prev
                    && should_insert_in_run_space(
                        prev_origin,
                        prev_code,
                        origin,
                        widths,
                        run.size,
                        500,
                        base_tc,
                        &out.text,
                        s,
                    )
                {
                    out.text.push(' ');
                }
                let (ox, oy) = origin;
                let at = (ox, oy);
                out.extent = Some(match out.extent {
                    Some((first, _)) => (first, at),
                    None => (at, at),
                });
                out.text.push(c);
                prev = Some((at, code));
            }
            None => out.unmapped += 1,
        }
    }
    Some(out)
}

/// Estimate uniform `Tc` character spacing when every inter-glyph gap in a run is equal.
fn estimate_base_char_spacing(
    codes: &[u32],
    origins: &[(f64, f64)],
    widths: Option<&mangle_font::metrics::DeclaredWidths>,
    size: f64,
    default_width: u16,
) -> f64 {
    if codes.len() < 3 || origins.len() < 3 {
        return 0.0;
    }
    let mut min_gap = f64::INFINITY;
    let mut max_gap = f64::NEG_INFINITY;
    let mut count = 0usize;
    for i in 0..codes.len().min(origins.len()).saturating_sub(1) {
        let (Some(&code), Some(&o0), Some(&o1)) =
            (codes.get(i), origins.get(i), origins.get(i + 1))
        else {
            continue;
        };
        if code == 32 {
            continue;
        }
        let w = widths
            .and_then(|fw| fw.width_of(code))
            .unwrap_or(default_width);
        let adv = (f64::from(w) / 1000.0) * size;
        let dist = (o1.0 - o0.0).hypot(o1.1 - o0.1);
        let gap = dist - adv;
        min_gap = min_gap.min(gap);
        max_gap = max_gap.max(gap);
        count += 1;
    }
    if count >= 2 && min_gap > size * 0.08 && (max_gap - min_gap).abs() <= size * 0.04 {
        min_gap
    } else {
        0.0
    }
}

/// Whether the gap between two glyph origins within one run is a word space.
///
/// The gap is how far the pen travelled less the previous glyph's advance and the run's own
/// character spacing, so a `TJ` adjustment shows up as a single number. It is a space when it
/// is more than a tenth of the type size, about where poppler breaks; a run that already has a
/// space on either side of the gap is left alone.
#[allow(clippy::too_many_arguments)]
fn should_insert_in_run_space(
    prev_origin: (f64, f64),
    prev_code: u32,
    curr_origin: (f64, f64),
    widths: Option<&mangle_font::metrics::DeclaredWidths>,
    size: f64,
    default_width: u16,
    base_tc: f64,
    accumulated: &str,
    next_str: &str,
) -> bool {
    if accumulated.is_empty()
        || accumulated.ends_with(char::is_whitespace)
        || next_str.starts_with(char::is_whitespace)
    {
        return false;
    }
    let w = widths
        .and_then(|fw| fw.width_of(prev_code))
        .unwrap_or(default_width);
    let adv = (f64::from(w) / 1000.0) * size + base_tc;
    let dist = (curr_origin.0 - prev_origin.0).hypot(curr_origin.1 - prev_origin.1);
    let gap = dist - adv;
    gap > size * 0.10
}

/// Join decoded runs into a page's text, in the order they are given.
///
/// **The separator comes from the geometry, not from the bytes.** A `Tj` names no space and a
/// file sets two words apart by placing the second further along the baseline, so joining runs
/// with nothing produces `NISTSpecialPublication800-88` and joining them all with a space
/// produces `NIS T Special` — both wrong. Two runs on different baselines are separated by a
/// newline; two on the same baseline are separated by a space when the gap between them is more
/// than a quarter of the type size, which is roughly a space's own width.
///
/// The threshold is a heuristic and it is stated as one: a page that sets its words a hair apart
/// will come out joined, and a page that kern its letters far apart will come out with spaces
/// inside words. Both are visible in the output, which is better than a separator that was
/// guessed from the bytes and could not be checked at all.
#[must_use]
pub fn text_of(runs: &[Decoded]) -> String {
    let mut out = String::new();
    let mut previous: Option<Placed> = None;
    for run in runs {
        let Some(extent) = run.extent else {
            continue;
        };
        let here = Placed::new(extent, run.size);
        if let Some(before) = previous {
            // A new baseline is a new line, and the larger of the two runs' sizes is what decides
            // how far apart two baselines have to be to count as different: a superscript sits
            // further from its line than its own size is tall.
            let tall = before.size.max(run.size);
            if (here.sy - before.ey).abs() > tall * 0.55 || here.sx < before.sx - tall * 0.5 {
                out.push('\n');
            } else if here.sx - before.ex > tall * 0.25
                && !out.ends_with(char::is_whitespace)
                && !run.text.starts_with(char::is_whitespace)
            {
                out.push(' ');
            }
        }
        out.push_str(&run.text);
        previous = Some(here);
    }
    out
}

/// Order decoded runs into reading order, using `/StructTreeRoot` `MCID`s when available
/// and falling back to recursive XY-cut column/baseline ordering for untagged runs.
#[must_use]
pub fn reading_order_runs(runs: &[(Option<i64>, Decoded)], struct_mcids: &[i64]) -> Vec<Decoded> {
    let valid: Vec<(usize, Option<i64>, &Decoded)> = runs
        .iter()
        .enumerate()
        .filter(|(_, (_, d))| d.extent.is_some() && !d.text.is_empty())
        .map(|(i, (m, d))| (i, *m, d))
        .collect();
    if valid.is_empty() {
        return Vec::new();
    }

    let has_tagged = !struct_mcids.is_empty()
        && valid
            .iter()
            .any(|(_, m, _)| m.is_some_and(|id| struct_mcids.contains(&id)));

    if has_tagged {
        let mut used = vec![false; valid.len()];
        let mut ordered = Vec::with_capacity(valid.len());
        for &target in struct_mcids {
            for (slot, (_, mcid, decoded)) in valid.iter().enumerate() {
                if *mcid == Some(target)
                    && let Some(flag) = used.get_mut(slot)
                    && !*flag
                {
                    *flag = true;
                    ordered.push((*decoded).clone());
                }
            }
        }
        let untagged: Vec<Decoded> = valid
            .iter()
            .enumerate()
            .filter(|(slot, _)| !used.get(*slot).copied().unwrap_or(false))
            .map(|(_, (_, _, d))| (*d).clone())
            .collect();
        ordered.extend(geometric_reading_order(&untagged));
        return ordered;
    }

    let plain: Vec<Decoded> = valid.into_iter().map(|(_, _, d)| d.clone()).collect();
    geometric_reading_order(&plain)
}

/// Order runs by recursive XY-cut (vertical column gutters and horizontal bands), then
/// top-to-bottom (`sy` descending) and left-to-right (`sx` ascending) within each block.
#[must_use]
pub fn geometric_reading_order(runs: &[Decoded]) -> Vec<Decoded> {
    let items: Vec<(usize, Placed)> = runs
        .iter()
        .enumerate()
        .filter_map(|(i, r)| Some((i, Placed::new(r.extent?, r.size.max(1.0)))))
        .collect();
    let mut order = Vec::with_capacity(items.len());
    xy_cut(&items, 0, &mut order);
    order
        .into_iter()
        .filter_map(|i| runs.get(i).cloned())
        .collect()
}

fn xy_cut(items: &[(usize, Placed)], depth: usize, out: &mut Vec<usize>) {
    if items.is_empty() {
        return;
    }
    if items.len() == 1 || depth >= 16 {
        for (idx, _) in items {
            out.push(*idx);
        }
        return;
    }

    let max_size = items.iter().map(|(_, p)| p.size).fold(1.0_f64, f64::max);

    // 1. Check for a vertical column gutter that cleanly separates left and right groups.
    let mut by_x = items.to_vec();
    by_x.sort_by(|(_, a), (_, b)| a.sx.min(a.ex).total_cmp(&b.sx.min(b.ex)));
    let mut running_max_x = f64::NEG_INFINITY;
    let mut best_x_split: Option<(usize, f64)> = None;
    for k in 1..by_x.len() {
        if let Some((_, prev)) = by_x.get(k - 1) {
            running_max_x = running_max_x.max(prev.sx.max(prev.ex));
        }
        if let Some((_, curr)) = by_x.get(k) {
            let gap = curr.sx.min(curr.ex) - running_max_x;
            if gap > max_size * 1.5 && best_x_split.is_none_or(|(_, best)| gap > best) {
                best_x_split = Some((k, gap));
            }
        }
    }
    if let Some((split_idx, _)) = best_x_split
        && let (Some(left), Some(right)) = (by_x.get(..split_idx), by_x.get(split_idx..))
    {
        xy_cut(left, depth + 1, out);
        xy_cut(right, depth + 1, out);
        return;
    }

    // 2. Check for a horizontal band gap (top-to-bottom in PDF space: y descending).
    let mut by_y = items.to_vec();
    by_y.sort_by(|(_, a), (_, b)| b.sy.max(b.ey).total_cmp(&a.sy.max(a.ey)));
    let mut running_min_y = f64::INFINITY;
    let mut best_y_split: Option<(usize, f64)> = None;
    for k in 1..by_y.len() {
        if let Some((_, prev)) = by_y.get(k - 1) {
            running_min_y = running_min_y.min(prev.sy.min(prev.ey));
        }
        if let Some((_, curr)) = by_y.get(k) {
            let gap = running_min_y - curr.sy.max(curr.ey);
            if gap > max_size * 0.6 && best_y_split.is_none_or(|(_, best)| gap > best) {
                best_y_split = Some((k, gap));
            }
        }
    }
    if let Some((split_idx, _)) = best_y_split
        && let (Some(top), Some(bottom)) = (by_y.get(..split_idx), by_y.get(split_idx..))
    {
        xy_cut(top, depth + 1, out);
        xy_cut(bottom, depth + 1, out);
        return;
    }

    // 3. Base case: group runs into baseline bands (keeping superscripts and subscripts with
    // their line), then sort top-to-bottom by line and left-to-right within each line.
    let mut by_baseline = items.to_vec();
    by_baseline.sort_by(|(_, a), (_, b)| {
        b.size
            .total_cmp(&a.size)
            .then_with(|| b.sy.total_cmp(&a.sy))
            .then_with(|| a.sx.total_cmp(&b.sx))
    });
    let mut lines: Vec<Band> = Vec::new();
    for (idx, placed) in by_baseline {
        if let Some(line) = lines.iter_mut().find(|line| {
            let tall = line.size.max(placed.size);
            (placed.sy - line.y).abs() <= tall * 0.55
        }) {
            line.size = line.size.max(placed.size);
            line.members.push((idx, placed));
        } else {
            lines.push(Band {
                y: placed.sy,
                size: placed.size,
                members: vec![(idx, placed)],
            });
        }
    }
    lines.sort_by(|a, b| b.y.total_cmp(&a.y));
    for mut line in lines {
        line.members.sort_by(|(_, a), (_, b)| a.sx.total_cmp(&b.sx));
        for (idx, _) in line.members {
            out.push(idx);
        }
    }
}

/// Decode and join a page's `Record`s in reading order (`struct_mcids` when tagged,
/// otherwise geometric XY-cut column/baseline order), skipping `/Artifact` marks when
/// non-artifact body text is present.
#[must_use]
pub fn extract_records_in_reading_order(
    records: &[mangle_content::Record],
    resources: &mangle_content::Resources,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    struct_mcids: &[i64],
) -> String {
    let has_non_artifact = records.iter().any(|r| {
        matches!(r.mark, mangle_content::Mark::Glyphs { .. })
            && r.tag.as_deref() != Some("Artifact")
    });
    let mut decoded = Vec::new();
    for record in records {
        if has_non_artifact && record.tag.as_deref() == Some("Artifact") {
            continue;
        }
        let mangle_content::Mark::Glyphs {
            text,
            placements,
            size,
            ..
        } = &record.mark
        else {
            continue;
        };
        let scale = (record.text_matrix.a * record.text_matrix.d
            - record.text_matrix.b * record.text_matrix.c)
            .abs()
            .sqrt()
            .max(1.0);
        let run = Run {
            form: record.form.as_deref(),
            font: record.text.font.as_deref(),
            text,
            origins: placements.iter().map(|m| (m.e, m.f)).collect(),
            size: (*size * scale).max(1.0),
        };
        if let Some(d) = decode(resources, resolve, &run) {
            decoded.push((record.mcid, d));
        }
    }
    let ordered = reading_order_runs(&decoded, struct_mcids);
    text_of(&ordered)
}

/// Where one run's text sits, flattened out of [`Decoded::extent`].
///
/// A struct rather than a nested tuple because the four numbers are four different questions —
/// where the text starts, where it ends, and how big it is — and a type that read `((f64, f64),
/// (f64, f64), f64)` at the point of use answered none of them by name.
#[derive(Debug, Clone, Copy)]
struct Placed {
    /// Where the first glyph that was read sits.
    sx: f64,
    sy: f64,
    /// Where the last glyph that was read sits.
    ex: f64,
    ey: f64,
    /// The type size the reader sees.
    size: f64,
}

impl Placed {
    fn new(((sx, sy), (ex, ey)): ((f64, f64), (f64, f64)), size: f64) -> Self {
        Self {
            sx,
            sy,
            ex,
            ey,
            size,
        }
    }
}

/// One line of text: where its baseline sits, the tallest type on it, and the runs on it.
///
/// Named rather than a `(f64, f64, Vec<..>)` for the same reason as [`Placed`].
struct Band {
    y: f64,
    size: f64,
    members: Vec<(usize, Placed)>,
}

// ---- search ----

/// Where a needle was found, as character offsets into the text that was searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// Where the match starts, in characters from the beginning.
    pub start: usize,
    /// Where it ends, exclusive.
    pub end: usize,
}

/// Every occurrence of `needle` in `haystack`, compared without regard to case.
///
/// Case-insensitive because a user who types "beautiful" means it whatever case the page happens
/// to have set it in — a title in small caps, a heading in caps — and a search that found nothing
/// for a word that is plainly on the page would be a search that lied about what was there.
///
/// The offsets are **bytes**, because that is what slicing a Rust string needs, and a hit that
/// could not be sliced back out of the text it was found in would be a hit nobody could show.
///
/// Matches may not overlap, and an empty needle finds nothing: every position in a string "matches"
/// an empty needle, which is an answer that tells a user nothing and would fill a hit list with
/// the whole page.
#[must_use]
pub fn find(haystack: &str, needle: &str) -> Vec<Hit> {
    if needle.is_empty() {
        return Vec::new();
    }
    // Lowercased once each rather than per comparison. `char::to_lowercase` can yield more than
    // one character, so the lowercased text is *not* the same length as the original and a plain
    // index into it is not a byte offset. Each output character therefore carries the byte offset
    // in the original where its source began, which is what makes the hit sliceable.
    let mut text: Vec<char> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for (at, c) in haystack.char_indices() {
        for lower in c.to_lowercase() {
            text.push(lower);
            starts.push(at);
        }
    }
    let want: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let mut out = Vec::new();
    let mut at = 0usize;
    // The window is taken with `get` and the `else` breaks, rather than indexed: this crate denies
    // indexing, and a search that could panic would be a search that could take the window down
    // with a page open. The loop condition is what stops it, so the `else` is the guard and not a
    // second way to say the same thing.
    // The window is taken with `get` rather than indexed: this crate denies indexing, and a search
    // that could panic would be a search that could take the window down with a page open.
    while let Some(window) = text.get(at..at + want.len()) {
        if window != want.as_slice() {
            at += 1;
            continue;
        }
        let Some(start) = starts.get(at).copied() else {
            break;
        };
        // The byte offset the character *after* the match began, or the end of the text.
        let end = starts
            .get(at + want.len())
            .copied()
            .unwrap_or(haystack.len());
        out.push(Hit { start, end });
        at += want.len();
    }
    out
}

/// Which object each hit falls in, given the text of each object in order.
///
/// A hit is attributed to the object it *starts* in, which is the one a user would point at. A
/// phrase that runs from the end of one object into the next is still found — it is matched
/// against the joined text — and is shown against the object where it begins.
#[must_use]
pub fn locate(texts: &[String], hits: &[Hit]) -> Vec<(usize, Hit)> {
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        // The running offset is per hit, not per call: it has to start at the beginning of the
        // first object every time, or the second hit on a page would be measured from wherever
        // the first one ended and every hit after the first would land in the wrong object.
        let mut at = 0usize;
        for (object, text) in texts.iter().enumerate() {
            let end = at + text.len();
            if hit.start >= at && hit.start < end {
                out.push((object, *hit));
                break;
            }
            at = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn run_at(text: &str, x0: f64, x1: f64, y: f64) -> Decoded {
        Decoded {
            text: text.to_string(),
            unmapped: 0,
            extent: Some(((x0, y), (x1, y))),
            size: 10.0,
        }
    }

    #[test]
    fn tagged_runs_follow_struct_tree_mcid_order() {
        let runs = vec![
            (Some(1), run_at("second", 72.0, 120.0, 680.0)),
            (Some(0), run_at("first", 72.0, 110.0, 700.0)),
            (Some(2), run_at("third", 320.0, 360.0, 700.0)),
        ];
        let ordered = reading_order_runs(&runs, &[0, 1, 2]);
        assert_eq!(text_of(&ordered), "first\nsecond\nthird");
    }

    #[test]
    fn untagged_two_column_runs_with_spanning_header_read_column_by_column() {
        // Drawn in row-interleaved order below a full-width header:
        // Header (x=72..500, y=750)
        // Col1 Row1 (x=72..200, y=700), Col2 Row1 (x=320..450, y=700)
        // Col1 Row2 (x=72..200, y=680), Col2 Row2 (x=320..450, y=680)
        let runs = vec![
            (None, run_at("Col1-Row1", 72.0, 200.0, 700.0)),
            (None, run_at("Col2-Row1", 320.0, 450.0, 700.0)),
            (None, run_at("Header", 72.0, 500.0, 750.0)),
            (None, run_at("Col1-Row2", 72.0, 200.0, 680.0)),
            (None, run_at("Col2-Row2", 320.0, 450.0, 680.0)),
        ];
        let ordered = reading_order_runs(&runs, &[]);
        assert_eq!(
            text_of(&ordered),
            "Header\nCol1-Row1\nCol1-Row2\nCol2-Row1\nCol2-Row2"
        );
    }

    /// Resources with one Helvetica face, `F1`, in WinAnsi.
    fn helvetica_resources() -> mangle_content::Resources {
        let mut font = mangle_syntax::object::Dict::new();
        font.set("Type", mangle_syntax::object::Object::name("Font"));
        font.set("Subtype", mangle_syntax::object::Object::name("Type1"));
        font.set("BaseFont", mangle_syntax::object::Object::name("Helvetica"));
        font.set(
            "Encoding",
            mangle_syntax::object::Object::name("WinAnsiEncoding"),
        );
        let mut fonts = mangle_syntax::object::Dict::new();
        fonts.set("F1", mangle_syntax::object::Object::Dict(font));
        let mut res_dict = mangle_syntax::object::Dict::new();
        res_dict.set("Font", mangle_syntax::object::Object::Dict(fonts));
        mangle_content::Resources::from_dict(&res_dict, &|o| Some(o.clone()))
    }

    #[test]
    fn decode_glyphs_has_one_character_per_code_where_decode_infers_word_spaces() {
        let resources = helvetica_resources();
        let stream = mangle_content::ContentStream::parse(
            b"BT /F1 10 Tf 72 700 Td [(The)-242(technical)-241(documentation)] TJ ET",
        );
        let content = mangle_content::interp::run_with(&stream, &resources);
        let raw = content
            .records
            .iter()
            .find_map(|r| match &r.mark {
                mangle_content::interp::Mark::Glyphs {
                    text,
                    placements,
                    size,
                    ..
                } => Some((
                    text.clone(),
                    placements.iter().map(|m| (m.e, m.f)).collect::<Vec<_>>(),
                    *size,
                )),
                _ => None,
            })
            .expect("a glyph run");
        let run = Run {
            form: None,
            font: Some("F1"),
            text: &raw.0,
            origins: raw.1.clone(),
            size: raw.2,
        };
        let resolve = |o: &mangle_syntax::object::Object| Some(o.clone());
        assert_eq!(
            decode(&resources, &resolve, &run).expect("readable").text,
            "The technical documentation"
        );
        let glyphs = decode_glyphs(&resources, &resolve, &run).expect("readable");
        assert_eq!(glyphs.text, "Thetechnicaldocumentation");
        assert_eq!(
            glyphs.text.chars().count(),
            run.text.len(),
            "one character for each code, which is what an editor cuts a retype by"
        );
    }

    #[test]
    fn tj_numeric_adjustments_insert_word_spaces_at_a_tenth_of_an_em() {
        let resources = helvetica_resources();

        let stream = mangle_content::ContentStream::parse(
            b"BT /F1 12 Tf 72 700 Td [(alpha) -150 (beta) -250 (gamma) -300 (delta) -500 (epsilon) -1000 (zeta)] TJ ET",
        );
        let content = mangle_content::interp::run_with(&stream, &resources);
        let text = extract_records_in_reading_order(
            &content.records,
            &resources,
            &|o| Some(o.clone()),
            &[],
        );
        assert_eq!(text, "alpha beta gamma delta epsilon zeta");

        let tex_stream = mangle_content::ContentStream::parse(
            b"BT /F1 10 Tf 72 700 Td [(The)-242(technical)-241(documentation)] TJ ET",
        );
        let tex_content = mangle_content::interp::run_with(&tex_stream, &resources);
        let tex_text = extract_records_in_reading_order(
            &tex_content.records,
            &resources,
            &|o| Some(o.clone()),
            &[],
        );
        assert_eq!(tex_text, "The technical documentation");
    }

    #[test]
    fn superscript_offset_above_three_tenths_of_an_em_stays_in_line_order() {
        let runs = vec![
            (
                None,
                Decoded {
                    text: "E = mc".into(),
                    unmapped: 0,
                    extent: Some(((72.0, 700.0), (105.0, 700.0))),
                    size: 10.0,
                },
            ),
            (
                None,
                Decoded {
                    text: "2".into(),
                    unmapped: 0,
                    extent: Some(((106.0, 704.2), (110.0, 704.2))),
                    size: 7.0,
                },
            ),
            (
                None,
                Decoded {
                    text: "and H".into(),
                    unmapped: 0,
                    extent: Some(((114.0, 700.0), (140.0, 700.0))),
                    size: 10.0,
                },
            ),
            (
                None,
                Decoded {
                    text: "2".into(),
                    unmapped: 0,
                    extent: Some(((144.0, 697.5), (148.0, 697.5))),
                    size: 7.0,
                },
            ),
            (
                None,
                Decoded {
                    text: "O".into(),
                    unmapped: 0,
                    extent: Some(((152.0, 700.0), (160.0, 700.0))),
                    size: 10.0,
                },
            ),
        ];
        let ordered = reading_order_runs(&runs, &[]);
        assert_eq!(text_of(&ordered), "E = mc2 and H 2 O");
    }
}
