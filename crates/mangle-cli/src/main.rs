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
                            --provenance [I]      which bytes drew it (the object named, or all)
                            --page N               which page (default 1)
                            --object I             which object (see --list)
                            --move dx dy           move it by dx, dy points
                            --scale s              scale it about its centre
                            --delete               remove it
                            --colour r,g,b         recolour it (0-1 each)
                            --tc N                 character spacing, points
                            --tw N                 word spacing, points
                            --tz N                 horizontal scale, a percentage
                            --tl N                 leading, points
                            --ts N                 baseline shift, points
                            --size N               the font size, points
                            --crop x0,y0,x1,y1      keep that part of an image, in page space
                            --retype TEXT          replace the run's string, in its own font
                            --reflow TEXT          relay out the block in its own width
                            --align MODE           left|center|right|justify (with --reflow, or alone)
                            --scope SCOPE          this-page|everywhere: edit a shared form object
                            --replace TEXT         replace bitmap text with live text
                            --flatten              flatten all annotations on the page
                            --flatten-markup       flatten markup annotations (keep links/forms)
                            --flatten-forms        flatten form widgets only
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
    if flag(args, "--provenance") {
        // The object named, or every one of them when no index is given.
        let chosen: Vec<usize> = match value(args, "--provenance").and_then(|v| v.parse().ok()) {
            Some(i) => vec![i],
            None => (0..model.objects().len()).collect(),
        };
        for i in chosen {
            let Some(object) = model.objects().get(i) else {
                eprintln!(
                    "object {i} is not one of the page's {} objects",
                    model.objects().len()
                );
                continue;
            };
            let _ = writeln!(
                out,
                "object {i}, a {:?}, at {x0:.2} {y0:.2}",
                object.kind,
                x0 = object.bounds.x0,
                y0 = object.bounds.y0
            );
            let provenance = mangle_edit::provenance_of(object, &stream);
            for line in &provenance.lines {
                let _ = writeln!(out, "  {line}");
            }
        }
        return Ok(());
    }
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

    if flag(args, "--flatten") || flag(args, "--flatten-markup") || flag(args, "--flatten-forms") {
        let filter = if flag(args, "--flatten-markup") {
            mangle_edit::FlattenFilter::MarkupOnly
        } else if flag(args, "--flatten-forms") {
            mangle_edit::FlattenFilter::FormsOnly
        } else {
            mangle_edit::FlattenFilter::All
        };
        let options = mangle_edit::FlattenOptions {
            filter,
            allow_signed: flag(args, "--allow-signed"),
            ..mangle_edit::FlattenOptions::default()
        };
        let report =
            mangle_edit::flatten_page(&doc, page, &options).map_err(|e| format!("{path}: {e}"))?;
        let out_path = value(args, "-o").unwrap_or_else(|| format!("{path}.edited.pdf"));
        write_atomic(&out_path, &report.save.bytes)?;
        let _ = writeln!(
            out,
            "flattened {} annotation(s), skipped {} hidden, kept {}, wrote {out_path}",
            report.flattened, report.skipped_hidden, report.kept
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
    let mut session = mangle_edit::Editor::open(std::sync::Arc::new(doc), page, &resources)
        .map_err(|e| format!("{path}: {e}"))?;
    let size_before = session.document().bytes().len();
    let original = session.document().bytes().to_vec();
    // The change as executed, for the check below: a fallback resolves to its installed
    // CIDs, and checking the requested change instead would fail a correct file.
    // Arrange takes no such path, so only the edit arm sets it.
    let mut executed: Option<mangle_edit::Change> = None;
    // A shared-form edit saves outside the session history: it needs the form's own
    // stream and an explicit scope, so it never goes through `session.apply`.
    if let Some(scope) = value(args, "--scope")
        .map(|s| parse_scope(&s))
        .transpose()?
    {
        if value(args, "--replace").is_some() {
            return Err(
                "--replace cannot target a shared form; use --retype with --scope".to_string(),
            );
        }
        if object.form.is_none() && object.records.iter().all(|r| r.form.is_none()) {
            return Err(format!(
                "--scope needs an object drawn inside a shared form, and object {which} is on \
                 the page itself; drop --scope to edit it"
            ));
        }
        let change = match value(args, "--align") {
            Some(mode) if value(args, "--reflow").is_none() => session
                .align_change(which, parse_alignment(&mode)?)
                .map_err(|e| format!("{path}: {e}"))?,
            _ => change_from_args(args, session.document(), &resources, object)?,
        };
        let save = session
            .apply_shared(which, &change, scope)
            .map_err(|e| format!("{path}: {e}"))?;
        let out_path = value(args, "-o").unwrap_or_else(|| format!("{path}.edited.pdf"));
        write_atomic(&out_path, &save.bytes)?;
        let _ = writeln!(
            out,
            "selected {which} on page {}, a {kind:?} at {x0:.2} {y0:.2}",
            index + 1,
            kind = object.kind,
            x0 = object.bounds.x0,
            y0 = object.bounds.y0,
        );
        let _ = writeln!(out, "did       shared-form edit with {scope:?}");
        let _ = writeln!(out, "wrote     {out_path}");
        let _ = writeln!(
            out,
            "bytes     {}, {} of them new",
            save.bytes.len(),
            save.bytes.len() - size_before
        );
        let _ = writeln!(
            out,
            "rewrote   {} object(s), installed {} object(s)",
            save.rewritten.len(),
            save.installed.len()
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
        let reopened = Document::open(save.bytes.clone(), OpenOptions::default())
            .map_err(|e| format!("{out_path}: {e}"))?;
        let _ = writeln!(
            out,
            "reopened  the file opens and has {} page(s)",
            reopened
                .page_count()
                .map_err(|e| format!("{out_path}: {e}"))?
        );
        let _ = writeln!(
            out,
            "verified  skipped: the edit is in the form's stream, not the page's"
        );
        return Ok(());
    }
    let report = match arrange {
        Some(arrange) => session
            .arrange(which, arrange)
            .map_err(|e| format!("{path}: {e}"))?,
        None => {
            // Replacing bitmap text is a session call rather than a change: it installs
            // a font first, which a byte-patch cannot do.
            if let Some(text) = value(args, "--replace") {
                if text.is_empty() {
                    return Err("--replace needs the replacement string".to_string());
                }
                let outcome = session
                    .replace_with_live_text(which, &text)
                    .map_err(|e| format!("{path}: {e}"))?;
                executed = Some(outcome.change);
                outcome.label
            } else if value(args, "--reflow").is_none() {
                if let Some(mode) = value(args, "--align") {
                    let alignment = parse_alignment(&mode)?;
                    let change = session
                        .align_change(which, alignment)
                        .map_err(|e| format!("{path}: {e}"))?;
                    let outcome = session
                        .apply(which, &change)
                        .map_err(|e| format!("{path}: {e}"))?;
                    executed = Some(outcome.change);
                    outcome.label
                } else {
                    let change = change_from_args(args, session.document(), &resources, object)?;
                    let outcome = session
                        .apply(which, &change)
                        .map_err(|e| format!("{path}: {e}"))?;
                    executed = Some(outcome.change);
                    outcome.label
                }
            } else {
                let change = change_from_args(args, session.document(), &resources, object)?;
                let outcome = session
                    .apply(which, &change)
                    .map_err(|e| format!("{path}: {e}"))?;
                executed = Some(outcome.change);
                outcome.label
            }
        }
    };
    let save = session.save().map_err(|e| format!("{path}: {e}"))?;
    let (back, forward) = session.depth();
    let history = session.labels().join(", ");

    // Write beside the target and rename, so an interrupted save never leaves a half-written file
    // where the real one was.
    let out_path = value(args, "-o").unwrap_or_else(|| format!("{path}.edited.pdf"));
    write_atomic(&out_path, &save.bytes)?;

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
    if !save.installed.is_empty() {
        let _ = writeln!(out, "installed {} object(s)", save.installed.len());
    }
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
            let change = match executed {
                Some(change) => change,
                None => change_from_args(args, session.document(), &resources, object)?,
            };
            // The reopened page's own font table checks a retype: the edit never touches
            // the fonts, so the table is the same one the edit read.
            let fonts = mangle_edit::Fonts {
                resources: &resources2,
                resolve: &|o| reopened.resolve_object(o),
            };
            match mangle_edit::verify_with(&run, &after, which, &change, Some(&fonts)) {
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
    doc: &Document,
    resources: &mangle_content::Resources,
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
    if let Some(crop) = value(args, "--crop") {
        let parts: Vec<f64> = crop.split(',').filter_map(|p| p.parse().ok()).collect();
        match (
            parts.first().copied(),
            parts.get(1).copied(),
            parts.get(2).copied(),
            parts.get(3).copied(),
        ) {
            (Some(x0), Some(y0), Some(x1), Some(y1)) => {
                return Ok(mangle_edit::Change::Crop {
                    keep: mangle_content::state::ClipBounds { x0, y0, x1, y1 },
                });
            }
            _ => return Err("--crop needs x0,y0,x1,y1".to_string()),
        }
    }
    for (flag, build) in [
        ("--tc", 0usize),
        ("--tw", 1),
        ("--tz", 2),
        ("--tl", 3),
        ("--ts", 4),
        ("--size", 5),
    ] {
        if let Some(v) = value(args, flag) {
            let n = number(&v, flag)?;
            let property = match build {
                0 => mangle_edit::TextProperty::CharacterSpacing(n),
                1 => mangle_edit::TextProperty::WordSpacing(n),
                2 => mangle_edit::TextProperty::HorizontalScale(n),
                3 => mangle_edit::TextProperty::Leading(n),
                4 => mangle_edit::TextProperty::BaselineShift(n),
                _ => mangle_edit::TextProperty::Size(n),
            };
            return Ok(mangle_edit::Change::text(property));
        }
    }
    if let Some(text) = value(args, "--retype") {
        if text.is_empty() {
            return Err("--retype needs the replacement string".to_string());
        }
        return Ok(mangle_edit::Change::Retype { text });
    }
    if let Some(text) = value(args, "--reflow") {
        if text.is_empty() {
            return Err("--reflow needs the replacement string".to_string());
        }
        let summary = mangle_edit::summarise(doc, resources, object);
        let base_font = summary
            .font_family
            .unwrap_or_else(|| "Helvetica".to_string());
        let alignment = match value(args, "--align") {
            Some(mode) => parse_alignment(&mode)?,
            None => mangle_edit::TextAlignment::Left,
        };
        return Ok(mangle_edit::Change::Reflow {
            text,
            base_font,
            alignment,
        });
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
        "say what to do: --move dx,dy, --scale s, --delete, --colour r,g,b, a text property \
         (--tc/--tw/--tz/--tl/--ts/--size), --crop x0,y0,x1,y1, --retype TEXT, --reflow TEXT, \
         --replace TEXT, or an arrange flag"
            .to_string(),
    )
}

/// An alignment mode as the `--align` flag spells it.
fn parse_alignment(mode: &str) -> Result<mangle_edit::TextAlignment, String> {
    match mode.to_ascii_lowercase().as_str() {
        "left" => Ok(mangle_edit::TextAlignment::Left),
        "center" | "centre" => Ok(mangle_edit::TextAlignment::Center),
        "right" => Ok(mangle_edit::TextAlignment::Right),
        "justify" | "justified" => Ok(mangle_edit::TextAlignment::Justify),
        _ => Err("--align needs left|center|right|justify".to_string()),
    }
}

/// A shared-form scope as the `--scope` flag spells it.
fn parse_scope(scope: &str) -> Result<mangle_edit::SharedFormScope, String> {
    match scope.to_ascii_lowercase().as_str() {
        "this-page" | "this-page-only" => Ok(mangle_edit::SharedFormScope::ThisPageOnly),
        "everywhere" => Ok(mangle_edit::SharedFormScope::Everywhere),
        _ => Err("--scope needs this-page|everywhere".to_string()),
    }
}

/// Write beside the target and rename, so an interrupted save never leaves a half-written file
/// where the real one was.
fn write_atomic(out_path: &str, bytes: &[u8]) -> Result<(), String> {
    let target = Path::new(out_path);
    let temp = target.with_extension("pdf.part");
    std::fs::write(&temp, bytes).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(())
}

/// Parse a number argument, saying which flag it came from.
fn number(text: &str, flag: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .map_err(|_| format!("{flag} needs a number, got `{text}`"))
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

fn render(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("render needs a file")?;
    let out_path = value(args, "-o").ok_or("render needs -o FILE")?;
    let page_idx = value(args, "-page")
        .or_else(|| value(args, "--page"))
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(0);
    let dpi = value(args, "-dpi")
        .or_else(|| value(args, "--dpi"))
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(150.0);
    let doc = open(path)?;
    let tree = page_tree(&doc)?;
    let Some(page) = tree.get(page_idx) else {
        return Err(format!("the file has no page {page_idx}"));
    };
    let resources = page_resources(&doc, page);
    let is_jpeg = Path::new(&out_path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    let bytes = if is_jpeg {
        mangle_edit::export_page_jpeg(&doc, page, &resources, dpi, 90)?
    } else {
        mangle_edit::export_page_png(&doc, page, &resources, dpi)
    };
    std::fs::write(&out_path, &bytes).map_err(|e| format!("{out_path}: {e}"))?;
    Ok(())
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
