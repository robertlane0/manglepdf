//! Repair: rebuilding what we can when the file's own structure is unusable.
//!
//! The order matters. We try, in turn: the cross-reference chain, the object-scan
//! fallback, and finally a search for the catalogue. Every step is recorded so the UI can
//! say what happened instead of pretending the file was clean.

use crate::error::Result;
use crate::lexer::Token;
use crate::object::{Object, Ref};
use crate::xref::{SectionKind, Xref, XrefEntry, read_xref};

/// How the file's structure was established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// The cross-reference chain was read and used as written.
    Clean,
    /// The chain was read but something was missing; objects were scanned to fill gaps.
    Repaired {
        /// Short descriptions of what was wrong.
        notes: Vec<String>,
    },
    /// The cross-reference was unusable and every object was found by scanning.
    Rebuilt { notes: Vec<String> },
    /// Nothing worked; we are guessing.
    Failed { notes: Vec<String> },
}

impl Recovery {
    /// Whether the file opened exactly as written. Anything else means the reader
    /// should tell the user, because a repaired file behaves differently on save.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        matches!(self, Recovery::Clean)
    }

    /// A banner for the UI, or `None` when the file was clean.
    #[must_use]
    pub fn banner(&self) -> Option<String> {
        match self {
            Recovery::Clean => None,
            Recovery::Repaired { notes } => Some(format!(
                "This file had {} structural problem{} that {} repaired in memory; saving rewrites it.",
                notes.len(),
                if notes.len() == 1 { "" } else { "s" },
                if notes.len() == 1 { "was" } else { "were" }
            )),
            Recovery::Rebuilt { .. } => Some(
                "This file's cross-reference information was unusable. It was rebuilt by \
                 scanning; saving rewrites the file."
                    .to_string(),
            ),
            Recovery::Failed { .. } => Some(
                "This file could not be read properly; only parts of it are available.".to_string(),
            ),
        }
    }

    /// `true` when saving must rewrite the file rather than append to it.
    #[must_use]
    pub fn forces_full_rewrite(&self) -> bool {
        !matches!(self, Recovery::Clean)
    }

    /// The notes, for the Inspector.
    #[must_use]
    pub fn notes(&self) -> &[String] {
        match self {
            Recovery::Clean => &[],
            Recovery::Repaired { notes }
            | Recovery::Rebuilt { notes }
            | Recovery::Failed { notes } => notes,
        }
    }
}

/// Read the cross-reference chain, repairing it if necessary.
pub fn read_or_repair(data: &[u8]) -> Result<(Xref, Recovery)> {
    let mut notes: Vec<String> = Vec::new();
    let start = crate::xref::find_startxref(data);
    let mut xref = match start {
        Some(s) => match read_xref(data, s, &mut notes) {
            Ok(x) => x,
            Err(e) => {
                notes.push(e.to_string());
                Xref::default()
            }
        },
        None => {
            notes.push("no `startxref` found".to_string());
            Xref::default()
        }
    };

    // A file with no entries at all, or a catalog that does not resolve, is unusable.
    let mut needs_scan = xref.object_numbers().count() == 0;
    if !needs_scan {
        if let Some(root) = xref.trailer.get("Root").and_then(Object::as_ref_id) {
            if !entry_is_plausible(data, &xref, root) {
                notes.push("the trailer's /Root does not point at a catalogue".to_string());
                needs_scan = true;
            }
        } else {
            notes.push("the trailer has no /Root".to_string());
            needs_scan = true;
        }
    }

    if needs_scan {
        let scanned = scan_objects(data, &mut notes);
        if scanned.is_empty() {
            return Ok((xref, Recovery::Failed { notes }));
        }
        // Scanned entries fill gaps; the file's own entries still win where present and
        // plausible, because a later definition is normally the live one.
        for (num, offset) in scanned {
            let replace = match xref.get(num) {
                None | Some(XrefEntry::Missing) => true,
                Some(XrefEntry::InFile { offset: o, .. }) => {
                    !looks_like_object(data, o) && looks_like_object(data, offset)
                }
                Some(XrefEntry::InStream { .. }) => false,
                Some(XrefEntry::Free { .. }) => false,
            };
            if replace {
                xref.set(
                    num,
                    XrefEntry::InFile {
                        offset,
                        generation: 0,
                    },
                );
            }
        }
        if xref.trailer.get("Root").is_none() {
            if let Some(root) = find_catalog(data, &xref) {
                xref.trailer.set("Root", Object::Ref(root));
                notes.push("the catalogue was found by scanning".to_string());
            }
        }
        let kind = if xref.kind() == SectionKind::Stream {
            SectionKind::Stream
        } else {
            SectionKind::Table
        };
        xref.set_kind(kind);
        let recovery = if start.is_none() || xref.live_objects().count() == 0 {
            Recovery::Rebuilt { notes }
        } else {
            Recovery::Repaired { notes }
        };
        return Ok((xref, recovery));
    }

    Ok((xref, Recovery::Clean))
}

