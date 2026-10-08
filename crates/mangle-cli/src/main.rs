//! The headless surface: what the tests and the acceptance harness drive.
//!
//! Every command here is a question about a file with a definite answer, and every
//! answer is printed in a form a script can check. Nothing about the product depends
//! on this binary existing; it exists so that "what does ManglePDF think of this file?"
//! can be asked without a window.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::Path;

use mangle_syntax::{Document, OpenOptions, SaveMode, SaveOptions};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        usage();
        return std::process::ExitCode::FAILURE;
    };
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();

    let result = match command.as_str() {
        "info" => info(&rest),
        "pages" => pages(&rest),
        "check" => check(&rest),
        "render" => render(&rest),
        "extract" => extract(&rest),
        "save" => save(&rest),
        "edit" => edit(&rest),
        "--help" | "-h" | "help" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command `{other}`")),
    };

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("manglepdf-cli: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!(
        "\
manglepdf-cli <command> <file.pdf> [options]

  info      [-json]        what the file is: version, pages, encryption, repair
  pages                  one line per page: index, size, rotation
  check                  validate structure; exit 0 clean, 1 repaired, 2 unusable
  render   -o FILE       rasterize a page (not implemented yet)
  extract  [-page N]     the text of a page
  save     [-o FILE] [--incremental] [--full] [--reset-prefs]
  edit     [options]     select and change a page object; save the result
                            --list                 what is on the page, one line each
                            --page N               which page (default 1)
                            --object I             which object (see --list)
                            --move dx dy           move it by dx, dy points
                            --scale s              scale it about its centre
                            --delete               remove it
                            --colour r,g,b         recolour it (0-1 each)
                            --forward|--backward   arrange it past the object it overlaps
                            --to-front|--to-back   arrange it to the ends of the page
                            -o FILE                where to write (default beside the input)
"
    );
}

