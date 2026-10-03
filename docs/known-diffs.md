# Known differences from the oracles

FINISH.md G3.1: *"per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31,
and Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root
cause and evidence."*

**The Tier-B target is not met, and not narrowly.** Against `mutool draw` at 150 DPI over the
77-file wild corpus: **460 of 542 pages were compared** and the median SSIM is **0.6929**, and
**436 of those 460 pages (94.8%) are below 0.95**. Thirty-four of the 77 files could not be
opened at all; **four cannot** now that D2 is fixed. This page records what the corpus found
and the evidence for each claim.

The corpus is `corpus/wild/`, pinned by SHA-256 in `corpus/wild/MANIFEST.toml`, fetched with
`cargo xtask corpus fetch`, and measured by `crates/mangle-render/tests/wild_corpus.rs`.
That test asserts nothing: the numbers below come from `corpus/wild/report/SUMMARY.md` and
the per-file reports beside it, which are git-ignored and regenerated on every run. Oracle
versions: `mutool 1.28.5`, `pdftotext` (poppler), `qpdf` (system).

> **The SSIM figures below predate [D2](#d2--a-cross-reference-stream-could-never-be-read-at-all-so-every-pdf-15-file-reported-zero-pages)
> and are now too pessimistic by one file in three.** They were measured when 34 of the 77
> files could not be opened at all, and a file that cannot be opened contributes no pages
> and so no SSIM. **73 of 77 now open**, against 43 before, and every one of them reports
> `as written` — the reader uses the file's own cross-reference information and repairs
> nothing. The 460-of-542 comparison count, the 0.6929 median and the 94.8%-below-0.95
> figure all describe a corpus with a third of it missing, and none of them should be read
> as a fidelity number until the corpus is re-run. The two-hour re-run is the next thing
> wanted, and this file will be wrong again until it happens.

**The test is `#[ignore]`d, on purpose.** One run over the corpus takes about two hours, so
leaving it in the default suite makes `cargo test --workspace` unusable. The two cheap tests
beside it — which check the harness itself — still run by default.

Reproduce any line:

```sh
cargo xtask corpus fetch
cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture
```

The 460/542 split matters as much as the SSIM does: **80 pages produced no SSIM at all**,
because their buffers came out one pixel larger than the oracle's and `compare()` refuses
images of different sizes. That was D3, and D3 is now fixed.

### The four files that still do not open

All four are reported, none is guessed at, and each is listed here so the count of 73 is
auditable rather than asserted.

| file | reason | why that is the right answer |
|---|---|---|
| `pdfjs__issue21579.pdf` | `the document is encrypted: incorrect password` | Encrypted, and the password is not derivable. Correct behaviour. |
| `pdfjs__bug1020226.pdf` | `no catalogue` | A 184-byte stub. `qpdf --check` ends `unable to find trailer dictionary while recovering damaged file`. There is nothing to read. |
| `pdfjs__REDHAT-1531897-0.pdf` | `no catalogue` | 871 bytes of fuzzed xref stream. `qpdf --check` ends `unable to find /Root dictionary`, having already rejected object 13 for `overflow/underflow converting 166666666666666666666666666`. |
| `pdfjs__issue19484_1.pdf` | `dangling reference: the page tree root is missing` | **A misdiagnosis, and the one real thing left here.** The file is encrypted — its xref stream carries `/Encrypt 16 0 R`, its manifest lists `encrypted`, and `qpdf` also refuses it with `invalid password`. This reader accepts an empty user password and then cannot read the content: the catalogue resolves, but `/Pages` is object 13 inside object stream 4, and that container decodes to **0 bytes** from 50. So a document that needs a password is reported as one with a broken page tree. The error is in the encrypted-content path, not in the cross-reference reader, and fixing it means deciding whether an empty password that validates should be trusted. Left as it is rather than papered over. |

---

## D1 — `render_page` rendered a blank page for any Flate-compressed content stream

**Severity: was critical — every page of every file. FIXED, and it was smaller than it looked.**

`render_page` called `Page::contents`, which returns the content stream's *encoded* bytes,
and handed them to the content-stream interpreter. The interpreter then reported the
compressed bytes as operator names and drew nothing. The decoder was never at fault:
`Page::decoded_contents` existed, worked, and was used by text extraction and by the CLI.

One line in `crates/mangle-render/src/page.rs`:

```rust
let content = page.contents(doc);          // encoded bytes
let content = page.decoded_contents(doc);  // decoded bytes
```

### Why the fixture suite missed it

Every hand-written fixture wrote its content stream unfiltered. `text_page`'s own comment
says the choice is deliberate — an embedded font program is written uncompressed so that a
comparison measures outlines rather than the font loader — and `page_with` wrote content in
the clear for the same reason. Nothing in the suite ever asked the renderer to inflate
anything, so no test could tell a renderer that decodes content from one that does not.

The fixture is now `page_with_flate`, which is `page_with`'s content behind `/FlateDecode`
and nothing else. Four tests in `crates/mangle-render/tests/pages.rs` hold it down, and all
four fail against the old code:

| test | what it pins |
|---|---|
| `a_compressed_content_stream_is_decoded_and_not_read_as_operators` | the regression: same content, filtered, same three marks and same four pixels |
| `a_content_stream_that_will_not_decode_is_reported` | a damaged `FlateDecode` names itself in the notes |
| `a_content_stream_behind_an_unknown_filter_is_reported_by_name` | an unimplemented filter is named, and its bytes are not drawn |
| `an_array_of_content_streams_names_the_one_that_failed` | a note says *which* of several `/Contents` streams failed |

### What the fix actually bought, measured

Before, **all 542 pages of the 77-file wild corpus rendered with `marks: 0` and no ink.**
After, 103 pages draw and 439 are still blank. So D1 was the cause of the blankness on
19% of pages, and on the other 81% it was hiding a second, larger defect (D7) behind silence.

The page this finding was opened with barely moves, and the reason matters:

| `gov__arxiv-1206.5537.pdf` page 1 | marks | ink pixels | SSIM |
|---|---|---|---|
| before | 0 | 0 | 0.791534 |
| after | 58 | **0** | 0.791534 |

Identical to six places. **The page is blank both before and after**, for two different
reasons: before because the content was never decoded, after because all seven of its fonts
refuse (D7). The SSIM of a blank page against a page with 5.4% ink is 0.7915 whatever the
reason for the blankness is, so this figure could not have moved and its not moving is not
evidence that the fix failed. Two other pages show the difference D1 made:

| page | before | after |
|---|---|---|
| `pdfjs__alphatrans.pdf` 1 | 0 marks, 0 ink, **0.89054** | 12 marks, 373 262 ink, **0.88921** |
| `pdfbox__openoffice-test-document.pdf` 1 | 0 marks, 0 ink, **0.99954** | 4 marks, 315 ink, **0.99998** |

`alphatrans` goes *down* by 0.0013 while gaining 373 262 ink pixels, because the page has a
shading named `/Sh1` that its resources do not define, so the part we now draw is drawn
without it. Drawing a page wrongly scores worse than not drawing it, and it also reports
why — which is the trade the second half of this fix is for.

**The high scorers in the old "What is not wrong" list were an artefact of this bug.**
`openoffice-test-document.pdf` scored 0.9995 while rendering nothing whatsoever, and scores
0.99998 now that it draws its 315 pixels. A blank page compared against a nearly-blank
oracle is a high score for the wrong reason, and the median was computed over 436 pages of
which the great majority were blank.

### The second half: a stream that will not decode has to say so

Decoding the content is the fix; *reporting* it is what stops the next file being another
two-hour mystery. A page that lost its content to a broken filter and a page with no content
look identical on screen and mean opposite things, so the reason now travels with the bytes:

- `Resolver::decoded_full` is the new primitive and returns the decoder's own `Decoded`,
  notes and all; `Resolver::decoded` is defined in terms of it, so the two cannot disagree.
- `Page::decoded_contents_full` concatenates the streams and prefixes every note with which
  stream it came from — `content stream 2 of 4: FlateDecode: input ended before a block
  header` — because `/Contents` is an array at least as often as it is one stream and "the
  second of four failed" is a different finding from "something failed".
- `Decoded` gained an `encoded` flag, because "the decode went wrong" and "these bytes are
  not decoded" are different facts. A truncated `FlateDecode` gives a prefix that is worth
  drawing; an **unimplemented** filter gives back bytes that are still encoded, and handing
  those to the interpreter is D1 all over again. When `encoded` is set the content path
  refuses and reports. That test deliberately writes readable content behind
  `/NoSuchDecode`, so that a renderer which drew it anyway would produce a plausible page
  and pass.

Nothing here guesses. There is no fallback that treats encoded bytes as content, and no
substitute drawn in place of content that could not be read.

---

## D2 — a cross-reference stream could never be read at all, so every PDF 1.5+ file reported zero pages

**Severity: was critical — 34 of the 77 files. FIXED. The corpus now opens 73 of 77.**

> **This entry previously named the wrong cause.** It said the PNG predictor in
> `/DecodeParms` was not decoded, and the corpus correlation below made that look
> airtight: every file with an xref stream and a predictor failed, and the one file with
> an xref stream and no predictor opened. That correlation was a coincidence of who writes
> xref streams — most PDF 1.5+ producers emit Flate with `/Predictor 12` — and the
> predictor decoder was never at fault. It is exercised by `stream::tests::` and works.
> Two defects sat between `startxref` and the predictor, both upstream of it. The
> correlation table is kept below as a record of how a plausible story survived three
> bisection experiments that all exonerated the thing it blamed.

The 34 failures, by the reader's own reason string:

| reason | files |
|---|---|
| `dangling reference: the page tree root is missing` | **29** |
| `no catalogue` | 4 |
| `the document is encrypted: incorrect password` | 1 |

All of the first two rows were this one bug; the encrypted file is legitimately unreadable
without a password.

### The first defect: `next_object` cannot return a stream

A cross-reference stream is an *indirect stream object* — `N G obj << dict >> stream …`.
`Parser::next_object` is a parser for **direct** objects: it sees `<<`, returns the
dictionary, and stops. It never looks for the `stream` keyword, so it can only ever return
an `Object::Dict`, never an `Object::Stream`. `Document::parse_at` knows this and assembles
the stream itself; `xref::read_section` did not, and asked:

```rust
let Some(Object::Stream(stream)) = p.next_object().ok()? else {
    return None;
};
```

which is unsatisfiable. Every xref stream therefore failed to parse, `read_section`
returned `None`, and the chain came back with no entries and no `/Root`.

### The second defect: `/Index` was tested in a way that can never be true

Even once the stream parsed, its entries were numbered from zero regardless of `/Index`:

```rust
let index: Vec<i64> = match stream.dict.get("Index").and_then(Object::as_i64) {
    Some(_) => /* read the array */,
    None => vec![0, size],          // ← always this branch
};
```

`/Index` is an array, and `as_i64` of an array is always `None`, so `get(..).and_then(..)`
is `None` whether or not `/Index` is present. Both cases fell through to `vec![0, size]`.
An incremental update that touches two objects — `/Index [2399 1 2422 1 2448 2 2451 3]` —
had its seven entries attributed to objects 0 through 6, which is how
`pdfbox__acroform.pdf` came to resolve its `/Root 12 0 R` to a **font**.

### Why the symptom was `the page tree root is missing`

With no usable cross-reference information, recovery falls back to scanning for `N G obj`
headers. That scan cannot see objects stored inside object streams, so `/Pages` is
unreachable and the document reports **0 pages** with the note
`dangling reference: the page tree root is missing`. The error names the page tree; the
fault was two layers above it, in the file's table of contents.

### Evidence, `gov__irs-fw4.pdf` — **live IRS Form W-4**, which `qpdf --check` reports as clean

```
$ manglepdf-cli info corpus/wild/gov__irs-fw4.pdf          # before
pages       0
structure   repaired (3 note(s))
            - no cross-reference section at offset 208493
            - 112 objects were located by scanning
            - the catalogue was found by scanning
```

`startxref` is `208493`, and the bytes there are
`3594 0 obj <</Length 44/Type/XRef/Root 3540 0 R/… /Index[3540 1 3563 1 3589 2 3592 3]/W[1 3 1]/DecodeParms<</Columns 5/Predictor 12>>/Filter/FlateDecode>>`.
A well-formed cross-reference stream that qpdf reads without a warning.

Two experiments, run before the cause was found, are what sent the investigation after the
predictor rather than past it. Rewriting the *same content* with the streams expanded and
nothing else changed:

```
$ qpdf --object-streams=disable irs-fw4.pdf irs-fw4-nostream.pdf
$ manglepdf-cli info irs-fw4-nostream.pdf                    # then
pages       5
structure   as written
```

and bisection on `pdfjs__bug1885505.pdf`, which reports `no cross-reference section at
offset 116` over a valid stream: removing `/DecodeParms`, removing `/ID`, removing
`/Index`, changing `/W`, correcting `/Length` by ±3 bytes, and rewriting every bare CR as
LF each left the failure in place. Every one of those edits was upstream of the predictor
and none touched the actual fault, which is that the stream object was never assembled at
all. The fix was found by asking what `next_object` returns for `N G obj <<…>> stream`, not
by reading the file harder.

### The result

```
$ manglepdf-cli info corpus/wild/gov__irs-f1040.pdf
pages       2
structure   as written
objects     2452
```

The reader now uses the file's own cross-reference information: **`as written`, with no
repair at all**, on every file this bug had broken. Page counts match `pdfinfo` on every one
of the 73 files that open.

| | before | after |
|---|---|---|
| files opened | 43 | **73** |
| files not opened | 34 | **4** |
| of the 29, `the page tree root is missing` | 29 | **0** |

Of the 29, 27 write a cross-reference stream and two (`pdfjs__issue19484_1.pdf`,
`pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf`) write a classic table with no `/Root` at all; the
latter now opens by rebuild and is reported as repaired.

Also fixed by this: **IRS Form 1040**, **NIST FIPS 197** (52 pp), **NIST SP 800-88** (44 pp),
NIST IR 7657, both USGS 1:25000 topo sheets, all six Microsoft Word 2013 exports carrying
annotations, both JPEG 2000 files, every Adobe InDesign output in the corpus, and
`bug1885505.pdf` (a pdfTeX paper).

### Two smaller defects the same code was hiding

Both were ways of *inventing* structure, which is the one thing this reader must never do,
and both were found by the fix above rather than looked for.

**A damaged cross-reference stream was accepted as if it were whole.** `read_xref_stream`
returned whatever entries it managed to read when the decoded body ran out early, so a
stream that decoded to a third of what `/Index` promised produced a third of a table and
was believed. `pdfjs__PDFBOX-3148-2-fuzzed.pdf` has exactly this: a corrupt `/ASCII85Decode`
body that qpdf also rejects, promising `/Index [0 18]` and delivering three entries. The
section is now refused, so recovery rebuilds the file by scanning and says so.

**An object number a `u32` cannot hold was renumbered onto object 0.** Both the classic
table reader and the stream reader did `u32::try_from(start.saturating_add(i)).unwrap_or(0)`,
so a corrupt subsection header silently overwrote the free-list head with an unrelated
offset and the rest of the file looked intact. `pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf`
carries the header `4294967296`; the table is now refused and the file opens by rebuild,
correctly reported as repaired.

---

## D3 — the page canvas gains a spurious row whenever the size in points times the scale should be a whole number

**Severity: was high. FIXED. 80 pages across 17 files had become impossible to compare.**

`render_page` computed `(points * scale).ceil()` with the division hoisted into `scale`.
In IEEE-754:

```
792.0 * (150.0/72.0) == 1650.0000000000002   ->  ceil == 1651
792.0 * 150.0 / 72.0 == 1650.0               ->  ceil == 1650
561.6 * (150.0/72.0) == 1170.0000000000002   ->  ceil == 1171
960.0 * (150.0/72.0) == 2000.0000000000002   ->  ceil == 2001
```

So US Letter (612 × 792 pt) came out **1651 rows tall where `mutool` produces 1650**, and a
960-point side came out one column too wide. `compare()` refuses to compare images of
different sizes, so every such page was recorded as
`size disagreement: we rendered 1275x1651, mutool rendered 1275x1650` and **scored nothing**.
All 80 had the same shape — one dimension exactly one pixel too big:

| we rendered | mutool rendered | pages |
|---|---|---|
| 1275x1651 | 1275x1650 | 70 |
| 1153x1651 | 1153x1650 | 4 |
| 1250x1751 | 1250x1750 | 2 |
| 915x901 | 915x900 | 1 |
| 2001x1125 | 2000x1125 | 1 |
| 1575x1171 | 1575x1170 | 1 |
| 1020x1531 | 1020x1530 | 1 |

70 of the 80 are a US-Letter page, which includes **all 23 pages of an arXiv paper** and
**all 12 pages of a second one** — the corpus was measuring almost no LaTeX at all.

### What was wrong, and what was not

The ceiling itself is **correct** and was kept. A page of 208.33 pixels needs 209 rows to
hold its last third of a pixel, rounding down clips it, and `mutool` rounds up too. Rounding
to *nearest* would have fixed the Letter page and re-broken the small page, and 208 is a
worse answer than 209: it loses part of the page rather than adding an empty row.

What was wrong was that the ceiling was applied to a value floating point had already put a
hair above the integer. `1650.0000000000002` is one unit in the last place above 1650 and
`ceil` turns that into 1651. The error is invisible at small sizes — one ulp of 208.33 is far
too small to reach the next integer — which is exactly why a 100-point page rounded up
correctly and a Letter page never did. So the epsilon has to be *relative*, not absolute:
one wide enough for 1650 would swallow a quarter of a pixel on a 208-pixel page, and one
narrow enough to spare that page would miss 1650.

`pixels_for` in `crates/mangle-render/src/page.rs` now snaps a value within `1e-9` of an
integer — relative, so about 4.5 million ulps — to that integer before applying the
ceiling. 1e-9 is four thousand times narrower than the smallest difference a real page size
can produce: a page 612.0001 points wide is 1275.0002 pixels, so it still rounds up.

### The evidence, before and after

The oracle's own rule was checked directly rather than assumed, by rendering 15 page sizes
from 1×1 to 1224×1584 with `mutool draw -r 150` and reading the PAM headers. `mutool`
matches ceil-with-epsilon-snap on **all 15**; plain `ceil` disagrees on two of them (792×612
and 1224×1584) and plain `round` disagrees on six.

Then one affected file, `gov__arxiv-1206.5537.pdf` page 1 (MediaBox 612 × 792), rendered
and compared the way the harness does:

| | we rendered | mutool rendered | SSIM |
|---|---|---|---|
| before | 1275x1651 | 1275x1650 | *not measured — size disagreement* |
| after | **1275x1650** | 1275x1650 | **0.7915** (rms 35.4, max delta 224, 116 023 of 2 103 750 pixels above tolerance) |

The off-by-one is gone and the page is compared. The 0.79 is D1 talking — that page's
content stream is Flate-compressed and is drawn blank — so the off-by-one was hiding a real
defect behind an absence of a measurement, which is the worst way for it to hide.

Two tests hold this down in `page.rs`: `a_whole_number_of_pixels_is_not_rounded_up_to_one_more`
(1275x1650, 1650x1275, 2000x1125, 2550x3300) and `a_fractional_page_size_still_rounds_up`
(209x209 for 100pt, 1240x1755 for A4, 600x600), alongside the original
`a_page_buffer_rounds_up_so_the_edge_is_not_clipped`. A fix that traded one for the other
would fail the second.

The existing fixtures did not catch this either: they are 200 × 200 pt and similar, where
`200 × (150/72) = 416.666…`, and both renderers round up to 417.

---

## D4 — an uncompressed cross-reference stream is not read

**Severity: was high. FIXED by [D2](#d2--a-cross-reference-stream-could-never-be-read-at-all-so-every-pdf-15-file-reported-zero-pages) — it was the same bug.**

`pdfjs__bug1539074.pdf`, a pdfTeX-1.40.17 page, whose only cross-reference section is
`<</Type /XRef /Index [0 18] /Size 18 /W [1 2 1] /Length 72>>` — **no `/Filter` at all**, so
the entries are raw bytes.

```
$ manglepdf-cli info bug1539074.pdf                         # before
pages       0
            - no cross-reference section at offset 5715
```

The absence of `/Filter` had nothing to do with it. This file has no `/DecodeParms` either,
which is what made it look like the odd one out that would separate "uncompressed" from
"predictor", and it is the reason D2's correlation looked exact. The section failed for the
same reason as every other: `next_object` returns the stream's dictionary and never the
stream. Now:

```
$ manglepdf-cli info bug1539074.pdf                         # after
pages       1
structure   as written
```

It is left as its own entry because it is the file that falsified the predictor theory, and
that is worth being able to point at.

---

## D5 — the fax scan on a multi-page TIFF page rendered as a black rectangle

**Severity: was high — 0.38414 SSIM, 60.6% of pixels wrong. FIXED, as a codec defect and then
as the placement defect D5b that sat on top of it. The page is now at 0.99747.**

`pdfbox__multitiff.pdf` page 1, measured against `mutool draw` at 150 DPI:

| | SSIM | RMS | above tolerance | ink ours / oracle |
|---|---|---|---|---|
| before any of the image work | 0.38414 | 197.78 | 1 318 947 | 59.0% / 8.0% |
| layout and value split apart | 0.91170 | 72.09 | 173 990 | 0.0% / 8.0% |
| and the T.6 codec reading the coding line's coordinates | 0.86317 | 88.84 | 264 199 | 7.98% / 8.0% |
| **and the image placed where its matrix says, the right way up** | **0.99747** | **4.72** | **746** | **7.99% / 8.0%** |

The third row scores lower than the one above it, and that was worth being explicit about
rather than leaving to be misread: 0.91170 was the score for drawing nothing at all, and a page
with no ink on it agrees very well with a page that is 92% paper. The third row draws the whole
scan — 173 595 pixels of ink against the oracle's 173 990 — into the oracle's horizontal band
and into the wrong rows, which is D5b and the whole of the gap to 0.99747.

What was wrong is recorded in full in `docs/STATUS.md` and `PLAN.md`. There were four defects
in one place, and each was invisible until the one before it was fixed, because a fax decoder
hands back one byte per pixel and *layout* (which byte a pixel lives in) and *value* (what a
sample is worth) are two different facts that only one number was standing in for.

The fifth was the codec underneath. Decoding this page's image — object 12, 344 by 287,
`/K -1` — recovers **287 of 287 rows, byte for byte against `libtiff`, with 0 damaged rows
and nothing truncated**, where it recovered 15 before. The four defects in `decode_line_2d`
were: run lengths added to `b1`, a position on the *reference* line, instead of to `a0`, the
coding line's own current element; `b1_b2` choosing the reference element by the *reference*
line's colour at `a0` rather than the coding line's; the changing elements' polarity inverted,
so every element was labelled with the colour it changes away from; and three of the eight
`MODE_CODES` entries mistranscribed with a ninth — *Vertical(+3)* and the codes `0000010` and
`0000001` among them — missing or wrong. Against the frozen `libtiff` ground truth, **all 15
cases now match byte for byte**, 896 of 896 rows.

What was left on this page after the codec work was **not** the codec and never was. The page's
content is a full-page colour scan — object 12 is 344 by 287 samples covering the whole 595.28
by 496.64 point width and height the content stream gives it, not a numeral, and it is fully
decoded. It landed in the wrong device rows, for the reason in D5b, which is now fixed.

---

## D5b — `image::draw` rebased its writes to the page origin and mirrored its samples

**Severity: was high, and every image on every page was affected. FIXED — both errors, and the
axis is fixed in the mapping rather than in the buffer, which is the part a fix by inspection
gets wrong.**

`crates/mangle-render/src/image.rs`, in `draw`. Two independent errors, either of which alone
puts an image in the wrong place.

**1. The write is rebased to the clipped area; the bounds and the sampling are not.** The
rectangle is built from `matrix.apply`, which is in absolute device pixels, and
`area = bounds.intersect(device.clip())` keeps it there, so `area.pixels()` yields absolute
device rows and columns. The loop then walks those absolute coordinates and maps them back
through the inverse — correctly, in absolute device space — but writes with

```rust
device.put(x - x0, y - y0, px);
```

`x0` and `y0` are `columns.start` and `rows.start`, the origin of the clipped area.
`Device::put` is documented and implemented as an absolute device write: it bounds-checks
against the whole image and then writes at those coordinates. So every pixel is written at its
own position *minus the clipped area's origin*, and every image not at the page origin is
drawn at the page origin. The displacement is exactly the area's origin and nothing else.

Measured on the corpus: the placement rectangle is device `(0.4, 719.3)..(1240.6, 1754.0)`
and `area.pixels()` gives rows `719..1754`. The image's ink lands at rows 141..1020 — 719
rows high — and its columns match the oracle exactly, because the area's column origin is 0
(the image is full-page-width). Changing only that one line to `device.put(x, y, px)` moves
the ink to rows 860..1739, which is 719 rows down and exactly where the rectangle says it
belongs.

Reproduced without the corpus at all: a 100 by 100 point page whose only mark is
`40 0 0 40 60 10 cm /Im1 Do`, so the image belongs at device rows 50..90 and columns 60..100.
Its ink is at rows **0..39** and columns **0..39**.

The fix is to write at the coordinate the pixel was computed for. `x0` and `y0` are gone rather
than left at zero, because a binding named for an origin that is no longer subtracted is a
second defect waiting: `columns.start` and `rows.start` are still the clipped area's, and
reading them as a device origin is exactly the mistake that was just fixed.

**2. The vertical axis is mirrored.** `to_device` is `placement.matrix.concat(record.ctm)`,
and `placement.matrix` inverts y because a canvas counts down and a page counts up. So the
unit square's `(0, 0)` is the placement's *bottom* and `inverse.apply(..)` returns `v = 0`
there. `Raster::sample(u, v)` reads `v = 0` as raster row 0, which is the scan's *top* row.
The image is therefore drawn upside down. Only `v` is affected: `u` is correct, which is why
the corpus page's horizontal band matches the oracle exactly while its rows do not.

Measured on the corpus by matching each source row of the frozen `libtiff` raster against the
device row that best reproduces it: the device row falls by 3.606 per source row here and
**rises** by 3.604 in `mutool`'s. Same magnitude, opposite sign — 1035 device pixels over 287
source rows is 3.606, so the scale is right and only the sense of the axis is wrong.

Reproduced without the corpus: a 100 by 100 point page with `100 0 0 100 0 0 cm /Im1 Do` and an
8 by 8 image whose top half is red and bottom half blue. The top quarter of the page renders
`[0, 0, 255]` and the bottom quarter `[255, 0, 0]`.

The fix folds the flip into the transformation rather than turning the raster's buffer over,
and the distinction is the whole of it:

```rust
let to_image = matrix.concat(Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0));
let Some(inverse) = to_image.inverse() else { … };
```

**The mapping, not the buffer.** A raster's row zero is its *top* row and an image's unit square
puts row zero at `v = 1`, so the inverse has to yield `v = 0` at the placement's *top* edge. A
flipped raster buffer gives the same pixels as this mapping for every image drawn square to the
page — which is all of the corpus's images, and all of the fixtures — and gives different ones
the moment the image is turned, because the turn belongs to the placement and a flipped buffer
is not carried by one. `mutool draw` agrees with the mapping and not with the buffer: a
200-point page carrying `0 200 -200 0 200 0 cm` over an image whose four quadrants are four
colours puts the image's top-left — red — in the page's **lower** left, which is where a
counter-clockwise quarter turn takes the top-left corner of a picture. The buffer-flip version
puts red in the page's upper left. `a_quarter_turned_image_samples_the_right_way_round` asserts
the oracle's answer, pixel by pixel.

**Why it is not a clean translation, and why the row count looked wrong.** The two errors
compose into a mirror about the middle of the placement rectangle rather than a shift, and a
mirror is not something a vertical shift can explain. The ink *band* also moved by 127 rows
rather than by 0 or by 719, because the band's extent is set by the first and last inked source
rows and a mirror puts the top row at the bottom: content spanning source rows 4 to 248
rendered at rows 860..1739 where the oracle had 733..1612, both bands exactly 879 rows tall.

## D5b, measured

`pdfbox__multitiff.pdf` page 1 against `mutool draw` at 150 DPI, and how much of the oracle's
ink a **pure vertical translation** accounts for at its best. Every shift is tried, over
per-row bitmaps, because a figure that depends on which shifts were considered cannot tell a
mirror from a displacement. Ink is "at least half dark" on luma; the oracle's page has 173 990
inked pixels and ours has 173 764 (99.87%).

| | SSIM | RMS | above tolerance | ink ours / oracle | best shift | translation explains |
|---|---|---|---|---|---|---|
| both defects | 0.86317 | 88.84 | 264 199 | 7.98% / 8.0% | 650 rows up | **0.6270** |
| mirror fixed, rebase not | — | — | — | 7.99% / 8.0% | 719 rows up | 0.9972 |
| rebase fixed, mirror not | — | — | — | 7.98% / 8.0% | 650 rows up | 0.6270 |
| **both fixed** | **0.99747** | **4.72** | **746** | **7.99% / 8.0%** | **none** | **0.9972** |

Two things in that table are worth more than the SSIM. First, **a fraction on its own cannot see
the rebase**: with the mirror fixed and the rebase not, the translation explains 0.9972 of the
oracle's ink while the image sits 719 rows in the wrong place, because a shift absorbs a
constant displacement completely. Only the *shift* sees that defect. Second, **a shift on its
own cannot see the mirror**, because a mirror is not a shift. So the test asserts both, and
neither is the SSIM — a score cannot say what kind of difference a page has, and 0.91170 was
this page's score for drawing nothing at all.

`pdfjs__issue13372.pdf` page 1 is **unchanged at 0.74347**, and it was always going to be: that
page draws nothing, for the unrelated reason below.

**Why nothing caught it.** `crates/mangle-render/tests/pages.rs` had five tests that draw an
image — `an_image_is_drawn_through_the_resources`, the three `an_image_mask_…` tests, and
`an_image_that_is_not_a_mask_is_unaffected_by_the_fill_colour` — and neither of their two
fixture images has any vertical structure in it: one is a 2 by 2 of pure red, the other is
`0b0000_1111` repeated on all eight of its rows. Every assertion is on columns, and both are
drawn by a `cm` whose translation is 0, so the clipped area's origin is the page's own and
the rebase subtracts nothing. A mirror and a rebase are both invisible to all five.

Four tests now cover what they did not, and each was confirmed to fail on the old code — all
four on the original, and one or two on each defect reverted alone:

| test | axis only reverted | rebase only reverted |
|---|---|---|
| `an_image_with_vertical_structure_lands_the_right_way_up_at_a_translation` | fails | fails |
| `the_same_image_at_two_translations_lands_in_the_two_places` | fails | fails |
| `a_quarter_turned_image_samples_the_right_way_round` | fails | passes |
| `an_image_at_the_page_origin_is_still_where_the_page_put_it` | fails | passes |
| `the_whole_page_scan_is_the_oracles_scan_and_not_a_mirror_of_it` | fails | fails |

The table is the reason each test is written the way it is. A fixture with **vertical
structure** is needed to see a mirror at all; a fixture with a **non-zero translation** is
needed to see a rebase; a fixture with **both** is needed to see them *compose*, since each one
alone looks like the other. And the two quarter-turn tests pass with a rebase defect but fail
with a mirror defect, which is the division of labour between the vertical axis and the
placement. Three more of the same shape are unit tests in `image.rs` itself, over a two-by-two
raster whose four samples are four colours, so the placement is pinned at the pixel rather than
only through a page.

**Also unresolved, and a separate defect: `pdfjs__issue13372.pdf` page 1 drew nothing at
all.** It is not this one, and the fix above did not move it: the page was **0.74347**,
because it drew zero ink pixels and drew zero before. Object 18 is a 646 by 761 CCITT G.4
`/ImageMask`, and the content stream draws it in a *pattern* colour — `/R9` is a `PatternType 2`
axial shading over `/DeviceRGB`. `page.rs` declined to draw an image mask whose fill colour
space was `Pattern` and recorded `an image mask painted in a pattern colour was found and not
drawn`. The placement rectangle is right (device rows 329..1530 against the oracle's ink at
329..1529), so nothing was misplaced; there was simply nothing drawn. The cause was that
pattern colours were not implemented as fills.

**Drawn at 0.83738 by that change, and at 0.92923 by [D10](#d10--the-type-2-function-adds-c1-where-the-specification-adds-c1-c0) since.**
A `/PatternType 2` pattern used as a fill colour — including the
colour of an image mask — evaluates the shading per pixel where the fill lands, and this
page's gradient is in the right place at the right strength: our ink over the mask's rectangle
is 1.96% and the oracle's is 11.87%, both measured the same way at 150 DPI. The difference
between those two numbers is not a missing region — it is that the oracle's colour is
**dithered**, so half its pixels are saturated ink and half are paper, where ours is a smooth
average of the same two. What was still missing from this page was the type-2 function's own
rule, which put it at **0.92923** once that one line was corrected — see
[D10](#d10--the-type-2-function-adds-c1-where-the-specification-adds-c1-c0), now fixed. It was
not caused by, and did not cause, the placement defect above. What is left here is the dither
and the fonts (D7).

---

## D6 — text extraction against `pdftotext`

**Severity: not a defect; a statement of what exists.**

Word-level F1 over the corpus runs from 0.000 to 1.000 with a median near zero. This measures
a **placeholder extractor in the test**, not the product: `mangle-text` is an empty crate, so
`our_words` in `wild_corpus.rs` walks the glyph marks and maps each code through the font's
`/Encoding` and the Adobe glyph list. It is right about simple fonts with a standard or
WinAnsi encoding and wrong about everything else, because a composite font's code is a CID
and wants a `/ToUnicode` CMap. The two files scoring 1.000 are scanned pages with no text at
all, where an empty extractor and an empty oracle agree perfectly.

Every report says so next to the number, so the F1 is never later mistaken for a verdict.
FINISH.md G3.3 is not yet assessable.

---

## What is not wrong

Worth recording, because a corpus that only finds faults is as misleading as one that only
finds faults being called a corpus.

**Read this list with D1 in hand.** Before that fix every one of these pages rendered with
`marks: 0` and no ink at all, so a score near 1.0 meant *our page and the oracle's page were
both nearly empty*. `openoffice-test-document.pdf` has 315 ink pixels on it now and scored
0.9995 while drawing nothing; the others are in the same position to a greater or lesser
degree. What follows is evidence that these files are *not* resisted by the parser — which is
still worth knowing — and it is not evidence that they render correctly.

Where pages *did* draw something:

- `pdfbox__openoffice-test-document.pdf` — **0.9995**
- `pdfbox__PDFA3A.pdf` (Word 2016 → PDF/A-3a, tagged, with transparency) — **0.9962**
- `pdfjs__arial_unicode_ab_cidfont.pdf` — **0.9982**
- `pdfjs__Pages-tree-refs.pdf` — **0.9941**
- `pdfjs__issue10529.pdf` (carries Adobe InDesign private data) — **0.9886**
- `pdfjs__issue20324.pdf` — **0.9720**
- `pdfjs__issue19360.pdf` — **0.9784**
- `pdfjs__cidfont_cmap_overflow.pdf` — **0.9590**
- `pdfjs__bug1795263.pdf` — **0.9564**

`ArabicCIDTrueType.pdf` scores 0.9646 and *does* draw, so composite-font glyph lookup and
the Arabic shaping order are in reasonable shape. And `bug1980958.pdf`, a 219-byte truncated
file, scores 1.0000: both renderers recover the same thing from it, which is what a graceful
degradation looks like.

---

## D8 — the CFF charset was read from the wrong offset, and looked plausible while it did it

**Severity: silent, and the reason the charset reader is written against an oracle rather
than against the specification text.**

D7's first half is fixed: a name-keyed CFF charset is read, in all three of its formats, and
`code → /Encoding name → charset name → GID → CharStrings` reaches a glyph. Two pages
measured blank now draw, against `mutool draw` at 150 DPI:

| page | SSIM before | SSIM after | our ink before | our ink after | `mutool`'s ink |
|---|---|---|---|---|---|
| `gov__arxiv-1206.5537.pdf` p1 (22 bare CMR/CMMI fonts, format 0) | 0.7915 | **0.8784** | 0 | **61 262** | 130 395 |
| `pdfjs__freeculture.pdf` p4 (9 bare CFF fonts, format 1) | 0.9476 | **0.9926** | 0 | **24 954** | 25 929 |

`freeculture` p4 is now within 4% of `mutool`'s ink at 0.9926 SSIM. The arXiv page draws
about half of what `mutool` draws, and the rest of that page is the base-14 stamp down its
margin — D7's second half, still missing.

What is worth recording is not the fix but how it went wrong first. The charset reader was
written from a description of the three formats that said each begins with a `u16` count.
**None of the three does.** Every charset is a format byte followed by entries, and the count
is the glyph count from the CharStrings INDEX:

* format 0 — one SID per glyph after `.notdef`, and no count;
* formats 1 and 2 — runs of `{first: u16, nLeft}`, one after another, and no count.

A reader that assumes a count reads a real entry as a header. For format 0 the first entry's
SID is usually below 256, so it reads as a count of **zero** and the whole charset comes back
empty. That is what happened to every font in the wild corpus: **74 charsets, 0 glyph names
resolved.** The three test builders had been written to match the same mistake, so all three
passed. The cross-check against `fontTools` over the corpus is the only thing that found it.

Two more layout facts came out of the same cross-check, and both are the shape one assumes
wrong:

* **A String INDEX entry has no length byte.** The length is the INDEX's own offset
  arithmetic. Across the 84 String INDEX entries in the corpus, not one begins with a byte
  equal to its own length, which is what a `Pascal string` would. A reader that strips a
  length byte reads the first character as a number, so for a name starting with a capital
  letter it truncates at 65 characters and refuses the rest.
* **The first glyph a run names is glyph 1, not glyph 0**, because `.notdef` is glyph 0 and
  no format lists it.

The 391-name Standard Strings were transcribed and then checked against two independent
sources, which agree on all 391 entries: Ghostscript's `CFFStandardStrings` pseudo-encoding,
whose own source annotates every name with the SID it holds, and the Adobe Glyph List
toolchain's `cffStandardStrings`.

The standing check is `crates/mangle-font/tests/charset_real.rs`: it lifts every name-keyed
CFF out of the corpus, hands the same bytes to `fontTools` and to this reader, and asserts
they agree on **every glyph name**. It reports

```
74 charsets, 1874 glyph names agree with fontTools, 0 do not, 2 unreadable
```

The 2 unreadable are a Private DICT this cannot bound — a different defect, and not the
charset.

---

## D7 — 78% of the corpus is blank because the font cannot be read, not because the content could not

**Severity: critical, and it is what D1 was hiding. 422 of 542 pages.**

> **Partly fixed — see [D8](#d8--the-cff-charset-was-read-from-the-wrong-offset-and-looked-plausible-while-it-did-it).**
> The charset half is done: a name-keyed CFF font's codes reach glyphs and the 355-page
> refusal is gone. The counts below were measured before that and are kept as they were
> measured. The remaining half — 67 pages naming a font that is not embedded at all — is
> untouched.

D1 is fixed and the corpus was re-measured, and this is what the numbers say. All 542 pages,
rendered at 150 DPI with the fix in place:

| | pages |
|---|---|
| draw something | **103** |
| still blank | **439** |

And of the 439 still-blank pages, the notes say why — which is the first time the corpus has
been able to answer that question at all:

| the reason in the notes | pages |
|---|---|
| a bare CFF font whose charset is not read | **355** |
| the font has no `/FontDescriptor` at all | **67** |
| something else | 17 |

22 of the 76 readable files are blank on every page. The other 17 are JPEG 2000 and JBIG2
images with no decoder (9), images claiming 0 by 0 pixels (4), two CID-keyed CFF variants,
one colour space, one truncated `FlateDecode` stream, and one page whose content still does
not parse.

### The dominant one: a bare CFF's charset is not read

355 pages — 65% of the whole corpus, and most of the LaTeX and comic-book material in it —
carry fonts like this:

```
$ qpdf --show-object=18 corpus/wild/gov__arxiv-1206.5537.pdf
<< /BaseFont /JEKHOJ+CMR10 /Encoding 612 0 R /FirstChar 0 /FontDescriptor 19 0 R
   /LastChar 122 /Subtype /Type1 /Type /Font /Widths [...] >>
```

A `Type1` simple font whose `/FontDescriptor` carries a `/FontFile3` — a **bare** CFF table,
with no `sfnt` wrapper and therefore no `cmap`. A character code reaches a glyph like this:

```
code  ->  /Encoding /Differences name  ->  CFF charset: name -> GID  ->  CharStrings
```

The first and last steps work. The middle one did not: `Program::code_refusal` refuses
rather than guess, because the charset is exactly the table that maps a glyph *name* to a
glyph number, and nothing here read it. So the whole font was refused on the first glyph and
every character on the page in it went undrawn. The note it left is accurate and was recorded
on every such page:

> the `/FontFile3` of `/R18` it is a bare CFF font whose codes name glyphs through an
> `/Encoding` this does not resolve, so no character on the page in it can be drawn

Refusing was the right call at the time and the notes it produced are why this was a two-hour
fix rather than a two-week mystery. The charset is now read — see [D8](#d8--the-cff-charset-was-read-from-the-wrong-offset-and-looked-plausible-while-it-did-it) —
and a name the charset lacks is a reason rather than a guess, which is the same rule applied
one level down.

### The second: fonts that are not embedded at all

67 pages name a base-14 font with no `/FontDescriptor` — `Times-Roman` on the arXiv page,
which is the vertical stamp down its left margin. A viewer with the standard 14 font
programs built in draws it; one without does not, and says so. This is a smaller job than
the charset and worth ordering first, because it is also what the `manglepdf-cli extract`
path needs to agree with `pdftotext`.

### What this means for the numbers in this file

The Tier-B median of 0.6929 was measured over pages that were blank, and blank pages scored
against pages with ink. It is a real measurement of the defect it describes, and the defect
it describes is overwhelmingly not the one named at the top of it. Nothing in the SSIM
figures in this file should be read as a fidelity claim until D7 is fixed, because a corpus
that cannot draw a single glyph cannot say anything about fidelity.
---

## D9 — a filled path is transformed by the content stream's `cm` twice

**Severity: was critical, and it was not confined to one operator. FIXED.**

`GraphicsState::device_path` applies the current transformation to each point of a path
before the mark is built, so a `Mark::Path`'s segments are already in the space that mark's
own CTM produces. `page.rs` then did this:

```rust
let to_device = placement.matrix.concat(record.ctm);   // render_page
let polygon = transform_path(segments, to_device);      // draw_mark, Mark::Path
```

so the CTM was applied to the shape **twice**, and a fill under `q 50 0 0 50 10 10 cm` landed at
2500 units across instead of 50 — off the page, and drawn nowhere. The evidence is a page of
its own, and `mutool` disagreed with us on every pixel of it:

```
q 50 0 0 50 10 10 cm 1 0 0 rg 0 0 1 1 re f Q      # a red square on the lower right
ours:   no ink at all
mutool: a 50 by 50 red square at page (10, 10) to (60, 60)
```

### Which of the two applications was the wrong one

The segments are multiplied by the CTM at *interpreting* time, and that is the right place for
it, so the application the renderer was making was the wrong one:

| mark | carries | transformed by |
|---|---|---|
| `Mark::Path` | points already multiplied by the CTM | `placement` alone |
| `Mark::ClipChanged` | a region already multiplied by the CTM | `placement` alone |
| `Mark::Glyphs` | one matrix per glyph, already composed with the CTM | `placement` alone |
| `Mark::Image`, `Mark::Shading` | the bare CTM, as `/Matrix` or the image unit square | `placement ∘ ctm` |

Three of the four marks are handed geometry the interpreter has already placed, which is why
`clip_region` and the glyph arm were already using `placement` alone and the path arm was the
odd one out. The fix is therefore one line, and it is the *renderer's* application that goes:

```rust
let polygon = transform_path(segments, placement);
```

The alternative — making `device_path` stop applying the CTM — is the one that breaks, and the
suite says so: `a_transformation_applies_to_what_follows_it`, `a_zoomed_transform_gives_larger_bounds`
and `the_device_path_follows_the_transformation` fail immediately, and
`a_clip_under_a_scaled_ctm_is_not_transformed_twice` and `our_glyphs_agree_with_mutools` go with
them. It is also the wrong answer on the merits: the interpreter's CTM is the one that the edit
path, the provenance ranges and the hit testing all read, so removing it there would move the
truth rather than the drawing.

### What it bought, measured against `mutool` at 150 DPI

| file | page | before | after | our ink | oracle ink |
|---|---|---|---|---|---|
| `pdfjs__bug1795263.pdf` | 1 | 0.96332 | **0.98826** | 2.30% | 2.39% |
| `pdfjs__issue20324.pdf` | 1 | 0.97993 | **0.99366** | 0.34% | 0.47% |
| `pdfjs__ArabicCIDTrueType.pdf` | 1 | 0.96460 | 0.96460 | 0.0% | — (D7) |
| `pdfbox__PDFA3A.pdf` | 1 | 0.99923 | 0.99923 | 0.1% | — |

Ink is the mean darkness over the page as a fraction of solid black. The two files that moved
are the two that were drawing almost nothing; the two that did not are the ones whose remaining
difference is D7 and nothing here.

### Strokes

The same polygon feeds `stroke_polygon`, and its width comes from `record.device_line_width`,
which is `stroke.width × ctm.mean_scale()`. **The fix did not change the stroke width** — that
number never passed through `transform_path` — and it did not need to: the CTM reached the
geometry twice before and reaches it once now, so geometry and width are scaled by the same
factor for the first time. Measured, `2 w` on `0 0 10 10 re` under `4 0 0 4 50 50 cm` puts its
ink exactly on page (46, 46) to (94, 94), which is the closed form: centre lines at 50 and 90,
half a scaled width of 4 either side.

What the measurement also turned up is a **separate** defect this change does not touch: the
width is scaled by the CTM and by nothing else, so it is not multiplied by the page placement.
The same stroke is 48 device pixels wide at scale 1 and 88 at scale 2, where the geometry scales
correctly and the answer should be 96 — a stroke does not thicken when you zoom in. That is a
missing factor rather than a doubled one and it predates this entry, so it was left out of it
rather than smuggled in beside a measurement taken without it.

#### That defect is now fixed: the placement reaches the width too

`page.rs` applies the placement to the path's points at draw time, because that is the only
layer that knows the canvas, the scale and `/Rotate`. The width is in the same boat and is
applied by the same code:

```rust
let width = device_line_width(record.device_line_width, placement);   // draw_mark, Mark::Path
```

`Record::device_line_width` still carries `w × ctm.mean_scale()` and still stops there — the
interpreter cannot know the canvas, exactly as it cannot decide `Placement::fit`. The two
factors are **multiplied** rather than composed into one matrix, because the geometry has
already had the CTM applied to it by `device_path`: asking `placement ∘ ctm` for its scale would
apply the CTM to the width a second time, which is this entry's defect wearing a different hat.

The same stroke is now 48, 96 and 144 device pixels across at scales 1, 2 and 3. `mutool`,
asked directly about that page at 72 and 144 DPI, draws 48 and 96 — the numbers this entry
predicted.

#### `0 w` is a hairline, and it is one device pixel

The old code drew a stroke only when `device_line_width > 0.0`, so `w 0` drew **nothing at all**,
which is not what the specification asks for. ISO 32000-1 Table 52: a `/LineWidth` of zero
"shall be rendered as the thinnest line that can be rendered". A zero that becomes zero pixels
is an absence, not a hairline, and `0 w` is how producers draw fine rules — so the width now
floors at one device pixel.

One pixel rather than a fraction of one, and one pixel *whatever the scale*: the thinnest line
a device can draw is a property of the device, so a hairline does not thicken when the page is
zoomed, which is the whole of what distinguishes it from a thin line. Poppler was asked and
gives a single hard row for `0 w` at 72, 150 and 300 DPI alike. `mutool` 1.28 draws something
much fainter — about 13% of black over two rows — so the two oracles differ here and the
specification and poppler are followed.

#### What a non-uniform `cm` still gets wrong

This is the part that is **not** fixed, and it is worth being exact about why. It is also
[D11](#d11--a-strokes-width-is-one-number-where-a-non-uniform-cm-makes-it-an-ellipse), which
holds the measurement and the rest of the reasoning.

A stroke of a non-uniform transformation is an ellipse: under `4 0 0 2` the pen is the image of
a circle of radius `w/2`, so its semi-axes are `4 × w/2` and `2 × w/2` — for `2 w`, 4 and 2, so
8 across and 4 up where the path runs horizontally. Both renderers on this machine draw that
ellipse: `mutool` and poppler both put `2 w` on `0 0 10 10 re` under `4 0 0 2 50 50 cm` on
**48 by 24** device pixels at scale 1, which is the 40-by-20 square with 4 and 2 either side.

This renderer has one width rather than a pen that can be elliptical. It takes
`Matrix::mean_scale` = `sqrt(|det|)`, the **geometric mean of the two axis scales**: the transform
multiplies an area by `|det| = s²`, so `s` is the factor by which it scales a region, and it is
the midpoint *between* the axes rather than outside them — the most a single number can be when
the truth is an ellipse. For that stroke it is `2 × sqrt(8) ≈ 5.657`, which is **45.7 by 25.7**
device pixels: too narrow across and too wide up, by about 5% and 7% at scale 1.

That is defensible but it is not the same as correct, and the reason it is not fixed here is a
size one rather than a difficulty: the exact ellipse means stroking in user space and
transforming the finished outline, which is a change to the stroker (`stroke_outline` takes a
half-width and offsets by it, twice) rather than to this arithmetic, and it would move every
stroked page on the corpus a second time. The arithmetic is pinned by
`a_stroke_under_a_non_uniform_ctm_takes_the_geometric_mean_of_its_axes`, with the two oracles'
numbers written out beside it, so a change to the choice fails there first.

#### What it bought, measured against `mutool` at 150 DPI

| file | page | before | after | our ink before | our ink after | oracle ink |
|---|---|---|---|---|---|---|
| `gov__irs-f1040` | 1 | 0.93970 | **0.97348** | 45 501 092 | 50 689 496 | 52 616 027 |
| `gov__irs-fw4` | 1 | 0.95949 | **0.97742** | 40 588 966 | 43 846 558 | 44 132 346 |
| `gov__arxiv-1512.03385` | 1 | 0.94453 | 0.94472 | 28 667 788 | 28 689 444 | 29 188 929 |

The two IRS forms are forms of ruled boxes and lines, so they are the pages where a stroke's
width is most of the drawing; `f1040` gains 0.034 of SSIM and reaches 96.3% of the oracle's ink
where it reached 86.5%. Five files this renderer draws no strokes on measured identical before
and after, which is what says the change is confined to strokes. A full 542-page re-run is
wanted and takes two hours.

#### The tests

Five new tests in `crates/mangle-render/tests/pages.rs`, all stated as closed forms rather than
as snapshots of a render. Each was checked by reverting the fix in `page.rs` alone:

| test | what it pins | on the old code |
|---|---|---|
| `a_strokes_width_grows_with_the_page_scale` | `2 w` on `0 0 10 10 re` under `4 0 0 4 50 50 cm` inks exactly 48·scale pixels across, at scales 1, 2 and 3 | **fails**: 88 at scale 2, where the closed form is 96 |
| `a_stroked_path_under_a_ctm_is_where_the_matrix_says` | the same stroke on page (46, 46)–(94, 94), pinned at three scales because "the geometry moved" and "the width moved" are different wrong answers | **fails** at scale 2 and 4, passes at scale 1 |
| `a_stroke_without_a_ctm_is_the_width_the_stream_asked_for` | a `6 w` line with no `cm` inks 6·scale pixels, centred on the line the stream drew | **fails** at scale 2: 6 pixels where 12 is the closed form |
| `a_hairline_is_one_device_pixel_at_every_scale` | `0 w` is one pixel's worth of ink at scales 1 to 4, measured as ink across the line rather than as a box | **fails**: nothing is drawn at all |
| `a_stroke_under_a_non_uniform_ctm_takes_the_geometric_mean_of_its_axes` | `sqrt(|det|)` either side of `4 0 0 2`, with the oracles' ellipse written out beside it | **fails**: `sqrt(8)` is not what the old code drew |

### Why the clip is not a defence

`a_clip_under_a_scaled_ctm_is_not_transformed_twice` passed the whole time this was true,
because a clip is installed from `placement` alone. It would still pass after the fix, because
its fill (`0 0 50 50 re` under a doubled CTM) was exactly its clip. The new tests are for the
case the clip could not see.

### The tests

Four new tests in `crates/mangle-render/tests/pages.rs`, all stated as closed forms rather than
as snapshots of a render, and all checked by reverting the one line:

| test | what it pins | on the old code |
|---|---|---|
| `a_filled_path_under_a_ctm_lands_where_the_matrix_says` | `50 0 0 50 10 10 cm` over `0 0 1 1 re` covers device pixels 10·scale..60·scale on both axes, the middle is the fill colour and the page around it is paper — at scales 1, 2 and 3 | **fails**: nothing is drawn |
| `a_filled_path_under_a_non_uniform_ctm_is_not_squashed_or_doubled` | `30 0 0 10 20 40 cm` gives page (20, 40)–(50, 50): a 3:1 rectangle, which a doubled CTM would turn into a 9:1 one off the page | **fails**: nothing is drawn |
| `a_filled_path_without_a_ctm_is_where_the_page_puts_it` | the same rectangle with no `cm` is where the page puts it — the case every other fixture in the file is | passes, and must stay passing |
| `an_image_and_a_glyph_under_a_ctm_are_moved_by_it_once` | an image and a glyph under the same kind of `cm` land at exactly the closed form, compared against the same mark with no `cm` | passes, and must stay passing |

The last one is a guard rather than a regression: images and glyphs were already right, and this
is what says so. `cm` with an empty operand stack turns out to be worth avoiding in a fixture
as well — `mutool` stops on the operator error and returns a blank page — so `text_page` takes
its six numbers and writes the operator only when there are any.

---

## D10 — the type-2 function adds `C1` where the specification adds `C1 − C0`

**Severity: was every gradient whose function has a non-zero `C0`. FIXED.**

ISO 32000-1 Table 42 defines a type 2 (exponential) function as

```
y = C0 + x^N × (C1 − C0)
```

where `C0` is the output at `x = 0` and `C1` the output at `x = 1`. `shading.rs` evaluated

```rust
out.push(c0 + c1 * power);   // Exponential::apply
```

which is the same formula only when `C0` is zero, which is why it went unnoticed: hand-written
gradients very often are, and the two rules agree there.

`pdfjs__issue13372.pdf` is the counter-example, and it is a test file Adobe wrote for exactly
this feature — `Pattyp2.ps`, a gradient portrait. Its two type-2 functions are

```
<< /FunctionType 2 /Domain [0 1] /C0 [0 1 1] /C1 [1 1 0] /N 1 >>   # cyan -> yellow
<< /FunctionType 2 /Domain [0 1] /C0 [1 1 0] /C1 [1 0 1] /N 1 >>   # yellow -> magenta
```

which the specification reads as cyan → yellow → magenta and the code read as cyan → white →
yellow → white. `mutool` agrees with the specification: at the gradient's `t = 0.1` it paints
`(50, 255, 205)`, and `C0 + 0.2·(C1 − C0)` is exactly `(51, 255, 204)`.

**Measured.** `pdfjs__issue13372.pdf` page 1 against `mutool` at 150 DPI: **0.83738 → 0.92923**.
What is left after that is the dither, which is a separate rasteriser feature this project does
not have: the oracle paints the gradient as ink dots at the coverage it wants and we paint the
same average smoothly, and `compare()` scores two correct answers differently for that reason
alone.

### Why it survived a test

The test that pinned the wrong rule was named after it — `an_exponential_function_is_c0_plus_c1_times_t_to_the_n`
— and used `C0 = 0.25`, `C1 = 0.75`, `N = 2`. **The two rules coincide when `C0` is zero and
only then**, so that test was one number away from catching this and did not get there: it
asserted `at(1.0) == 1.0`, which is `C0 + C1` under the wrong rule and `C1` under the right
one. It is now `an_exponential_function_is_c0_plus_t_to_the_n_times_c1_minus_c0`, with the
same `C0 = 0.25`, and it fails on the old expression at exactly that assertion.

Nothing else in the repository exercised a type 2 function with a non-zero `C0` at all. The
pattern-fill fixtures in `pages.rs` and the analytic gradient fixtures both write `C0 [0]` or
`C0 [0 0 0]`, precisely so that what they assert is the specification's closed form rather than
this codebase's, so every one of them is unchanged by the fix and none of them could have
caught it. The one other test that broke is worth naming:
`a_stitching_function_picks_the_right_part` built its second half as `C0 = 0.5, C1 = 0.5`, which
under the wrong rule is a ramp from 0.5 to 1.0 and under the specification's is a *constant*
0.5 — the stitch would have ended the gradient halfway. It is now `C0 = 0.5, C1 = 1.0`, which is
the ramp both rules agree is a ramp.

---

## D11 — a stroke's width is one number, where a non-uniform `cm` makes it an ellipse

**Severity: only a stroke drawn under a non-uniform transformation. Open, and recorded rather
than closed.**

A stroke's width is a scalar everywhere in this renderer: `Record::device_line_width` is
`w × mean_scale(ctm)`, and `mangle_render::device_line_width` multiplies that by
`mean_scale(placement)`. Both factors are now applied to the geometry's own scale as well
(D9, which is fixed), so a stroke thickens with the page. But a non-uniform `cm` makes the
correct stroke an **ellipse** — the image of the pen under the transformation — and one number
cannot be an ellipse.

### What the oracles draw, measured

`2 w` on `0 0 10 10 re` under `4 0 0 2 50 50 cm`, on a 200 point page at 72 DPI. The path is
40 by 20 on the page, from (50, 50) to (90, 70), and the pen is a circle of radius 1 which the
matrix turns into an ellipse of semi-axes 4 and 2 — so 4 either side across and 2 either side
up.

| renderer | ink box, device pixels | that is |
|---|---|---|
| `mutool` 1.28.5 | 48 × 24 | the exact ellipse |
| poppler 26.08 | 48 × 24 | the exact ellipse |
| this renderer | **45.7 × 25.7** | `sqrt(|det|) = sqrt(8) ≈ 2.828` either side |

5% narrow across and 7% wide up. Both oracles agree with each other and both disagree with us,
so this is a real difference and not a rendering of a different fixture.

### What this renderer does, and why

`Matrix::mean_scale` is `sqrt(|det|)`: the **geometric mean** of the two axis scales, and the
factor by which the transform scales a region, since an area is multiplied by `|det| = s²`. It
is the midpoint *between* the axes rather than outside them, which is the most a single number
can be when the truth is an ellipse. The arithmetic mean is worse on both axes at once:
`(4 + 2)/2 = 3` would give 6 across and 6 up, where the oracles have 4 and 2 and `sqrt(8)` has
2.83 and 2.83.

The arithmetic is pinned by `a_stroke_under_a_non_uniform_ctm_takes_the_geometric_mean_of_its_axes`,
which writes out the whole derivation and fails if the choice changes.

### What fixing it would mean

Stroking in user space and transforming the finished outline afterwards — which is what
"the stroke is the path offset by `width/2`" means under a transformation, and what the module
comment in `mangle-render/src/lib.rs` used to claim before it stopped being true. It is a
change to `stroke_outline`, which offsets a polyline by a single half-width on both sides and
adds the caps and joins by hand; a matrix-aware pen is a different data structure, not a new
factor in an existing expression. It would also move every stroked page on the corpus a second
time, so it wants its own entry and its own measurements rather than a line inside D9's.

### What is not wrong here

- **A uniform `cm`**, which is nearly every `cm` in practice: the pen stays a circle, `sqrt(|det|)`
  is exactly its radius, and D9's fixture is drawn on the pixel both oracles put it on.
- **A rotation**, which `|det|` ignores and must: `sqrt(|det|)` of a rotation is 1, and the
  `a_rotated_page_puts_its_ink_where_the_rotation_says` test is unchanged.
- **The placement**, which is always a uniform scale composed with a rotation and a shift.
- **A dash pattern's lengths**, which were the same defect in another place: user-space lengths
  that nothing scaled, so a `[6 3]` pattern was six points of ink whatever the page scale. That is
  fixed — see [D11b](#d11b--a-dash-patterns-lengths-were-never-scaled-and-every-pattern-drew-solid) —
  and the `sqrt(|det|)` compromise above applies to a dash exactly as it does to a width.

---

## D11b — a dash pattern's lengths were never scaled, and every pattern drew solid

**Severity: every dashed line on every page, at every scale, and worse the further in you went.
FIXED.**

Two defects in one place, and the second hid the first. `walk_dashes` computed on-runs and
off-runs correctly and said which was which; `stroke_polygon` **threw the flag away** and
stroked every run, which fills the gaps back in and draws any pattern as one solid line. And
`page.rs` handed `record.dash` to the stroker unchanged, so the lengths were user-space lengths
that nothing turned into pixels.

So a `[6 3] 0 d` line was a solid line at 72 DPI, 144 DPI and 288 DPI alike, and the way to see
that the lengths were wrong was to compare the run lengths once the gaps were there at all.

### What the oracles draw, measured

`[6 3] 0 d` on a two-point-wide horizontal line, 160 points long, at 72, 144 and 288 DPI. Both
oracles agree with each other at every resolution, and now so do we, to the pixel:

| on-run and gap | 72 DPI | 144 DPI | 288 DPI | what we gave before |
|---|---|---|---|---|
| `mutool draw` | 6 / 3 | 12 / 6 | 24 / 12 | — |
| `pdftoppm` | 6 / 3 | — | — | — |
| this renderer | **6 / 3** | **12 / 6** | **24 / 12** | one solid 160-point run |

### What fixing it meant, and where

The **phase** is what a scaling fix can get wrong. `/Phase` is a distance into the pattern in the
same user space as the array, and `walk_dashes` measures it against the pattern's own total, so
scaling the lengths without it moves every dash along the line as the page is zoomed — a
different picture rather than the same one drawn larger. Both are scaled by one factor, which
leaves the *fraction* of the pattern the phase names unchanged; `a_dash_pattern_and_its_phase_are_scaled_by_both_factors`
pins that fraction at 2/9 across both factors.

The scaling happens in `page.rs`, beside the geometry, and **not in the interpreter**: the
interpreter knows the content stream's `cm` and no more, so scaling there would put one factor
where the canvas's zoom is not, and the record would carry a pattern already half-converted. The
two factors are multiplied, exactly as `device_line_width` multiplies them for a stroke's width:
`mean_scale(ctm) × mean_scale(placement)`, because the path's points have already had the CTM
applied to them by `device_path`.

A **zero-length entry** is legal and means "the same colour twice". It is the one place scaling
could do damage, and it cannot: a factor times a zero is a zero, so nothing is scaled *into* a
division by zero, and `walk_dashes` walks a zero-length element as a hairline rather than a spin.
`[6 0 3 4] 0 d` is nine points of ink and four of paper, repeating, and both oracles draw
exactly that — 9 / 4 at 72 DPI and 36 / 16 at 288 DPI, as we do.

### What is still not right

- **An array that sums to zero** is drawn **solid**, and both oracles draw **nothing at all**:
  `mutool draw` and `pdftoppm` put no ink on the page for `[0 0] 0 d`. Drawing it solid is the
  handling the rest of this renderer gives an unusable pattern and what `Dash::is_solid` already
  documents, and it is a handled answer rather than a hang or a division by the total — but it is
  a difference, it is on the oracles' side, and it is recorded rather than defended. Closing it
  means a stroke whose pattern has no length drawing nothing, which is one more case in
  `stroke_polygon`.
- **A non-uniform `cm`** is D11's compromise: the true dash lengths are the ellipse the matrix
  gives a segment, and a scalar takes `sqrt(|det|)` — right between the two axes, wrong on both.
