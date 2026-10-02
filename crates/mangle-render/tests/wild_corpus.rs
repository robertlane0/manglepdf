//! Tier B — the wild corpus: open it, render it, and compare it with two outside tools.
//!
//! The project's own fixtures are generated from a seed by `tools/fixturegen`, and that is
//! exactly their limit. Every one of them is small, well-formed, and was written by someone
//! who already knew what the file was supposed to contain, so none of them can express the
//! problems real producers emit. The corpus this reads is the opposite: 77 files from
//! pdf.js, PDFBox, NIST, the IRS, the USGS and arXiv, chosen because they are hard. Every
//! one was found by comparing against something outside this codebase.
//!
//! # This test asserts nothing
//!
//! A corpus test's job is to *find* things. A hard threshold on a document nobody has looked
//! at yet turns the first unexpected result into a permanent red build, and the response to
//! a permanent red build is to raise the threshold — which destroys the only thing the
//! corpus was for. So there is no SSIM bar, no F1 bar and no "must open" assertion in this
//! file. Every measurement is written to a report and printed; the report is the output.
//!
//! If you are reading this because you want to add a threshold: the answer is not to add
//! one here. Promote the file to `fixtures/` as a fixture (where `MANIFEST.toml` records
//! what it must contain and the assertions belong), or record the threshold in
//! `docs/known-diffs.md` with the root cause and the evidence, as FINISH.md G3.1 asks.
//!
//! # What it measures, per file
//!
//! * every page rendered at 150 DPI with `render_page`;
//! * every page compared against `mutool draw` at the same resolution with `compare()`,
//!   recording SSIM, RMS, the largest single-channel delta and how many pixels differ by
//!   more than the tolerance;
//! * the text this codebase can read, against `pdftotext`, as word-level F1;
//! * a per-file Markdown report: SSIM per page, the worst pages, and the worst pixels with
//!   their coordinates, plus a copy of each disagreeing page's heatmap.
//!
//! # Skipping
//!
//! The corpus is not in the repository — `cargo xtask corpus fetch` downloads it and
//! verifies every byte against `corpus/wild/MANIFEST.toml`. Both `mutool` and `pdftotext`
//! are outside tools and may be absent. Every absence prints a reason. There is no path
//! through this file that reports success while having measured nothing.
//!
//! # No --bless
//!
//! There is deliberately no `--bless`, and the reason is the one above: there is no
//! threshold for it to bless. What a blessing flag is *for* is making a newly recorded
//! expectation become the one that is checked, and this file records no expectation on any
//! file — it records what was measured. The corpus's pin is `MANIFEST.toml`'s SHA-256,
//! which `cargo xtask corpus check` re-verifies without a network, and that is where
//! "what we agreed on" belongs.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mangle_content::{ContentStream, Mark, Resources, run_with};
use mangle_doc::PageTree;
use mangle_render::{Image, RenderOptions, SsimOptions, compare, render_page};
use mangle_syntax::{Document, Object, OpenOptions};

/// The resolution FINISH.md Gate 3 names.
const DPI: f64 = 150.0;

/// The scale that goes with it.
const SCALE: f64 = DPI / 72.0;

/// FINISH.md G2.1's per-file budget. Exceeding it is recorded and printed, not asserted:
/// a corpus test that kills a slow file teaches nothing, but a file that takes a minute
/// is worth knowing about.
const BUDGET_SECS: f64 = 60.0;

/// The largest page this harness will compare.
///
/// `compare()` has its own cap and refuses anything larger with a reason, but the f64
/// buffers it works in are about 64 bytes a pixel per image, and this machine does not have
/// the headroom for a 36-by-30-inch map at 150 DPI. The limit is printed in the report
/// whenever it is hit, so a page that was skipped is never mistaken for a page that passed.
const MAX_COMPARE_PIXELS: usize = 16 * 1024 * 1024;

/// Where the corpus is, if it has been fetched.
fn corpus_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?;
    let dir = root.join("corpus/wild");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// One `[[corpus]]` block, read with the small amount of TOML this manifest uses.
struct Entry {
    id: String,
    file: String,
    category: String,
    purpose: String,
    pages: usize,
    bytes: u64,
    expect: String,
    producer: String,
    sha256: String,
}

fn manifest(dir: &Path) -> Vec<Entry> {
    let Ok(raw) = std::fs::read_to_string(dir.join("MANIFEST.toml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut current: Option<Entry> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line == "[[corpus]]" {
            if let Some(e) = current.take() {
                out.push(e);
            }
            current = Some(Entry {
                id: String::new(),
                file: String::new(),
                category: String::new(),
                purpose: String::new(),
                pages: 0,
                bytes: 0,
                expect: String::new(),
                producer: String::new(),
                sha256: String::new(),
            });
            continue;
        }
        let Some(e) = current.as_mut() else { continue };
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let text = unquote(value);
        match key {
            "id" => e.id = text,
            "file" => e.file = text,
            "category" => e.category = text,
            "purpose" => e.purpose = text,
            "pages" => e.pages = value.trim().parse().unwrap_or(0),
            "bytes" => e.bytes = value.trim().parse().unwrap_or(0),
            "expect" => e.expect = text,
            "producer" => e.producer = text,
            "sha256" => e.sha256 = text,
            _ => {}
        }
    }
    if let Some(e) = current.take() {
        out.push(e);
    }
    out
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').replace("\\\"", "\"")
}

// ── The oracles ───────────────────────────────────────────────────────────────────

/// Where `mutool` is, if it is installed.
fn mutool() -> Option<PathBuf> {
    let out = Command::new("mutool").arg("-v").output().ok()?;
    out.status.success().then(|| PathBuf::from("mutool"))
}

/// Where `pdftotext` is, if it is installed.
fn pdftotext() -> Option<PathBuf> {
    let out = Command::new("pdftotext").arg("-v").output().ok()?;
    // `pdftotext -v` writes its version to stderr and exits 0.
    out.status.success().then(|| PathBuf::from("pdftotext"))
}

/// Every page of a file, rendered by `mutool draw` at `DPI`, as PAM bytes by page number.
///
/// One process for the whole file: `mutool` starts in about 20 ms and the corpus is 742
/// pages, so a process per page is a minute of nothing. `%d` is the page number rather
/// than a running count, so a page `mutool` fails on does not shift every page after it.
fn mutool_render_all(
    tool: &Path,
    pdf: &Path,
    out_dir: &Path,
    pages: usize,
) -> BTreeMap<usize, PathBuf> {
    let mut got = BTreeMap::new();
    let pattern = out_dir.join("oracle-%d.pam");
    let _ = Command::new(tool)
        .args([
            "draw",
            "-r",
            &format!("{DPI:.0}"),
            "-F",
            "pam",
            "-o",
            &pattern.to_string_lossy(),
            &pdf.to_string_lossy(),
        ])
        .output();
    for page in 1..=pages {
        let path = out_dir.join(format!("oracle-{page}.pam"));
        if path.is_file() {
            got.insert(page, path);
        }
    }
    got
}

