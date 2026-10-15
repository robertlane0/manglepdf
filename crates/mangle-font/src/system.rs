//! System font directory enumeration for `--font-dirs` (GOAL.md §8.6, FINISH.md S1.3).
//!
//! `--font-dirs PATH[,PATH]` replaces the default system font discovery so
//! font-fallback behaviour is testable and reproducible. This module is the
//! enumeration half: given replacement directories, list the candidate font
//! files inside them deterministically. Deciding which candidate backs which
//! `/BaseFont` (the fallback chain into `substitutes`) comes next and is still
//! open.
//!
//! Hostile-input discipline applies to directories too: the walk is bounded in
//! roots, depth, visited entries, and reported files; symlinks are never
//! followed (no cycles, no escape from the listed roots); unreadable entries
//! are skipped; output is sorted so the same directories always give the same
//! list.

use std::path::{Path, PathBuf};

/// The most `--font-dirs` roots honoured; the rest are ignored.
pub const MAX_ROOTS: usize = 32;
/// How deep below a root the walk descends.
pub const MAX_DEPTH: usize = 8;
/// The most font files reported, keeping the list — and whatever reads it — small.
pub const MAX_FILES: usize = 4096;
/// The most directory entries inspected, bounding walks of huge trees.
pub const MAX_VISITED: usize = 65_536;
/// The most bytes read from one candidate file to parse its names.
pub const MAX_FONT_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Font file extensions recognised by the scan, matched case-insensitively.
fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("ttf")
                || ext.eq_ignore_ascii_case("otf")
                || ext.eq_ignore_ascii_case("ttc")
        })
}

/// List the candidate font files under `dirs`, sorted and deduplicated.
///
/// An empty `dirs` lists nothing (bundled faces only); a missing or unreadable
/// directory contributes nothing rather than failing. A root naming a font file
/// directly lists that file.
#[must_use]
pub fn discover_font_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut visited: usize = 0;
    for root in dirs.iter().take(MAX_ROOTS) {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_file() {
            if out.len() < MAX_FILES && is_font_file(root) && !out.contains(root) {
                out.push(root.clone());
            }
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_DEPTH || out.len() >= MAX_FILES {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                visited = visited.saturating_add(1);
                if visited > MAX_VISITED || out.len() >= MAX_FILES {
                    break;
                }
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_file() {
                    if is_font_file(&path) && !out.contains(&path) {
                        out.push(path);
                    }
                } else if kind.is_dir() && depth.saturating_add(1) <= MAX_DEPTH {
                    stack.push((path, depth.saturating_add(1)));
                }
            }
        }
    }
    out.sort();
    out
}

/// A system font file with the names read out of its `name` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFontInfo {
    /// Where the file lives.
    pub path: PathBuf,
    /// Name ID 16 (or 1): e.g. `Liberation Sans`.
    pub family: String,
    /// Name ID 17 (or 2): e.g. `Bold`.
    pub subfamily: String,
}

/// Read a font file's family and subfamily names.
///
/// `None` for anything that is not a readable, sensibly sized, parseable font
/// with Unicode names — a hostile or foreign file is skipped, never reported
/// as a match.
#[must_use]
pub fn inspect_font_file(path: &Path) -> Option<SystemFontInfo> {
    let size = std::fs::metadata(path).ok()?.len();
    if size == 0 || size > MAX_FONT_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let face = ttf_parser::Face::parse(&bytes, 0).ok()?;
    let mut family: Option<String> = None;
    let mut subfamily: Option<String> = None;
    for name in face.names() {
        let slot = match name.name_id {
            ttf_parser::name_id::TYPOGRAPHIC_FAMILY => &mut family,
            ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY => &mut subfamily,
            ttf_parser::name_id::FAMILY if family.is_none() => &mut family,
            ttf_parser::name_id::SUBFAMILY if subfamily.is_none() => &mut subfamily,
            _ => continue,
        };
        if slot.is_none() {
            *slot = name.to_string();
        }
    }
    Some(SystemFontInfo {
        path: path.to_path_buf(),
        family: family?,
        subfamily: subfamily?,
    })
}

/// The Standard 14 family a program family stands in for, or the name itself.
///
/// The file-side mirror of the request-side alias table: a system `Arial`
/// stands in for Helvetica through [`crate::metrics::canonical_family`], and
/// the bundled Liberation programs name their own families (`Liberation Sans`)
/// rather than the ones they stand in for (see `substitutes`).
fn candidate_canonical(family: &str) -> &str {
    let flat: String = family.chars().filter(|c| *c != ' ').collect();
    for (program, target) in [
        ("LiberationSans", "Helvetica"),
        ("LiberationSerif", "Times"),
        ("LiberationMono", "Courier"),
    ] {
        if flat == program {
            return target;
        }
    }
    crate::metrics::canonical_family(family)
}

