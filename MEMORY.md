# MEMORY.md

Session continuity notes. Not part of the project spec — see `PLAN.md` for planning,
`docs/STATUS.md` for the living dashboard.

## Where the project stands

Not finished. **Gate 0 is green** (10/10). **Gate 3.1 still fails** and Gates 4–9 have no
acceptance evidence — there is no `docs/acceptance/` directory. Milestone **M4 is mostly done**
(surgical write-back, edits, arrange, session, save→reopen, and the window wired to all of it);
**M5–M12 are unstarted**.

Test suite: **1120 passing, 0 failing, 1 ignored** over 39 binaries. The ignored one is the
Tier-B wild-corpus run.

## Conventions worth reusing

- Test modules carry `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` with a
  comment saying the panic-free rule is about what the product does with a file — see
  `mangle-content`'s test mods. Add `float_cmp`, `indexing_slicing` where the tests need them.
- A byte offset in a test is **computed from the fixture** (`windows(5).position(…)`) rather than
  hand-counted, because a hand-counted one is wrong the moment the fixture's whitespace changes.
- A span from `Record` is **the operation that drew it** (`/PxARRO Do`), not the `cm` that
  positioned it — so moving a picture means editing the `cm` before it.
- **Assert the effect, not the string.** Three bugs shipped that every unit test passed because
  the test checked the patch *text*; the corpus test checked the value the *interpreter reports*.
- A fixture must be **asymmetric**. A symmetric page scores above 0.99 whether or not the
  arithmetic is right; the diagonal-clip page scored 0.51652 as a box and 0.99639 as a path.
- Everything the window decides is separated from the windowing it is decided in
  (`pointer_outcome`, `panel_row`, `stepped`, `panel_for`), so it is tested without a mouse.

## Memory discipline (this machine has 15 GB and swapping hangs it)

- Build with `CARGO_BUILD_JOBS=2`; for the gate, `ulimit -v 10485760` and run it detached:
  `setsid nohup bash -c 'ulimit -v 10485760; CARGO_BUILD_JOBS=2 cargo xtask policy' > log &`.
- Do **not** run the full `cargo xtask policy` unbounded; it builds everything in release and has
  exhausted memory.
- The GUI binary holds a few hundred MB while open: kill it once a screenshot has been taken.
- A full corpus run is ~4.6 h wall-clock, single-threaded, CPU-bound in an unoptimized build, and
  runs under a hard cgroup ceiling (see `docs/TESTING.md`).
- `/tmp/opencode` is wiped across server restarts, so a run's log can vanish while the report under
  `corpus/wild/report/` survives — check `SUMMARY.md`, not the log.
- `cargo xtask` only resolves because of the untracked `.cargo/config.toml`. Without it, use
  `cargo run -p xtask --`.

## Two bugs worth remembering

**egui calls `ui` only when something asks for a repaint.** A window that had drawn its chrome and
was waiting on a worker never pumped the channel again, so the finished page sat unread and the
window looked *exactly* like a working one in a screenshot. Found by watching the worker's CPU stop
at one page's render. Fix: `request_repaint_after(33ms)`.

**The canvas answered for the whole window.** A click on a stepper in the right panel reached the
pointer handler first, missed the paper, and took the selection away: the panel redrew itself with
nothing selected, its controls were never drawn, and the click did nothing. Fix: the handler returns
early unless the press is on the page.

## What is left, in the order it blocks

1. **M5 text editing** — the first rung has landed (`Change::Retype`: one run, plain
   `Tj`, own font and encoding, verified through the interpreter, end-to-end on a corpus
   page). Still open: reflow inside the block's original width, the font-resolution ladder
   of GOAL §4.4 beyond the own font (same family in the document → bundled/system subset →
   metric match → per-script fallback, each with its notice), and subset embedding for
   glyphs the font does not have. The panel's steppers and the `TextProperty` edits exist;
   *typing* through the panel does not yet.
2. **UI breadth** — thumbnails that navigate, bookmarks and layers panels, the tool-tab row.
3. **Type 3 fonts** — design written in `PLAN.md`, not built. Corpus scope is three files.
4. **Gate 3.1** — 164 of 740 corpus pages below 0.95; ~62 are JPEG 2000 (declined, D22) and ~38 are
   this project's font-substitution policy (D24). Both are decisions, not defects.
