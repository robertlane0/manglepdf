//! `cargo xtask <command>`: build automation for the gates in `FINISH.md`.
//!
//! Every command prints a report and returns a non-zero exit code when something
//! failed, so it can be wired straight into CI or the gauntlet runner.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

mod ast;
mod deps;
mod fixtures;
mod icons;
mod policy;
mod report;
mod workspace;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("help", String::as_str);
    let rest = args.get(1..).unwrap_or_default();

    let result = match command {
        "policy" => policy::cmd(rest),
        "fixtures" => fixtures::cmd(rest),
        "icons" => icons::cmd(rest),
        "deps" => deps::cmd(),
        "gauntlet" => Err(not_yet("gauntlet", "M12")),
        "fuzz" => Err(not_yet("fuzz", "M10")),
        "perf" => Err(not_yet("perf", "M10")),
        "help" | "-h" | "--help" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n\n{}", usage())),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn not_yet(what: &str, milestone: &str) -> String {
    format!("`cargo xtask {what}` is not implemented yet; it lands in {milestone}.")
}

fn usage() -> String {
    "\
cargo xtask <command>

  policy    [--only G0.1,G0.4]   Gate 0: hygiene, dependencies, docs, icons
  fixtures  [--seed N] [--out D] regenerate the Tier-A fixture corpus
  icons     gallery|lint         build or check assets/icons
  deps                          print the third-party dependency table
  gauntlet  [--seed N]           the full acceptance run
  fuzz      [--seed N]           mutation fuzzing
  perf      [--seed N]           performance floors
"
    .to_string()
}
