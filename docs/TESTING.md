# Testing

## Layers

| Kind | Where | What it proves |
|---|---|---|
| Unit | `#[cfg(test)]` in the crate that owns the code | One function, one behaviour, stated expectations |
| Round trip | `crates/*/tests/` | Open a file, change it, write it, reopen it, compare |
| Oracle | `crates/*/tests/oracle.rs` | Our answer matches an independent implementation's |
| Fuzz | `crates/mangle-syntax/tests/fuzz.rs` | No panic, hang or runaway allocation on mutated input |
| Acceptance | `cargo xtask gauntlet` | The end-to-end scenarios in `FINISH.md` |

## Oracles

Independent implementations may be used **as test oracles only** — never linked, never
called from product code, and never required for `cargo test` to pass. When one is
absent the test skips, records the skip, and the gauntlet report lists it.

Currently used, each skipped when absent:

- `qpdf` — cross-reference and encryption dictionaries, `--check` on generated output
- `mutool` — page counts and rendering
- `pdftotext` / `pdfinfo` — text extraction and document metadata

`scripts/make-crypto-fixtures.sh` produces encryption dictionaries with `qpdf`; the
values it prints are pasted into `mangle-crypto`'s tests as literals, so `cargo test`
never needs `qpdf`.

## What the corpus caught that the fixtures did not

Every defect in `docs/BUGS.md` was found by a test that **runs the interpreter over the
result** on a real Tier B page, and every one of them passed the unit tests that existed at
the time. Seven of the eight are the same shape: a byte-range edit that does not carry its
own separators, or that writes its operands after its operator, produces a stream that is a
*different stream*. Nothing downstream reports it — the renderer draws a plausible page, the
file opens, the edit appears to have worked, and the change is simply not there.

The rule the corpus tests enforce is one line: **assert the effect, not the string.** A unit
test that asserts the bytes an edit *wrote* will pass on `Tc 2`; the test that asserts the
character spacing the interpreter *reports* will not.

## Fixtures

Tier A comes from `tools/fixturegen`, which depends on **no** `mangle-*` crate. That
independence is the point: a fixture built by the same misunderstanding as the parser
proves nothing. `cargo xtask policy` fails the build if that ever changes.

Tier B is real-world files in `corpus/wild/`. They are **not** in the repository: the PDFs
are large and often someone else's, so `corpus/wild/` holds only `MANIFEST.toml` (every
file's URL, licence, SHA-256 and expected structure), `SOURCES.md` (the same, written for a
person) and a `.gitignore`. `cargo xtask corpus fetch` downloads them and refuses any file
whose SHA-256 does not match — a corpus whose contents are not pinned is a test that
changes under you. `corpus/wild/SOURCES.md` states the licence of every file and says which
suggested public corpora were *not* used and why.

Tier C is held out until final acceptance and must stay unread until then.

Regenerate and check determinism:

```sh
cargo xtask fixtures --seed 20260101          # write fixtures/
cargo xtask fixtures --seed 20260101 --check  # regenerate and compare byte for byte
cargo xtask corpus fetch                      # download Tier B and verify every hash
cargo xtask corpus check                      # verify what is on disk; needs no network
cargo xtask corpus list                       # one line per file
```

## The wild corpus harness

`crates/mangle-render/tests/wild_corpus.rs` walks the manifest and, for every file, renders
every page at 150 DPI, compares it with `mutool draw` through `compare()`, and compares the
text with `pdftotext` at word level. It writes `corpus/wild/report/SUMMARY.md`, a Markdown
report per file, and PNGs of the worst pages: ours, mutool's, and the heatmap between them.

**It is `#[ignore]`d, and the other two tests in that file are not.** One run is about two
hours, which is fine for a corpus and fatal for `cargo test --workspace`. So:

```sh
cargo test -p mangle-render --test wild_corpus                  # the two cheap harness tests
cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture   # the two-hour run
```

The cheap pair check the harness itself — that a PAM round-trips, and that a heatmap lands
under the name the report links to — and a missing heatmap looks exactly like a page that
passed, so they are worth having in the default suite. The long one needs the corpus fetched
(`cargo xtask corpus fetch`) and `mutool` and `pdftotext` installed for the halves that
compare against them.

**Run the long one under a memory ceiling and in the background.** It renders every page
twice and holds several pages of raw pixels at a time, and three runs have been killed partway
through — the last of them after 36 of 77 files, which left a report describing a third of the
corpus. The harness now writes each file's report and its rows in `results.tsv` as that file
finishes, so a killed run leaves most of its work behind, but the run still has to survive to be
worth anything. A cgroup bound is the way to give it the chance:

```sh
setsid nohup systemd-run --user --scope --quiet \
  -p MemoryMax=8G -p MemorySwapMax=0 \
  env CARGO_INCREMENTAL=0 cargo test -j 1 -p mangle-render --test wild_corpus \
  -- --ignored --nocapture > /tmp/corpus-run.log 2>&1 < /dev/null & disown
```