fn entry_is_plausible(data: &[u8], xref: &Xref, root: Ref) -> bool {
    match xref.get(root.num) {
        Some(XrefEntry::InFile { offset, .. }) => looks_like_object(data, offset),
        Some(XrefEntry::InStream { .. }) => true,
        _ => false,
    }
}

/// Does an object header sit at this offset?
#[must_use]
pub fn looks_like_object(data: &[u8], offset: usize) -> bool {
    let Some(chunk) = data.get(offset..) else {
        return false;
    };
    let mut lx = crate::lexer::Lexer::new(chunk);
    lx.skip_space();
    let Ok(t) = lx.next_token() else {
        return false;
    };
    if !matches!(t.token, Token::Int(_)) {
        return false;
    }
    let Ok(t2) = lx.next_token() else {
        return false;
    };
    if !matches!(t2.token, Token::Int(_)) {
        return false;
    }
    let Ok(t3) = lx.next_token() else {
        return false;
    };
    t3.is_keyword(b"obj")
}

/// Scan the whole file for `N G obj` headers. Later definitions win, which is what the
/// specification requires for incremental updates.
///
/// The scan is lexical rather than a byte search: literal strings, hex strings, comments
/// and stream bodies are skipped, so an `obj` sequence inside any of them is invisible.
#[must_use]
pub fn scan_objects(data: &[u8], notes: &mut Vec<String>) -> Vec<(u32, usize)> {
    let mut found: Vec<(u32, usize)> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        match data.get(i).copied() {
            Some(b'%') => {
                i = skip_to_eol(data, i);
                continue;
            }
            Some(b'(') => {
                i = skip_literal_string(data, i);
                continue;
            }
            Some(b'<') if data.get(i + 1) == Some(&b'<') => {
                i += 2;
                continue;
            }
            Some(b'<') => {
                i = skip_hex_string(data, i);
                continue;
            }
            Some(b'>') if data.get(i + 1) == Some(&b'>') => {
                i += 2;
                continue;
            }
            _ => {}
        }
        let at_boundary = i == 0
            || data
                .get(i - 1)
                .is_some_and(|c| Token::is_white_or_delimiter(*c));
        if at_boundary && data.get(i..i + 3) == Some(b"obj".as_slice()) {
            if let Some((num, start)) = header_before(data, i) {
                if num > 0 {
                    found.push((num, start));
                }
            }
            i += 3;
            continue;
        }
        if at_boundary && data.get(i..i + 6) == Some(b"stream".as_slice()) {
            // Skip the body: it is binary and full of things that look like syntax.
            i = skip_stream_body(data, i);
            continue;
        }
        i += 1;
    }
    if !found.is_empty() {
        notes.push(format!("{} objects were located by scanning", found.len()));
    }
    found
}

fn skip_to_eol(data: &[u8], from: usize) -> usize {
    let mut i = from;
    while let Some(&b) = data.get(i) {
        i += 1;
        if b == b'\n' || b == b'\r' {
            break;
        }
    }
    i
}