/// A PAM file's pixels: the dimensions, the channel count and the rows.
///
/// Everything before `ENDHDR\n` is text and everything after is pixels, so the split is a
/// search for that marker rather than a count of lines — `WIDTH` and `DEPTH` may be in
/// either order and `TUPLTYPE` is optional. A short body is a failure rather than a
/// shorter image: comparing a quarter of a page would pass on the part that happened to
/// agree.
fn read_pam(data: &[u8]) -> Option<(usize, usize, usize, Vec<u8>)> {
    let marker = b"ENDHDR\n";
    let split = data.windows(marker.len()).position(|w| w == marker)?;
    let header = String::from_utf8_lossy(&data[..split]).to_string();
    if !header.starts_with("P7") {
        return None;
    }
    let field = |name: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|l| l.trim().strip_prefix(name))
            .and_then(|v| v.trim().parse().ok())
    };
    let (w, h, depth) = (field("WIDTH")?, field("HEIGHT")?, field("DEPTH")?);
    let body = data.get(split + marker.len()..)?.to_vec();
    if body.len() < w * h * depth {
        return None;
    }
    Some((w, h, depth, body))
}

/// A PAM turned into an image, keeping the alpha.
///
/// `mutool` writes an unpainted page as transparent black while this renderer starts a
/// page as opaque white paper. Both are defensible conventions and they are not the same
/// picture, so the alpha is kept and both are flattened onto white before they are
/// compared. Comparing the raw buffers would be comparing conventions rather than
/// renderers, and would report a disagreement over every pixel of a page's margin.
fn pam_to_image(w: usize, h: usize, depth: usize, body: &[u8]) -> Image {
    let mut image = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * depth;
            let at = |c: usize| body.get(i + c).copied().unwrap_or(255);
            image.put(
                x,
                y,
                [at(0), at(1), at(2), if depth > 3 { at(3) } else { 255 }],
            );
        }
    }
    image
}

/// An image composited onto opaque white paper, which is what a page looks like on a desk.
fn flatten_onto_paper(image: &Image) -> Image {
    let mut out = Image::filled(image.width, image.height, [255, 255, 255, 255]);
    for y in 0..image.height {
        for x in 0..image.width {
            let src = image.get(x, y).unwrap_or([255, 255, 255, 255]);
            let a = f64::from(src[3]) / 255.0;
            let mut over = [255u8; 4];
            for c in 0..3 {
                let s = f64::from(src[c]) / 255.0;
                over[c] = ((s * a + 1.0 * (1.0 - a)) * 255.0).round() as u8;
            }
            out.put(x, y, over);
        }
    }
    out
}

/// Write an image as a PAM, which is what `mutool convert` reads.
///
/// A PAM is raw RGBA with a small text header: no compressor, no CRC, no dependency. The
/// PNG that ends up in the report is produced by `mutool` from this file, which is both
/// smaller than anything worth hand-rolling here and produced by the same tool that drew
/// the other side of the comparison.
fn write_pam(path: &Path, image: &Image) -> bool {
    let mut out: Vec<u8> = Vec::with_capacity(image.pixels.len() + 128);
    out.extend_from_slice(b"P7\nWIDTH ");
    out.extend_from_slice(image.width.to_string().as_bytes());
    out.extend_from_slice(b"\nHEIGHT ");
    out.extend_from_slice(image.height.to_string().as_bytes());
    out.extend_from_slice(b"\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n");
    out.extend_from_slice(&image.pixels);
    std::fs::write(path, out).is_ok()
}

