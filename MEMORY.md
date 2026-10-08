# MEMORY.md

Session continuity notes. Not part of the project spec — see `PLAN.md` for planning.

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

**M4 has started.** Three modules in `crates/mangle-edit`:
- `history.rs` — `History<T>` over `Arc`-shared immutable snapshots, bounded at 512 steps. The
  bounded case needed **rebasing**: dropping the oldest `Edit` leaves the next one's `before`
  naming a state nobody saw, so `trim` copies the dropped entry's `before` onto the survivor. A
  test found that.
- `page_objects.rs` — `PageModel`, grouping glyph runs into selectable lines over the byte spans
  write-back needs, plus `tests/page_objects_corpus.rs` which runs it over four real corpus pages.
- `surgery.rs` — the write-back. `Patch` ranges applied by position, overlap and out-of-range
  refused up front, `is_balanced` for the result, plus `tests/surgery_corpus.rs` which scales an
  image on a real page and proves every byte outside the patched range is identical.
- `edits.rs` — the edits. `Change` (transform / delete / recolour) becomes patches over a
  `PageModel` object's spans, each a `q … Q` wrapper around the object's own operators so those
  bytes come out untouched, plus `tests/edits_corpus.rs` which runs move, delete and recolour over
  three real pages and checks the effect *through the interpreter*, not the patch text.
