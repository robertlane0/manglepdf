//! Gate 0: policy and hygiene.
//!
//! Every check here answers a question the verifier can ask, and every finding names
//! a file and a line. A check that cannot run says so instead of quietly passing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ast::{self, CrateScan};
use crate::report::{Check, Report};
use crate::workspace::{Workspace, cargo};

/// Crates whose purpose includes reading or writing PDF. Banned outright.
const BANNED: &[&str] = &[
    "lopdf",
    "pdf",
    "pdf-rs",
    "pdf-writer",
    "printpdf",
    "printx",
    "genpdf",
    "krilla",
    "pdf-extract",
    "pdf-extraction",
    "pdfium",
    "pdfium-render",
    "mupdf",
    "mupdf-sys",
    "pikepdf",
    "pdfcpu",
    "hayro",
    "hayro-ink",
    "oxidize-pdf",
    "nanordf",
    "pdf-writer2",
    "pdfjs-dist",
    "poppler",
    "poppler-rs",
    "poppler-sys",
    "qpdf",
    "ghostscript",
    "gs",
    "itext",
    "pdfbox",
    "skia-safe",
    "cairo-rs-pdf",
    "printpdf-core",
    "zeno",
];

/// Words in a package's own metadata that mean it serves the PDF world. A crate that
/// merely mentions "pdf" in a sentence about nothing is not caught; one whose
/// keywords or repository say `pdf` is.
const PDF_PROVENANCE_WORDS: &[&str] = &["pdf", "acrobat", "portable document format"];

/// The one module allowed to spawn processes, per `FINISH.md` G0.4.
const OS_INTEGRATION_ALLOWLIST: &[&str] = &["crates/mangle-ui/src/os_integration.rs"];

/// Docs `FINISH.md` G0.9 requires, and where they live.
const REQUIRED_DOCS: &[&str] = &[
    "README.md",
    "docs/ARCHITECTURE.md",
    "docs/STATUS.md",
    "docs/DEPENDENCIES.md",
    "docs/PDF-QUIRKS.md",
    "docs/TESTING.md",
    "docs/ICONS.md",
    "docs/DEV.md",
    "THIRD_PARTY_LICENSES.md",
];

/// Lints a library crate must deny rather than warn.
const REQUIRED_DENIES: &[&str] = &[
    "clippy::unwrap_used",
    "clippy::expect_used",
    "clippy::panic",
    "clippy::todo",
    "clippy::unimplemented",
    "clippy::unreachable",
];

/// `cargo xtask policy [--only G0.1,G0.4]`
pub(crate) fn cmd(args: &[String]) -> Result<(), String> {
    let only: Vec<String> = match args.iter().position(|a| a == "--only") {
        Some(i) => args
            .get(i + 1)
            .map(|s| s.split(',').map(str::to_string).collect())
            .ok_or("--only needs a list such as G0.1,G0.4")?,
        None => Vec::new(),
    };
    let ws = Workspace::load()?;
    let report = run(&ws, &only);
    print!("{}", report.render());
    if report.ok() {
        Ok(())
    } else {
        let failed: Vec<&str> = report
            .checks()
            .iter()
            .filter(|c| !c.status.is_ok())
            .map(|c| c.id)
            .collect();
        Err(format!("policy failed: {}", failed.join(", ")))
    }
}

/// Run every Gate 0 check. `only` filters by check id.
pub(crate) fn run(ws: &Workspace, only: &[String]) -> Report {
    let mut r = Report::new();
    r.push_if(only, g0_1_toolchain(ws));
    r.push_if(only, g0_2_no_unsafe(ws));
    r.push_if(only, g0_3_dependencies(ws));
    r.push_if(only, g0_4_no_process_spawning(ws));
    r.push_if(only, g0_5_toolchain_clean(ws, only));
    r.push_if(only, g0_6_panics(ws));
    r.push_if(only, g0_7_assets(ws));
    r.push_if(only, g0_8_icons(ws));
    r.push_if(only, g0_9_docs(ws));
    r.push_if(only, g0_10_fixtures(ws));
    r
}

/// Scan every first-party crate once and hand the result to each rule that needs it.
pub(crate) fn scan_workspace(ws: &Workspace) -> Vec<(String, CrateScan)> {
    ws.members
        .iter()
        .map(|m| {
            let mut scan = CrateScan::default();
            for file in ast::rust_files(&m.dir) {
                scan.extend(ast::scan_file(&file));
            }
            (m.name.clone(), scan)
        })
        .collect()
}