fn open(path: &str) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    Document::open(bytes, OpenOptions::default()).map_err(|e| format!("{path}: {e}"))
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Select an object on a page, change it, and save the file.
///
/// This is the whole editing loop on one command, and it exists so the loop can be driven from a
/// script and from the acceptance harness without a window. The printout names what was selected,
/// what changed, and what the save rewrote, because an edit whose effect cannot be read is an edit
/// nobody can check.
fn edit(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("edit needs a file")?;
    let doc = open(path)?;
    let cat = doc.catalog().map_err(|e| format!("{path}: {e}"))?;
    let root = cat
        .get("Pages")
        .and_then(mangle_syntax::object::Object::as_ref_id)
        .ok_or("the page tree root is missing")?;
    let tree = mangle_doc::PageTree::build(&doc, root).map_err(|e| format!("{path}: {e}"))?;
    let index = value(args, "--page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let page = tree.get(index).ok_or_else(|| {
        format!(
            "page {index} is not one of this file's {} pages",
            tree.len()
        )
    })?;
    let resources = page_resources(&doc, page);
    let stream = page.decoded_contents(&doc);
    let run = mangle_content::interp::run_with(
        &mangle_content::ContentStream::parse(&stream),
        &resources,
    );
    let model = mangle_edit::PageModel::build(&run.records);

    let mut out = std::io::stdout().lock();
    if flag(args, "--list") {
        for (i, o) in model.objects().iter().enumerate() {
            let _ = writeln!(
                out,
                "{i}\t{kind:?}\t{x0:.2} {y0:.2} {x1:.2} {y1:.2}\t{spans} span(s)\t{form}",
                kind = o.kind,
                x0 = o.bounds.x0,
                y0 = o.bounds.y0,
                x1 = o.bounds.x1,
                y1 = o.bounds.y1,
                spans = o.spans.len(),
                form = o.form.as_deref().unwrap_or("page"),
            );
        }
        let _ = writeln!(
            out,
            "{} object(s) on page {}",
            model.objects().len(),
            index + 1
        );
        return Ok(());
    }

    let which = value(args, "--object")
        .and_then(|v| v.parse::<usize>().ok())
        .ok_or("--object needs an index; --list prints them")?;
    let object = model.objects().get(which).ok_or_else(|| {
        format!(
            "object {which} is not one of the page's {} objects",
            model.objects().len()
        )
    })?;

    // Arrange is a different kind of command from the others: it has to know the object's
    // neighbours to know where to move it, so it is not a change to one object.
    let arrange = if flag(args, "--forward") {
        Some(mangle_edit::Arrange::Forward)
    } else if flag(args, "--backward") {
        Some(mangle_edit::Arrange::Backward)
    } else if flag(args, "--to-front") {
        Some(mangle_edit::Arrange::ToFront)
    } else if flag(args, "--to-back") {
        Some(mangle_edit::Arrange::ToBack)
    } else {
        None
    };

    // A **session**, not a one-shot edit: the loop is the same one a window drives, and the
    // history is what makes an undo exact. A command line cannot hold a session between two
    // invocations, so this reports the depth rather than pretending there is a stack to pop.
    let mut session =
        mangle_edit::Editor::open(doc, page, &resources).map_err(|e| format!("{path}: {e}"))?;
    let size_before = session.document().bytes().len();
    let original = session.document().bytes().to_vec();
    let report = match arrange {
        Some(arrange) => session
            .arrange(which, arrange)
            .map_err(|e| format!("{path}: {e}"))?,
        None => {
            let change = change_from_args(args, object)?;
            session
                .apply(which, &change)
                .map_err(|e| format!("{path}: {e}"))?
        }
    };
    let save = session.save().map_err(|e| format!("{path}: {e}"))?;
    let (back, forward) = session.depth();
    let history = session.labels().join(", ");

    // Write beside the target and rename, so an interrupted save never leaves a half-written file
    // where the real one was.
    let out_path = value(args, "-o").unwrap_or_else(|| format!("{path}.edited.pdf"));
    let target = Path::new(&out_path);
    let temp = target.with_extension("pdf.part");
    std::fs::write(&temp, &save.bytes).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, target).map_err(|e| format!("{}: {e}", target.display()))?;

    let _ = writeln!(
        out,
        "selected {which} on page {}, a {kind:?} at {x0:.2} {y0:.2}",
        index + 1,
        kind = object.kind,
        x0 = object.bounds.x0,
        y0 = object.bounds.y0,
    );
    let _ = writeln!(out, "did       {report}");
    let _ = writeln!(
        out,
        "history   {back} step(s) back, {forward} forward: {history}"
    );
    let _ = writeln!(out, "wrote     {out_path}");
    let _ = writeln!(
        out,
        "bytes     {}, {} of them new",
        save.bytes.len(),
        save.bytes.len() - size_before
    );
    let _ = writeln!(
        out,
        "rewrote   {} content stream object(s){}",
        save.rewritten.len(),
        if save.re_encoded {
            ", re-encoded in the file's own filter"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "appended  {}",
        if save.appended_only(&original) {
            "the file's existing bytes are untouched"
        } else {
            "NO: the file was rewritten, which a save must never be"
        }
    );

    // Reopen what was written and check the edit is in it, which is the only evidence that counts.
    let reopened = Document::open(save.bytes.clone(), OpenOptions::default())
        .map_err(|e| format!("{out_path}: {e}"))?;
    let cat2 = reopened.catalog().map_err(|e| format!("{out_path}: {e}"))?;
    let root2 = cat2
        .get("Pages")
        .and_then(mangle_syntax::object::Object::as_ref_id)
        .ok_or("the saved file has no page tree")?;
    let tree2 =
        mangle_doc::PageTree::build(&reopened, root2).map_err(|e| format!("{out_path}: {e}"))?;
    let page2 = tree2
        .get(index)
        .ok_or_else(|| format!("{out_path}: page {index} is gone"))?;
    let resources2 = page_resources(&reopened, page2);
    let stream2 = page2.decoded_contents(&reopened);
    let after = mangle_content::interp::run_with(
        &mangle_content::ContentStream::parse(&stream2),
        &resources2,
    );

    // Reopen what was written and check the page is still there, which is the only evidence
    // that counts: a file that will not open is a save that lost.
    let _ = writeln!(
        out,
        "reopened  the file opens and page {} is still there",
        index + 1
    );
    match arrange {
        Some(_) => {}
        None => {
            let change = change_from_args(args, object)?;
            match mangle_edit::verify(&run, &after, which, &change) {
                Ok(()) => {
                    let _ = writeln!(
                        out,
                        "verified  the reopened page shows the change that was asked for"
                    );
                }
                Err(e) => {
                    let _ = writeln!(out, "verified  NO: {e}");
                }
            }
        }
    }
    Ok(())
}

