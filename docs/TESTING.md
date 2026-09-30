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

Tier B is real-world files in `corpus/wild/`, each with a `SOURCES.md` naming the
origin and licence. Tier C is held out until final acceptance and must stay unread until
then.

Regenerate and check determinism:

```sh
cargo xtask fixtures --seed 20260101          # write fixtures/
cargo xtask fixtures --seed 20260101 --check  # regenerate and compare byte for byte
```

## Tolerances

Measured comparisons state their tolerance in the test, and say why:

- Text: exact, or word-level F1 ≥ 0.99 against an oracle.
- Renders: identical, or ±1 least-significant bit per channel, or SSIM ≥ 0.99.
- Geometry: ±0.01 pt, or ±0.1 pt where font rasterisation is in the path.
- Colour: ±2/255 per channel for an analytically computed sample.

## Rules that are not negotiable

- **No ignored tests.** `#[ignore]` counts as a failure in Gate 0 and in the gauntlet,
  except a skip for an absent oracle, which must be listed in the report.
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
