# Bugs

Every defect this project has found, in one log, with the severity scale FINISH.md gives it:

* **S1** — data loss, corrupted output, crash, UI hang > 5 s, silent content loss, redaction leak,
  or signature invalidation without warning.
* **S2** — wrong result of a P0 feature, visible render error on P0 content, invalid-but-openable
  output, or missing P0 UI.
* **S3** — cosmetic or non-P0.

Acceptance requires no S1 or S2 open. Every entry below is **closed**, and each names the test that
caught it, because a bug whose test is not named is a bug that can come back.

## Closed

### B1 — An insertion with no whitespace of its own became part of the token it landed in

**S1.** `pdfjs__TAMReview.pdf` writes `0.000 Tc(Working Papers on Information Systems)Tj` — an
operator immediately against its operand, which the lexer handles and which nothing in the file
forbids. Inserting a `q`-wrapper at the start of that string produced `Tcq 1 0 0 1 …`: `Tcq` is **one
keyword**, so the wrapper was not an operator at all and the `Q` meant to close it closed nothing.
The stream was left unbalanced and the page drew under a wrong graphics state, with nothing
anywhere reporting it. The same class applies to deletions, where a delete that leaves `Td` against
`Q` writes `TdQ` and removes an operator from the page.

*Caught by* `crates/mangle-edit/tests/edits_corpus.rs`, which runs the interpreter over the result.
Every unit test in `mangle-edit` had passed, because a fixture always writes whitespace.

### B2 — A colour written operands-after-operator

**S1.** A recolour emitted `rg 0.784 0.063 0.18`. A content stream is written operands first:
`0.784 0.063 0.18 rg`. What was written instead reads as an `rg` with *no operands* followed by four
stray numbers, so the colour was never set, the next operator quietly collected the numbers as its
own operands, and the page rendered perfectly with **the old colour**.

*Caught by* the same corpus test, which asserts the colour the interpreter reports rather than the
patch text. The unit test that had asserted the text passed.

### B3 — An inline image's provenance span was one byte

**S1.** `mangle-content`'s tokeniser gave an inline image the span `start..start + 1` — the `B` of
`BI` — instead of the whole `BI … EI` region. Every content test passed, because a span that is too
short does not change what is *read*. It changed what was *written*: the first edit on
`pdfjs__TAMReview.pdf` inserted its wrapper at that byte, split the keyword into `B` + `I`, and the
image vanished from the reopened page with nothing reporting it.

*Caught by* `crates/mangle-edit/tests/writeback_corpus.rs`, an edit → save → reopen round trip.
GOAL.md §4.1's seventh law is that an object knows exactly which bytes produced it, and a span that
is a lie about that is a provenance defect before it is a rendering one.

### B4 — A stream split back into parts by the lengths they started with

**S1.** The editing session needed the page's content streams back as separate objects, and split
them by the lengths they started from. An edit that inserted bytes into the first stream therefore
handed them to the second: the save moved bytes into the wrong object, and nothing reported it.
The split now goes by where the patches landed, which is what the save already did.

*Caught by* `crates/mangle-edit/src/session.rs`'s own tests, which assert a save writes the stream
that changed and no other.

### B5 — The operand order, a third time

**S1.** A text property was emitted as `Tc 2` rather than `2 Tc` — a `Tc` with no operands followed
by a stray number, so the property was never set and the page drew exactly as it did. The third
occurrence of B2's shape is what settled the lesson, and the comment on the function that formats it
now says so.

*Caught by* `crates/mangle-edit/tests/edits_corpus.rs`'s text-property test.

### B6 — Arrange moved the object the wrong way

**S2.** Later operators draw on top, so a move to the *front* is a move to **after** the neighbour
and a move to the back is a move to **before** it. The first version had it inverted: an arrange that
appeared to work and moved the object the opposite way from the one asked for.

*Caught by* the arrange unit test that reads the bytes back and asserts the order of the two images.

### B7 — A stroked line had no box to click on

**S2.** GOAL.md §4.2 asks for "exact bounds (including stroke width and glyph bounds)". The stroke
half was missing, so a horizontal rule drawn `2 w S` had a box of **zero height** and a click
square on the ink — a quarter of a unit above the centreline — missed. `Record::bounds` now grows a
stroked path's box by half the *device* width, which is where a stroke actually sits; a filled path's
box is still its geometry alone.

*Caught by* a test in `crates/mangle-content`, written from the complaint rather than from the code.

### B8 — An insertion glued onto `Tc` again, in the arrange path

**S1.** Arrange inserts the moved bytes and their re-materialised state at the destination. Without
a separator between the state it wrote and the bytes it moved, the two joined into one keyword for
exactly the reason B1 gives. Fixed by the same rule, applied at the insertion rather than at the
edit.

*Caught by* the arrange unit test that asserts the result is still balanced.

## What the pattern is

Seven of the eight are the same shape, and it is worth stating plainly rather than filing them
individually: **a byte-range edit that does not carry its own separators, or writes its operands
after its operator, produces a stream that is a different stream.** Nothing downstream reports it.
The renderer draws a plausible page, the file opens, the edit appears to have worked — and the change
is simply not there.

The second thing they have in common is that **every one passed the unit tests that existed at the
time** and was caught by a test that runs the interpreter over the result on a real corpus page. That
is the argument STATUS.md's preamble makes, and these eight are the evidence for it: the fixture
decides how much of a renderer's — or an editor's — arithmetic it actually exercises, and a fixture
written to be simple is symmetric in exactly the way that hides this class of mistake.

The rule that follows, and which the corpus tests now enforce: **assert the effect, not the string.**