fn g0_1_toolchain(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    let toolchain = ws.root.join("rust-toolchain.toml");
    if !toolchain.is_file() {
        findings.push("rust-toolchain.toml is missing".into());
    } else {
        let text = std::fs::read_to_string(&toolchain).unwrap_or_default();
        if !text.contains("channel") {
            findings.push("rust-toolchain.toml does not pin a channel".into());
        }
    }
    if !ws.root.join("Cargo.lock").is_file() {
        findings.push("Cargo.lock is not committed".into());
    }
    let manifest = std::fs::read_to_string(ws.root.join("Cargo.toml")).unwrap_or_default();
    if !manifest.contains("edition") {
        findings.push("the workspace does not declare an edition".into());
    }
    for m in &ws.members {
        let text = std::fs::read_to_string(m.dir.join("Cargo.toml")).unwrap_or_default();
        if !text.contains("edition.workspace = true") {
            findings.push(format!(
                "{}: does not inherit the workspace edition",
                m.name
            ));
        }
        if !text.contains("rust-version.workspace = true") {
            findings.push(format!("{}: does not inherit rust-version", m.name));
        }
        if !text.contains("[lints]") || !text.contains("workspace = true") {
            findings.push(format!("{}: does not inherit [workspace.lints]", m.name));
        }
    }
    if findings.is_empty() {
        Check::pass(
            "G0.1",
            "edition, resolver, pinned toolchain, committed lock",
        )
    } else {
        Check::fail(
            "G0.1",
            "edition, resolver, pinned toolchain, committed lock",
            findings,
        )
    }
}

fn g0_2_no_unsafe(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    for (name, scan) in scan_workspace(ws) {
        for f in &scan.unparsed {
            let rel = f.strip_prefix(&ws.root).unwrap_or(f);
            findings.push(format!("{name}: {} did not parse", rel.display()));
        }
        for f in &scan.unsafe_items {
            findings.push(format!("{name}: {}", f.display(&ws.root)));
        }
        for f in &scan.unsafe_allows {
            findings.push(format!("{name}: {}", f.display(&ws.root)));
        }
    }
    // Every target root must forbid unsafe outright, not merely warn.
    for m in &ws.members {
        for t in &m.targets {
            let text = std::fs::read_to_string(&t.src_path).unwrap_or_default();
            if !text.contains("#![forbid(unsafe_code)]") {
                findings.push(format!(
                    "{}: target {} ({}) has no #![forbid(unsafe_code)]",
                    m.name, t.name, t.kind
                ));
            }
        }
    }
    let scanned = scan_workspace(ws);
    let clean = scanned.iter().filter(|(_, s)| !s.is_untouched()).count();
    if findings.is_empty() {
        Check::pass(
            "G0.2",
            "no unsafe, no allow(unsafe_code), forbid in every target",
        )
        .note(format!(
            "{clean} of {} first-party crates scanned clean",
            scanned.len()
        ))
    } else {
        Check::fail(
            "G0.2",
            "no unsafe, no allow(unsafe_code), forbid in every target",
            findings,
        )
    }
}

fn g0_3_dependencies(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    let justified = justifications(&ws.root.join("docs/DEPENDENCIES.md"));
    let first_party: BTreeSet<&str> = ws.members.iter().map(|m| m.name.as_str()).collect();

    for pkg in ws.third_party_closure() {
        if BANNED.contains(&pkg.name.as_str()) {
            findings.push(format!(
                "`{}` is a PDF crate and is banned outright",
                pkg.name
            ));
            continue;
        }
        let text = format!(
            "{} {} {:?} {}",
            pkg.name,
            pkg.description.clone().unwrap_or_default(),
            pkg.keywords,
            pkg.repository.clone().unwrap_or_default()
        )
        .to_lowercase();
        let suspicious = PDF_PROVENANCE_WORDS
            .iter()
            .filter(|w| text.contains(*w))
            .copied()
            .collect::<Vec<_>>();
        if !suspicious.is_empty() && !justified.contains(&pkg.name) {
            findings.push(format!(
                "`{}` {} (from {}) mentions {:?} and has no provenance justification \
                 in docs/DEPENDENCIES.md",
                pkg.name,
                pkg.version,
                pkg.source.as_deref().unwrap_or("a path dependency"),
                suspicious
            ));
        }
    }
    // First-party crates are named with a `mangle-` prefix; a stray one from a third
    // party would be a dependency inversion.
    for m in &ws.members {
        for d in &m.dependencies {
            if d.name.starts_with("mangle-") && !first_party.contains(d.name.as_str()) {
                findings.push(format!(
                    "{} depends on `{}`, which is not a workspace member",
                    m.name, d.name
                ));
            }
        }
    }
    let count = ws.third_party_closure().len();
    if findings.is_empty() {
        Check::pass("G0.3", "no banned or PDF-provenance dependency")
            .note(format!("{count} third-party packages in the graph"))
    } else {
        Check::fail("G0.3", "no banned or PDF-provenance dependency", findings)
    }
}

