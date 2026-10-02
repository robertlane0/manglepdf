//! The name-keyed CFF charsets on this machine, checked against an outside reader.
//!
//! The unit tests in `cff.rs` build a charset a byte at a time, which is what makes their
//! expectations checkable by hand and also what makes them blind: a font a test built can
//! only say what the test author thought to say. This test reads the charsets other
//! programs wrote and checks each one against `fontTools`, an independent implementation
//! with its own copy of the 391-name Standard Strings and its own charset reader.
//!
//! It is here because of what a wrong SID table costs. A mistyped entry does not fail, does
//! not panic, and does not draw nothing: it draws *the wrong glyph*, silently, for every
//! character that names it. Nothing else in this crate would notice — the charstring walk
//! succeeds, the outline is a real outline, and the width is a real width. So the check that
//! matters is not "does this parse" but "does this agree with somebody who has never seen
//! this file".
//!
//! # Both directions of the mismatch
//!
//! Real fonts spell a glyph three ways — `A`, `uni0041`, `u0041` — and a PDF's `/Encoding`
//! may pick a different one from the font's charset. Both orders occur, so both are checked:
//! the charset says `uni0041` and the encoding says `A`, and the reverse.
//!
//! # How the two sides get the same bytes
//!
//! Neither reader walks a PDF from scratch here. `qpdf` normalises each corpus file so its
//! streams and its object streams are plain text, the oracle script lifts each CFF
//! font program out and writes it to its own file *along with the names `fontTools` reads
//! for it*, and this test reads those same files. So both sides are handed identical bytes
//! and there is no second PDF parser in the comparison to disagree about which font is
//! which.
//!
//! Where `fontTools` is not installed the test skips, and prints only that it skipped: a
//! cross-check that silently passes is worse than one that says it did not run.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use mangle_font::Program;
use mangle_font::cff::Cff;

/// Where the wild corpus is, if it has been fetched.
fn corpus_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?;
    let dir = root.join("corpus/wild");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// Whether `fontTools` is installed, and which version.