- `arrange.rs` — the one edit that is not a wrapper. Arrange changes the order operators appear
  in, so the bytes move and the CTM, colours, stroke width, caps, joins and dash are written back
  around them. Text, clipped objects, alpha and blend are all refused **by name** (the record does
  not carry the text state, a clip's pathOps, or a named `/ExtGState`) — 201 objects refused to
  582 arranged across three corpus pages, and the refusals are the finding.
- `writeback.rs` — the save. `save_page` appends an incremental update, rewriting only the content
  streams that changed and re-encoding them with the file's own filter, plus
  `tests/writeback_corpus.rs`: edit → save → reopen, with the untouched image's SHA-256 asserted
  unchanged. That is FINISH.md S2's exit criterion and it passes.
- The CLI can drive it: `cargo run -p mangle-cli -- edit FILE --page 1 --list`, then `--object I
  --move 10,5 | --scale 1.2 | --delete | --colour r,g,b -o OUT.pdf`. It prints what it selected,
  what the save rewrote, whether the file was appended rather than rewritten, and whether the
  reopened page shows the change. A stroke-only path refuses a recolour ("a Path has no fill
  colour to change"), which is the honesty law working through the CLI.

**Running a model over real corpus pages found a bug no unit test could.** `Record::bounds` for a
glyph run took the union of glyph *origins*, and every glyph on a line shares a baseline — so the
boxes had **zero height**. Grouping still looked plausible on fixtures because runs on one baseline
have identical `y`. `bounds_of` now derives ascent/descent from the placement's em (0.75/0.25).
`f1040` went 613 → 589 objects, 102 → 81 lines. **Keep the corpus test; it is the check that
catches this class of mistake.**

Conventions worth reusing:
- Test modules carry `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` with a
  comment saying the panic-free rule is about what the product does with a file — see
  `mangle-content`'s test mods. Add `float_cmp`, `indexing_slicing` where the tests need them.
- `Snapshot<T>`'s `Clone` and `PartialEq` are hand-written, not derived: a derive would put
  `T: Clone` on both, and avoiding that bound is the point of the type.
- `clippy::single_range_in_vec_init` fires on one-element range lists; allow it in test mods
  rather than writing `vec![x][..].to_vec()`.

**Next in M4:** the edits are built (`edits.rs` — a `Change` becomes patches over a
`PageModel` object's spans, each a `q … Q` wrapper around the object's own operators), so is the
save (`writeback.rs` — `save_page` appends an incremental update, rewriting only the content
streams that changed and re-encoding them with the file's own filter), so is **Arrange**
(`arrange.rs` — the one edit that cannot be a wrapper, so the bytes move and the state they were
drawn under is re-materialised at the destination), and so is the **session** (`session.rs` —
`Editor` owns the document, the page, the resources and the `History`, so an undo restores the
content byte for byte and a redo returns it). FINISH.md S2's exit criterion — edit → save → reopen
with the untouched JPEG stream's SHA-256 unchanged — is asserted in `tests/writeback_corpus.rs` and
passes; `tests/session_corpus.rs` does the whole loop over a real 4-stream page. What is left in M4
is the UI wiring.

Two conventions the surgery and edits tests rely on, both worth reusing: a byte offset in a test is
**computed from the fixture** (`windows(5).position(…)`) rather than hand-counted, because a
hand-counted one is wrong the moment the fixture's whitespace changes; and a span from `Record`
is **the operation that drew it** (`/PxARRO Do`), not the `cm` that positioned it — so moving a
picture means editing the `cm` before it, which is the first thing to get wrong when writing an
edit from a record's span.

**Three bugs the corpus tests found that every unit test passed, all recorded in `PLAN.md`:**
**an insertion that carries no whitespace of its own glues onto the token it lands in** (a real page
writes `0.000 Tc(Working Papers)Tj`; inserting `q …` there produced `Tcq`, one keyword, so the
wrapper was never an operator and the stream was unbalanced), **a colour written
operands-after-operator** (`rg 0.784 0.063 0.18` rather than `0.784 0.063 0.18 rg` is an `rg` with no
operands, so the colour never changed and the page rendered perfectly with the old one), and **an
inline image's span was one byte** (`mangle-content` gave it `start..start+1`, the `B` of `BI`, so
the first edit split the keyword and the image vanished from the reopened page). The second is the
one to remember: my unit test asserted the patch *text* and passed, the corpus test asserted the
*colour the interpreter reports* and failed. **Assert the effect, not the string.** And a span that
is too short passes every read test and breaks the first write — provenance is what the write-back
rests on.

**The Text State is on `Record` now** (`text` + `text_matrix`), so arrange can write a moved run
back instead of refusing it: three corpus pages went from 201 refusals to 168 and 582 arrangements
to 615. §4.4's text controls are built on it too — `--tc/--tw/--tz/--tl/--ts/--size` (and `Tr`) are
each a `q … Q` around the run's own `Tj` with the one operator inside it.

**Third occurrence of the operand-order bug, and it settled the lesson:** the text property was
emitted as `Tc 2` instead of `2 Tc`, so the property was never set and the page drew exactly as it
did. The unit test asserting the patch text passed; the corpus test asserting the value the
interpreter reports caught it. There is now a comment on the function that formats it.

**And the verification for a text property had to move, for a reason worth knowing: a text property
*moves the glyphs*.** On a sheared text matrix the run spreads along the shear, so it leaves its
line and the model regroups — `irs-f1040` page 1 goes 589 → 656 objects on one character-spacing
change, correctly. So the text property is verified by matching the run **by its string** and reading
the value back, not by the object count or the old centre.

**Next after that, in order:** the UI wiring (the `mangle-ui` window shell), then §4.4's Text
Properties panel and layout (recompute positions / `Tw` inside the edited block's original width,
keeping untouched lines' operators byte-identical).

Two conventions the surgery tests rely on, both worth reusing: a byte offset in a test is
**computed from the fixture** (`windows(5).position(…)`) rather than hand-counted, because a
hand-counted one is wrong the moment the fixture's whitespace changes; and a span from `Record`
is **the operation that drew it** (`/PxARRO Do`), not the `cm` that positioned it — so moving a
picture means editing the `cm` before it, which is the first thing to get wrong when writing an
edit from a record's span.

**Remaining corpus candidates, none of them a corpus number:** `TAMReview` (22 pages, D24 policy),
`freeculture` (10 pages, 2 with damaged stencils).

## Things that will waste time if forgotten

- `cargo xtask` only resolves because of the untracked-style `.cargo/config.toml` I added. Without
  it, use `cargo run -p xtask --`.
- A full corpus run is ~4.6 h of wall clock, single-threaded, CPU-bound in an unoptimized build.
- `/tmp/opencode` is wiped across server restarts, so a run's log can vanish while the report
  under `corpus/wild/report/` survives — check `SUMMARY.md`, not the log.
- Do **not** run the full `cargo xtask policy` unbounded; it builds everything in release and has
  exhausted memory. Use `ulimit -v 10485760` and `CARGO_BUILD_JOBS=2`.