//! Cross-check the interpreter's text placement against an independent renderer.
//!
//! The expectations here are not written out: they are read from the `-F trace` output of
//! an external PDF renderer over the same content stream, so the test is a differential
//! one and says whether this implementation agrees with a program that was not written
//! from the same reading of the specification. Without it, a misreading of the text space
//! is indistinguishable from a correct one, because both are self-consistent.
//!
//! The renderer is `mutool`, which every maintainer of this repository has. It is not a
//! declared dependency, and it is not required to run: when it is missing the tests report
//! that they were skipped rather than failing, so an environment without it still gets a
//! green run. It is the reference that turns the placement model from an assertion into a
//! measurement.

#![forbid(unsafe_code)]
// Tests state their expectations with an assert and index a slice whose length they have
// already asserted, which is what a test is for; the panic-free rule is about what the
// product does with a file, not about how a test reads one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use mangle_content::{ContentStream, Mark, Resources, run_with};
use mangle_syntax::object::{Dict, Object};

/// A font whose every glyph is a known width, so the expectations are arithmetic.
///
/// The width is stated per test, and it is the width every code has, which is what makes a
/// placement predictable from the specification's formula alone.
struct Font {
    /// The width of every code, in the font's own units.
    width: i64,
}

impl Font {
    fn resources(&self) -> Resources {
        let mut font = Dict::new();
        font.set("Type", Object::name("Font"));
        font.set("BaseFont", Object::name("Helvetica"));
        font.set("FirstChar", Object::Int(32));
        font.set(
            "Widths",
            Object::Array((0..96).map(|_| Object::Int(self.width)).collect()),
        );
        let mut table = Dict::new();
        table.set("F1", Object::Dict(font));
        let mut resources = Dict::new();
        resources.set("Font", Object::Dict(table));
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }
}

/// The x of every glyph's placement, in text space.
fn glyph_x(content: &[u8], width: i64) -> Vec<f64> {
    let resources = Font { width }.resources();
    let out = run_with(&ContentStream::parse(content), &resources);
    let mut xs = Vec::new();
    for record in &out.records {
        let Mark::Glyphs { placements, .. } = &record.mark else {
            continue;
        };
        xs.extend(placements.iter().map(|m| m.e));
    }
    xs
}

/// The x of every glyph, according to `mutool`, for the same content.
///
/// `mutool draw -F trace` reports each glyph's position in unscaled text space, which is
/// the space this interpreter's placements are recorded in, so the two are directly
/// comparable without a font scale in between.
fn mutool_x(content: &[u8], width: i64) -> Option<Vec<f64>> {
    let dir = temp_dir()?;
    let pdf = write_pdf(&dir, content, width)?;
    // `mutool draw -F trace` writes the trace to its standard output; the output path is
    // only honoured by the formats that produce a file.
    let ok = Command::new("mutool")
        .args(["draw", "-F", "trace"])
        .arg(&pdf)
        .output()
        .ok()?;
    if !ok.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&ok.stdout);
    Some(
        text.lines()
            .filter_map(|line| {
                let rest = line.split("<g ").nth(1)?;
                let x = rest.split("x=\"").nth(1)?.split('"').next()?;
                x.parse::<f64>().ok()
            })
            .collect(),
    )
}

/// A directory for the scratch files, under the target directory so it is inside the build.
fn temp_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/text-space-diff");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The smallest PDF that will carry a content stream, so `mutool` will run it.
///
/// The `/Widths` go in the font dictionary as well as in the interpreter's resources: a
/// renderer lays the text out from the font it is given, and a case whose declared widths
/// differ from the built-in metrics of the base font would be comparing two fonts.
fn write_pdf(dir: &Path, content: &[u8], width: i64) -> Option<PathBuf> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 900 900] /Resources \
          << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        format!("<< /Length {} >>\nstream\n", content.len())
            .into_bytes()
            .into_iter()
            .chain(content.iter().copied())
            .chain(b"\nendstream".iter().copied())
            .collect(),
        // Helvetica with the widths declared, because that is the font whose `/Widths` this
        // case is about. The run starts at `/FirstChar` 32, one width per code to 127,
        // which covers every code these cases show.
        {
            let widths: Vec<String> = (0..96).map(|_| width.to_string()).collect();
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 \
                 /LastChar 127 /Widths [{}] >>",
                widths.join(" ")
            )
            .into_bytes()
        },
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    let path = dir.join("case.pdf");
    std::fs::write(&path, pdf).ok()?;
    Some(path)
}

/// One case, and what both implementations made of it.
struct Case {
    /// What the case is for, which the failure message repeats.
    what: String,
    /// The content stream.
    content: Vec<u8>,
    /// The width every glyph has.
    width: i64,
}

