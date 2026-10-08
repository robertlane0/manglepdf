# MEMORY.md

Session continuity notes. Not part of the project spec — see `PLAN.md` for planning and
`AGENTS.md` for the working rules.

## Where the project stands

Not finished. Gate 0 is fully green (10/10, 960 tests). **Gate 3.1 still fails** and Gates 4–9
have no acceptance evidence — there is no `docs/acceptance/` directory and no `docs/BUGS.md`.
Milestones M4 onwards are unstarted (`docs/STATUS.md`).

## Completed this session

1. **Re-baselined the Tier B corpus** under a hard cgroup ceiling:
   `systemd-run --user --scope --quiet -p MemoryMax=8G -p MemorySwapMax=0`. All 77 files,
   16642.40s, 0 panicked, 745 pages, 740 with an SSIM. Page median 0.96020, 164 pages below
   0.95 (22.2%), per-file median (worst page) 0.97375. Peak RSS 1.6 GiB, so the cap was
   headroom, not the constraint. Figures are in `docs/known-diffs.md` and `docs/STATUS.md`.
   **Verify the ceiling took** — `systemctl --user show <scope> -p MemoryMax -p MemorySwapMax`.

2. **D30 — fixed a real corpus gap.** `fips197` page 1 refused as `ICCBased` with no
   `/Alternate`. The premise was wrong: an ICC profile names its own space at bytes 16–20 of its
   header. Page 1 **0.7564 → 0.9932**; `issue10529` 0.9886 → 0.9973. The expensive detail:
   `fips197` stores its profiles behind `/FlateDecode`, so the header is behind the filter —
   reading `Stream::raw` finds nothing. Both paths now covered by tests.

3. **Fixed the `cargo xtask` alias**, which was breaking every documented command, and added a
   G0.9 check so it cannot regress. Verify by deleting `.cargo/config.toml`: G0.9 fails.

4. **Refused a picture whose decode reported damage** (see "The open fix" below), and corrected the
   mis-encoded fax fixture that blocked it.

## Two changes made and reverted (both recorded in D31)

- **Image memory bound in bytes, not pixels.** Arithmetic was right (a 1-bit CCITT stencil is
  14 MB, not 453 MB) but it drew the corpus's worst page *worse*: 0.5716 → 0.4718. The decode
  was desynchronised, so drawing replaced one wrong page with a worse one.
- **Hypothesis that `WHITE_TERMINATING[2]` was mistranscribed — wrong.** The reasoning held at
  every step; the table was fine. `crates/mangle-filters/tests/ccitt_libtiff.rs` compares 14
  fixtures byte-for-byte against libtiff: **14/14 matched before the edit, 4 failed after.**
  Then asked directly: encoding *white 2, black 1, white 2* through `tiffcp -c g3:1d` gives the
  line `011101 011000`, which the repo's tables decode as **white 2** (`0111`), black 1 (`010`),
  white 5 (`1100`). **libtiff emits `0111` for white 2 — the table was right.** The failing
  fixture encoded white 2 as `011`, a valid *prefix* that walks on looking for a longer code.

  To reproduce: use `scripts/ccitt_fixture_tiff.py`'s `write_bilevel_tiff`, then
  `tiffcp -c g3:1d`, then read the strip at tag 273 with length from tag 279.

**Lesson worth keeping: the oracle was in the repo the whole time. Two failing hand-written tests
were the symptom, not the bug.**

## The open fix — now landed

`pdfjs__freeculture.pdf` pages 171 and 255 hold a Group 4 stencil whose decode desynchronises
(232 damaged rows, `Decoded::complete == false`), after which rows alternate between entirely
black and nearly empty. Ink fraction 0.553 against the oracle's 0.471.

**Done:** a **picture** whose decode reported `Decoded::complete == false` is refused with a note
naming the size, in `crates/mangle-render/src/image.rs`. Pictures only — masks are judged on their
own terms and the verdict would refuse some that pass. **Caveat recorded in D31: the corpus does
not exercise it**, because the size bound fires first on both pages; `freeculture` re-measured
identical (0.5716 / 0.8688, median 0.9606, 10 below 0.95). It earns its place by making a future
raise of the bound safe.

Also fixed: the `fax_image` fixture encoded white 2 as `011`, a code T.4 does not have (it is a
prefix that never terminates). libtiff emits `100010011100` for that row; the fixture now does too.
**That fixture was wrong for as long as it existed and nothing caught it.**

## What is left

**The image bound thread is closed.** `MAX_IMAGE_PIXELS` is still charged per pixel, over-refusing
1-bit stencils by 8×. Measured against the corpus *before* writing the change: every image a
byte-based bound would admit is either a `freeculture` stencil that the decode verdict then refuses,
or the `/SMask` of a 2×2 image in `issue16263`, which is asked the pixel bound anyway. It would
change **no page**, so it was not written. Recorded at the end of D31.

**Next candidates, none of them a corpus number:**
- `pdfjs__TAMReview.pdf` — 22 pages below 0.95, all unembedded-font (D24 policy).
- `pdfjs__freeculture.pdf` — 10 pages, dominated by 2 pages whose stencil decodes damaged.
- M4+ (page objects, select/move/scale, undo/redo) — entirely unstarted, and the real bulk of the
  remaining project. Gates 4–9 need it.

## Things that will waste time if forgotten

- `cargo xtask` only resolves because of the untracked-style `.cargo/config.toml` I added. Without
  it, use `cargo run -p xtask --`.
- A full corpus run is ~4.6 h of wall clock, single-threaded, CPU-bound in an unoptimized build.
- `/tmp/opencode` is wiped across server restarts, so a run's log can vanish while the report
  under `corpus/wild/report/` survives — check `SUMMARY.md`, not the log.
- Do **not** run the full `cargo xtask policy` unbounded; it builds everything in release and has
  exhausted memory. Use `ulimit -v 10485760` and `CARGO_BUILD_JOBS=2`.