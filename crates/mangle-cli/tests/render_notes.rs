//! `render` says what it could not draw, and fails on request.
//!
//! Each page is built in the test, so these run without the corpus: one image in a colour space
//! that cannot be read, which the renderer reports and leaves out, and one that draws.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};

use mangle_testkit::{Scratch, single_page_pdf};

/// A one-page PDF drawing one two-by-two image in `space`.
fn pdf_with_image_in(space: &str) -> Vec<u8> {
    let entries = format!(
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace {space} /BitsPerComponent 8"
    );
    single_page_pdf(
        "/XObject << /Im1 5 0 R >>",
        b"q 60 0 0 60 20 20 cm /Im1 Do Q",
        &[(entries.as_str(), &[40u8; 4])],
    )
}

/// Run `manglepdf-cli render` over `pdf` into `out`, at a size that keeps the test quick.
fn render(pdf: &Path, out: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_manglepdf-cli"))
        .arg("render")
        .arg(pdf)
        .arg("-o")
        .arg(out)
        .args(["-dpi", "72"])
        .args(extra)
        .output()
        .expect("the binary runs")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn render_lists_what_it_could_not_draw_on_stderr_and_still_succeeds() {
    let scratch = Scratch::new("cli-render-notes-listed").expect("scratch");
    let pdf = scratch.write("page.pdf", &pdf_with_image_in("/NoSuchSpace"));
    let out = scratch.path().join("page.png");
    let result = render(&pdf, &out, &[]);
    let stderr = stderr_of(&result);
    assert!(
        result.status.success(),
        "a page with something missing is still a page: {stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("note ") && l.contains("colour space this cannot read")),
        "the image that was not drawn is named: {stderr}"
    );
    let png = std::fs::read(&out).expect("the file is written");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn strict_turns_a_note_into_a_failure_and_still_writes_the_file() {
    let scratch = Scratch::new("cli-render-notes-strict").expect("scratch");
    let pdf = scratch.write("page.pdf", &pdf_with_image_in("/NoSuchSpace"));
    let out = scratch.path().join("page.png");
    let result = render(&pdf, &out, &["--strict"]);
    let stderr = stderr_of(&result);
    assert!(
        !result.status.success(),
        "--strict fails when something was left out: {stderr}"
    );
    assert!(
        stderr.contains("colour space this cannot read"),
        "the note is still printed: {stderr}"
    );
    assert!(
        stderr.contains("manglepdf-cli:") && stderr.contains("1 note(s)"),
        "the failure says why: {stderr}"
    );
    assert!(out.is_file(), "the picture is written either way");
}

#[test]
fn a_page_that_drew_completely_is_silent_and_passes_strict() {
    let scratch = Scratch::new("cli-render-notes-clean").expect("scratch");
    let pdf = scratch.write("page.pdf", &pdf_with_image_in("/DeviceGray"));
    let out = scratch.path().join("page.png");
    let result = render(&pdf, &out, &["--strict"]);
    let stderr = stderr_of(&result);
    assert!(result.status.success(), "nothing was left out: {stderr}");
    assert!(stderr.is_empty(), "so there is nothing to say: {stderr}");
    assert!(out.is_file());
}

#[test]
fn a_jpeg_render_reports_the_same_notes() {
    let scratch = Scratch::new("cli-render-notes-jpeg").expect("scratch");
    let pdf = scratch.write("page.pdf", &pdf_with_image_in("/NoSuchSpace"));
    let out = scratch.path().join("page.jpg");
    let result = render(&pdf, &out, &["--strict"]);
    let stderr = stderr_of(&result);
    assert!(!result.status.success(), "{stderr}");
    assert!(stderr.contains("colour space this cannot read"), "{stderr}");
    assert!(
        std::fs::read(&out)
            .expect("written")
            .starts_with(&[0xFF, 0xD8])
    );
}