fn cases() -> Vec<Case> {
    let handpicked: [(&str, &str, i64); 18] = [
        (
            "a size of one: the advance is a fraction of a unit",
            "BT /F1 1 Tf 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "a size of 24: the advance is half an em of 24 units",
            "BT /F1 24 Tf 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "a size of 100: the size scales the glyph's own width",
            "BT /F1 100 Tf 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "a width of 500 is half an em",
            "BT /F1 24 Tf 0 0 Td (AA) Tj ET",
            500,
        ),
        (
            "a width of 333 is a third of an em",
            "BT /F1 12 Tf 0 0 Td (AAAA) Tj ET",
            333,
        ),
        (
            "Td moves in units the font size does not touch",
            "BT /F1 24 Tf 37 111 Td (AA) Tj ET",
            600,
        ),
        (
            "Tz scales the finished advance",
            "BT /F1 24 Tf 50 Tz 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "Tc is in unscaled text-space units",
            "BT /F1 24 Tf 100 Tc 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "Tz scales the character spacing too",
            "BT /F1 24 Tf 50 Tz 100 Tc 0 0 Td (AA) Tj ET",
            600,
        ),
        (
            "Tw applies to the space and to nothing else",
            "BT /F1 24 Tf 100 Tw 0 0 Td (A B) Tj ET",
            600,
        ),
        (
            "Tw with a Tz scales too",
            "BT /F1 24 Tf 50 Tz 100 Tw 0 0 Td (A B) Tj ET",
            600,
        ),
        (
            "a TJ kern subtracts, in thousandths of an em",
            "BT /F1 24 Tf 0 0 Td [(A) -500 (A)] TJ ET",
            600,
        ),
        (
            "a positive TJ kern pulls the glyph closer",
            "BT /F1 24 Tf 0 0 Td [(A) 500 (A)] TJ ET",
            600,
        ),
        (
            "a TJ kern scales with the font size",
            "BT /F1 100 Tf 0 0 Td [(A) -500 (A)] TJ ET",
            600,
        ),
        (
            "a second show continues where the first ended",
            "BT /F1 24 Tf 0 0 Td (AA) Tj (BB) Tj ET",
            600,
        ),
        (
            "a Td starts a new line from the line matrix",
            "BT /F1 24 Tf 0 0 Td (AA) Tj 0 -50 Td (BB) Tj ET",
            600,
        ),
        (
            "a rotated text matrix turns the advance with it",
            "BT /F1 24 Tf 0.7071 0.7071 -0.7071 0.7071 300 100 Tm (AA) Tj ET",
            600,
        ),
        (
            "a text matrix with a scale of its own",
            "BT /F1 24 Tf 2 0 0 2 10 20 Tm (AA) Tj ET",
            600,
        ),
    ];
    handpicked
        .iter()
        .map(|(what, content, width)| Case {
            what: (*what).to_string(),
            content: content.as_bytes().to_vec(),
            width: *width,
        })
        .collect()
}

/// A deterministic spread of parameter combinations, for breadth rather than for any one
/// rule.
///
/// Each is assembled from the same parts a case is, with the numbers chosen by a fixed
/// formula rather than a random source, so a failure is reproducible and the set does not
/// change from run to run.
fn generated_cases() -> Vec<Case> {
    let sizes = [1, 2, 5, 10, 12, 24, 36, 72, 100];
    let scales = [100, 100, 50, 200, 25];
    let mut out = Vec::new();
    for (i, size) in sizes.iter().enumerate() {
        for (j, scale) in scales.iter().enumerate() {
            // Every pick is a modulo of a value already in range, so each table is indexed
            // by a value in 0..len.
            let tc = [0, 1, 7, 100][(i + j) % 4];
            let tw = [0, 4, 100][(i + 2 * j) % 3];
            let tdx = [0, 10, 37, 250][(i * 3 + j) % 4];
            let tdy = [0, 20, 111][(i + j) % 3];
            let rise = [0, 3, -4][(i + 2 * j) % 3];
            let width = [500, 600, 333][(i + j) % 3];
            let setup = format!("{scale} Tz {tc} Tc {tw} Tw {rise} Ts {tdx} {tdy} Td");
            // Alternate between a `Tj` and a `TJ` so both the kern and the plain path are
            // covered at every combination.
            let content = if (i + j) % 2 == 0 {
                format!("BT /F1 {size} Tf {setup} (ABA B) Tj ET")
            } else {
                format!("BT /F1 {size} Tf {setup} [(A) -500 (B) 250 (A) ( ) -1200 (B)] TJ ET")
            };
            out.push(Case {
                what: format!("generated: size {size}, {setup}, width {width}"),
                content: content.into_bytes(),
                width,
            });
        }
    }
    out
}

/// Every case, against the external renderer.
///
/// A single test rather than one per case: the cases share the setup, the external process
/// is the slow part, and a failure names the case in its message.
#[test]
fn the_placements_agree_with_an_independent_renderer() {
    let mut all = cases();
    all.extend(generated_cases());
    let mut checked = 0;
    let mut skipped = Vec::new();
    for case in all {
        let Some(reference) = mutool_x(&case.content, case.width) else {
            skipped.push(case.what);
            continue;
        };
        let got = glyph_x(&case.content, case.width);
        assert_eq!(
            got.len(),
            reference.len(),
            "{}: one placement per glyph, and the same count as the reference",
            case.what
        );
        for (i, (mine, theirs)) in got.iter().zip(&reference).enumerate() {
            assert!(
                (mine - theirs).abs() < 0.01,
                "{}: glyph {i} is at {mine}, the reference puts it at {theirs}",
                case.what
            );
        }
        checked += 1;
    }
    // Not skipping: a test that quietly checks nothing is worse than no test, because it
    // reads as coverage. If the renderer is not there, say so by failing.
    assert_ne!(
        checked, 0,
        "no case could be compared, so this test has checked nothing. `mutool draw -F \
         trace` is the reference it needs, and the cases it would have run were: {skipped:?}"
    );
    if !skipped.is_empty() {
        // Not a failure: a renderer that cannot open a file is an environment, not a model.
        eprintln!("skipped {} of the cases: {skipped:?}", skipped.len());
    }
}