/// The change the arguments ask for, read out of them once so the verification can use the same.
fn change_from_args(
    args: &[String],
    object: &mangle_edit::PageObject,
) -> Result<mangle_edit::Change, String> {
    if flag(args, "--delete") {
        return Ok(mangle_edit::Change::Delete);
    }
    if let Some(moved) = value(args, "--move") {
        let parts: Vec<f64> = moved.split(',').filter_map(|p| p.parse().ok()).collect();
        let dx = parts.first().copied().ok_or("--move needs dx,dy")?;
        let dy = parts.get(1).copied().ok_or("--move needs dx,dy")?;
        return Ok(mangle_edit::Change::move_by(dx, dy));
    }
    if let Some(scale) = value(args, "--scale") {
        let s: f64 = scale
            .parse()
            .map_err(|_| format!("--scale needs a number, got `{scale}`"))?;
        let b = object.bounds;
        return Ok(mangle_edit::Change::scale_about(
            f64::midpoint(b.x0, b.x1),
            f64::midpoint(b.y0, b.y1),
            s,
            s,
        ));
    }
    if let Some(colour) = value(args, "--colour") {
        let parts: Vec<f64> = colour.split(',').filter_map(|p| p.parse().ok()).collect();
        match (
            parts.first().copied(),
            parts.get(1).copied(),
            parts.get(2).copied(),
        ) {
            (Some(r), Some(g), Some(b)) => {
                return Ok(mangle_edit::Change::recolour(mangle_content::state::Rgba {
                    r,
                    g,
                    b,
                    a: 1.0,
                }));
            }
            _ => return Err("--colour needs r,g,b in 0..1".to_string()),
        }
    }
    Err(
        "say what to do: --move dx,dy, --scale s, --delete, --colour r,g,b, or an arrange flag"
            .to_string(),
    )
}

/// A page's resources, resolved the way the interpreter needs.
fn page_resources(doc: &Document, page: &mangle_doc::Page) -> mangle_content::Resources {
    page.inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| mangle_content::Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default()
}

fn info(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("info needs a file")?;
    let doc = open(path)?;
    let info = doc.info();
    let pages = doc.page_count().unwrap_or(0);

    if flag(args, "-json") {
        let notes: Vec<String> = info.recovery.notes().to_vec();
        println!("{{");
        println!("  \"file\": \"{}\",", path.replace('"', "'"));
        println!("  \"version\": \"{}\",", info.version);
        println!("  \"pages\": {pages},");
        println!("  \"clean\": {},", info.recovery.is_clean());
        println!("  \"notes\": [{}],", quoted(&notes));
        println!("  \"encrypted\": {},", info.encryption.encrypted);
        println!("  \"objects\": {}", doc.object_numbers().len());
        println!("}}");
        return Ok(());
    }

    println!("file        {path}");
    println!("version     {}", info.version);
    println!("pages       {pages}");
    println!(
        "structure   {}",
        if info.recovery.is_clean() {
            "as written".to_string()
        } else {
            format!("repaired ({} note(s))", info.recovery.notes().len())
        }
    );
    for note in info.recovery.notes() {
        println!("            - {note}");
    }
    if let Some(banner) = info.recovery.banner() {
        println!("banner      {banner}");
    }
    if info.encryption.encrypted {
        println!(
            "encryption  revision {}, filter {}",
            info.encryption.revision,
            info.encryption.filter.as_deref().unwrap_or("unknown")
        );
    }
    println!("objects     {}", doc.object_numbers().len());
    Ok(())
}

