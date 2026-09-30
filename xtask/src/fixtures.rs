//! Fixture corpus management.
//!
//! Tier A comes from `tools/fixturegen`, which shares no code with the product, so a
//! fixture that the product can read is evidence rather than a tautology.

use std::path::PathBuf;

use crate::workspace::{cargo, repo_root};

/// `cargo xtask fixtures [--seed N] [--out DIR] [--check]`
pub(crate) fn cmd(args: &[String]) -> Result<(), String> {
    let mut seed = 20260101u64;
    let mut out = repo_root()?.join("fixtures");
    let mut check = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => {
                i += 1;
                seed = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .ok_or("--seed needs a number")?;
            }
            "--out" => {
                i += 1;
                out = PathBuf::from(args.get(i).ok_or("--out needs a directory")?);
            }
            "--check" => check = true,
            other => return Err(format!("unknown fixturegen option `{other}`")),
        }
        i += 1;
    }
    let root = repo_root()?;
    let out = if out.is_absolute() {
        out
    } else {
        root.join(out)
    };
    if check {
        return verify_determinism(&root, seed, &out);
    }
    generate(&root, seed, &out)
}

fn generate(root: &std::path::Path, seed: u64, out: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    run(
        root,
        &[
            "run",
            "--quiet",
            "-p",
            "fixturegen",
            "--",
            "--seed",
            &seed.to_string(),
            "--out",
            &out.to_string_lossy(),
        ],
    )?;
    println!("fixtures written to {}", out.display());
    Ok(())
}

/// Regenerate into a scratch directory and compare, which is the only honest way to
/// claim the corpus is byte-identical across runs.
fn verify_determinism(
    root: &std::path::Path,
    seed: u64,
    out: &std::path::Path,
) -> Result<(), String> {
    let scratch = root.join("target/xtask/fixture-check");
    let _ = std::fs::remove_dir_all(&scratch);
    run(
        root,
        &[
            "run",
            "--quiet",
            "-p",
            "fixturegen",
            "--",
            "--seed",
            &seed.to_string(),
            "--out",
            &scratch.to_string_lossy(),
        ],
    )?;
    let (checked, differing) = diff_dirs(out, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    if differing.is_empty() {
        println!("{checked} fixtures are byte-identical for seed {seed}");
        Ok(())
    } else {
        Err(format!(
            "{} of {checked} fixtures differ for seed {seed}:\n  {}",
            differing.len(),
            differing.join("\n  ")
        ))
    }
}

fn diff_dirs(a: &std::path::Path, b: &std::path::Path) -> (usize, Vec<String>) {
    let mut checked = 0usize;
    let mut differing = Vec::new();
    let Ok(entries) = std::fs::read_dir(a) else {
        return (0, vec![format!("{} is missing", a.display())]);
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "toml" || x == "json") {
            continue;
        }
        if !p.is_file() {
            continue;
        }
        checked += 1;
        let other = b.join(e.file_name());
        let (x, y) = match (std::fs::read(&p), std::fs::read(&other)) {
            (Ok(x), Ok(y)) => (x, y),
            _ => {
                differing.push(format!("{} could not be regenerated", p.display()));
                continue;
            }
        };
        if x != y {
            differing.push(e.file_name().to_string_lossy().to_string());
        }
    }
    (checked, differing)
}

fn run(root: &std::path::Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(cargo())
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("running cargo: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "cargo {} failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}