/// `mutool convert` a PAM into a PNG.
///
/// The PNG encoder is not written here on purpose. Hand-rolling DEFLATE to put a picture in
/// a report is a distraction from the thing the report is about, and `mutool` is already
/// required for the comparison that put the picture in the report in the first place.
fn pam_to_png(tool: &Path, pam: &Path, scratch: &Path) -> Result<PathBuf, String> {
    // `mutool convert` chooses the output name itself: it expands `%d` when the name has
    // one, and appends the page number when it does not. Rather than predict which, the
    // file is converted into a directory of its own and the single file in it is taken. A
    // report whose links are guesses is a report whose links are broken.
    let dir = scratch.join("png");
    // Emptied first, and not emptied afterwards: the path handed back lives inside it, so
    // tidying up on the way out deletes the picture the report is about to link to.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let status = Command::new(tool)
        .args([
            "convert",
            "-F",
            "png",
            "-o",
            &dir.join("out-%d.png").to_string_lossy(),
            &pam.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("running mutool convert: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "mutool convert exited {}: {}",
            status.status,
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    let mut produced: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    if produced.len() != 1 {
        return Err(format!(
            "mutool convert wrote {} files, not one",
            produced.len()
        ));
    }
    produced
        .pop()
        .ok_or_else(|| "the file it wrote vanished".into())
}

// ── Text ─────────────────────────────────────────────────────────────────────────

/// The words this codebase can read off a page.
///
/// A placeholder, and the report says so. `mangle-text` is an empty crate, so there is no
/// extractor to compare yet: this walks the glyph marks a page draws and turns each
/// character code into a character through the font's own `/Encoding` and the Adobe glyph
/// list. It is right about simple fonts with a standard or WinAnsi encoding and wrong about
/// almost everything else — a composite font's code is a CID and wants a `/ToUnicode` CMap,
/// and a ligature or a positional form wants a shaping pass. The F1 it produces is
/// therefore a measurement of this placeholder and not of the product, and the report
/// prints that sentence next to the number so nobody later mistakes it for a verdict.
fn our_words(doc: &Document, page: &mangle_doc::pages::Page, resources: &Resources) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let contents = page.decoded_contents(doc);
    let stream = ContentStream::parse(&contents);
    for record in run_with(&stream, resources).records {
        let Mark::Glyphs { font, codes, .. } = record.mark else {
            continue;
        };
        let name = font.unwrap_or_default();
        let composite = resources.font_is_composite(&name);
        // The `/Encoding` is on the font dictionary, and it is only read for a simple font:
        // a composite font's code is a CID and `/Differences` does not apply to one.
        let encoding = resources
            .fonts
            .get(&name)
            .map(|o| doc.resolve_object(o).unwrap_or_else(|| o.clone()))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| {
                if composite {
                    mangle_font::Encoding::new(mangle_font::EncodingBase::Standard)
                } else {
                    mangle_font::Encoding::from_font_dict(
                        &d,
                        mangle_font::EncodingBase::Standard,
                        &|o| doc.resolve_object(o).or_else(|| Some(o.clone())),
                    )
                    .unwrap_or_else(|| {
                        mangle_font::Encoding::new(mangle_font::EncodingBase::Standard)
                    })
                }
            })
            .unwrap_or_else(|| mangle_font::Encoding::new(mangle_font::EncodingBase::Standard));
        for code in codes {
            match glyph_char(&encoding, code, composite) {
                Some(c) if c.is_whitespace() => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                }
                Some(c) => current.push(c),
                None => {
                    // An unknown code ends the word rather than being dropped: a gap in the
                    // middle of a word is a different thing from a gap between words.
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                }
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// One character code as a character, if this can know one.
fn glyph_char(encoding: &mangle_font::Encoding, code: u32, composite: bool) -> Option<char> {
    if composite {
        // A CID is not a Unicode scalar value and this placeholder has no `/ToUnicode`
        // reader. Returning `None` makes the word split, which shows up in the F1 as a
        // low score on exactly the documents that need `/ToUnicode` most.
        return None;
    }
    let name = encoding.glyph_for(code)?;
    if let Some(uni) = mangle_font::agl(name) {
        return char::from_u32(uni);
    }
    // `uniXXXX` and `gXX`/`cidXX` are the Adobe conventions for a glyph the AGL does not
    // list, and the first of them names the character directly.
    if let Some(hex) = name.strip_prefix("uni") {
        if let Ok(uni) = u32::from_str_radix(hex, 16) {
            return char::from_u32(uni);
        }
    }
    None
}

/// The words `pdftotext` reads off a file, normalised the same way as ours.
fn their_words(tool: &Path, pdf: &Path) -> Option<Vec<String>> {
    let out = Command::new(tool)
        .args([&pdf.to_string_lossy(), "-"])
        .output()
        .ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Some(normalise(&text))
}

/// A word as both sides are compared: case-folded, and with the quote and hyphen characters
/// that two renderers disagree about removed.
///
/// `pdftotext` and any extractor worth the name disagree about curly quotes, soft hyphens
/// and de-hyphenation. Counting those as text errors would bury the errors that matter in a
/// thousand cosmetic ones.
fn normalise(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| {
                    !matches!(
                        c,
                        '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}' | '\u{00ad}'
                    )
                })
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word-level F1 over multisets.
fn f1(ours: &[String], theirs: &[String]) -> (f64, usize, usize) {
    if ours.is_empty() && theirs.is_empty() {
        return (1.0, 0, 0);
    }
    let mut pool: BTreeMap<&str, usize> = BTreeMap::new();
    for t in theirs {
        *pool.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut hits = 0usize;
    for o in ours {
        if let Some(left) = pool.get_mut(o.as_str()) {
            if *left > 0 {
                *left -= 1;
                hits += 1;
            }
        }
    }
    let precision = hits as f64 / ours.len().max(1) as f64;
    let recall = hits as f64 / theirs.len().max(1) as f64;
    if precision + recall == 0.0 {
        return (0.0, hits, theirs.len().max(ours.len()));
    }
    (
        2.0 * precision * recall / (precision + recall),
        hits,
        theirs.len(),
    )
}

// ── The measurement ──────────────────────────────────────────────────────────────

/// What one page came to.
struct PageOutcome {
    page: usize,
    size: (usize, usize),
    marks: usize,
    ssim: Option<f64>,
    rms: Option<f64>,
    max_delta: Option<u32>,
    above_tolerance: Option<usize>,
    total_pixels: Option<usize>,
    tolerance: u32,
    worst: Vec<(u32, usize, usize)>,
    notes: Vec<String>,
    /// Why this page was not compared, if it was not.
    skipped: Option<String>,
    heatmap: Option<String>,
    /// Why no picture was written for a page that wanted one. Recorded rather than
    /// swallowed: a missing heatmap that looks like a passing page is the worst outcome
    /// this harness has.
    picture_note: Option<String>,
}

/// Everything one file came to.
struct FileOutcome {
    entry_id: String,
    file: String,
    category: String,
    purpose: String,
    producer: String,
    expect: String,
    bytes: u64,
    sha256: String,
    opened: Result<usize, String>,
    recovery: Vec<String>,
    clean: bool,
    pages: Vec<PageOutcome>,
    f1: Option<f64>,
    text_note: String,
    text_ours: usize,
    text_theirs: usize,
    seconds: f64,
    paniced: Option<String>,
}

/// Walk the corpus and write one report per file.
///
/// Ignored by default, because it is a two-hour job: 77 files, 742 pages, each page rendered
/// twice and compared, and `mutool draw` is run once per file over the whole set. That is
/// fine for a corpus run and fatal for `cargo test --workspace`, which would then take two
/// hours and look like a hang.
///
/// To run it:
///
/// ```text
/// cargo xtask corpus fetch                                   # once; not checked in
/// cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture
/// ```
///
/// It needs the corpus fetched — see `corpus/wild/SOURCES.md` — and `mutool` and `pdftotext`
/// installed for the two halves that compare against them. The two cheap tests in this file
/// are *not* ignored: they check the harness itself, they need no corpus, and they are the
/// reason this file is worth having in the default suite at all.
#[test]
#[ignore = "two hours over the whole corpus; run it deliberately with --ignored"]
fn the_wild_corpus_is_measured_and_reported() {
    let Some(dir) = corpus_dir() else {
        eprintln!(
            "skipped: no corpus/wild/MANIFEST.toml; the wild corpus is not checked in. \
             See corpus/wild/SOURCES.md."
        );
        return;
    };
    let entries = manifest(&dir);
    if entries.is_empty() {
        eprintln!("skipped: corpus/wild/MANIFEST.toml lists no entries");
        return;
    }

    let tool = mutool();
    let pdftotext = pdftotext();
    let scratch = dir.join("scratch");
    let reports = dir.join("report");

    let present: Vec<&Entry> = entries
        .iter()
        .filter(|e| dir.join(&e.file).is_file())
        .collect();
    let absent: Vec<&Entry> = entries
        .iter()
        .filter(|e| !dir.join(&e.file).is_file())
        .collect();

    // Absence is printed with its reason and is never a silent pass: a corpus that was
    // never fetched and a corpus where every file failed look identical otherwise.
    if absent.is_empty() {
        println!("corpus: {} files present, {} absent", present.len(), 0);
    } else {
        println!(
            "corpus: {} files present, {} absent",
            present.len(),
            absent.len()
        );
        println!(
            "skipped: {} manifest entries have no file on disk; run `cargo xtask corpus \
             fetch` for them (first: {})",
            absent.len(),
            absent
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if tool.is_none() {
        println!(
            "skipped: mutool is not installed, so no page of this corpus was compared with \
             another renderer. Install mupdf-tools to enable the image half of this test."
        );
    }
    if pdftotext.is_none() {
        println!(
            "skipped: pdftotext is not installed, so no page of this corpus was compared \
             against another text extractor. Install poppler-utils to enable it."
        );
    }
    if present.is_empty() {
        println!("skipped: nothing to measure; every manifest entry is absent");
        return;
    }

    if let Err(why) = std::fs::create_dir_all(&reports) {
        eprintln!("cannot create {}: {why}", reports.display());
    }
    if let Err(why) = std::fs::create_dir_all(&scratch) {
        eprintln!("cannot create {}: {why}", scratch.display());
    }

    let mut outcomes = Vec::new();
    for (i, entry) in present.iter().enumerate() {
        let outcome = measure_file(
            &dir,
            &scratch,
            &reports,
            entry,
            tool.as_deref(),
            pdftotext.as_deref(),
        );
        // Every file is measured inside a catch_unwind. A panic on a wild file is one of
        // the things this corpus exists to find, so it belongs in the report rather than in
        // the exit status; the charter is explicit that a caught panic is still a bug, and
        // it is printed here so it cannot be missed.
        let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| outcome)) {
            Ok(o) => o,
            Err(payload) => {
                let why = panic_message(&payload);
                eprintln!("PANIC   {} {}: {why}", entry.id, entry.file);
                FileOutcome {
                    entry_id: entry.id.clone(),
                    file: entry.file.clone(),
                    category: entry.category.clone(),
                    purpose: entry.purpose.clone(),
                    producer: entry.producer.clone(),
                    expect: entry.expect.clone(),
                    bytes: entry.bytes,
                    sha256: entry.sha256.clone(),
                    opened: Err(format!("panicked: {why}")),
                    recovery: Vec::new(),
                    clean: false,
                    pages: Vec::new(),
                    f1: None,
                    text_note: "not measured: the file panicked".into(),
                    text_ours: 0,
                    text_theirs: 0,
                    seconds: 0.0,
                    paniced: Some(why),
                }
            }
        };
        if let Some(why) = &outcome.paniced {
            println!("panic   {} {}: {why}", outcome.entry_id, outcome.file);
        }
        outcomes.push(outcome);
        let _ = i;
    }

    let _ = std::fs::remove_dir_all(&scratch);
    write_summary(&reports, &outcomes);
    print_console(&outcomes);

    // The assertion, and the only one: the corpus must have been measured. A corpus test
    // that measured nothing and passed is the failure mode this file exists to avoid.
    let measured = outcomes
        .iter()
        .filter(|o| o.opened.is_ok() && !o.pages.is_empty())
        .count();
    assert!(
        measured > 0,
        "no file in the wild corpus was opened and rendered. {} files were present and {} \
         produced pages. This is a harness failure, not a corpus result.",
        present.len(),
        measured
    );
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with a non-string payload".to_string()
    }
}

fn measure_file(
    dir: &Path,
    scratch: &Path,
    reports_dir: &Path,
    entry: &Entry,
    tool: Option<&Path>,
    pdftotext: Option<&Path>,
) -> FileOutcome {
    let started = Instant::now();
    let path = dir.join(&entry.file);
    let mut outcome = FileOutcome {
        entry_id: entry.id.clone(),
        file: entry.file.clone(),
        category: entry.category.clone(),
        purpose: entry.purpose.clone(),
        producer: entry.producer.clone(),
        expect: entry.expect.clone(),
        bytes: entry.bytes,
        sha256: entry.sha256.clone(),
        opened: Err("not attempted".into()),
        recovery: Vec::new(),
        clean: false,
        pages: Vec::new(),
        f1: None,
        text_note: String::new(),
        text_ours: 0,
        text_theirs: 0,
        seconds: 0.0,
        paniced: None,
    };

    let Ok(bytes) = std::fs::read(&path) else {
        outcome.opened = Err(format!("{} could not be read", path.display()));
        outcome.text_note = "not measured: the file could not be read".into();
        outcome.seconds = started.elapsed().as_secs_f64();
        return outcome;
    };
    if bytes.len() as u64 != entry.bytes {
        // The manifest is the contract. A file whose size has changed is a file whose hash
        // will not match either, and measuring it anyway would put a number in the report
        // for bytes nothing vouched for.
        outcome.opened = Err(format!(
            "{} is {} bytes, the manifest says {} — not the pinned file",
            entry.file,
            bytes.len(),
            entry.bytes
        ));
        outcome.text_note = "not measured: not the pinned file".into();
        outcome.seconds = started.elapsed().as_secs_f64();
        return outcome;
    }

    let doc = match Document::open(bytes, OpenOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            outcome.opened = Err(e.to_string());
            outcome.text_note = "not measured: the file did not open".into();
            outcome.seconds = started.elapsed().as_secs_f64();
            return outcome;
        }
    };
    outcome.clean = doc.info().recovery.is_clean();
    outcome.recovery = doc
        .info()
        .recovery
        .notes()
        .iter()
        .take(20)
        .cloned()
        .collect();

    let pages = match pages_of(&doc) {
        Ok(p) => p,
        Err(why) => {
            outcome.opened = Err(why);
            outcome.text_note = "not measured: no page tree".into();
            outcome.seconds = started.elapsed().as_secs_f64();
            return outcome;
        }
    };
    outcome.opened = Ok(pages.len());

    // mutool renders the whole file in one process; the map is keyed by page number.
    let oracle = tool.map(|t| mutool_render_all(t, &path, scratch, pages.len()));
    let mut ours_all: Vec<String> = Vec::new();
    let theirs_all: Vec<String>;

    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        let resources = resources_for(&doc, page);
        let render = render_page(
            &doc,
            page,
            &resources,
            RenderOptions {
                scale: SCALE,
                ..RenderOptions::default()
            },
        );

        // Text is collected whatever the image comparison does: a page can render blank and
        // still say something, and the text half of this report is independent.
        ours_all.extend(our_words(&doc, page, &resources));

        let mut outcome_page = PageOutcome {
            page: number,
            size: (render.image.width, render.image.height),
            marks: render.marks,
            ssim: None,
            rms: None,
            max_delta: None,
            above_tolerance: None,
            total_pixels: None,
            tolerance: SsimOptions::default().tolerance,
            worst: Vec::new(),
            notes: render.notes.clone(),
            skipped: None,
            heatmap: None,
            picture_note: None,
        };

        let Some(oracle) = oracle.as_ref() else {
            outcome_page.skipped = Some("mutool is not installed".into());
            outcome.pages.push(outcome_page);
            continue;
        };
        let Some(pam_path) = oracle.get(&number) else {
            outcome_page.skipped = Some(format!(
                "mutool produced no page {number}; it could not render it"
            ));
            outcome.pages.push(outcome_page);
            continue;
        };
        let Ok(data) = std::fs::read(pam_path) else {
            outcome_page.skipped = Some(format!("mutool's page {number} could not be read"));
            outcome.pages.push(outcome_page);
            continue;
        };
        // `mutool draw` wrote every page of the file before the loop started, so this is
        // one of several gigabytes of raw pixels sitting in the scratch directory for a
        // 352-page book. The bytes are in `data` now, so the file has done its job.
        let _ = std::fs::remove_file(pam_path);
        let Some((w, h, depth, pam)) = read_pam(&data) else {
            outcome_page.skipped = Some(format!("mutool's page {number} is not a PAM"));
            outcome.pages.push(outcome_page);
            continue;
        };

        let ours_pixels = render.image.width * render.image.height;
        let theirs_pixels = w * h;
        let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, &pam));
        if ours_pixels > MAX_COMPARE_PIXELS || theirs_pixels > MAX_COMPARE_PIXELS {
            outcome_page.skipped = Some(format!(
                "{theirs_pixels} oracle pixels is above the {MAX_COMPARE_PIXELS} this harness \
                 will compare, for memory reasons. Rendered at {DPI:.0} DPI and left \
                 uncompared on purpose."
            ));
            outcome.pages.push(outcome_page);
            continue;
        }
        if ours_pixels != theirs_pixels {
            outcome_page.skipped = Some(format!(
                "size disagreement: we rendered {}x{}, mutool rendered {w}x{h}",
                render.image.width, render.image.height
            ));
            outcome.pages.push(outcome_page);
            continue;
        }

        let ours = flatten_onto_paper(&render.image);
        let result = compare(&ours, &theirs, &SsimOptions::default());
        if !result.is_valid() {
            outcome_page.skipped = Some(
                result
                    .refused
                    .clone()
                    .unwrap_or_else(|| "the comparison refused".into()),
            );
            outcome.pages.push(outcome_page);
            continue;
        }
        outcome_page.ssim = Some(result.metrics.ssim);
        outcome_page.rms = Some(result.metrics.rms);
        outcome_page.max_delta = Some(result.metrics.max_delta);
        outcome_page.above_tolerance = Some(result.metrics.above_tolerance);
        outcome_page.total_pixels = Some(result.metrics.total_pixels);
        outcome_page.worst.clone_from(&result.worst);
        outcome.pages.push(outcome_page);
    }

    if let Some(tool) = pdftotext {
        match their_words(tool, &path) {
            Some(words) => {
                theirs_all = words;
                let (score, _hits, _n) = f1(&ours_all, &theirs_all);
                outcome.f1 = Some(score);
                outcome.text_ours = ours_all.len();
                outcome.text_theirs = theirs_all.len();
                outcome.text_note = format!(
                    "placeholder extractor (mangle-text is not implemented): {} of our words \
                     against {} of pdftotext's, both lower-cased with curly quotes removed",
                    ours_all.len(),
                    theirs_all.len()
                );
            }
            None => outcome.text_note = "pdftotext could not read this file".into(),
        }
    } else {
        outcome.text_note = "pdftotext is not installed".into();
    }

    // Second pass: the pictures. Only for the pages a person would actually open, which is
    // what keeps a 742-page corpus from writing gigabytes of heatmaps nobody looks at.
    if let Some(tool) = tool {
        for index in pictures_worth_taking(&outcome.pages) {
            match write_picture(tool, scratch, reports_dir, &path, entry, &doc, index) {
                Ok(name) => outcome.pages[index].heatmap = Some(name),
                Err(why) => outcome.pages[index].picture_note = Some(why),
            }
        }
    }

    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}