/// Find the system file backing `/BaseFont` `requested` (S1.3 rung 3).
///
/// The name is split with the shared [`crate::metrics::split_name`] parser, the
/// family is resolved through the same alias table the bundled substitution
/// uses, and the style must match exactly — an unknown style is refused rather
/// than guessed at, exactly like [`crate::substitute`]. First match in sorted
/// discovery order wins, so the answer is deterministic.
#[must_use]
pub fn find_system_font(requested: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let (family, style) = crate::metrics::split_name(requested)?;
    let want_family = crate::metrics::canonical_family(family);
    let want_style = crate::metrics::style_words(&style)?;
    for path in discover_font_files(dirs) {
        let Some(info) = inspect_font_file(&path) else {
            continue;
        };
        if candidate_canonical(&info.family) != want_family {
            continue;
        }
        let flat: String = info.subfamily.chars().filter(|c| *c != ' ').collect();
        let Some(got_style) = crate::metrics::style_words(&flat) else {
            continue;
        };
        if got_style == want_style {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"").expect("writes a fixture file");
    }

    #[test]
    fn nested_fonts_are_found_sorted_and_non_fonts_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("b.ttf"));
        touch(&root.join("a.otf"));
        touch(&root.join("notes.txt"));
        touch(&root.join("no-extension"));
        let sub = root.join("sub");
        std::fs::create_dir(&sub).expect("subdir");
        touch(&sub.join("c.TTC"));

        let first = discover_font_files(&[root.to_path_buf()]);
        let second = discover_font_files(&[root.to_path_buf()]);
        assert_eq!(first, second, "enumeration is deterministic");
        let names: Vec<String> = first
            .iter()
            .map(|p| {
                p.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["a.otf", "b.ttf", "c.TTC"]);
    }

    #[test]
    fn missing_unreadable_and_empty_inputs_list_nothing() {
        assert!(discover_font_files(&[]).is_empty());
        assert!(
            discover_font_files(&[PathBuf::from("/no/such/dir/anywhere")]).is_empty(),
            "a missing directory is skipped, not an error"
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(discover_font_files(&[tmp.path().to_path_buf()]).is_empty());
    }

    #[test]
    fn a_root_naming_a_file_directly_lists_it_when_it_is_a_font() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let font = tmp.path().join("face.ttf");
        let text = tmp.path().join("readme.txt");
        touch(&font);
        touch(&text);
        assert_eq!(discover_font_files(std::slice::from_ref(&font)), vec![font]);
        assert!(discover_font_files(&[text]).is_empty());
    }

    #[test]
    fn symlinks_are_never_followed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let real = root.join("real");
        std::fs::create_dir(&real).expect("real dir");
        touch(&real.join("face.ttf"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&real, root.join("link")).expect("symlink");
            let found = discover_font_files(&[root.to_path_buf()]);
            assert_eq!(
                found.len(),
                1,
                "the linked tree is not walked twice: {found:?}"
            );
            assert!(discover_font_files(&[root.join("link")]).is_empty());
        }
    }

    #[test]
    fn descent_stops_at_the_depth_bound() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut dir = tmp.path().to_path_buf();
        for i in 0..MAX_DEPTH + 3 {
            dir = dir.join(format!("d{i}"));
            std::fs::create_dir(&dir).expect("nested dir");
        }
        touch(&dir.join("deep.ttf"));
        assert!(
            discover_font_files(&[tmp.path().to_path_buf()]).is_empty(),
            "a font deeper than MAX_DEPTH is not reported"
        );
    }

    fn bundled_fonts_dir() -> PathBuf {
        // Unit tests run with the package directory as cwd, unlike `include_bytes!`
        // paths which resolve from the source file.
        PathBuf::from("../../assets/fonts")
    }

    #[test]
    fn the_bundled_liberation_files_match_their_standard_names() {
        let dirs = [bundled_fonts_dir()];
        for (requested, file) in [
            ("Helvetica", "LiberationSans-Regular.ttf"),
            ("Helvetica-Bold", "LiberationSans-Bold.ttf"),
            ("Helvetica-Oblique", "LiberationSans-Italic.ttf"),
            ("Helvetica-BoldOblique", "LiberationSans-BoldItalic.ttf"),
            ("Times-Roman", "LiberationSerif-Regular.ttf"),
            ("Times-BoldItalic", "LiberationSerif-BoldItalic.ttf"),
            ("Courier-Bold", "LiberationMono-Bold.ttf"),
            ("ABCDEF+Helvetica-Bold", "LiberationSans-Bold.ttf"),
            ("Arial-Bold", "LiberationSans-Bold.ttf"),
            ("TimesNewRomanPS-ItalicMT", "LiberationSerif-Italic.ttf"),
        ] {
            let found = find_system_font(requested, &dirs);
            assert_eq!(
                found.and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
                Some(file.to_string()),
                "{requested} should resolve to {file}"
            );
        }
    }

    #[test]
    fn unknown_families_and_styles_are_refused_rather_than_guessed() {
        let dirs = [bundled_fonts_dir()];
        for requested in [
            "Symbol",
            "NoSuchFont",
            "Helvetica-Extrabold",
            "HelveticaNeue-Bold",
        ] {
            assert!(
                find_system_font(requested, &dirs).is_none(),
                "{requested} must still refuse"
            );
        }
    }

    #[test]
    fn a_non_font_file_inspects_as_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text = tmp.path().join("readme.txt");
        touch(&text);
        assert!(inspect_font_file(&text).is_none());
        assert!(inspect_font_file(&tmp.path().join("missing.ttf")).is_none());
    }
}