`MemorySwapMax=0` matters as much as `MemoryMax`: with swap available the kernel will happily
push a runaway page out to disk and the run slows to a crawl instead of failing, which is worse
than either outcome. Confirm the ceiling took rather than assuming it:

```sh
systemctl --user show run-p<cargo-pid>-i<n>.scope -p MemoryMax -p MemorySwapMax -p MemoryPeak
```

The bound is headroom rather than a constraint — a full 77-file run peaked at 1.6 GiB, so 8 GiB
leaves room for a machine that is doing something else at the time. Raise it for a machine with
more headroom; do not lower it below what the largest single page needs. Build the test binary
before starting (`cargo test -p mangle-render --test wild_corpus --no-run`) so that
compilation does not compete with the run for the same ceiling.

**It asserts nothing about any individual file**, on purpose. A corpus test's job is to
find things; a threshold on a document nobody has read yet turns the first unexpected result
into a permanent red build, and the response to a permanent red build is to raise the
threshold. The report is the output. A file that needs an assertion belongs in `fixtures/`,
where the manifest can say what it must contain.

Every absence is printed with its reason — no oracle installed, a file not fetched, a page
too large to compare, a file that panicked — because a harness that reports success having
measured nothing is worse than no harness.

## Tolerances

Measured comparisons state their tolerance in the test, and say why:

- Text: exact, or word-level F1 ≥ 0.99 against an oracle.
- Renders: identical, or ±1 least-significant bit per channel, or SSIM ≥ 0.99.
- Geometry: ±0.01 pt, or ±0.1 pt where font rasterisation is in the path.
- Colour: ±2/255 per channel for an analytically computed sample.

## Driving the GUI, and what that found

The window is driven by real mouse and keyboard events, not through any test hook in the
product. That mattered: the two hardest bugs in the interaction layer were found by driving it
and neither showed in a unit test.

**egui calls `ui` only when something asks for a repaint.** A window that has drawn its chrome
and is waiting on a worker never asks again, so the worker's answer sits in the channel with
nothing reading it — and a screenshot looks exactly like a working window. Found by watching the
worker's CPU stop at one page's worth of rendering while the channel held the answer. Fixed with
one line: ask for the next frame at a fixed cadence, which is a ceiling on noticing rather than the
latency.

**The canvas answers for the whole window unless it is told not to.** A click on a stepper in the
right-hand panel reached the pointer handler first, missed the paper, and took the selection away:
the panel redrew itself with nothing selected, its controls were never drawn, and the click did
nothing at all. Found by instrumenting the click's window coordinates and watching the panel go
back to "Document Properties" on the frame after the click.

**A limitation of this harness, recorded rather than worked around.** Synthetic pointer input
delivers *clicks* with their positions but does not deliver the *motion* events that egui's
`hover_pos()` tracks, so the reported pointer position stays where the last real click landed.
Every control that reads a hover — which is all of them — is therefore unreachable by clicking
alone, though clicking works for anything that does not need a hover first. The window still has
to be inspected by eye: `screenshot_capture_window` on the app window, with the working terminal
dragged off the right-hand panel first, because it sits over it and its own pixels are wrong
behind it.

**The design consequence, which is the useful part.** Everything a window decides is separated
from the windowing it is decided in, so it can be tested without a mouse at all: `pointer_outcome`
decides whether an event is a click or a drag and on what, `panel_row` decides what each panel row
is, `stepped` decides what one click of a stepper writes, and `panel_for` decides which panel a
selection opens. A test that needed a real mouse could not have answered any of them, and the
wired-up path is then checked through the worker against real pages of the Tier-B corpus.

## Rules that are not negotiable

- **An ignored test is not a silent one.** `#[ignore]` means "do not run this by
  default", so it is neither a pass nor a failure and Gate 0 judges it on neither count:
  a failed test fails the gate by name, and a run that reports no test at all fails it
  too. Every ignored test is counted and named in the gate's report instead, so a
  deferred test is visible rather than hidden. The one currently ignored is the two-hour
  Tier-B corpus run, whose deliberate command is given above.
- **No mocking the parser, renderer or writer** in acceptance or UI tests. The product
  binary contains no fixture data and no expected outputs.
- **No test-only behaviour in product paths.** No `cfg(test)` differences, no env-var
  toggles, no feature flags that change behaviour under test.
- **A golden blessed by the code under test is not evidence.** Goldens are reviewed with
  vision against an independent oracle first.
- **Every historical crash gets a regression test** in `tests/regressions/`, and the
  mutation fuzzer must not rediscover it.

## Running everything

```sh
cargo test --workspace            # unit and round trip
cargo xtask policy                # Gate 0, including fmt, clippy and a release build
cargo xtask fixtures --check      # the corpus is reproducible
cargo xtask gauntlet --seed N     # the acceptance run; see FINISH.md
```
