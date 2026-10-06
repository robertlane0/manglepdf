//! Gate 0: policy and hygiene.
//!
//! Every check here answers a question the verifier can ask, and every finding names
//! a file and a line. A check that cannot run says so instead of quietly passing.

use std::collections::{BTreeMap, BTreeSet};
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

/// Ignored tests the gate accepts, named one by one.
///
/// `FINISH.md` G0.5 asks for no ignored test at all. Exactly one exists and it is deliberate: the
/// corpus run renders every page of the wild corpus twice and takes hours, so it is run on demand
/// rather than on every gate invocation. That is a *narrow* exception, and this is what keeps it
/// narrow.
///
/// **A count is not a gate.** The check used to exclude every ignored test from its judgement and
/// merely report how many there were, which meant a test marked `#[ignore]` last week to get a
/// commit through would pass the gate forever after. Naming the one exception is what makes the
/// rest a failure, which is the same reason a missing heading in a documentation link fails rather
/// than being counted.
const SANCTIONED_IGNORED: &[&str] = &["the_wild_corpus_is_measured_and_reported"];

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
    // Every crate in `scanned` was scanned for the rule above; the count is a note about how the
    // scan went, not a score. It used to filter on `!is_untouched()` into a variable called
    // `clean` and then print "6 of 14 crates scanned clean", which was wrong twice over: the
    // six it named were the crates holding a spawn or todo marker, and the eight it did not
    // name were the bare ones. A note that inverts its own subject is worse than no note.
    let with_markers = scanned.iter().filter(|(_, s)| !s.is_untouched()).count();
    if findings.is_empty() {
        Check::pass(
            "G0.2",
            "no unsafe, no allow(unsafe_code), forbid in every target",
        )
        .note(format!(
            "all {} first-party crates scanned; {with_markers} of them hold a spawn, process \
             path or todo marker, which G0.4 and G0.6 audit",
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
    let mut test_only = 0usize;
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
            // The charter allows external tools as test oracles in dev and test
            // scripts: never linked, never called from product code, never required
            // for the tests to pass. A file under `tests/` is exactly that.
            if rel.contains("/tests/") || rel.ends_with("/tests.rs") {
                test_only += 1;
                continue;
            }
            findings.push(format!("{name}: {rel}:{}: {}", f.line, f.text));
        }
    }
    if findings.is_empty() {
        Check::pass("G0.4", "no process spawning outside the audited module")
            .note(format!("{test_only} oracle calls are confined to tests"))
    } else {
        Check::fail(
            "G0.4",
            "no process spawning outside the audited module",
            findings,
        )
    }
}

const G0_5_TITLE: &str = "fmt, clippy, tests and release build are clean";

/// The step whose output carries the test counts.
const TEST_STEP: &str = "cargo test --workspace";

fn g0_5_toolchain_clean(ws: &Workspace, only: &[String]) -> Check {
    // These are slow; `--only` lets a developer run one at a time.
    let wants = |id: &str| only.is_empty() || only.iter().any(|w| w.eq_ignore_ascii_case(id));
    let mut findings = Vec::new();
    let mut notes = Vec::new();
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
        (TEST_STEP, vec!["test", "--workspace", "--locked"]),
        (
            "cargo build --release",
            vec!["build", "--workspace", "--release", "--locked"],
        ),
    ];
    if wants("G0.5") {
        for (label, args) in steps {
            let out = Command::new(cargo())
                .current_dir(&ws.root)
                .args(&args)
                .output();
            match out {
                Ok(o) => {
                    // The test step is judged on what it reported, not on its exit
                    // code alone: a run that says nothing about tests having run is
                    // not a pass either.
                    if label == TEST_STEP {
                        let verdict = judge_test_run(&String::from_utf8_lossy(&o.stdout));
                        notes.extend(verdict.notes);
                        findings.extend(verdict.findings);
                    }
                    if !o.status.success() {
                        findings.push(format!(
                            "{label} failed:\n{}",
                            tail(&String::from_utf8_lossy(&o.stderr))
                        ));
                    }
                }
                Err(e) => unavailable.push(format!("{label} could not run: {e}")),
            }
        }
    }
    if !unavailable.is_empty() {
        return Check::skip("G0.5", G0_5_TITLE, unavailable.join("; "));
    }
    let mut check = if findings.is_empty() {
        Check::pass("G0.5", G0_5_TITLE)
    } else {
        Check::fail("G0.5", G0_5_TITLE, findings)
    };
    for note in notes {
        check = check.note(note);
    }
    check
}