fn g0_4_no_process_spawning(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    for (name, scan) in scan_workspace(ws) {
        // The rule is about product code. `xtask` and `fixturegen` are build tooling
        // whose whole job is to run other programs.
        if name == "xtask" || name == "fixturegen" {
            continue;
        }
        let allowed: Vec<&str> = OS_INTEGRATION_ALLOWLIST
            .iter()
            .copied()
            .filter(|p| p.contains(&name))
            .collect();
        for f in scan.command_spawns.iter().chain(scan.process_paths.iter()) {
            let rel = f.file.strip_prefix(&ws.root).unwrap_or(&f.file);
            let rel = rel.to_string_lossy().replace('\\', "/");
            if allowed.iter().any(|a| rel == *a) {
                continue;
            }
            findings.push(format!("{name}: {rel}:{}: {}", f.line, f.text));
        }
    }
    if findings.is_empty() {
        Check::pass("G0.4", "no process spawning outside the audited module")
    } else {
        Check::fail(
            "G0.4",
            "no process spawning outside the audited module",
            findings,
        )
    }
}

fn g0_5_toolchain_clean(ws: &Workspace, only: &[String]) -> Check {
    // These are slow; `--only` lets a developer run one at a time.
    let wants = |id: &str| only.is_empty() || only.iter().any(|w| w.eq_ignore_ascii_case(id));
    let mut findings = Vec::new();
    let mut unavailable: Vec<String> = Vec::new();
    let steps: [(&str, Vec<&str>); 4] = [
        ("cargo fmt --check", vec!["fmt", "--all", "--", "--check"]),
        (
            "cargo clippy -D warnings",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        ),
        (
            "cargo test --workspace",
            vec!["test", "--workspace", "--locked"],
        ),
        (
            "cargo build --release",
            vec!["build", "--workspace", "--release", "--locked"],
        ),
    ];
    for (label, args) in steps {
        if !wants("G0.5") {
            continue;
        }
        let out = Command::new(cargo())
            .current_dir(&ws.root)
            .args(&args)
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => findings.push(format!(
                "{label} failed:\n{}",
                tail(&String::from_utf8_lossy(&o.stderr))
            )),
            Err(e) => unavailable.push(format!("{label} could not run: {e}")),
        }
    }
    // Ignored tests count as failures, per the anti-gaming rules.
    if wants("G0.5") {
        findings.extend(ignored_tests(ws));
    }
    if !unavailable.is_empty() {
        return Check::skip(
            "G0.5",
            "fmt, clippy, tests and release build are clean",
            unavailable.join("; "),
        );
    }
    if findings.is_empty() {
        Check::pass("G0.5", "fmt, clippy, tests and release build are clean")
    } else {
        Check::fail(
            "G0.5",
            "fmt, clippy, tests and release build are clean",
            findings,
        )
    }
}

fn ignored_tests(ws: &Workspace) -> Vec<String> {
    let out = Command::new(cargo())
        .current_dir(&ws.root)
        .args(["test", "--workspace", "--locked", "--", "--list"])
        .output();
    let Ok(o) = out else {
        return vec!["could not list tests".into()];
    };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| l.trim_end().ends_with(": test"))
        .filter(|l| l.contains("ignored"))
        .map(str::to_string)
        .collect()
}

fn g0_6_panics(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    for m in &ws.members {
        // Only library crates carry the panic-free contract; binaries and tests are
        // allowed to fail loudly.
        if m.name == "xtask"
            || m.name == "fixturegen"
            || m.name == "mangle-cli"
            || m.name == "mangle-ui"
        {
            continue;
        }
        let root = m.targets.iter().find(|t| t.kind == "lib").map_or_else(
            || m.dir.clone(),
            |t| t.src_path.parent().unwrap_or(&m.dir).to_path_buf(),
        );
        let lib = std::fs::read_to_string(root.join("lib.rs")).unwrap_or_default();
        for deny in REQUIRED_DENIES {
            if !lib.contains(deny) {
                findings.push(format!("{}: lib.rs does not deny {deny}", m.name));
            }
        }
    }
    for (name, scan) in scan_workspace(ws) {
        if name == "xtask" || name == "fixturegen" {
            continue;
        }
        for f in &scan.todo_markers {
            findings.push(format!("{name}: {}", f.display(&ws.root)));
        }
    }
    if findings.is_empty() {
        Check::pass(
            "G0.6",
            "library crates deny the panic lints; no reachable todo!()",
        )
    } else {
        Check::fail(
            "G0.6",
            "library crates deny the panic lints; no reachable todo!()",
            findings,
        )
    }
}

