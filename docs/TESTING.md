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