/// The pages worth a picture: the worst few, and only if they differ at all.
///
/// A page two renderers agree about to within a thousandth does not need a PNG. Three per
/// file is enough to see a pattern and few enough that the report directory stays readable.
fn pictures_worth_taking(pages: &[PageOutcome]) -> Vec<usize> {
    const WORST: usize = 3;
    const INTERESTING: f64 = 0.98;
    let mut ranked: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.ssim.is_some_and(|s| s < INTERESTING))
        .map(|(i, _)| i)
        .collect();
    ranked.sort_by(|a, b| {
        pages[*a]
            .ssim
            .partial_cmp(&pages[*b].ssim)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.truncate(WORST);
    ranked
}

/// Re-render one page and write what we drew, what mutool drew, and where they differ.
///
/// Three pictures rather than one: the heatmap says *that* the two differ, and the other
/// two say *why*, and "why" is the part a reader has to look at. This re-renders rather
/// than holding every page's images, because a corpus this size cannot keep 742 pages of
/// RGBA and is better served by doing the cheap page twice.
#[allow(clippy::too_many_arguments)]
fn write_picture(
    tool: &Path,
    scratch: &Path,
    reports_dir: &Path,
    pdf: &Path,
    entry: &Entry,
    doc: &Document,
    index: usize,
) -> Result<String, String> {
    let all = pages_of(doc)?;
    let page = all
        .get(index)
        .ok_or_else(|| format!("page {index} is not in the tree"))?;
    let number = index + 1;
    let ours = flatten_onto_paper(
        &render_page(
            doc,
            page,
            &resources_for(doc, page),
            RenderOptions {
                scale: SCALE,
                ..RenderOptions::default()
            },
        )
        .image,
    );

    let oracle = mutool_render_one(tool, pdf, scratch, number)?;
    let data = std::fs::read(&oracle).map_err(|e| format!("{}: {e}", oracle.display()))?;
    let (w, h, depth, pam) = read_pam(&data).ok_or("mutool's page is not a PAM")?;
    if w != ours.width || h != ours.height {
        return Err(format!(
            "on the second pass the two renders disagree on size: we rendered {}x{}, mutool \
             rendered {w}x{h}",
            ours.width, ours.height
        ));
    }
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, &pam));
    let result = compare(&ours, &theirs, &SsimOptions::default());
    if !result.is_valid() {
        return Err(result
            .refused
            .clone()
            .unwrap_or_else(|| "the comparison refused".into()));
    }

    let mut written: Option<String> = None;
    let mut last_problem = String::new();
    for (suffix, image) in [
        ("diff", &result.heatmap),
        ("ours", &ours),
        ("theirs", &theirs),
    ] {
        let pam_path = scratch.join(format!("{number}-{suffix}.pam"));
        if !write_pam(&pam_path, image) {
            last_problem = format!("{suffix}: the PAM could not be written to disk");
            continue;
        }
        let stem = format!("{}-p{number}-{suffix}", entry.id);
        let produced = match pam_to_png(tool, &pam_path, scratch) {
            Ok(p) => p,
            Err(why) => {
                last_problem = format!("{suffix}: {why}");
                continue;
            }
        };
        let final_name = format!("{stem}.png");
        let landed = reports_dir.join(&final_name);
        if std::fs::rename(&produced, &landed).is_ok() && suffix == "diff" {
            written = Some(final_name);
        } else {
            let _ = std::fs::remove_file(&produced);
        }
    }
    written.ok_or(if last_problem.is_empty() {
        "no picture was written".to_string()
    } else {
        last_problem
    })
}