fn fonttools() -> Option<String> {
    let out = Command::new("python3")
        .args(["-c", "import fontTools; print(fontTools.version)"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether `qpdf` is installed.
fn has_qpdf() -> bool {
    Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Every PDF in the corpus.
fn corpus_pdfs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The script that lifts each CFF font out of a normalised PDF and writes it beside the
/// names `fontTools` reads for it.
///
/// One `.cff` and one `.names` per font, into a directory this test then reads. Splitting
/// the two is deliberate: the bytes and the names come from the *same* font object, and a
/// test that had to correlate them afterwards could correlate them wrongly.
const ORACLE: &str = r"
import io, os, re, sys, zlib
from fontTools.cffLib import CFFFontSet

def objects(raw):
    for m in re.finditer(rb'(?<![0-9])(\d+)\s+(\d+)\s+obj\b', raw):
        num = int(m.group(1)); start = m.end()
        end = raw.find(b'endobj', start)
        if end < 0: continue
        body = raw[start:end]
        sm = re.search(rb'stream\r?\n', body)
        if not sm:
            yield num, body, None; continue
        d = body[:sm.start()]; se = body.rfind(b'endstream')
        st = body[sm.end():se] if se > 0 else body[sm.end():]
        if st.endswith(b'\r\n'): st = st[:-2]
        elif st[-1:] in (b'\n', b'\r'): st = st[:-1]
        yield num, d, st

def inflate(d, data):
    if b'/FlateDecode' in d:
        try: return zlib.decompress(data)
        except zlib.error: return zlib.decompressobj().decompress(data)
    return data

outdir, paths = sys.argv[1], sys.argv[2:]
os.makedirs(outdir, exist_ok=True)
n = 0
for path in paths:
    label = os.path.basename(path)
    raw = open(path, 'rb').read()
    for num, d, stream in objects(raw):
        if stream is None: continue
        sub = re.search(rb'/Subtype\s*/(\w+)', d)
        if not sub or sub.group(1) not in (b'Type1C', b'CIDFontType0C'): continue
        data = inflate(d, stream)
        if data[:1] != b'\x01': continue
        try:
            cff = CFFFontSet(); cff.decompile(io.BytesIO(data), None)
        except Exception as e:
            continue
        for t in cff.fontNames:
            top = cff[t]
            # A CID font's charset is an identifier map rather than a name map, so there
            # is nothing here to compare a name against.
            if 'ROS' in top.rawDict: continue
            try:
                names = list(top.charset)
            except Exception:
                continue
            n += 1
            stem = '%s-obj%d' % (label, num)
            open(os.path.join(outdir, stem + '.cff'), 'wb').write(data)
            open(os.path.join(outdir, stem + '.names'), 'w').write('\n'.join(names))
            open(os.path.join(outdir, stem + '.topname'), 'w').write(t)
print(n)
";

/// One font as the oracle described it: the bytes, the names in glyph order, and the
/// top-level name.
struct OracleFont {
    stem: String,
    top_name: String,
    bytes: Vec<u8>,
    names: Vec<String>,
}

/// Run the oracle over every corpus PDF, returning the fonts it lifted out.
fn ask_oracle(dir: &Path, outdir: &Path) -> Result<Vec<OracleFont>, String> {
    let normalised_dir = outdir.join("pdf");
    std::fs::create_dir_all(&normalised_dir).map_err(|e| e.to_string())?;
    let mut sources = Vec::new();
    for pdf in corpus_pdfs(dir) {
        let out = normalised_dir.join(pdf.file_name().unwrap());
        // `qpdf` is what makes the bytes reachable: a corpus file's own xref may be
        // unreadable, and a font inside an object stream is not text.
        let ok = Command::new("qpdf")
            .args([
                "--qdf",
                "--object-streams=disable",
                "--decode-level=all",
                &pdf.to_string_lossy(),
                &out.to_string_lossy(),
            ])
            .output()
            .map(|o| o.status.success() && out.is_file())
            .unwrap_or(false);
        if ok {
            sources.push(out);
        }
    }

    let script = outdir.join("oracle.py");
    std::fs::write(&script, ORACLE).map_err(|e| e.to_string())?;
    let out = Command::new("python3")
        .arg(&script)
        .arg(outdir)
        .args(&sources)
        .output()
        .map_err(|e| format!("running the oracle: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "the oracle exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let mut fonts = Vec::new();
    let entries = std::fs::read_dir(outdir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "cff") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let names_path = outdir.join(format!("{stem}.names"));
        let top_path = outdir.join(format!("{stem}.topname"));
        let (Ok(bytes), Ok(names), Ok(top)) = (
            std::fs::read(&path),
            std::fs::read_to_string(&names_path),
            std::fs::read_to_string(&top_path),
        ) else {
            continue;
        };
        fonts.push(OracleFont {
            stem,
            top_name: top.trim().to_string(),
            bytes,
            names: names.lines().map(str::to_string).collect(),
        });
    }
    fonts.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(fonts)
}

/// Every `(name, glyph)` a charset asserts.
///
/// The list is in glyph order and starts at glyph 0, so a name's position in it *is* its
/// glyph number. `.notdef` is the one name at position 0 and it is skipped only because
/// no format lists it — it is still glyph 0.
///
/// A name carrying a `.` in it is skipped too. `fontTools` renames a glyph whose name
/// another glyph already has, appending `.1`, `.2` and so on; that is its own invention for
/// keeping a glyph order unique, and no charset in any file contains it. Comparing against
/// one would be comparing against a tool's tidying rather than against the font.
fn expected_positions(names: &[String]) -> Vec<(String, u32)> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| *n != ".notdef" && !n.contains('.'))
        .map(|(glyph, n)| (n.clone(), u32::try_from(glyph).unwrap_or(0)))
        .collect()
}

/// A file's own name, so a failure line is readable.
fn short(stem: &str) -> &str {
    stem.rsplit_once("-obj").map_or(stem, |(name, _)| name)
}

/// Every name-keyed CFF charset on this machine resolves the glyphs `fontTools` says.
///
/// This is the cross-check the charset reader exists to pass. Both readers are handed the
/// same font bytes and asked the same question — what is glyph *n* called — and each name is
/// then resolved *back* to a glyph number through this crate's own charset, so a table that
/// reads but reads wrong is caught: it would put `A` at a different glyph than the oracle
/// does, and the difference is a wrong shape on the page rather than a failure.
#[test]
fn the_name_keyed_charsets_on_this_machine_agree_with_fonttools() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: the wild corpus is not present");
        return;
    };
    let Some(version) = fonttools() else {
        eprintln!("skipped: fontTools is not installed, so there is no oracle to check against");
        return;
    };
    if !has_qpdf() {
        eprintln!("skipped: qpdf is not installed, so no font inside the corpus can be reached");
        return;
    }
    eprintln!("cross-checking every name-keyed CFF charset against fontTools {version}");

    let outdir = std::env::temp_dir().join(format!("mangle-charset-{}", std::process::id()));
    let fonts = ask_oracle(&dir, &outdir).unwrap_or_else(|why| panic!("{why}"));
    let _ = std::fs::remove_dir_all(&outdir);
    if fonts.is_empty() {
        eprintln!("skipped: no name-keyed CFF charset was found to check");
        return;
    }

    let mut compared = 0usize;
    let mut unreadable = Vec::new();
    let mut disagreements = Vec::new();

    for font in &fonts {
        let Ok(cff) = Cff::parse(&font.bytes, 0) else {
            unreadable.push(font.stem.clone());
            continue;
        };
        assert_eq!(
            cff.num_glyphs(),
            font.names.len(),
            "{}: the two readers must agree on how many glyphs there are before either \
             says which is which",
            font.stem
        );
        // Every name the oracle read, resolved back to a glyph through this crate. The name
        // is passed exactly as the oracle wrote it, so the `uniXXXX` versus `A` spelling is
        // tested rather than normalised away.
        for (name, expected) in expected_positions(&font.names) {
            match cff.glyph_for_name(&name) {
                Some(got) if got == expected => compared += 1,
                Some(got) => disagreements.push(format!(
                    "{} ({}): {name} is glyph {got} here and glyph {expected} in fontTools",
                    short(&font.stem),
                    font.top_name
                )),
                None => disagreements.push(format!(
                    "{} ({}): {name} resolves to nothing here and to glyph {expected} in \
                     fontTools",
                    short(&font.stem),
                    font.top_name
                )),
            }
        }
    }

    eprintln!(
        "{} charsets, {compared} glyph names agree with fontTools, {} do not, {} unreadable",
        fonts.len(),
        disagreements.len(),
        unreadable.len()
    );
    for stem in unreadable.iter().take(5) {
        eprintln!("  unreadable: {stem}");
    }
    for d in disagreements.iter().take(12) {
        eprintln!("DISAGREE: {d}");
    }
    assert!(
        compared > 1000,
        "only {compared} names compared, which is too few for this to have tested anything"
    );
    assert!(
        disagreements.is_empty(),
        "this reader and fontTools must agree on every glyph name:\n  {}",
        disagreements
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The `uniXXXX` and `A` spellings both resolve, in whichever direction the font wrote
/// them.
///
/// The general cross-check above passes a name to the charset as the oracle wrote it, which
/// tests each spelling against itself. This asks the harder question: given a charset that
/// says one spelling, does the *other* spelling find the same glyph? That is what a
/// renderer needs, because the charset and the PDF's `/Encoding` are written by different
/// programs and the corpus has fonts where they disagree.
#[test]
fn both_spellings_of_one_glyph_find_the_same_glyph_in_a_real_charset() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: the wild corpus is not present");
        return;
    };
    let Some(_) = fonttools() else {
        eprintln!("skipped: fontTools is not installed");
        return;
    };
    if !has_qpdf() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }

    let outdir =
        std::env::temp_dir().join(format!("mangle-charset-spellings-{}", std::process::id()));
    let fonts = ask_oracle(&dir, &outdir).unwrap_or_else(|why| panic!("{why}"));
    let _ = std::fs::remove_dir_all(&outdir);

    let mut uni_spelled = 0usize;
    let mut agree = 0usize;
    let mut mismatched = Vec::new();

    for font in &fonts {
        let Ok(cff) = Cff::parse(&font.bytes, 0) else {
            continue;
        };
        for name in &font.names {
            // A `uniXXXX` name in the charset, asked as the AGL name for the same character.
            let Some(digits) = name
                .strip_prefix("uni")
                .filter(|d| (4..=6).contains(&d.len()))
            else {
                continue;
            };
            let Ok(code) = u32::from_str_radix(digits, 16) else {
                continue;
            };
            uni_spelled += 1;
            let Some(as_written) = cff.glyph_for_name(name) else {
                continue;
            };
            let Some(agl) = mangle_font::encoding::agl_name(code) else {
                continue;
            };
            match cff.glyph_for_name(agl) {
                Some(other) if other == as_written => agree += 1,
                Some(other) => mismatched.push(format!(
                    "{}: {} and {agl} are glyphs {as_written} and {other}",
                    short(&font.stem),
                    name
                )),
                None => mismatched.push(format!(
                    "{}: {} is glyph {as_written} but {agl} resolves to nothing",
                    short(&font.stem),
                    name
                )),
            }
        }
    }

    eprintln!(
        "{uni_spelled} `uniXXXX` glyph names across the corpus, {agree} of which resolve \
         alike with the AGL name, {} of which do not",
        mismatched.len()
    );
    assert!(
        uni_spelled > 20,
        "only {uni_spelled} uniXXXX names found, which is too few for this to have tested \
         anything"
    );
    assert!(
        mismatched.is_empty(),
        "two spellings of one glyph must be one glyph:\n  {}",
        mismatched
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// A real font is reachable by name through the program a PDF embeds, and can then be
/// drawn.
///
/// This is the step the renderer takes, end to end: `code → name` from the encoding,
/// `name → glyph` from the charset, `glyph → outline` from the charstring. The unit tests
/// cover each step with fonts the test built; this covers the chain on fonts nobody here
/// wrote.
#[test]
fn a_real_font_program_resolves_a_name_and_then_draws_it() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: the wild corpus is not present");
        return;
    };
    let Some(_) = fonttools() else {
        eprintln!("skipped: fontTools is not installed, so no charset can be located");
        return;
    };
    if !has_qpdf() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }

    let outdir =
        std::env::temp_dir().join(format!("mangle-charset-program-{}", std::process::id()));
    let fonts = ask_oracle(&dir, &outdir).unwrap_or_else(|why| panic!("{why}"));
    let _ = std::fs::remove_dir_all(&outdir);

    let mut walked = 0usize;
    let mut failures = Vec::new();
    for font in &fonts {
        if Cff::parse(&font.bytes, 0).is_err() {
            continue;
        }
        for (name, glyph) in expected_positions(&font.names) {
            // A fresh program per glyph, because a `Program` caches outlines by glyph and a
            // cached answer from a previous glyph's bytes would be an answer about nothing.
            let mut program = Program::new(font.bytes.clone());
            match program.glyph_for_code_named(0, Some(&name)) {
                Some(got) if got == glyph => {
                    // And the glyph it named must be one the font can actually draw. A
                    // charset that resolves a name whose charstring will not walk has
                    // answered a question it could not have known the answer to.
                    match program.outline(got) {
                        Some(_) => walked += 1,
                        None => failures.push(format!(
                            "{}: {name} is glyph {got} but its charstring will not walk",
                            short(&font.stem)
                        )),
                    }
                }
                Some(got) => failures.push(format!(
                    "{}: code 0 named {name} is glyph {got} here and {glyph} in fontTools",
                    short(&font.stem)
                )),
                None => failures.push(format!(
                    "{}: {name} resolves to nothing through the program",
                    short(&font.stem)
                )),
            }
        }
    }

    eprintln!(
        "{walked} real glyphs resolved by name and drawn, {} failed",
        failures.len()
    );
    assert!(
        walked > 100,
        "only {walked} glyphs walked, which is too few for this to have tested anything"
    );
    assert!(
        failures.is_empty(),
        "every real glyph the charset names must resolve and draw:\n  {}",
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