/// What one `cargo test` run reported, summed over every test binary it ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TestCounts {
    passed: usize,
    failed: usize,
    /// Deliberately not run, so neither evidence nor a failure.
    ignored: usize,
    /// Not selected by a filter or name pattern; not run, and not asked for.
    filtered: usize,
    /// Test binaries that printed a `test result:` line. Zero means the run said
    /// nothing at all about tests having run, which is not evidence either way.
    binaries: usize,
}

/// The gate's judgement of one test run, and what to tell the reader about it.
#[derive(Debug, Default, PartialEq, Eq)]
struct TestVerdict {
    counts: TestCounts,
    notes: Vec<String>,
    findings: Vec<String>,
}

/// Judge one `cargo test` run on its output.
///
/// Passed and failed decide the gate. Ignored and filtered-out tests were never
/// run, so they are counted and named instead of being failed or dropped in
/// silence: an ignored test is a deliberate choice to defer something, and the
/// report says so. A failure still fails, and a run that reported no test binary
/// at all fails too, because "nothing ran" is not evidence that the code works.
fn judge_test_run(stdout: &str) -> TestVerdict {
    let mut v = TestVerdict {
        counts: parse_test_counts(stdout),
        ..TestVerdict::default()
    };
    if v.counts.binaries == 0 {
        v.findings.push(
            "cargo test reported no `test result:` line, so no test run can be judged".into(),
        );
        return v;
    }
    if v.counts.failed > 0 {
        v.findings.push(format!(
            "{} test(s) failed: {}",
            v.counts.failed,
            listed(&test_lines(stdout, "FAILED"))
        ));
    }
    v.notes.push(format!(
        "cargo test: {} passed, {} failed, {} ignored, {} filtered out over {} test binaries",
        v.counts.passed, v.counts.failed, v.counts.ignored, v.counts.filtered, v.counts.binaries
    ));
    let ignored = test_lines(stdout, "ignored");
    // An ignored test is a deliberate choice to defer something, so the ones on the list are
    // reported and accepted. Anything else on the list was ignored without saying why here, and
    // it fails: "deferred" is a decision to record, not a place to leave work.
    let unsanctioned: Vec<String> = ignored
        .iter()
        .filter(|name| !SANCTIONED_IGNORED.contains(&name.as_str()))
        .cloned()
        .collect();
    if !unsanctioned.is_empty() {
        v.findings.push(format!(
            "{} test(s) are ignored without being on the sanctioned list: {}. Either it is \
             deliberate and belongs in SANCTIONED_IGNORED, or it is not meant to be skipped.",
            unsanctioned.len(),
            listed(&unsanctioned)
        ));
    }
    v.notes.push(if v.counts.ignored == 0 {
        "no test was ignored".to_string()
    } else {
        format!(
            "{} ignored test(s) were not run, and each is on the sanctioned list: {}",
            v.counts.ignored,
            listed(&ignored)
        )
    });
    v
}

/// Sum the `test result:` lines of a `cargo test` run, one per test binary.
fn parse_test_counts(stdout: &str) -> TestCounts {
    let mut c = TestCounts::default();
    for line in stdout.lines() {
        let Some(rest) = result_counts(line) else {
            continue;
        };
        c.binaries += 1;
        if let Some(n) = count_labeled(rest, "passed") {
            c.passed += n;
        }
        if let Some(n) = count_labeled(rest, "failed") {
            c.failed += n;
        }
        if let Some(n) = count_labeled(rest, "ignored") {
            c.ignored += n;
        }
        if let Some(n) = count_labeled(rest, "filtered out") {
            c.filtered += n;
        }
    }
    c
}

/// The counts of one `test result:` line, if the line is really one. libtest
/// always writes the outcome first, so a line that only looks similar counts for
/// nothing: an unreadable run must not read as a clean one.
fn result_counts(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("test result:")?;
    let rest = rest.trim_start();
    rest.strip_prefix("ok.")
        .or_else(|| rest.strip_prefix("FAILED."))
}