fn quoted(items: &[String]) -> String {
    items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The document's page tree.
///
/// The root is the catalogue's `/Pages`, not the catalogue: the catalogue is one level
/// above it and has no `/Kids` of its own.
fn page_tree(doc: &Document) -> Result<mangle_doc::PageTree, String> {
    let catalog = doc.catalog().map_err(|e| e.to_string())?;
    let root = catalog
        .get("Pages")
        .and_then(mangle_syntax::object::Object::as_ref_id)
        .ok_or("the catalogue has no /Pages")?;
    mangle_doc::PageTree::build(doc, root).map_err(|e| e.to_string())
}

fn pages(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("pages needs a file")?;
    let doc = open(path)?;
    let tree = page_tree(&doc)?;
    for (i, page) in tree.pages().iter().enumerate() {
        let (w, h) = page.inherited.displayed_size();
        let label = tree_label(&doc, i);
        let annots = page.annots(&doc).len();
        println!(
            "{i:>5}  {w:>7.1} x {h:<7.1}  rot {:>3}  label {:<8} annots {annots}",
            page.inherited.rotation(),
            label
        );
    }
    Ok(())
}

fn tree_label(doc: &Document, index: usize) -> String {
    let Ok(catalog) = doc.catalog() else {
        return String::new();
    };
    let Some(dict) = catalog.as_dict() else {
        return String::new();
    };
    mangle_doc::PageLabel::build(doc, dict).label_text(index)
}

fn check(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("check needs a file")?;
    let doc = open(path)?;
    let mut problems: Vec<String> = Vec::new();

    if let Err(e) = doc.page_count() {
        problems.push(e.to_string());
    }
    if doc.info().encryption.encrypted && doc.catalog().is_err() {
        problems.push("the file is encrypted and could not be opened".into());
    }
    for problem in &problems {
        println!("problem  {problem}");
    }
    if !doc.info().recovery.notes().is_empty() {
        for note in doc.info().recovery.notes() {
            println!("note     {note}");
        }
    }
    if problems.is_empty() && doc.info().recovery.is_clean() {
        println!("ok       {path}");
        return Ok(());
    }
    if problems.is_empty() {
        println!("repaired {path}");
        std::process::exit(1);
    }
    println!("unusable {path}");
    std::process::exit(2);
}

fn render(_args: &[String]) -> Result<(), String> {
    Err("render is not implemented yet; it lands with the renderer".into())
}

/// The text of a page, in the order a reader would read it.
fn extract(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("extract needs a file")?;
    let doc = open(path)?;
    let page = value(args, "-page")
        .and_then(|p| p.parse::<usize>().ok())
        .ok_or("extract needs -page N")?;
    let tree = page_tree(&doc)?;
    let Some(target) = tree.get(page) else {
        return Err(format!("the file has no page {page}"));
    };
    print!("{}", text_of(&target.decoded_contents(&doc)));
    Ok(())
}

/// The strings a page draws, in the order the operators give them.
///
/// Deliberately a scanner over the bytes rather than a full content-stream
/// interpreter: the point is a quick look at what a page says, not the rendered text
/// with its positions. A real extractor arrives with `mangle-text`.
fn text_of(content: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < content.len() {
        let Some(relative) = content
            .get(i..)
            .and_then(|rest| rest.iter().position(|b| *b == b'('))
        else {
            break;
        };
        let start = i + relative + 1;
        let mut depth = 1i32;
        let mut j = start;
        let mut text: Vec<u8> = Vec::new();
        while j < content.len() && depth > 0 {
            let Some(&b) = content.get(j) else { break };
            if b == b'\\' {
                match content.get(j + 1) {
                    Some(b'n') => text.push(b'\n'),
                    Some(b'r') => text.push(b'\r'),
                    Some(b't') => text.push(b'\t'),
                    Some(other) => text.push(*other),
                    // A backslash at the very end of the buffer escapes nothing.
                    None => text.push(b'\\'),
                }
                j += 2;
                continue;
            }
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                other => text.push(other),
            }
            j += 1;
        }
        out.push_str(&String::from_utf8_lossy(&text));
        out.push('\n');
        i = j.saturating_add(1);
    }
    out
}

fn save(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("save needs a file")?;
    let doc = open(path)?;
    let mode = if flag(args, "--full") || flag(args, "--reset-prefs") {
        // A full rewrite is also what a repaired file gets regardless.
        if !flag(args, "--incremental") {
            SaveMode::Full
        } else {
            SaveMode::Incremental
        }
    } else if flag(args, "--incremental") {
        SaveMode::Incremental
    } else {
        SaveMode::Full
    };
    let out_path = value(args, "-o").unwrap_or_else(|| format!("{path}.out.pdf"));
    let report = doc
        .save(&SaveOptions {
            mode,
            ..SaveOptions::default()
        })
        .map_err(|e| format!("{path}: {e}"))?;

    // Write beside the target and rename, so an interrupted save never leaves a
    // half-written file where the real one was.
    let target = Path::new(&out_path);
    let temp = target.with_extension("pdf.part");
    std::fs::write(&temp, &report.bytes).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, target).map_err(|e| format!("{}: {e}", target.display()))?;

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "wrote     {out_path}");
    let _ = writeln!(stdout, "bytes     {}", report.bytes.len());
    let _ = writeln!(
        stdout,
        "mode      {}",
        if report.forced_full {
            "full (forced)"
        } else {
            match mode {
                SaveMode::Full => "full",
                SaveMode::Incremental => "incremental",
            }
        }
    );
    if let Some(reason) = &report.forced_reason {
        let _ = writeln!(stdout, "reason    {reason}");
    }
    for dropped in &report.dropped {
        let _ = writeln!(
            stdout,
            "dangling  {} {} 0 R",
            dropped.num, dropped.generation
        );
    }
    Ok(())
}
