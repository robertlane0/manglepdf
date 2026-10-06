# Development

## Prerequisites

A pinned Rust toolchain (`rust-toolchain.toml` picks it up automatically) and nothing
else. The build is offline after the first dependency fetch:

```sh
cargo fetch                 # once
cargo vendor                # optional: to vendor for an air-gapped machine
```

`cargo xtask` is an alias for the `xtask` binary, so `cargo xtask policy` works from a
clean checkout.

## The loop

```sh
cargo xtask policy                 # everything Gate 0 checks; use this before pushing
cargo xtask policy --only G0.2     # one rule, when you are iterating on it
cargo test -p mangle-syntax        # one crate
cargo clippy -p mangle-syntax --all-targets -- -D warnings
cargo fmt --all
```

`cargo xtask policy` runs `fmt`, `clippy -D warnings`, the whole test suite and a
release build, so it is the command that matters at the end of a change. Use
`--only` while working so the loop stays fast.

**G0.9 also resolves the documentation's own links.** Beyond confirming the required files
exist, it checks every `[text](path#heading)` in `PLAN.md`, `README.md` and everything under
`docs/`, and fails on one whose heading no longer exists. That check is there because a link to a
heading that has been renamed sends a reader somewhere that does not say what they were told it
says — the same failure as a comment claiming a feature is unimplemented one screen above the code
that implements it. Both have been found here, which is why it is a gate rather than something to
remember to run.

## Layout

```
crates/          the product, in dependency order
tools/fixturegen deterministic fixtures; no mangle-* dependency
xtask/           gate checks, fixtures, icons, the gauntlet
corpus/          real-world PDFs; git-ignored except SOURCES.md
fixtures/        generated Tier A; checked in
scripts/         dev scripts that use external tools as oracles
docs/            this documentation and the decision records
```

## Adding code

- Respect the crate dependency order in `docs/ARCHITECTURE.md`. A lower crate naming a
  higher one will not compile, which is the point.
- Every first-party target starts with `#![forbid(unsafe_code)]` and inherits
  `[workspace.lints]`. `xtask policy` checks.
- A **library** crate denies `unwrap`, `expect`, `panic`, `todo`, `unimplemented` and
  `unreachable`. Binaries and tests may fail loudly. `xtask policy` checks.
- Every loop, recursion and allocation driven by file data is bounded. A bound is not
  optional, and it belongs next to the code it protects.
- Prefer a partial result with a note over an error. Render what you can and mark what
  you cannot.

## Writing a test

State the expectation, name the behaviour, and say in a comment when the case is a
regression and what used to go wrong. `docs/PDF-QUIRKS.md` lists the facts worth
testing that the specification's happy path does not cover.

## Decision records

Anything with a lasting cost goes in `docs/decisions/NNNN-short-name.md`: the bet, why,
what it rules out. `PLAN.md` links to them.