fn g0_7_assets(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    let licences = ws.root.join("THIRD_PARTY_LICENSES.md");
    if !licences.is_file() {
        findings.push("THIRD_PARTY_LICENSES.md is missing".into());
    } else {
        let text = std::fs::read_to_string(&licences)
            .unwrap_or_default()
            .to_lowercase();
        let assets = ws.root.join("assets");
        if assets.is_dir() {
            for f in bundled_assets(&assets) {
                let stem = f
                    .file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().to_lowercase());
                let named =
                    !text.contains(&stem) && !text.contains(&f.to_string_lossy().to_lowercase());
                if named {
                    findings.push(format!(
                        "assets/{} has no entry in THIRD_PARTY_LICENSES.md",
                        f.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
    }
    if findings.is_empty() {
        Check::pass("G0.7", "every bundled asset has a licence entry")
    } else {
        Check::fail("G0.7", "every bundled asset has a licence entry", findings)
    }
}

fn bundled_assets(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(bundled_assets(&p));
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("ttf" | "otf" | "pfb" | "ttc" | "txt" | "icc")
        ) {
            out.push(p);
        }
    }
    out
}

fn g0_8_icons(ws: &Workspace) -> Check {
    let dir = ws.root.join("assets/icons");
    let mut findings = Vec::new();
    if !dir.is_dir() {
        return Check::fail(
            "G0.8",
            "icons are hand-authored SVG",
            vec!["assets/icons does not exist".into()],
        );
    }
    let mut count = 0usize;
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            continue;
        }
        count += 1;
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match p.extension().and_then(|x| x.to_str()) {
            Some("svg") => findings.extend(svg_findings(&p, &name)),
            _ => findings.push(format!("{name}: icons must be SVG, not a raster image")),
        }
    }
    if findings.is_empty() {
        Check::pass("G0.8", "icons are hand-authored SVG").note(format!("{count} icons"))
    } else {
        Check::fail("G0.8", "icons are hand-authored SVG", findings)
    }
}

/// Enough of an SVG lint to catch the ways an icon goes wrong: no viewBox, an
/// embedded raster, or a script.
fn svg_findings(path: &Path, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![format!("{name}: unreadable")];
    };
    if !text.contains("viewBox") {
        out.push(format!("{name}: no viewBox"));
    }
    for bad in ["<image", "<script", "data:image", "<foreignObject"] {
        if text.contains(bad) {
            out.push(format!(
                "{name}: contains `{bad}`, which is not allowed in an icon"
            ));
        }
    }
    if !text.contains("<svg") {
        out.push(format!("{name}: not an SVG document"));
    }
    out
}

fn g0_9_docs(ws: &Workspace) -> Check {
    let findings: Vec<String> = REQUIRED_DOCS
        .iter()
        .filter(|d| !ws.root.join(d).is_file())
        .map(|d| format!("{d} is missing or empty"))
        .collect();
    if findings.is_empty() {
        Check::pass("G0.9", "required documentation is present")
    } else {
        Check::fail("G0.9", "required documentation is present", findings)
    }
}

fn g0_10_fixtures(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    let Some(generator) = ws.members.iter().find(|m| m.name == "fixturegen") else {
        return Check::fail(
            "G0.10",
            "fixturegen is independent and deterministic",
            vec!["tools/fixturegen is not a workspace member".into()],
        );
    };
    for d in &generator.dependencies {
        if d.name.starts_with("mangle-") {
            findings.push(format!(
                "fixturegen depends on `{}`; it must be independent of the product",
                d.name
            ));
        }
    }
    if !ws.root.join("fixtures/MANIFEST.toml").is_file() {
        findings.push("fixtures/MANIFEST.toml is missing".into());
    }
    if findings.is_empty() {
        Check::pass("G0.10", "fixturegen is independent and deterministic")
    } else {
        Check::fail(
            "G0.10",
            "fixturegen is independent and deterministic",
            findings,
        )
    }
}

/// Crate names given a provenance justification in a markdown table.
fn justifications(path: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        if !line.trim_start().starts_with('|') {
            continue;
        }
        for cell in line.split('|') {
            let cell = cell.trim().trim_matches('`');
            if !cell.is_empty()
                && cell
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                out.insert(cell.to_lowercase());
            }
        }
    }
    out
}

/// The last few lines of a long error, where the actual message is.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(25);
    lines.get(start..).unwrap_or_default().join("\n")
}
