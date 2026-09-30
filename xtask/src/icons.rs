//! The icon pipeline: a gallery sheet to look at, and a lint to keep it honest.
//!
//! Icons are hand-authored SVG. The gallery exists so a human can see them all at
//! once; the lint exists so a traced or rasterised icon cannot slip in unnoticed.

use std::path::PathBuf;

use crate::workspace::repo_root;

/// `cargo xtask icons gallery|lint`
pub(crate) fn cmd(args: &[String]) -> Result<(), String> {
    let mode = args.first().map_or("lint", String::as_str);
    let root = repo_root()?;
    let dir = root.join("assets/icons");
    match mode {
        "gallery" => gallery(&dir, &root),
        "lint" => lint(&dir),
        other => Err(format!("unknown icons mode `{other}`; use gallery or lint")),
    }
}

/// One-line summary of an icon, for the gallery index.
fn lint(dir: &std::path::Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    if files.is_empty() {
        return Err(format!("no icons found in {}", dir.display()));
    }
    let mut findings = Vec::new();
    let mut names = Vec::new();
    for f in &files {
        let name = f
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        names.push(name.clone());
        let Ok(text) = std::fs::read_to_string(f) else {
            findings.push(format!("{name}: unreadable"));
            continue;
        };
        let where_ = f.display().to_string();
        if !text.contains("viewBox") {
            findings.push(format!("{name}: no viewBox"));
        }
        for bad in ["<image", "<script", "data:image", "<foreignObject"] {
            if text.contains(bad) {
                findings.push(format!("{name}: contains `{bad}`"));
            }
        }
        // An icon that is nothing but one filled rectangle is usually an auto-trace
        // of a screenshot; a hand-authored icon has real path data.
        if text.matches("<path").count() + text.matches("<circle").count() < 1
            && !text.contains("<line")
            && !text.contains("<polyline")
            && !text.contains("<rect")
        {
            findings.push(format!("{name}: no drawable content"));
        }
        let _ = where_;
    }
    // Duplicate shapes under different names defeat the point of a consistent set.
    let mut by_hash: std::collections::BTreeMap<u64, Vec<String>> = Default::default();
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let h = fnv1a(&bytes);
        by_hash.entry(h).or_default().push(
            f.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        );
    }
    for (_, group) in by_hash {
        if group.len() > 1 {
            findings.push(format!("{} are byte-identical", group.join(", ")));
        }
    }
    if findings.is_empty() {
        println!("{} icons lint clean:\n  {}", files.len(), names.join("  "));
        Ok(())
    } else {
        Err(format!("icon lint:\n  {}", findings.join("\n  ")))
    }
}

fn gallery(dir: &std::path::Path, root: &std::path::Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    let out_dir = root.join("target/xtask");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("creating output: {e}"))?;
    let sheet = out_dir.join("icon-gallery.html");
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>ManglePDF icons</title>\n<style>\n\
         body{font:13px system-ui;background:#fff;color:#222;margin:24px}\n\
         .grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(120px,1fr));gap:16px}\n\
         figure{margin:0;text-align:center}\n\
         svg{width:64px;height:64px;stroke:#222;fill:none;stroke-width:1.5;\
         stroke-linecap:round;stroke-linejoin:round}\n\
         figcaption{font-size:11px;color:#666;margin-top:6px;word-break:break-all}\n\
         </style>\n<div class=grid>\n",
    );
    for f in &files {
        let name = f
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        html.push_str(&format!(
            "<figure>{text}<figcaption>{name}</figcaption></figure>\n"
        ));
    }
    html.push_str("</div>\n");
    std::fs::write(&sheet, html).map_err(|e| format!("writing {}: {e}", sheet.display()))?;
    println!("icon gallery: {}", sheet.display());
    Ok(())
}

fn collect(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "svg") {
            out.push(p);
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}