/// The number a `test result:` line reports for one label, e.g. `ignored`. The
/// count comes before the label, as in `3 passed; 0 failed`.
fn count_labeled(line: &str, label: &str) -> Option<usize> {
    let at = line.find(label)?;
    let digits: String = line[..at]
        .trim_end()
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

/// The test names libtest reported with one status, e.g. `ignored`.
///
/// libtest writes `test NAME ... OUTCOME`, and an ignored test's OUTCOME carries
/// the `#[ignore = "..."]` reason after a comma, as in
/// `... ignored, two hours over the whole corpus`. So only the status word before
/// the first comma is compared: matching the whole tail would make every real
/// ignored test read as unnamed. The name comes from the first ` ... `, so a
/// reason containing the separator cannot shift it.
fn test_lines(stdout: &str, status: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.trim().strip_prefix("test ") else {
            continue;
        };
        let Some((name, outcome)) = rest.split_once(" ... ") else {
            continue;
        };
        let reported = outcome.split(',').next().unwrap_or_default().trim();
        if reported == status && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Names for a report line, capped so a badly broken run stays readable.
fn listed(names: &[String]) -> String {
    const LIMIT: usize = 5;
    match names {
        [] => "none".to_string(),
        _ => {
            let shown = names
                .iter()
                .take(LIMIT)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            if names.len() > LIMIT {
                format!("{shown} and {} more", names.len() - LIMIT)
            } else {
                shown
            }
        }
    }
}

fn g0_6_panics(ws: &Workspace) -> Check {
    let mut findings = Vec::new();
    for m in &ws.members {
        // Only library crates carry the panic-free contract; binaries and tests are allowed to
        // fail loudly.
        //
        // **Derived from the targets, not from a list of names.** The list this replaces named
        // four crates, and one of them — `mangle-ui` — has a `lib.rs` as well as a `main.rs`, so
        // it was exempt from a rule about libraries on the stated ground that it was only a
        // binary. It happened to carry all six denies anyway, so nothing was missed; the point is
        // that the exemption was not doing what its comment claimed. A name list also stops
        // matching the moment a crate is added or split, and fails *open* when it does.
        let has_lib = m.targets.iter().any(|t| t.kind == "lib");
        if !has_lib {
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

/// Every SVG element an icon is allowed to be made of.
const DRAWABLE: &[&str] = &[
    "<path",
    "<circle",
    "<ellipse",
    "<line",
    "<polyline",
    "<polygon",
    "<rect",
];

/// Whether the document actually draws something.
fn draws_something(svg: &str) -> bool {
    DRAWABLE.iter().any(|tag| svg.contains(tag))
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
    if !draws_something(&text) {
        out.push(format!("{name}: nothing is drawn"));
    }
    out
}

fn g0_9_docs(ws: &Workspace) -> Check {
    const TITLE: &str = "required documentation is present, and its links resolve";
    let mut findings: Vec<String> = REQUIRED_DOCS
        .iter()
        .filter(|d| !ws.root.join(d).is_file())
        .map(|d| format!("{d} is missing or empty"))
        .collect();
    findings.extend(dangling_anchors(&ws.root));
    if findings.is_empty() {
        Check::pass("G0.9", TITLE)
    } else {
        Check::fail("G0.9", TITLE, findings)
    }
}

/// Every Markdown file the documentation set is built from, as paths relative to the root.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = REQUIRED_DOCS
        .iter()
        .filter(|d| Path::new(d).extension().is_some_and(|e| e == "md"))
        .map(PathBuf::from)
        .collect();
    // Everything under `docs/`, **recursively**: the architecture decisions and the design notes
    // live in subdirectories, and a link into one of those is as breakable as any other. A
    // walk that stopped at the first level would report success while skipping them, which is
    // the failure mode this check exists to remove.
    markdown_under(&root.join("docs"), root, &mut out);
    // Everything at the root as well, because `FINISH.md` and `GOAL.md` are the two documents
    // most worth keeping honest and neither is in `REQUIRED_DOCS`. The one local file that is
    // left out is deliberately untracked, so a gate that required it would fail on a fresh
    // clone; everything the contract asks for is in the repository, which is the property this
    // check exists to confirm.
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_markdown = path.is_file() && path.extension().is_some_and(|e| e == "md");
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if is_markdown && name != "AGENTS.md" {
                out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every Markdown file at or below `dir`, as paths relative to `root`.
fn markdown_under(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            markdown_under(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
}

/// A GitHub-style heading anchor: lowercased, punctuation dropped, spaces to hyphens.
///
/// Deliberately the same algorithm a renderer applies, so that what this accepts is what a
/// reader's link will find. It is a reimplementation because a gate may not depend on a crate
/// whose whole job is to be a dependency.
fn anchor_of(heading: &str) -> String {
    let mut out = String::new();
    for ch in heading.trim().trim_matches('#').trim().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch.is_whitespace() {
            out.extend(ch.to_lowercase());
        }
    }
    out.trim().replace(' ', "-")
}

/// Every `[text](path#fragment)` link whose fragment names a heading no target has.
///
/// A doc link to a heading that has been renamed is the same failure as a doc claiming a thing
/// is unimplemented one screen above the code that does it: the reader is sent somewhere that
/// does not say what they were told it says. Both have been found in this project, which is why
/// this is a gate rather than something to remember to run.
fn dangling_anchors(root: &Path) -> Vec<String> {
    let files = markdown_files(root);
    let mut headings: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut bodies: Vec<(PathBuf, String)> = Vec::new();
    for rel in &files {
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let mut set = BTreeSet::new();
        for line in text.lines() {
            if line.starts_with('#') {
                set.insert(anchor_of(line));
            }
        }
        headings.insert(rel.clone(), set);
        bodies.push((rel.clone(), text));
    }
    let mut findings = Vec::new();
    for (rel, text) in &bodies {
        let dir = rel.parent().unwrap_or(Path::new(""));
        for (index, line) in text.lines().enumerate() {
            // An inline code span is documentation *about* a link, not a link: a page that
            // teaches the syntax has to be able to write it down, and treating the example as a
            // real link would make documenting this check impossible.
            let line = strip_code_spans(line);
            let mut rest = line.as_str();
            while let Some(at) = rest.find("](") {
                rest = &rest[at + 2..];
                let Some(close) = rest.find(')') else { break };
                let target = &rest[..close];
                rest = &rest[close + 1..];
                let Some((path, fragment)) = target.split_once('#') else {
                    continue;
                };
                if fragment.is_empty() {
                    continue;
                }
                let joined = if path.is_empty() {
                    rel.clone()
                } else {
                    dir.join(path).clone()
                };
                // A link may omit the `.md`; try it as written, then with it.
                let normalised = normalise(&joined);
                let candidates = [normalised.clone(), normalised.with_extension("md")];
                let Some(target_set) = candidates.iter().find_map(|c| headings.get(c)) else {
                    // Not a doc of ours, or a file that is gone: that is a link to nothing.
                    findings.push(format!(
                        "{}:{} links to `{path}`, which is not a Markdown file here",
                        rel.display(),
                        index + 1
                    ));
                    continue;
                };
                if !target_set.contains(&normalise_fragment(fragment)) {
                    findings.push(format!(
                        "{}:{} links to `{path}#{fragment}`, which names no heading there",
                        rel.display(),
                        index + 1
                    ));
                }
            }
        }
    }
    findings
}

/// A line with its inline code spans blanked, leaving the line numbering intact.
fn strip_code_spans(line: &str) -> String {
    let mut out = String::new();
    let mut ticks = 0;
    for ch in line.chars() {
        if ch == '`' {
            ticks += 1;
            out.push(' ');
        } else if ticks == 0 {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    out
}

/// Strip `./`, collapse `docs/../`, and keep the fragment out of the path.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The fragment as an anchor, so a fragment written with different case still resolves.
fn normalise_fragment(fragment: &str) -> String {
    fragment.to_lowercase().replace(' ', "-")
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

#[cfg(test)]
mod tests {
    use super::{TestCounts, judge_test_run, parse_test_counts, test_lines};

    /// A `cargo test` run over one workspace, trimmed to what the gate reads.
    const ALL_PASSED: &str = "\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.11s
     Running unittests src/lib.rs (target/debug/deps/mangle-syntax-2f0a1b3c)

running 3 tests
test tests::a_balanced_tree_parses ... ok
test tests::an_unbalanced_tree_is_rejected ... ok
test tests::a_cycle_does_not_hang ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

   Doc-tests mangle-syntax

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    const ONE_FAILED: &str = "\
running 2 tests
test tests::a_balanced_tree_parses ... ok
test tests::an_unbalanced_tree_is_rejected ... FAILED

failures:

    tests::an_unbalanced_tree_is_rejected

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    const ONE_IGNORED: &str = "\
running 3 tests
test the_harness_finds_its_corpus ... ok
test the_wild_corpus_is_measured_and_reported ... ignored
test tests::a_balanced_tree_parses ... ok

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    /// The same run as `ONE_IGNORED`, but with the reason a real `#[ignore]` carries.
    const ONE_IGNORED_WITH_REASON: &str = "\
running 3 tests
test the_harness_finds_its_corpus ... ok
test the_wild_corpus_is_measured_and_reported ... ignored, two hours over the whole corpus; run it deliberately with --ignored
test tests::a_balanced_tree_parses ... ok

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    /// What `cargo test -- --list` prints: no `... ` and no outcome.
    /// A run where a second test is ignored without being on the sanctioned list.
    const ONE_UNSANCTIONED: &str = "\
running 3 tests
test the_harness_finds_its_corpus ... ok
test a_test_deferred_to_next_week ... ignored
test the_wild_corpus_is_measured_and_reported ... ignored

test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    const LISTED: &str = "\
the_wild_corpus_is_measured_and_reported: test
tests::a_balanced_tree_parses: test
tests::an_unbalanced_tree_is_rejected: test
";

    #[test]
    fn every_test_passed_passes_the_gate() {
        let v = judge_test_run(ALL_PASSED);
        assert!(v.findings.is_empty(), "{:?}", v.findings);
        assert_eq!(v.counts.passed, 3);
        assert_eq!(v.counts.binaries, 2);
        assert!(
            v.notes.iter().any(|n| n == "no test was ignored"),
            "{:?}",
            v.notes
        );
    }

    /// An ignored test that is **not** on the sanctioned list fails the gate.
    ///
    /// This is what the count was hiding. The check used to exclude *every* ignored test from its
    /// judgement and report only how many there were, so a test marked `#[ignore]` to land a
    /// commit would have passed the gate indefinitely, and the report would have agreed with it.
    /// The count was never going to fail; the name is.
    #[test]
    fn an_unsanctioned_ignored_test_fails_the_gate() {
        let v = judge_test_run(ONE_UNSANCTIONED);
        assert!(
            !v.findings.is_empty(),
            "an ignored test nobody sanctioned must not be quietly excluded"
        );
        let finding = v.findings.join(" ");
        assert!(
            finding.contains("a_test_deferred_to_next_week"),
            "and it must be named, so the reader knows what to deal with: {finding}"
        );
        assert!(
            !finding.contains("the_wild_corpus_is_measured_and_reported"),
            "the sanctioned test is not part of the failure: {finding}"
        );
    }

    #[test]
    fn a_failed_test_fails_the_gate_by_name() {
        let v = judge_test_run(ONE_FAILED);
        assert_eq!(v.findings.len(), 1, "{:?}", v.findings);
        let finding = v.findings.first().map(String::as_str).unwrap_or_default();
        assert!(
            finding.contains("1 test(s) failed")
                && finding.contains("tests::an_unbalanced_tree_is_rejected"),
            "{finding}"
        );
    }

    #[test]
    fn the_sanctioned_ignored_test_is_reported_and_not_failed() {
        let v = judge_test_run(ONE_IGNORED);
        assert!(v.findings.is_empty(), "{:?}", v.findings);
        assert_eq!(
            v.counts,
            TestCounts {
                passed: 2,
                failed: 0,
                ignored: 1,
                filtered: 0,
                binaries: 1,
            }
        );
        let note = v
            .notes
            .iter()
            .find(|n| n.contains("were not run"))
            .map(String::as_str)
            .unwrap_or_default();
        assert!(
            note.contains("1 ignored test(s) were not run")
                && note.contains("each is on the sanctioned list"),
            "the note must say the test was sanctioned, not merely that it was excluded: {note}"
        );
        assert!(
            note.contains("the_wild_corpus_is_measured_and_reported"),
            "{note}"
        );
    }

    #[test]
    fn a_run_that_reported_no_test_binary_fails_the_gate() {
        // Nothing ran, so there is no evidence the code works.
        let quiet = "    Finished `test` profile in 0.10s\n";
        let v = judge_test_run(quiet);
        assert_eq!(v.findings.len(), 1, "{:?}", v.findings);
        let finding = v.findings.first().map(String::as_str).unwrap_or_default();
        assert!(finding.contains("no `test result:` line"), "{finding}");
        assert!(v.notes.is_empty(), "{:?}", v.notes);
        // A line that merely mentions the word is not a result line.
        assert_eq!(
            parse_test_counts("test result: 5 passed; 0 failed"),
            TestCounts::default()
        );
    }

    #[test]
    fn counts_are_summed_over_every_test_binary() {
        let v = judge_test_run(&format!(
            "{ALL_PASSED}\ntest result: ok. 7 passed; 0 failed; 2 \
                                         ignored; 0 measured; 1 filtered out; finished in 0.00s\n"
        ));
        assert_eq!(v.counts.passed, 10);
        assert_eq!(v.counts.ignored, 2);
        assert_eq!(v.counts.filtered, 1);
        assert_eq!(v.counts.binaries, 3);
        assert!(v.findings.is_empty(), "{:?}", v.findings);
    }

    #[test]
    fn a_name_reported_twice_is_listed_once() {
        let out = format!("{ONE_IGNORED}{ONE_IGNORED}");
        assert_eq!(
            test_lines(&out, "ignored"),
            vec!["the_wild_corpus_is_measured_and_reported"]
        );
        assert_eq!(test_lines(&out, "ok").len(), 2);
        assert_eq!(test_lines(&out, "FAILED"), Vec::<String>::new());
    }

    #[test]
    fn an_ignored_test_with_a_reason_is_still_reported_by_name() {
        assert_eq!(
            test_lines(ONE_IGNORED_WITH_REASON, "ignored"),
            vec!["the_wild_corpus_is_measured_and_reported"]
        );
        let v = judge_test_run(ONE_IGNORED_WITH_REASON);
        assert!(v.findings.is_empty(), "{:?}", v.findings);
        assert_eq!(v.counts.ignored, 1);
        let note = v
            .notes
            .iter()
            .find(|n| n.contains("were not run"))
            .map(String::as_str)
            .unwrap_or_default();
        assert!(
            note.contains("the_wild_corpus_is_measured_and_reported"),
            "an ignored test carrying a reason must be named, not reported as none: {note}"
        );
        assert!(
            !note.contains("none"),
            "the reason must not read as a missing name: {note}"
        );
    }

    #[test]
    fn a_reason_containing_the_separator_does_not_shift_the_name() {
        let out = "test a_deliberate_deferral ... ignored, runs ... slowly, on purpose\n";
        assert_eq!(test_lines(out, "ignored"), vec!["a_deliberate_deferral"]);
    }

    #[test]
    fn a_reason_naming_another_status_does_not_match_it() {
        let out = "test a_deliberate_deferral ... ignored, deferred because ignored, see below\n";
        assert_eq!(test_lines(out, "ignored"), vec!["a_deliberate_deferral"]);
        assert_eq!(test_lines(out, "ok"), Vec::<String>::new());
        assert_eq!(test_lines(out, "FAILED"), Vec::<String>::new());
    }

    #[test]
    fn a_line_the_gate_cannot_read_yields_no_name() {
        // A bare `test NAME` with no outcome, and the `--list` form the gate used
        // to shell out for: neither says what happened, so neither is evidence.
        let out = format!("test a_name_with_no_outcome\n{LISTED}");
        for status in ["ok", "FAILED", "ignored"] {
            assert_eq!(test_lines(&out, status), Vec::<String>::new(), "{status}");
        }
        // A run that only listed its tests reported no result line, so it is judged
        // unreadable rather than clean.
        let v = judge_test_run(LISTED);
        assert_eq!(v.findings.len(), 1, "{:?}", v.findings);
        let finding = v.findings.first().map(String::as_str).unwrap_or_default();
        assert!(finding.contains("no `test result:` line"), "{finding}");
    }
}