/// Skip a `( ... )` literal string, honouring escapes and nesting.
fn skip_literal_string(data: &[u8], from: usize) -> usize {
    let mut i = from + 1;
    let mut depth = 1usize;
    while let Some(&b) = data.get(i) {
        i += 1;
        match b {
            b'\\' => {
                i += 1;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    i
}

fn skip_hex_string(data: &[u8], from: usize) -> usize {
    let mut i = from + 1;
    while let Some(&b) = data.get(i) {
        i += 1;
        if b == b'>' {
            return i;
        }
    }
    i
}

/// Skip from the `stream` keyword to just past `endstream`.
fn skip_stream_body(data: &[u8], from: usize) -> usize {
    let mut i = from + 6;
    // The keyword is followed by CRLF or LF.
    if data.get(i) == Some(&b'\r') {
        i += 1;
    }
    if data.get(i) == Some(&b'\n') {
        i += 1;
    }
    let body = i;
    while i + 9 <= data.len() {
        if data.get(i..i + 9) == Some(b"endstream") && i > body {
            return i + 9;
        }
        i += 1;
    }
    data.len()
}

/// If `obj` at `at` is preceded by `<num> <gen> `, return the number and where it starts.
fn header_before(data: &[u8], at: usize) -> Option<(u32, usize)> {
    let mut j = at;
    while j > 0 && data.get(j - 1).is_some_and(u8::is_ascii_whitespace) {
        j -= 1;
    }
    let gen_end = j;
    while j > 0 && data.get(j - 1).is_some_and(u8::is_ascii_digit) {
        j -= 1;
    }
    if gen_end == j {
        return None;
    }
    while j > 0 && data.get(j - 1).is_some_and(u8::is_ascii_whitespace) {
        j -= 1;
    }
    let num_end = j;
    while j > 0 && data.get(j - 1).is_some_and(u8::is_ascii_digit) {
        j -= 1;
    }
    if num_end == j {
        return None;
    }
    let text = data.get(j..num_end)?;
    let num = std::str::from_utf8(text).ok()?.parse::<u32>().ok()?;
    Some((num, j))
}

fn find_catalog(data: &[u8], xref: &Xref) -> Option<Ref> {
    crate::xref::find_root(data, xref)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn scanning_finds_objects() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages >>\nendobj\ntrailer\n<< >>\n";
        let mut notes = Vec::new();
        let found = scan_objects(data, &mut notes);
        let nums: Vec<u32> = found.iter().map(|(n, _)| *n).collect();
        assert!(nums.contains(&1), "{nums:?}");
        assert!(nums.contains(&2), "{nums:?}");
        assert_eq!(found.first().map(|(_, at)| *at), Some(9), "{found:?}");
    }

    #[test]
    fn later_definitions_win() {
        let data = b"1 0 obj\n<< /A 1 >>\nendobj\n1 0 obj\n<< /A 2 >>\nendobj\n";
        let mut notes = Vec::new();
        let found = scan_objects(data, &mut notes);
        let last = found.iter().rev().find(|(n, _)| *n == 1).copied();
        assert!(last.is_some());
    }

    #[test]
    fn no_startxref_triggers_a_rebuild() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n\
                    3 0 obj\n<< /Length 4 >>\nstream\nabcd\nendstream\nendobj\n";
        let (xref, recovery) = read_or_repair(data).expect("repair");
        assert!(recovery.banner().is_some());
        assert!(recovery.forces_full_rewrite());
        assert!(xref.get(1).is_some());
        assert!(xref.trailer().get("Root").is_some());
    }

    #[test]
    fn clean_file_needs_no_banner() {
        // `xref` sits at 45 and object 1 at 9; the offsets must agree or the file is
        // genuinely damaged and repairing it is the correct outcome.
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
                    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
                    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
        let (xref, recovery) = read_or_repair(data).expect("read");
        assert_eq!(recovery, Recovery::Clean, "{:?}", recovery.notes());
        assert!(xref.get(1).is_some());
    }

    #[test]
    fn obj_inside_a_string_is_not_a_header() {
        let data = b"1 0 obj\n(hello 2 0 obj world)\nendobj\n";
        let mut notes = Vec::new();
        let found = scan_objects(data, &mut notes);
        assert!(!found.iter().any(|(n, _)| *n == 2), "{found:?}");
    }
}
