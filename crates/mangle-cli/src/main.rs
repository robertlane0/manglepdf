#![forbid(unsafe_code)]
//! The headless surface: what the tests and the acceptance harness drive.
//!
//! Every command here is a question about a file with a definite answer, and every
//! answer is printed in a form a script can check. Nothing about the product depends
//! on this binary existing; it exists so that "what does ManglePDF think of this file?"
//! can be asked without a window.

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