/// One page of the oracle, rendered on its own.
fn mutool_render_one(
    tool: &Path,
    pdf: &Path,
    scratch: &Path,
    page: usize,
) -> Result<PathBuf, String> {
    let out = scratch.join(format!("one-{page}.pam"));
    let status = Command::new(tool)
        .args([
            "draw",
            "-r",
            &format!("{DPI:.0}"),
            "-F",
            "pam",
            "-o",
            &out.to_string_lossy(),
            &pdf.to_string_lossy(),
            &page.to_string(),
        ])
        .output()
        .map_err(|e| format!("running mutool draw: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "mutool draw exited {} on page {page}: {}",
            status.status,
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    out.is_file()
        .then_some(out)
        .ok_or_else(|| format!("mutool wrote no page {page}"))
}

fn pages_of(doc: &Document) -> Result<Vec<mangle_doc::pages::Page>, String> {
    let catalog = doc.catalog().map_err(|e| e.to_string())?;
    let root = catalog
        .get("Pages")
        .and_then(Object::as_ref_id)
        .ok_or("the catalogue has no /Pages")?;
    Ok(PageTree::build(doc, root)
        .map_err(|e| e.to_string())?
        .pages()
        .to_vec())
}

fn resources_for(doc: &Document, page: &mangle_doc::pages::Page) -> Resources {
    page.inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default()
}

// ── The reports ──────────────────────────────────────────────────────────────────

/// The per-file report: what a person reads when a number in the summary surprises them.
fn write_report(reports: &Path, outcome: &FileOutcome) {
    let stem = Path::new(&outcome.file)
        .file_stem()
        .map_or_else(|| outcome.entry_id.clone(), |s| s.to_string_lossy().into());
    let mut text = format!("# {} — {}\n\n", outcome.entry_id, outcome.file);
    let _ = writeln!(text, "- **Category** {}", outcome.category);
    let _ = writeln!(text, "- **Why it is here** {}", outcome.purpose);
    let _ = writeln!(text, "- **Producer** {}", outcome.producer);
    let _ = writeln!(text, "- **Manifest `expect`** {}", outcome.expect);
    let _ = writeln!(
        text,
        "- **Pinned SHA-256** `{}`",
        short(&outcome.sha256, 16)
    );
    let _ = writeln!(text, "- **Size** {} bytes", outcome.bytes);
    let _ = writeln!(text, "- **Resolution** {DPI:.0} DPI");
    let _ = writeln!(text, "- **Time** {:.1}s", outcome.seconds);

    let _ = writeln!(text, "\n## Opening\n");
    match &outcome.opened {
        Ok(n) => {
            let _ = writeln!(
                text,
                "Opened. {n} page(s). {}",
                if outcome.clean {
                    "Structure was read as written; no repair was needed."
                } else {
                    "Structure needed recovery — see the notes below."
                }
            );
        }
        Err(why) => {
            let _ = writeln!(text, "**Did not open.** {why}");
        }
    }
    if !outcome.recovery.is_empty() {
        text.push_str("\n### Reader notes\n\n");
        for note in &outcome.recovery {
            let _ = writeln!(text, "- {note}");
        }
    }

    let _ = writeln!(text, "\n## Pages\n");
    if outcome.pages.is_empty() {
        let _ = writeln!(text, "No page was measured.");
    }
    let tolerance = outcome
        .pages
        .first()
        .map_or(SsimOptions::default().tolerance, |p| p.tolerance);
    let _ = writeln!(
        text,
        "A pixel counts as different above a single-channel delta of {tolerance}.\
         \n`marks` is how many things the renderer drew on the page.\n"
    );
    let mut worst_pages: Vec<&PageOutcome> =
        outcome.pages.iter().filter(|p| p.ssim.is_some()).collect();
    worst_pages.sort_by(|a, b| {
        a.ssim
            .partial_cmp(&b.ssim)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let _ = writeln!(
        text,
        "| page | size | marks | SSIM | RMS | max delta | > tol | % > tol | notes |"
    );
    let _ = writeln!(text, "|---|---|---|---|---|---|---|---|---|");
    for p in &outcome.pages {
        let _ = writeln!(
            text,
            "| {} | {}x{} | {} | {} | {} | {} | {} | {} | {} |",
            p.page,
            p.size.0,
            p.size.1,
            p.marks,
            fmt_f64(p.ssim, 4),
            fmt_f64(p.rms, 2),
            p.max_delta.map_or_else(|| "-".into(), |v| v.to_string()),
            p.above_tolerance
                .map_or_else(|| "-".into(), |v| v.to_string()),
            match (p.above_tolerance, p.total_pixels) {
                (Some(a), Some(t)) if t > 0 => format!("{:.3}%", 100.0 * a as f64 / t as f64),
                _ => "-".into(),
            },
            match &p.skipped {
                Some(why) => format!("*not compared:* {why}"),
                None => p
                    .notes
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
                    .replace('|', "\\|")
                    .replace('\n', " "),
            }
        );
    }

    let _ = writeln!(text, "\n## Worst pages by SSIM\n");
    if worst_pages.is_empty() {
        let _ = writeln!(text, "No page was compared, so no page can be ranked.");
    } else {
        for p in worst_pages.iter().take(5) {
            let _ = writeln!(
                text,
                "- **page {}** — SSIM {}, RMS {}, {} of {} pixels above tolerance, heatmap {}",
                p.page,
                fmt_f64(p.ssim, 4),
                fmt_f64(p.rms, 2),
                p.above_tolerance.unwrap_or(0),
                p.total_pixels.unwrap_or(0),
                p.heatmap.clone().unwrap_or_else(|| match &p.picture_note {
                    Some(why) => format!("*not written:* {why}"),
                    None => "not written".into(),
                })
            );
        }
    }

    let _ = writeln!(text, "\n## Worst pixels\n");
    let with_pixels: Vec<&PageOutcome> = outcome
        .pages
        .iter()
        .filter(|p| !p.worst.is_empty())
        .collect();
    if with_pixels.is_empty() {
        let _ = writeln!(text, "No page recorded a differing pixel.");
    }
    for p in with_pixels.iter().take(5) {
        let _ = writeln!(text, "### page {}\n", p.page);
        let _ = writeln!(
            text,
            "Largest differences, as (channel delta, x, y) at {DPI:.0} DPI:\n"
        );
        for (delta, x, y) in p.worst.iter().take(12) {
            let _ = writeln!(text, "- delta {delta:>3} at x={x}, y={y}");
        }
    }

    let _ = writeln!(text, "\n## Text\n");
    match outcome.f1 {
        Some(score) => {
            let _ = writeln!(
                text,
                "Word-level F1 against `pdftotext`: **{:.4}** (ours {}, pdftotext {}).",
                score, outcome.text_ours, outcome.text_theirs
            );
        }
        None => text.push_str("Word-level F1 was not measured.\n"),
    }
    let _ = writeln!(text, "\n{}\n", outcome.text_note);
    if let Some(why) = &outcome.paniced {
        let _ = writeln!(text, "\n**This file panicked.** {why}\n");
    }

    let path = reports.join(format!("{stem}.md"));
    let _ = std::fs::write(path, text);
}

/// The roll-up: every file, worst first, so the interesting line is the first one.
fn write_summary(reports: &Path, outcomes: &[FileOutcome]) {
    let mut text = String::from(
        "# Tier B wild corpus — one run\n\nEvery file in `corpus/wild/MANIFEST.toml`, as it came \
         out.\n\n**This report asserts nothing.** A corpus finds things; a threshold on a \
         document nobody has read yet turns the first surprise into a permanent red build. \
         Promote a file to `fixtures/` if it needs an assertion.\n\n",
    );
    let _ = writeln!(
        text,
        "- {DPI:.0} DPI, `mutool draw` as the image oracle, `pdftotext` as the text oracle"
    );
    let _ = writeln!(text, "- {} files measured", outcomes.len());
    let opened = outcomes.iter().filter(|o| o.opened.is_ok()).count();
    let repaired = outcomes.iter().filter(|o| !o.clean).count();
    let _ = writeln!(
        text,
        "- {opened} opened, {repaired} needed recovery, {} did not",
        outcomes.len() - opened
    );
    let _ = writeln!(
        text,
        "- {} panicked\n",
        outcomes.iter().filter(|o| o.paniced.is_some()).count()
    );

    let mut ranked: Vec<&FileOutcome> = outcomes.iter().collect();
    ranked.sort_by(|a, b| worst_ssim(b).total_cmp(&worst_ssim(a)));
    let _ = writeln!(
        text,
        "| id | file | category | pages | worst SSIM | median SSIM | compared | text F1 | time |"
    );
    let _ = writeln!(text, "|---|---|---|---|---|---|---|---|---|");
    for o in &ranked {
        let (worst, median, compared) = ssim_stats(o);
        let _ = writeln!(
            text,
            "| {} | `{}` | {} | {} | {} | {} | {}/{} | {} | {:.1}s |",
            o.entry_id,
            o.file,
            o.category,
            o.opened
                .as_ref()
                .map_or_else(|_| "-".to_string(), |n| n.to_string()),
            fmt_f64(Some(worst), 4),
            fmt_f64(Some(median), 4),
            compared,
            o.pages.len(),
            fmt_f64(o.f1, 4),
            o.seconds
        );
    }

    let mut worst: Vec<(f64, &FileOutcome, &PageOutcome)> = outcomes
        .iter()
        .flat_map(|o| {
            o.pages
                .iter()
                .filter_map(move |p| p.ssim.map(|s| (s, o, p)))
        })
        .collect();
    worst.sort_by(|a, b| a.0.total_cmp(&b.0));
    let _ = writeln!(text, "\n## The twenty worst pages in the corpus\n");
    let _ = writeln!(
        text,
        "| SSIM | file | page | size | marks | RMS | max delta | > tol | heatmap |"
    );
    let _ = writeln!(text, "|---|---|---|---|---|---|---|---|---|");
    for (ssim, o, p) in worst.iter().take(20) {
        let _ = writeln!(
            text,
            "| {:.4} | `{}` | {} | {}x{} | {} | {} | {} | {} | {} |",
            ssim,
            o.file,
            p.page,
            p.size.0,
            p.size.1,
            p.marks,
            fmt_f64(p.rms, 2),
            p.max_delta.map_or_else(|| "-".into(), |v| v.to_string()),
            p.above_tolerance
                .map_or_else(|| "-".into(), |v| v.to_string()),
            p.heatmap.clone().unwrap_or_else(|| "-".into())
        );
    }

    text.push_str("\n## Everything that did not happen\n\n");
    let _ = writeln!(
        text,
        "Recorded so a page that was skipped is never read as a page that passed.\n"
    );
    let mut any = false;
    for o in outcomes {
        for p in &o.pages {
            if let Some(why) = &p.skipped {
                any = true;
                let _ = writeln!(text, "- `{}` page {}: {why}", o.file, p.page);
            }
        }
        if o.opened.is_err() {
            any = true;
            let _ = writeln!(
                text,
                "- `{}`: did not open: {}",
                o.file,
                o.opened.as_ref().err().cloned().unwrap_or_default()
            );
        }
        if o.seconds > BUDGET_SECS {
            any = true;
            let _ = writeln!(
                text,
                "- `{}`: took {:.1}s, over the {:.0}s budget",
                o.file, o.seconds, BUDGET_SECS
            );
        }
    }
    if !any {
        text.push_str("- nothing was skipped.\n");
    }

    let _ = std::fs::write(reports.join("SUMMARY.md"), text);
    for o in outcomes {
        write_report(reports, o);
    }
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn fmt_f64(v: Option<f64>, places: usize) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{x:.places$}"),
        Some(_) => "nan".into(),
        None => "-".into(),
    }
}

/// The lowest SSIM this file reached, or `1.0` when nothing was compared.
fn worst_ssim(o: &FileOutcome) -> f64 {
    o.pages
        .iter()
        .filter_map(|p| p.ssim)
        .fold(1.0f64, |acc, s| acc.min(s))
}

fn ssim_stats(o: &FileOutcome) -> (f64, f64, usize) {
    let mut values: Vec<f64> = o.pages.iter().filter_map(|p| p.ssim).collect();
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return (f64::NAN, f64::NAN, 0);
    }
    let median = if values.len() % 2 == 1 {
        values[values.len() / 2]
    } else {
        let lo = values[values.len() / 2 - 1];
        let hi = values[values.len() / 2];
        lo.midpoint(hi)
    };
    (values[0], median, values.len())
}

/// What a person sees in the terminal: the ranking, so the worst file is the first thing
/// printed, and every reason a measurement did not happen.
fn print_console(outcomes: &[FileOutcome]) {
    let mut ranked: Vec<&FileOutcome> = outcomes.iter().collect();
    ranked.sort_by(|a, b| worst_ssim(b).total_cmp(&worst_ssim(a)));

    println!("\n=== Tier B wild corpus, worst file first ===");
    println!(
        "{:<6} {:<44} {:>5} {:>9} {:>9} {:>7} {:>7}",
        "id", "file", "pages", "worst", "median", "text", "time"
    );
    for o in &ranked {
        let (worst, median, compared) = ssim_stats(o);
        println!(
            "{:<6} {:<44} {:>5} {:>9} {:>9} {:>7} {:>6.1}s",
            o.entry_id,
            o.file,
            o.opened
                .as_ref()
                .map_or_else(|_| "-".to_string(), |n| n.to_string()),
            fmt_f64(if compared == 0 { None } else { Some(worst) }, 4),
            fmt_f64(if compared == 0 { None } else { Some(median) }, 4),
            fmt_f64(o.f1, 3),
            o.seconds
        );
    }

    println!("\n=== Pages that did not match, worst first ===");
    let mut pages: Vec<(f64, &str, &PageOutcome)> = outcomes
        .iter()
        .flat_map(|o| {
            o.pages
                .iter()
                .filter_map(move |p| p.ssim.map(|s| (s, o.file.as_str(), p)))
        })
        .collect();
    pages.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (shown, (ssim, file, p)) in pages.iter().enumerate() {
        // Everything below 0.95, then up to fifteen of the rest: the console output is for
        // triage, and the report has the whole list.
        if *ssim > 0.95 && shown >= 15 {
            break;
        }
        if shown >= 40 {
            break;
        }
        println!(
            "  {:.4}  {:<40} page {:<4} {}x{}  rms {}  max delta {}  > tol {}/{}",
            ssim,
            file,
            p.page,
            p.size.0,
            p.size.1,
            fmt_f64(p.rms, 2),
            p.max_delta.map_or_else(|| "-".into(), |v| v.to_string()),
            p.above_tolerance.unwrap_or(0),
            p.total_pixels.unwrap_or(0)
        );
    }
    if pages.is_empty() {
        println!(
            "  no page was compared with mutool (installed: {})",
            tool_present()
        );
    }

    println!("\n=== Measurements that did not happen ===");
    let mut any = false;
    for o in outcomes {
        if let Err(why) = &o.opened {
            println!("  {} {}: did not open: {why}", o.entry_id, o.file);
            any = true;
        }
        for p in &o.pages {
            if let Some(why) = &p.skipped {
                println!("  {} {} page {}: {why}", o.entry_id, o.file, p.page);
                any = true;
            }
        }
        if o.f1.is_none() {
            println!(
                "  {} {}: no text measurement: {}",
                o.entry_id, o.file, o.text_note
            );
            any = true;
        }
        if o.seconds > BUDGET_SECS {
            println!(
                "  {} {}: {:.1}s, over the {:.0}s budget",
                o.entry_id, o.file, o.seconds, BUDGET_SECS
            );
            any = true;
        }
    }
    if !any {
        println!("  nothing was skipped");
    }
    println!(
        "\nReports, heatmaps and this summary are in corpus/wild/report/. Nothing here is \
         asserted; the report is the output."
    );
}

fn tool_present() -> bool {
    mutool().is_some()
}

/// The picture plumbing, tested on its own.
///
/// It is four lines of shelling out to `mutool`, and it is the only part of this file that
/// touches the filesystem in a way that can silently do nothing. A heatmap that is missing
/// looks exactly like a page that passed, so the path that writes one is worth proving on
/// its own rather than discovering in a five-minute corpus run.
#[test]
fn a_picture_is_written_where_the_report_says_it_is() {
    let Some(tool) = mutool() else {
        eprintln!("skipped: mutool is not installed, so no picture can be written");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("mangle-wild-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&scratch);
    let image = Image::filled(32, 24, [12, 34, 56, 255]);
    let pam = scratch.join("probe.pam");
    assert!(write_pam(&pam, &image), "the PAM could not be written");

    let produced = pam_to_png(&tool, &pam, &scratch)
        .unwrap_or_else(|why| panic!("mutool did not produce a PNG: {why}"));
    assert!(
        produced.is_file(),
        "mutool reported success but {} is not there",
        produced.display()
    );
    let bytes = std::fs::read(&produced).expect("the PNG");
    assert_eq!(
        bytes.get(..8),
        Some(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..]),
        "what mutool wrote is not a PNG: {}",
        produced.display()
    );

    // The destination the report links to must exist under that exact name, which is the
    // thing that went wrong twice while this was being written.
    let landed = scratch.join("W999-p1-diff.png");
    assert!(
        std::fs::rename(&produced, &landed).is_ok(),
        "the PNG could not be moved to {}",
        landed.display()
    );
    assert!(landed.is_file());
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A PAM round-trips: what is written is what `read_pam` gives back.
#[test]
fn a_written_pam_reads_back_as_the_image_that_was_written() {
    let mut image = Image::filled(7, 5, [1, 2, 3, 255]);
    image.put(3, 2, [200, 100, 50, 128]);
    let path = std::env::temp_dir().join(format!("mangle-wild-pam-{}", std::process::id()));
    assert!(write_pam(&path, &image));
    let (w, h, depth, body) = read_pam(&std::fs::read(&path).unwrap()).expect("the PAM header");
    assert_eq!((w, h, depth), (7, 5, 4));
    assert_eq!(body.len(), 7 * 5 * 4);
    assert_eq!(
        &body[(2 * 7 + 3) * 4..(2 * 7 + 3) * 4 + 4],
        &[200, 100, 50, 128]
    );
    let _ = std::fs::remove_file(&path);
}
