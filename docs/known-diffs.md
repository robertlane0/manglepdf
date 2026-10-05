# Known differences from the oracles

FINISH.md G3.1: *"per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31,
and Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root
cause and evidence."*

**The Tier-B target is not met, and not narrowly.** Against `mutool draw` at 150 DPI over the
77-file wild corpus: **738 of 745 pages produced an SSIM** and the page-level median is
**0.95795**, and **254 of those 738 pages (34.4%) are below 0.95**. Seventy-three of the 77
files open; **four cannot**. This page records what the corpus found and the evidence for each
claim.

The corpus is `corpus/wild/`, pinned by SHA-256 in `corpus/wild/MANIFEST.toml`, fetched with
`cargo xtask corpus fetch`, and measured by `crates/mangle-render/tests/wild_corpus.rs`.
That test asserts nothing: the numbers below come from `corpus/wild/report/SUMMARY.md` and
the per-file reports beside it, which are git-ignored and regenerated on every run. Oracle
versions: `mutool 1.28.5`, `pdftotext` (poppler), `qpdf` (system).

> **These figures are a re-run, not an estimate.** The block above is the whole 77-file corpus
> measured end to end after D2 and D3 were fixed: 77 files, 16195.80s, **0 panicked**, 73
> opened and 4 closed. It replaces a run that was made when 34 of the 77 files could not be
> opened at all, and every figure in it — the 460-of-542 comparison count, the 0.6929 median and
> the 94.8%-below-0.95 figure the previous version of this file carried — described a corpus
> with a third of it missing. **Of the 73 that open, 67 read their structure as written; 6 needed
> recovery** (`bug1795263`, `bug1980958`, `GHOSTSCRIPT-698804-1-fuzzed`, `issue15590`,
> `issue15893_reduced`, `PDFBOX-3148-2-fuzzed`), and the run's own summary counts 9 of the 77 as
> having needed recovery once the three that did not open but left reader notes are included.

**The test is `#[ignore]`d, on purpose.** One run over the corpus takes about two hours, so
leaving it in the default suite makes `cargo test --workspace` unusable. The two cheap tests
beside it — which check the harness itself — still run by default.

Reproduce any line:

```sh
cargo xtask corpus fetch
cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture
```

### The run, in full

| | |
|---|---|
| files | **77**, of which **73 opened** and **4 did not** |
| wall clock | **16195.80s** |
| panicked | **0** |
| pages measured | **745** |
| pages with an SSIM | **738** |
| page-level median SSIM | **0.95795** |
| size disagreements | **2** |
| pages left uncompared on purpose | 2 |
| pages the oracle could not render | 3 |

The page-level median is not stated in the run's summary — it is computed from
`corpus/wild/report/pages.tsv` over the **738** rows that carry an SSIM. Nothing else in this
section is derived; the rest is quoted from the run.

### Per file, by its worst page

Over the **68** files that have at least one page with an SSIM (the four that do not open
contribute none, and neither do the five whose only pages were skipped):

| worst-page SSIM | files |
|---|---|
| ≥ 0.99 | 20 |
| ≥ 0.95 | 22 |
| ≥ 0.90 | 6 |
| ≥ 0.80 | 14 |
| < 0.80 | **6** |

**Median 0.9698, mean 0.9214.** The mean is 0.048 below the median because those six files are
not a little way under 0.80 — the best of the six is 0.7564 and the worst is 0.5716, and one of
them is a single page.

> **W075 has been re-measured since that run, and has moved out of this table.**
> `gov__nist-sp800-88.pdf` had 41 of its 44 pages below 0.95 and a median of 0.7727 (0.7741 as
> the run's own per-file summary rounds it);
> [D19](#d19--an-iccbased-colour-was-refused-where-the-file-named-the-answer-and-a-mask-never-read-was-never-reported)
> below brings it to **10 of 44 below 0.95 and a median of 0.96095**, so it is no longer one of
> the four worst files, and the corpus-wide count of pages below 0.95 is *at least 31 lower* than
> the 254 that run measured — a projection rather than a measurement, since every corpus-wide
> figure on this page is still that run's until the next full one.

### Per page

Over the **738** pages that carry an SSIM:

| | pages | share |
|---|---|---|
| below 0.95 | **254** | **34.4%** |
| below 0.90 | 174 | 23.6% |
| below 0.80 | 94 | 12.7% |
| below 0.50 | **0** | 0% |

**Zero pages below 0.50** is the one unambiguous good number in the run, and it is worth saying
what it means: no page is blank where the oracle has a page, and none is grossly wrong. Every
one of the 254 is a page that draws *something* and draws it incompletely, in the wrong place, or
in the wrong colour.

### The seven pages with no SSIM, and why each is missing

The 745/738 split matters as much as the SSIM does, because a page that produced no number is a
page that was never measured and must never read as one that passed.

| page | why there is no SSIM |
|---|---|
| `gov__usgs-topo-cnmi-1.pdf` 1 | 25119300 oracle pixels, above the 16777216 this harness will compare. Rendered at 150 DPI and **left uncompared on purpose** — the bound is the harness's own memory ceiling, not a property of the file. |
| `gov__usgs-topo-cnmi-3.pdf` 1 | 25127777 oracle pixels, same bound. |
| `pdfjs__freeculture.pdf` 1 | **size disagreement**: we rendered 1020x1531, mutool rendered 1020x1530 |
| `pdfjs__freeculture.pdf` 2 | **size disagreement**: we rendered 915x901, mutool rendered 915x900 |
| `pdfjs__issue15590.pdf` 1 | mutool produced no page 1; **it could not render it** |
| `pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf` 1 | mutool produced no page 1; **it could not render it** |
| `pdfjs__issue15893_reduced.pdf` 1 | mutool produced no page 1; **it could not render it** |

**Both size disagreements are ours and they are the same defect: we are one pixel taller than
`mutool` on both pages.** It is D3's shape and D3's fix did not cover this case — `pixels_for`
settles a value within `WHOLE_PIXEL_EPSILON` of an integer onto that integer, and whatever
`freeculture`'s box asks for lands outside the epsilon on the height and inside it on the width.
A real, narrow, reproducible defect, and the only thing standing between two pages and a
measurement.

The three pages `mutool` cannot render are not differences at all: there is no second opinion to
have. They are recorded because a page the *oracle* refuses is a page nobody has checked.

### Where the 254 are

They are not scattered. **134 pages — 18.2% of everything measured — are four files**, and those
four sit at medians between 0.64 and 0.79:

| id | file | pages | median SSIM | pages below 0.95 |
|---|---|---|---|---|
| W034 | `gov__nist-nistir7255.pdf` | 66 | 0.7842 | 61 |
| ~~W075~~ | ~~`gov__nist-sp800-88.pdf`~~ | 44 | ~~0.7741~~ **0.96095** | ~~41~~ **0** |
| W076 | `pdfjs__TAMReview.pdf` | 23 | ~~0.7663~~ **0.8879** | 22 |
| W038 | `pdfjs__S2.pdf` | 1 | 0.6386 | 1 |
| | | **134** | | **84** |

Each of the four has one named cause, and all four are **missing features rather than bugs** — a
codec or a colour-space conversion this project does not have. That distinction is the most
useful thing the diagnosis produced, because it says what to build next:

| id | what is missing | what the renderer says |
|---|---|---|
| W034 | a **JPEG 2000** decoder (`JPXDecode`) | `a JPEG 2000 image was found but no decoder exists yet`, twice, on **all 66 pages**. Both images the page draws are JPX, and one of them carries a `JBIG2Decode` `/Mask`. **Fixed as a report: the mask is now named too** (see [D19](#d19--an-iccbased-colour-was-refused-where-the-file-named-the-answer-and-a-skipped-mask-was-never-reported) below) — the decoder itself is still missing. |
| ~~W075~~ | ~~**`ICCBased` colour conversion`~~ | **FIXED — see [D19](#d19--an-iccbased-colour-was-refused-where-the-file-named-the-answer-and-a-skipped-mask-was-never-reported).** `CS0` is `[/ICCBased …]` over an sRGB profile that carries `/Alternate /DeviceRGB`, and that alternate is now what the colour is read through. **43 of the file's 44 pages went from zero ink pixels to ink, and the file's median SSIM from 0.7741 to 0.96095.** |
| ~~W076~~ | ~~**`Separation`/`DeviceN` tint-transform evaluation`~~ | **PARTLY FIXED — see [D20](#d20--a-separation-was-painted-black-where-the-file-asked-for-a-tint-and-now-its-tint-transform-is-evaluated).** `Cs8` is `[/Separation /Black <ICCBased sRGB> <FunctionType 0, 255 samples>]`, and it is the colour of the entire article body. **The transform is evaluated now: no page of the file reports `could not be converted`, and the six pages that drew 5,937 ink pixels draw 100k–174k. The file's median SSIM went from 0.7692 to 0.8879.** The 22 pages still below 0.95 are now limited by something else — see D20 — and not by colour. |
| W038 | a **JPEG 2000** decoder, again | `a JPEG 2000 image was found but no decoder exists yet` six times, plus one naming `/Im7`. 13 JPX images carry the whole figure: we draw 31987 ink pixels against the oracle's 838668, and **99.8% of what we do draw is in the right place**. |

`Colour::to_rgba` in `crates/mangle-content/src/state.rs` had arms for DeviceGray/CalGray,
DeviceRGB/CalRGB and DeviceCMYK, a fallback for Separation/DeviceN, and **no arm at all for
`ICCBased`, `Indexed` or `Lab`** — each of those fell through to `None`, and the mark was
reported and dropped. The *image* path in `crates/mangle-render/src/image.rs` did read
`ICCBased`, by counting `/N`, so the same colour space was approximated on an image and refused
on a fill. That asymmetry was where W075 lived, and it was 42 blank pages. **`Indexed` is still
a gap** — it needs its palette, and there is no alternate to fall through to — but
**`Separation` and `DeviceN` are not**: D19 read an `ICCBased` space through its `/Alternate`,
and D20 evaluates a separation's `/TintTransform` and reads its `/Alternate` the same way.

### The result against Gate 3.1

Gate 3.1 asks three things of this corpus. **None of the three is met.**

> *"At 150 DPI: per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31, and
> Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root cause and
> evidence."*

- **per-page ≥ 0.95 — NOT MET.** 254 of the 738 pages are below it. **65.6%** of pages clear the
  floor; 34.4% do not.
- **median ≥ 0.985 — NOT MET on Tier B.** The Tier-B page median is **0.95795**, 0.027 short. The
  gate's median is taken across F12–F22, F31 *and* Tier B, and this run measures only the last
  of those, so 0.95795 is not by itself the gate's number — but 738 of the pages in that median
  are Tier-B pages, and Tier B's own median is 0.958.
- **≤ 3% of pages below 0.95 — NOT MET.** 34.4% against a 3% bound: at 738 pages the gate allows
  **22** pages below 0.95 and there are **254**, a factor of 11.5.

The page median is also the number most likely to be misread, so both halves belong here.
**0.958 is a large improvement on 0.6929 and it is not a pass**: it is the median of a
distribution whose worst file sits at 0.6386, and the four clusters above are 18.2% of the pages
— very nearly the fifth of all pages that the gate's 3% allowance assumes can be wrong at once.
Anyone deciding whether this is nearly done needs both numbers: **median 0.958, and 254 of 738
pages below 0.95.** The second is the one the gate is written against.

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

Those 542 pages are the corpus as it stood then, when 34 files could not be opened. The corpus
is now **745 pages** across 77 files, and none of those 439 is blank for this reason — the
[re-run](#the-run-in-full) has **zero pages below 0.50**. Blank pages are not gone, but they now
have one named cause: `gov__nist-sp800-88.pdf` alone renders **42 pages with no ink at all**,
every mark on them painted in a colour this cannot convert (see [the 254](#where-the-254-are)).

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
as the placement defect D5b that sat on top of it. The page is now at 0.99747, which the
[re-run](#the-run-in-full) confirms: `pdfbox__multitiff.pdf` pages 1–3 measure 0.9975, 0.9977
and 0.9984.** The three-page file is new to the numbers here — the table below was measured
on page 1 alone.

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

**Severity: critical, and it is what D1 was hiding. 422 of 542 pages — of the 542 the corpus
had then. The corpus is 745 pages now; see the [re-run](#the-run-in-full).**

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

**That 0.6929 is no longer the Tier-B median.** It is kept here as the measurement it was, of
the code as it stood, and the current figure — 0.95795 over 738 pages — is at the
[top of this file](#the-result-against-gate-31). The 422 of 542 in this entry's severity line
counts the same way: it is what the corpus looked like then, not what it looks like now.

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
and after, which is what says the change is confined to strokes. The full re-run has since been
done — 745 pages, and `gov__irs-f1040` page 1 measures **0.9827** in the
[corpus report](#the-run-in-full), past the 0.97348 above.

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

**Measured.** `pdfjs__issue13372.pdf` page 1 against `mutool` at 150 DPI: **0.83738 → 0.92923**,
and the [re-run](#the-run-in-full) puts it at **0.9292** with 411430 of 2176200 pixels above
tolerance, so that figure stands. What is left after that is the dither, which is a separate
rasteriser feature this project does not have: the oracle paints the gradient as ink dots at the
coverage it wants and we paint the same average smoothly, and `compare()` scores two correct
answers differently for that reason alone.

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

---

## D12 — a stroke painted in a pattern was dropped, so a page whose only ink was patterned strokes rendered blank

**Severity: every page whose only ink is stroked in a `/Pattern` colour space. FIXED.**

`corpus/wild/pdfjs__issue13325_reduced.pdf` came out with **zero ink** while `mutool draw`
drew a full page of marks. It was worth chasing, and the cause was not one of the four a blank
page usually turns out to be — it was not a clip, not a placement, not an alpha of zero, and not
content hiding inside an XObject.

### The evidence

The page's content stream is one form XObject, seven stroked paths, and two white rectangles.
The seven strokes are the page's entire ink, and they are set up like this:

```
/CS1 CS /P0 SCN        % /CS1 is [/Pattern], /P0 is a /PatternType 2 shading
2 w 4 M [11.133 11.133]0 d
q 1 0 0 1 299.3702 566.1895 cm  0 0 m  -12.097 -5.016 -21.376 -15.455 -24.806 -28.285 c  S  Q
```

So every one of the seven is stroked with a **pattern**, which is what `SCN` is for: `G`, `RG`,
`K` and `g`/`rg`/`k` cannot name a pattern resource at all, and `SCN` is the operator that does.

The four candidate causes, each ruled out by what the renderer reported rather than by argument:

| candidate | what the renderer said | verdict |
|---|---|---|
| a clip that computes to empty | the clip on all seven strokes is the whole mediabox, `0 0 595.276 841.89` | not it |
| marks placed off-page | our CTM for the first stroke is `1 0 0 1 299.3702 566.1895`; `mutool trace` gives `1 0 0 -1 299.3702 275.7005`, and `841.89 − 275.7005 = 566.1895` | not it |
| alpha zero everywhere | `/GS0` is `ca 1 CA 1`, and no note mentions alpha | not it |
| content in a form XObject | `/Fm0 Do` is object 33, and its whole content stream is `0 TL q Q` — empty | not it |

What the renderer *did* say, seven times, once per stroke:

```
a stroke colour in Pattern could not be converted, so the outline was not drawn
```

`Colour::to_rgba` answers `None` for a `Pattern` space **on purpose** — `state.rs` says so
("a `Pattern` space is not a colour at all … a caller asking here is a caller that could only
paint one flat colour, so the honest answer is nothing"). The fill arm of `draw_mark` already
knew that and special-cased `Pattern`, reading the pattern and evaluating its shading per pixel.
The **stroke** arm did not: it called `to_rgba` unconditionally, got `None`, and dropped the
mark. On a page whose every mark was a patterned stroke that is a blank page, and the note
named the cause exactly.

The fourth row above is a **real defect of its own** that this page happened to expose: `Do`
of a `/Subtype /Form` XObject was not implemented, so `/Fm0 Do` became `Mark::Image` and
produced `an image claims to be 0 by 0 pixels and was not drawn`. That form draws nothing, so
it is not this page's blankness, but a `Do` that reads a form as an image is wrong. **It is
now fixed** — a form is executed as a nested content stream, with its own `/Matrix`, `/BBox`
and `/Resources` — and the finding it leaves behind is
[D17](#d17--a-cid-was-looked-up-through-the-fonts-cmap-instead-of-its-cidtogidmap).

### What fixing it meant

`Device::stroke_polygon` builds the stroke's outline — dashes, caps, joins — and fills it with
one colour. That outline is now `Device::stroke_outline`, returning the outline per subpath, and
`fill::stroke` fills it through `fill::polygon` with a `FillColour`, which already knows how to
be a shading. So the outline is computed by **one** piece of code and a patterned stroke cannot
come out dashed differently from a flat one; `a_pattern_stroke_covers_exactly_where_a_one_colour_stroke_does`
pins that by comparing a patterned stroke against a one-colour one pixel for pixel.

A refusal now names the operator that asked for the pattern — `a stroke in the PatternType 1
tiling pattern /P0 was found and not drawn` rather than `a fill in …` — because the two reach
the same function and the note is read by someone looking for the line they asked for.

### What the oracles draw, measured

Page 1 of `pdfjs__issue13325_reduced.pdf`, `mutool draw -r 150` against `render_page`, 1241×1754:

| | SSIM | RMS | pixels above tolerance | our ink |
|---|---|---|---|---|
| before | 0.99644 | 4.85033 | 2 643 | **0** |
| after | **0.99741** | **3.69345** | **2 466** | **3 439** |
| `mutool` | — | — | — | 2 743 |

Zero ink became a page that draws, which is the whole of this entry. The metrics moved far less
than the ink did, and that is worth saying plainly rather than letting the SSIM imply a smaller
change than it was: **the page is 99.9% white**, so a blank page and a page with all its marks
score 0.996 between them. Of the 2 466 pixels still above tolerance, most are the marks landing
in slightly the wrong place or the wrong colour rather than marks that are missing — and that
residual is not this defect. It is [D13](#d13--a-two-point-dash-run-drawn-in-the-negative-x-direction-becomes-a-bowtie),
which this page was the first to expose because until now nothing here drew a stroke in a
pattern at all.

### What is still not right on this page

- **`/Fm0 Do` is read as an image**, not descended into as a form. Harmless here because the
  form is empty; wrong in general, and it is a page-level feature rather than a paint one.
- **`/CS0` cannot be converted**, twice: `a fill colour in CS0 could not be converted`. `/CS0` is
  `[/ICCBased …]`, and the ICC profile is not read. Both fills are white (`1 1 1`), so this costs
  nothing on this page and is not a fidelity problem here — but it is the same missing feature
  for any page that fills in an ICC colour, which is most of them.

---

## D13 — a two-point dash run drawn in the negative x direction becomes a bowtie

**Severity: was any dash run whose path segment did not run in the positive x direction — which
is half of all horizontal and vertical dashes and every diagonal. Fixed; the measurements are
below. Found while diagnosing D12.**

`stroke_outline` offsets a polyline on both sides and joins the ends. For a **two-point** path —
one `m` and one `l` — the two offset points at each end are supposed to pair up into a
rectangle. They did not always.

### The evidence

A 100×100 page, `2 w [12 12] 0 d`, the line at y = 50. Each row is 100 pixels of one row of the
render, `0` darkest and `.` paper, at 1 pixel per point:

```
80 50 m 20 50 l S     ....................034689986430............034689986430...........
20 50 m 80 50 l S     ....................000000000000............000000000000...........
```

The same line, the same dashes, drawn in the two directions. One is solid; the other was a
**bowtie** — dark at the ends and hollow in the middle, every dash, every time. A horizontal
dash drawn left-to-right was right and the same dash drawn right-to-left was wrong, which is
enough to say the geometry and not the paint.

It reproduced through `Device::stroke_polygon` with a plain `0 0 0 RG` stroke, so it predated
D12 and was independent of it. And the outline said why. Asking `stroke_outline` for the two
runs' polygons gave:

```
80 50 m 20 50 l   [(80,49), (20,51), (20,49), (80,51)]     <- a bowtie
20 50 m 80 50 l   [(20,51), (80,51), (80,49), (20,49)]     <- a rectangle
```

### The root cause, confirmed

`offset_sides` asked for the normal of the segment *arriving* at each vertex:

```rust
let before = normal_at(path, i.wrapping_sub(1), 0);
```

`normal_at(points, index, after)` already means "the normal of the segment ending at `index`"
when `after == 0` — it computes `(index - 1, index)` internally. So the call above asked for the
segment ending at `i - 1`, **one segment too early**, and the correct call is
`normal_at(path, i, 0)`. `add_joins` made the same call and was wrong the same way.

One refinement to what was recorded here before, because it changes which shapes the defect
reached and so how it should have been described. The claim was that at the last vertex of
*every* open path both lookups miss and the normal falls back to the fixed `(0, 1)`. They only
both miss for a **two-point** path. At the last vertex of a longer open path `before` is `Some`
— it is simply the normal of segment `i - 2` instead of `i - 1` — so the offset is taken along
the wrong segment's normal with no fallback at all. That is a smaller error than the fallback
but it is the same error, and it is why a three-point path did not come out hollow: it came out
*kinked*, with a mitre taken against a direction the path never went in.

And the fallback was worse than "negative x". `(0, 1)` is the left normal of a segment running in
the **positive** x direction and of nothing else, so every two-point path whose segment was
vertical or diagonal was a bowtie *in both directions* — the entry's title understated it.

### What else the same wrong argument reached

Checking rather than assuming turned up three more places, all of them one index off in the same
way, and all now fixed:

- **The joins.** `add_joins` names `normal_at(path, i - 1, 0)` as the arriving normal, so both
  the bevel test and the two bevel corners were computed from the wrong pair of segments.
- **Both caps.** `stroke_outline` asked for `normal_at(path, len - 1, 1)` at the end point and
  `normal_at(path, 0, 0)` at the start point — the segment *after* the last point and the one
  *before* the first, neither of which exists. Both returned `None` and both caps were placed
  against the same fixed `(0, 1)`. A cap is at the end of a stroke, so it has to be measured
  against the segment that ends there: `normal_at(path, len - 1, 0)` and `normal_at(path, 0, 1)`.
  Before the fix a round or projecting cap on a vertical line grew out of the *side* of the
  stroke rather than out of its end.
- **The seam of a ring.** A closed path's last point joins its first, so both of the seam's
  points have a segment on each side like every other point, and the closing segment is the one
  arriving at the first and the one leaving the last. Reading the ring as an open list — which is
  what trimming the repeated final point leaves — gives each of them one neighbour and loses the
  mitre at the seam, so the ring's outer boundary stopped short of that corner.

The last of those is worth singling out because it is the one place where fixing the index
*looked* like a regression. The three existing tests that measure a stroked square under a `cm`
(`a_strokes_width_grows_with_the_page_scale` and the two beside it) failed the moment the index
was corrected, with the ink box's left edge four pixels short. They were not wrong: the old
seam offset was along a *different wrong* segment's normal, and for a square that segment's
normal happened to point the way the mitre would have. The right answer needed the closing
segment to be looked up, which is what `normals_at` now does.

### The fix

`normals_at(path, index, closed)` returns the two normals meeting at a point — the segment
arriving and the segment leaving — and is the single place that knows about a ring's closing
segment. `offset_sides` and `add_joins` both go through it, and both caps ask it for the
segment at their own end point.

### What the oracles draw, measured

Against `mutool draw` at 150 DPI, page 1 of each file. The before column is the same code with
this entry's fix reverted, measured in the same session, so it is a like-for-like pair and not a
comparison against a figure recorded before the dash and pattern-stroke work.

Page 1 of `corpus/wild/pdfjs__bug1795263.pdf`, 1240×1755:

| | SSIM | RMS | pixels above tolerance | our ink | `mutool`'s |
|---|---|---|---|---|---|
| before | 0.98826 | 10.48991 | 14 320 | 87 612 | 90 115 |
| after | **0.98826** | **10.48991** | **14 320** | **87 612** | 90 115 |

**This page did not move, at all.** Every figure is identical to the last digit. That is worth
saying plainly rather than dressing up: page 1 of this file carries no stroke that reaches the
offset code, so there was nothing on it for this fix to change. Its 14 320 pixels above
tolerance are something else, and the residual this entry used to be blamed for was never on
this page.

Page 1 of `corpus/wild/gov__irs-f1040.pdf`, 1275×1650:

| | SSIM | RMS | pixels above tolerance | our ink | `mutool`'s |
|---|---|---|---|---|---|
| before | 0.97408 | 14.57954 | 111 459 | 1 121 543 | 1 123 089 |
| after | **0.98250** | **12.74925** | **98 550** | **1 122 642** | 1 123 089 |

SSIM by 0.0084, RMS down 1.83, and 12 909 fewer pixels above tolerance — 11.6% of them — on a
form that is nearly all thin rules. Our ink came 1 099 pixels closer to `mutool`'s and is now
447 away on a page with 1.12 million marks on it.

This is the small movement that was expected and it is worth not over-reading. The fix changes
every stroke on every page, but most strokes on this page are a point or two long and the bowtie
was a few pixels, so a form of rules gains a fraction of a percent. What the measurement does
say is that the direction of the error was real and systematic — the arrow points at the ink and
not away from it — and that `pdfjs__bug1795263` was never evidence for this defect either way.

### What is pinned now

Five tests, all of which fail on the code above and pass on this one:

- `a_dashed_line_drawn_right_to_left_is_not_a_bowtie` — the reproduction, both directions, at
  pixel level, plus a sample in the middle of the middle dash.
- `a_three_point_path_in_the_negative_direction_is_not_hollow` — a `V` opening right, five
  samples on the path away from the corner, and reversal invariance.
- `a_sharp_corner_in_the_negative_direction_has_no_notch` — the mitre on the outside of a right
  angle six points off the path, walked from both ends.
- `a_cap_lands_at_the_ends_of_a_negatively_drawn_path` — butt, round and projecting, on four
  axis-aligned and two diagonal segments, the reach asserted from arithmetic and reversal
  invariance asserted for all three cap styles.
- `a_stroke_does_not_depend_on_which_end_the_path_was_written_from` — twelve fixtures across four
  widths and three cap styles, plus a dash case, written as a loop so it covers shapes nobody
  picked.

**No existing test needed changing.** That is the check worth making explicitly, because three of
them did fail on the first attempt and the reason was the ring seam rather than anything wrong
with the fix.

---

## D14 — an open path of three or more points is stroked as a closed ring

**Severity: high, and much wider than D13 — every stroked polyline that is not a single straight
segment, which is most of them. Open, found while fixing D13, and recorded rather than closed.**

`Device::stroke_outline` normalises a subpath before walking its dashes:

```rust
let closed: Vec<(f64, f64)> = if subpath.len() > 2 {
    let mut c = subpath.clone();
    if let (Some(first), Some(last)) = (c.first().copied(), c.last().copied())
        && (first.0 != last.0 || first.1 != last.1)
    {
        c.push(first);
    }
    c
} else {
    subpath.clone()
};
```

It pushes the first point onto the end of the subpath whenever the last point is not already the
first — which is precisely the condition for the subpath being **open**. So every open subpath of
three or more points is closed, and then stroked as a ring.

`transform_path` already appends the first point itself when it meets `PathSegment::Close`, so a
genuinely closed subpath arrives with `first == last` and the push is skipped. The branch is not
doing what its comment-free shape suggests: it is doing the opposite.

### The evidence

A 100×100 page, `0 0 0 RG 6 w 20 20 m 80 20 l 80 80 l S` — an `L`, two segments, no `h`. The
leftmost inked column of each device row, `#` for ink and `.` for paper at 1 pixel per point,
against `mutool draw -r 72` on the same file:

```
ours                                 mutool
rows 20..76: a band whose             rows 20..76: nothing at all
left edge steps one column           rows 77..82: 20-82, the horizontal bar
left per row, from column 76          rows 20..76: 77-82, the vertical bar
at row 21 to column 21 at
row 76 — a 45° band across the
corner the path never asks for
rows 77..82: 20-82, the bar
```

Ours has a 45° band running from device `(76, 21)` down to `(21, 76)` that the path does not
contain, and it has no caps at the two ends the path does have. Both are the closing segment:
the stroke runs from page `(80, 80)` back to `(20, 20)`, and the outline comes out as two rings
rather than one open outline, so there is nowhere for a cap to go. Asking for the outline says
the same thing in one line — two subpaths, and neither of them has a cap:

```
SUBPATH [(20,83), (83,83), (83,20)]
SUBPATH [(77,20), (77,77), (20,77)]
```

which is the outer and the inner boundary of a ring whose four corners are mitred, drawn as a
closed loop, for a path with two corners and two ends.

### What fixing it would mean

Not deleting the branch but making it able to tell an open subpath from a closed one, which means
the subpath has to say which it is — `transform_path`'s `Polygon` is `Vec<Vec<(f64, f64)>>` and
the information is thrown away at `PathSegment::Close`. That is a change to the path
representation rather than to the outline, and it moves every stroked path with three or more
points on every page, so it wants its own entry, its own corpus run and probably its own
fixtures for the cap and join work it unblocks. **It is the next thing on the stroke item.**

### What it blocks

`a_stroke_does_not_depend_on_which_end_the_path_was_written_from` deliberately leaves three-or-more
point *open* paths out of its list, and says so: comparing two renderings of a ring is comparing
two rings, which says nothing about the outline. The joins and caps of such a path are pinned
geometrically instead, by the two tests above it, because a sample read off a ring's corner is
measuring this defect and not D13.

---

## D15 — a bevel's corners are appended to the end of the assembled outline, not put at the corner

**Severity: low on an open path, and only where the miter limit refuses. Open, found while fixing
D13.**

When the miter limit refuses a mitre, `add_joins` pushes the two offset corners onto the outline
it was handed. For a **ring** that is the right place — the side's own point vector, and the
outline closes back to its first point — except that it is the right place only for the seam, and
the seam is the one vertex the loop skips. For an **open** path it is the wrong place outright:
`outline` has already been assembled as left side, end cap, right side back, start cap, so a
corner belonging at vertex `i` is appended after the start cap, where it draws a stray spike in
the middle of nowhere.

The loop also skips the seam on a ring (`first = usize::from(closed)`,
`last = path.len() - usize::from(closed)`), which after D13's fix is now the one corner that is
mitred but never bevelled.

**Left open on purpose.** It is only reached when `/ML` is small enough to refuse a mitre, which
is not the default, and fixing it means moving points inside a polygon that is already assembled
rather than passing an index — a change to the shape of `add_joins`, not to an argument in it.

---

## D16 — the fill's coverage depends on where the polygon's edge list starts

**Severity: very low — one boundary pixel, a few units out of 255. Open, found while fixing D13,
and the reason `RENDERING_SLACK` is 32 and not 0.**

Two renderings of the same stroke from opposite ends produce the same cycle of outline points,
but not the same *first* point, and the rasteriser's coverage is not invariant to that. Measured
across the fixtures in `a_stroke_does_not_depend_on_which_end_the_path_was_written_from`: a
sixty-point diagonal `6 w` butt cap differs by **4** units on one pixel; a horizontal line with a
round cap differs by **18** on the outermost pixel of the cap's sixteen-step arc; a two-point
vertical and a closed square ring differ by **0**.

The cause is in `accumulate_band`: crossings are sorted by `x` with a stable sort, so two edges
crossing at the same `x` — which is what happens at a vertex on a band boundary — are ordered by
their index in the edge list, and the pairing that follows reads the bottom of the band off
whichever edge came first.

`RENDERING_SLACK` in `tests/pages.rs` is 32 because of this, and the comment there says so. A
bowtie is a difference of ninety on every dash of every run, so the slack costs the test nothing
it needs.

---

## D17 — a CID was looked up through the font's `cmap` instead of its `/CIDToGIDMap`

**Severity: was every glyph drawn in a CID-keyed font, on the wrong reasoning. Fixed; and the
diagnosis this entry was opened with was wrong, in a way worth setting out.**

`corpus/wild/pdfjs__issue16263.pdf` page 1 is 40 copies of an equation whose content is inside
Form `Meta6`. That form was never executed, so the page drew **zero** ink and the defect below
never appeared; now that it is, 40 copies of the equation are drawn in the right places.

### What the file actually says

`mutool show` on the file, before anything was changed:

```
10 0 obj <</BaseFont/SymbolMT/DescendantFonts 11 0 R/Encoding/Identity-H/Subtype/Type0/ToUnicode 66 0 R/Type/Font>>
12 0 obj <</Type/Font/Subtype/CIDFontType2/BaseFont/SymbolMT/CIDSystemInfo 13 0 R
         /CIDToGIDMap/Identity/DW 1000/FontDescriptor 14 0 R/W 68 0 R>>
13 0 obj <</Ordering (Identity)/Registry (Adobe)/Supplement 0>>
```

Three facts, and the third is the one the fix is built on:

- The font is **CID-keyed**, not a simple font: `/Subtype /Type0` over a `/Subtype
  /CIDFontType2` descendant with `/CIDSystemInfo` present and `/Encoding /Identity-H` on the
  parent. `/CIDToGIDMap` is read from the *descendant*.
- `/CIDToGIDMap` is the **name `/Identity`** — declared outright, not absent and not a stream.
  Either way it means the same thing, because identity is what absence means, and that is
  what makes the rule below a rule and not a special case.
- The embedded program **does** have a `(3, 0)` subtable. It is 354 bytes of `cmap` with a
  format 4 table whose segments cover `U+F021`–`U+F072` only — six entries, mapping to glyphs
  4, 5, 14, 32, 48, 71 and 85 — plus a `(1, 0)` format 0 table of eleven codes. So the
  premise this entry was opened on, that a symbolic font has no `(3, 0)` subtable, is false
  for this font, and the arrow is *not* drawn as `.notdef`.

### What was wrong, and what the entry above claimed

The old `glyph_for_cid` consulted the `(3, 0)` subtable and fell back to reading the code as a
glyph number. On this font the `(3, 0)` subtable does not cover `0x000E`, the lookup returns
nothing, the fallback answers, and CID `0x000E` is glyph **14** — which is `uniF02B`, and which
`/ToUnicode` confirms is what the file meant:

```
outline_for_cid(0x000E)  13 segments, bounds (0, 0) – (0.5127, 0.5127)   glyph 14, uniF02B
outline_for_cid(0x0020)  10 segments, bounds (0, 0.1401) – (0.5127, 0.3706)   glyph 32
outline_for_cid(0x0041)   0 segments                                     glyph 65, a blank
```

Those are the numbers this entry was opened with, and they were read as glyph 0 because glyph
0's bounds are `(0.0503, 0) – (0.5503, 0.625)` and do not match them either. The arithmetic in
the old entry was wrong; the arrows were already the right glyphs. The render path agrees — with
the old code, `glyph_for_cid(0x000E)` returned `Some(14)` and the drawn outline was glyph 14's.

**The rule was still wrong, and this font proves it.** `glyph_for_cid(0xF02B)` returned
`Some(14)` — the `(3, 0)` subtable's answer — where `/CIDToGIDMap /Identity` says the glyph
number *is* `0xF02B`, which is past the font's 192 glyphs, so the answer is nothing. A code in
that range would have drawn a glyph the file never named. The old lookup was right by luck on
this page and wrong by construction everywhere else, and the luck was in the subtable missing
the two codes the page happens to use.

### The rule now

A CID is answered by the font's `/CIDToGIDMap` and by nothing else:

- **Absent, or the name `Identity`** — the identifier is the glyph number. The
  specification's default, and what this file declares.
- **A stream** — one two-byte big-endian entry per CID, indexed by it. A CID past the last
  entry maps to GID 0.

The font's `cmap` is not consulted, and that is the whole point: a `cmap` maps *characters* —
Unicode, or the symbolic codes a font's author chose — to glyphs, and for a symbolic font that
mapping is neither the identity nor anything the PDF declared. `CidToGid` is in
`metrics.rs` beside `CidWidths`, because both are read from the same descendant dictionary;
`mangle-render` reads it in `font_for` and carries it on `FontProgram`, since it is a property
of the *font dictionary* and not of the font program — the same TrueType file is reached as a
composite font's descendant in one page and as a simple font in another, and nothing inside the
program can tell those apart.

**A simple font's two-byte code is a different question and now has its own answer.** A simple
font with a CMap `/Encoding` has codes two bytes wide and they are character codes, so
`glyph_for_code16` answers them from the font's own `/Encoding`, `post` and `cmap` —
deliberately **without** the "the code is the glyph number" step that `glyph_for_code` has.
That step is right for a single-byte code, where a subsetted symbolic font's codes are its own
0-to-255 choice and the specification provides for reading one as a glyph number; it is wrong
for a two-byte code, where a `/CIDToGIDMap` stream is indexed over hundreds of plausible CIDs
and a code that names no character is a code the font does not have.

### `.notdef` is not drawn, and that is a decision

The specification maps a CID past the end of a stream to GID 0 so that "this font has no glyph
here" has an answer, and GID 0 is `.notdef` — a hollow box in most TrueType fonts. So a code
that reaches GID 0 draws **nothing**, and a code whose glyph the font does not have is `None`
rather than `Some(0)`. Painting it would put a character on the page the document never asked
for, which is the exact shape the mistaken diagnosis above was looking for and did not find.

### What is pinned now

Six tests, all verified to fail on the code above by reverting the source and keeping them:

- `a_cid_with_no_cid_to_gid_map_is_the_glyph_number` — CID 2 is glyph 2 and CID 1 is glyph 1
  on a font whose `(3, 0)` subtable says 1 and 4, checked on the **outline** and not only on
  the number, and contrasted with `glyph_for_code(2)` which legitimately answers 1.
- `a_cid_to_gid_map_stream_is_the_numbering_the_file_declared` — the same CID through a stream
  gives glyph 2, where the identity map would say 1 and the symbol subtable 4: three answers,
  one of which is the file's.
- `a_two_byte_character_code_in_a_simple_font_is_a_character_code` — codes 65 and 2 are
  answered from the `(3, 1)` and `(3, 0)` subtables, and code 3 is `None` even though the font
  has a glyph 3.
- `a_plausible_cid_that_names_no_character_is_not_turned_into_a_glyph_number` — the two routes
  side by side on one font: `glyph_for_code(3)` is `Some(3)` and `glyph_for_code16(3)` is
  `None`.
- `a_code_the_font_does_not_have_is_reported_rather_than_drawn_as_notdef` — four ways of
  reaching nothing, including a stream entry of 0 and a CID past the stream's end.
- `a_cid_to_gid_map_stream_is_the_only_thing_a_cid_is_looked_up_through` — end to end in the
  renderer: the same page under `/CIDToGIDMap /Identity` has ink, and under a stream pointing
  every CID at a blank glyph has none. It fails with 723 331 pixels of ink where it wants 0.

Plus `a_cid_to_gid_map_is_the_identity_map_unless_it_is_a_stream` for reading the entry itself.

**No existing test needed changing.** There were no tests of `glyph_for_cid` before these, and
the render-side composite fixtures all declare `/CIDToGIDMap /Identity` already, so they pass
unchanged.

### The page did not move, and why

`mutool draw -r 150` against page 1, 2000×1125, ink counted as pixels of luminance below 250:

| | SSIM | RMS | pixels above tolerance | our ink | `mutool`'s |
|---|---|---|---|---|---|
| before | 0.84906 | 73.51 | 237 475 (10.55%) | 272 715 | 104 890 |
| after | 0.84906 | 73.51 | 237 475 (10.55%) | 272 715 | 104 890 |

**Not one pixel moved, and the honest reading is that the fix does not touch this page.** The
arrows were already the right glyphs, as the table above shows; the change alters what happens
for codes in the range the `(3, 0)` subtable covers, and this page uses none of them. The
2.6× excess ink is somewhere else entirely, and it is worth being precise about where.

The excess is **`/Image15`**, the 2×2 indexed image inside the form, whose `/SMask` is a
34862×4332 `DeviceGray` image. Above each equation the oracle draws three thin arrows and
nothing else; this draws a **solid black bar** 285 by 17 pixels, one per column, in the band
where the arrows belong. The bar sits inside the image's placement box (`147.14 0 0 18.28 769.04
505.42`) and is present for all eight of the page's row bands, which accounts for 176 190 of the
~168 000 pixels of excess. Skipping the image and drawing nothing else moves the page to
**SSIM 0.94306, RMS 21.33, 66 655 pixels above tolerance and 70 595 pixels of ink** — below the
oracle's. So the page's remaining difference is an image-mask defect, not a font one, and it is
a separate finding.

**Left open in the other direction too, and that is not a defect here:** `/F1` in the same form
is `/TimesNewRomanPSMT`, which the document did not embed, so a metric-compatible face stands in
and the substitution is reported. The oracle draws the original outlines; this draws Times'.
That is D6's subject and the project's stated policy, not a bug.

## D18 — an `/SMask` was never checked against the image it masks, and a hostile one cost a gigabyte

**Severity: a page from the internet could make this renderer allocate a quarter of a gigabyte,
and the picture it was protecting was drawn solid with no word about why. Fixed. The visible
pixels on the corpus page do not move, and that is a decision rather than a null.**

D17 left one thing on this page and named it: `/Image15`, a 2×2 indexed image whose `/SMask` is
a 34862×4332 `DeviceGray` image. Three candidate causes were on the table and they are
different bugs, so it is worth saying which one it was.

### Which of the three it was

**The first: the soft mask's size was never validated against the image's.**

- **Not** `Raster::sample` clamping out of range. There is no separate mask type; a mask is a
  second `Raster` sampled at the same `(u, v)` as the picture, which is the specification's own
  arrangement, and `sample` clamps `u` and `v` into `0..=1` and then indexes the mask's *own*
  width. Sampling a wider mask at a narrower image's coordinates is not out of range at all —
  it is the whole of what a mask is for.
- **Not** the alpha read at the wrong scale. `mask_alpha(u, v)` is handed the image's own
  fractional coordinates, so one mask sample serves one image sample, and the mask's *luminance*
  becomes the alpha with the right weights.

What was actually there: `decode` called itself on `/SMask` with **no check of any kind** — not
the size, not the bound, not whether the samples covered the size. A mask 17 431 times the width
of the image it masks therefore decoded, and a decoded mask larger than its image samples a
constant, and a constant alpha paints the picture solid. The second and third candidates are
the *symptom* of the missing first one; the missing check is the defect.

### The hostile half, which is the half that matters

`MAX_IMAGE_PIXELS` was applied to a mask, because a mask is decoded by the same function as an
image — but it was applied **after** the mask's stream had been decoded, and under a codec there
was no check at all. Both measured, in a process whose own peak virtual size was the instrument,
against a file of a few hundred bytes:

| hostile `/SMask` | before | after |
|---|---|---|
| `/FlateDecode`, `/Width 34862 /Height 4332`, 408 kB inflating to 400 MB | peak grew **1 114 624 kB**, mask decoded to 34862 wide, **no note at all** | peak grew **400 kB**, refused, reported |
| `/DCTDecode`, 354 bytes claiming 16000×16000 | peak grew **750 004 kB**, refused for a reason about Huffman tables rather than about size | peak grew **0 kB**, refused on the bound |

The first row is the quarter of a gigabyte, and it was silent: the image drew, and nothing said
why it had no transparency. The second is worse in principle and the same in kind —
`zune_jpeg::decode` allocates `width × height × 3` up front from a `/SOF` marker, so a file is a
request for that much memory with a header and nothing behind it.

The corpus page itself cost 19 MB of transient allocation per draw — the mask's stream inflates
to 18 878 856 bytes, and the page draws the form containing it **35 times**. Rendering page 1
went from a 650 MB peak and 38 s to a 41 MB peak and 20 s, and that is the same defect seen from
the other end.

### What fixing it meant

`decode` now answers "how big is this" from the **dictionary**, before anything is decoded, and a
size above the bound is a refusal there rather than a warning 40 lines later. The exception is a
stream under an image codec, where the codec's own header carries the size and the dictionary may
be lying; there the header is read on its own and the bound asked of *it*, which costs a few
hundred bytes of parsing and no pixels.

`/SMask` goes through its own entry point, `decode_soft_mask`, and four things are refused there,
each with its own note because they are four different faults:

1. a mask **above `MAX_IMAGE_PIXELS`** — the one about memory rather than meaning;
2. a mask whose `/Width` and `/Height` **disagree with the image's**, which is what the
   specification requires and what a damaged or hostile file breaks;
3. a mask that **decoded short**. Padding a mask with zeroes is not a wrong colour, it is a
   *hole in the transparency*: `sample_at` past the end of the buffer answers zero, and for a
   picture zero may be a colour the file meant, while for a mask it is invisible content;
4. the recursive case, a mask carrying a mask of its own, which is not a thing and is where an
   unbounded recursion would start.

**Every one of them leaves the image drawn with its own colours at full alpha.** The colours are
real and worth showing; the mask is a claim about transparency that could not be checked, and it
is reported rather than believed. Dropping the picture would answer a loss of content with a
note, and treating the mask as zero alpha would hide content and call it a rendering.

### One more defect the mask found

`image_for` collected a decode's notes and then **threw them away whenever the decode
succeeded** — they were only read on the `Err` path. So a mask that could not be read was
reported nowhere at all, even though the picture drew. The lookup now returns the notes beside
the raster, `draw_mark` puts them on the page, and they are de-duplicated: this page draws the
form 35 times, and 35 identical lines of report is a report nobody reads. Before this change the
page carried **no note about its image at all**; it now carries one line naming `/Image15` and
both of the mask's dimensions.

### The corpus page did not move, and why

| | SSIM | RMS | above tolerance | our ink | `mutool`'s |
|---|---|---|---|---|---|
| before | 0.84906 | 73.51 | 237 475 | 272 715 | 104 890 |
| after | 0.84906 | 73.51 | 237 475 | 272 715 | 104 890 |

Not one pixel moved, and **the reason is the second of the two answers above.** `mutool` refuses
this image outright — it draws nothing in that band — and skipping the image here takes the page
to SSIM 0.94306 and *below* the oracle's ink, which is what makes the difference visible at all.
This renderer now draws the image at full alpha, because the rule is that an image whose mask
cannot be used is reported and drawn rather than dropped or hidden. So the bar is still drawn;
what changed is that the page now says why the picture has no transparency, and that asking the
question costs 400 kB instead of a gigabyte.

That is a real cost and it is recorded rather than dressed: **a hostile file can still make this
renderer allocate, and the guard is a bound rather than a policy.** The one number above it —
`MAX_IMAGE_PIXELS` — is 64 · 1024 · 1024 samples, which is 604 MB as RGBA, so a page carrying
several images *at* the bound can still reach a gigabyte in ordinary use. Tightening it is a
separate decision with a cost of its own: a 64-megapixel photograph is a real page, not an
attack.

### What is pinned now

Eleven tests, each confirmed to fail on the code before this change and to pass on it after.
Six in `image.rs`, five end to end in `pages.rs`:

- a mask whose size disagrees with its image's is reported, with both sizes in the note, and the
  image draws at full alpha in its own colours;
- a mask above the bound is refused, and **its stream is never decoded** — asserted by the
  address space it does *not* grow, in a child process, because a test that only checks the
  outcome cannot tell an early refusal from a late one;
- a mask of the **correct** dimensions still works, at a small size, asserted both as a raster
  and through `draw` so that the half that is transparent and the half that is opaque are
  distinguished;
- the **boundary**: exactly `MAX_IMAGE_PIXELS` samples is inside the bound and one pixel over is
  outside it, asserted on *which* refusal happened, because a mask at the bound cannot be
  decoded in a test and the reason is the only place the boundary is visible;
- a mask that decoded short is reported rather than padded with zeroes;
- a mask **under** a mask is reported rather than followed, because a chain of them is a file
  choosing the depth of a recursion rather than the renderer discovering one;
- the same bound on the **codec** path, where the size comes from a `/SOF` marker rather than
  from the dictionary, and where the decoder allocates before it knows anything else;
- and a mask's absence is full opacity rather than zero, which is the third of the three
  candidates this entry opened on, pinned so that it cannot drift into the second.

## D19 — an `ICCBased` colour was refused where the file named the answer, and a mask never read was never reported

**Severity: 42 of a 44-page NIST special publication rendered with zero ink pixels, and the
report said `could not be converted` 211 times without saying which gap it was. Both are fixed.**

Two defects, found together because they were found by reading the same file's report: a colour
conversion that refused a space the file had already given the answer to, and a decoder that
returned before it had looked at the rest of the image dictionary — so a picture it could not
read also hid a mask it could not read.

### First: `Colour` kept a name and threw away everything the name stood for

`ColourSpace` was `{ name: String, colorant: Option<String> }`, and `cs`/`CS` filled `name` with
the resource's name verbatim — `CS0` — so by the time a colour reached `Colour::to_rgba` the
only thing left of `[/ICCBased 107]` was the two characters `CS0`. `to_rgba` matched on
`name.as_str()` against a list of space *names*, none of which is `CS0`, so every colour in the
space fell through to `None`, and the fill, the stroke and the text that used it were each
reported and dropped. The file's own notes said so 211 times, 43 pages of a 44-page file.

The information was destroyed at the one place it was available. `cs` runs where the resource
table is in scope; `to_rgba` runs on a value carried out of the content stream with no document
behind it, which is why it could never recover it. So the fix carries it rather than looking it
up later:

- `ColourSpace` gains `icc: Option<IccBased>`, carrying the profile's `/N` and `/Alternate`
  (`crates/mangle-content/src/state.rs`);
- `Resources` gains a table of what each ICC-based colour space declares, built in `read_at`
  where the resolver lives — `/N` and `/Alternate` are keys of the profile stream, normally an
  indirect object away from the array that names it, and the interpreter holds no document
  (`crates/mangle-content/src/lib.rs`);
- `set_colour_space` copies that entry onto the space (`crates/mangle-content/src/interp.rs`),
  keeping the resource's own name for the report.

`to_rgba` then reads the colour through `/Alternate` — which is the file telling the reader what
to do, not an approximation of a profile this does not apply:

```rust
if self.space.icc.is_some() {
    let through = self.space.through_alternate()?;
    let mut resolved = self.clone();
    resolved.space = through;
    return resolved.to_rgba(ink);
}
```

**`?` on an absent `/Alternate` is the whole of requirement 3.** No alternate means no answer,
and the mark is reported by name — `the colour text is painted in the `ICCBased` space `CS0`,
whose profile names no `/Alternate` to read it through`. A fallback to RGB would paint the page
and be wrong about every pixel of it.

### Second: three call sites, and they had to become one

The fill, the stroke and the text each had their own `match colour.to_rgba(None)` and their own
note. That is three places for a conversion to be right and three for it to be wrong, and the
test that catches the difference is a page: the same space has to give the same colour as a fill,
as a stroke and as text **on one page**, because that is the property a unit test of the converter
cannot see. All four flat-colour sites — fill, stroke, text, and an image mask — now go through
`flat_colour(colour, what, notes)` in `crates/mangle-render/src/page.rs`, which converts once and
emits one note per distinct failure rather than one per mark.

One of those four changed behaviour for a good reason. An image that is not a mask paints its own
samples and never asks for the graphics state's colour, so a colour this cannot convert is no
reason to refuse it; an image mask is painted *entirely* in that colour, so the conversion is
now only made for a stencil. That was true before and is now said where the branch is.

### Third: should the *image* path also route through `/Alternate`?

`read_space` in `image.rs` already handled `ICCBased` — by counting `/N`, which is the asymmetry
this whole defect was: the same profile approximated on an image and refused on a fill. **It now
routes through `/Alternate` as well, and keeps `/N` as the fallback**, for two reasons:

1. `/Alternate` is the specification's own answer for a reader that cannot apply the profile, so
   it is the more correct reading of a profile that carries one — including the case where the
   two disagree, which is where `/N` alone gets it wrong.
2. The fallback is kept because an image and a colour fail differently. A colour with no answer
   is reported and the mark is not drawn, which is one visible hole. An *image* with no answer
   is a page with a photograph missing; reading its samples by `/N` and showing them approximately
   beats showing nothing, and that asymmetry is a deliberate difference rather than an oversight.

### Fourth: a mask that was never read was never reported

`decode_role` returned `None` for `JPXDecode` about 125 lines **before** the `/Mask` branch, and
for JBIG2 just above it. So an image whose samples could not be decoded also carried a `/Mask`
that was never examined, and the note said nothing about it — one gap reported where there were
two. `gov__nist-nistir7255.pdf` does exactly this, and its page carried no mention of its JBIG2
mask at all.

The codec arms are now the *only* thing that names the codec; everything else about the
dictionary is reported by a wrapper that runs on every refusal:

```rust
fn note_skipped(dict: &Dict, resolve: &dyn Fn(&Object) -> Option<Object>,
                role: Role, notes: &mut Vec<String>)
```

It reports the `/SMask`, the `/Mask`, and the codec of the `/Mask` where that codec is one this
does not decode — so `a JPEG 2000 image was found but no decoder exists yet; the image's
`/Mask` is a JBIG2 image and no decoder exists for one either, so it was not read` says that
there are two gaps and that JPEG 2000 work alone will not close this page. An image with neither
key reports only its own gap, which is the other half of the property: a note that lists
everything is no use if it lists things that are not there.

This matters for the JPEG 2000 work that is queued: it would have surfaced this gap the moment
JPX was attempted, and finding it then costs a debugging session.

### What it bought, measured against `mutool draw` at 150 DPI

`gov__nist-sp800-88.pdf` (W075), all 44 pages, before and after. The "before" column is that
file's own rows in `corpus/wild/report/pages.tsv` from the last full corpus run, so it is a
measurement rather than a recollection:

| | before | after |
|---|---|---|
| median SSIM | 0.7727 | **0.96095** |
| pages below 0.95 | 41 of 44 | **10 of 44** |
| worst page | **0.6352** (page 35) | **0.8103** (page 1) |
| median RMS | 50.54 | **20.38** |
| median pixels above tolerance | 166 481 | **85 951** |
| pages with zero ink pixels | 42 | **1** |

Page 35, the corpus's worst NIST page at 0.6352, is now **0.9023** — RMS 30.64, 117 245 pixels
above tolerance (5.573%), 225 656 ink against the oracle's 275 874. Its whole remaining
difference is unembedded-font substitution, which is D6's subject and the project's stated
policy. That is what "comparable" looks like: this file's median is now 0.961 against a
corpus-wide median of 0.958.

Page 1 was the one page still below 0.81, and it was **not** the ICC gap:

| page 1 | SSIM 0.8103 · RMS 36.09 · 232 787 above tolerance (11.065%) · 69 319 ink vs 316 363 |
|---|---|
| what it said | `a fill colour in Cs8 could not be converted` · `the colour text is painted in Cs8 could not be converted` |

`Cs8` is `[/Separation /Black 1209 0 R 3124 0 R]` — the tint-transform gap W076 lives in, and
the same one that dominates `pdfjs__TAMReview.pdf`. So the one page of 44 that was still badly
wrong was the one whose blocker was a *different* missing feature, which is the useful shape for
the diagnosis to have: this change removed the ICC gap and did not disguise the separation one.

**[D20](#d20--a-separation-was-painted-black-where-the-file-asked-for-a-tint-and-now-its-tint-transform-is-evaluated)
then removed the other gap too, and page 1 is now 0.98501 with 313 368 ink against the oracle's
316 364.**

**Page 2 is the remaining zero-ink page, and it is not a colour space at all.** Its `/Contents` is
an array of eight content streams, of which the reader resolves none — `page.content_streams`
returns empty for it, so the page renders as paper with no note. It has no `/ColorSpace` entry
beyond `/CS0`, and it draws 60 867 ink pixels' worth of vector outlines in the oracle. That is a
content-stream defect of its own, not this one, and it is recorded here rather than fixed here.

### The two files this was not supposed to disturb

Both were measured page by page against the same oracle, and both are **unchanged**:

| file | pages checked | SSIM before | SSIM after |
|---|---|---|---|
| `gov__nist-nistir7255.pdf` | 1, and 5–65 at stride 5 | 0.8655 / 0.6981 / 0.7949 / 0.7401 / 0.6706 / 0.6185 / 0.9883 | **identical** |
| `pdfjs__TAMReview.pdf` | 1, and 5–20 at stride 5 | 0.9357 / 0.6786 / 0.8021 / 0.7789 | **identical** |

`TAMReview` uses `Cs6` = `[/ICCBased …]` (64 `cs` and 5 `CS` across the file) and `Cs8` =
`[/Separation /Black …]` (82 `cs`). `Cs6` now converts, and the pages do not move: the text was
already drawn in a substitued face and the *body* of the article is in `Cs8`, which was still
refused. `the colour text is painted in Cs8 could not be converted` is unchanged and is
**correct**: a separation needs its tint transform evaluated, and there is no `/Alternate` to
fall through to. A change that turned every unconvertible space into RGB would have moved these
pages and been wrong.

**[D20](#d20--a-separation-was-painted-black-where-the-file-asked-for-a-tint-and-now-its-tint-transform-is-evaluated)
evaluated the tint transform, and the file's median moved from 0.7692 to 0.8879** — by evaluating
`Cs8`, not by turning it into RGB. The 22 pages still below 0.95 are now limited by a `TJ` array
truncation in the content lexer, which D20 names.

`nistir7255` contains no `ICCBased`, no `Separation` and no `Indexed` at all — 132 `JPXDecode`
and 66 `JBIG2Decode`, which is the codec gap — so the numbers are identical to the last digit,
which is the expected result and worth stating as such.

### What the file actually uses

`gov__nist-sp800-88.pdf`, counted across its 44 pages' `/ColorSpace` resource tables and its
content streams:

| name | resource | `cs`/`CS` uses | what it is |
|---|---|---|---|
| `CS0` | `[/ICCBased 1234 0 R]` | 43 `cs`, 17 `CS` | sRGB IEC61966-2.1, `/Alternate /DeviceRGB`, `/N 3` — **43 of the 44 pages** |
| `Cs6` | `[/ICCBased 3122 0 R]` | 10 `cs` | sRGB, `/Alternate /DeviceRGB`, `/N 3` |
| `Cs8` | `[/Separation /Black 1209 0 R 3124 0 R]` | 10 `cs` | spot black over `[/ICCBased 3122 0 R]`, tint transform in `3124` |
| `Cs9` | `[/Indexed 1209 0 R 202 3126 0 R]` | 0 | a 203-entry palette over the same ICC space |

So `ICCBased` was indeed the blocker and not `Indexed` or `Separation`: `Cs9` is declared and
never selected by any content stream, and `Cs8` is selected on one page only. `ICCBased` was the
answer to "which spaces does the file actually use", and the count confirms it.

### What is pinned now

**Fourteen tests**, each confirmed to fail on the code before this change and to pass on it
after. Six in `state.rs`, four in `interp.rs`, six end to end in `pages.rs`, and one on the image
path:

- an `[/ICCBased …]` colour with `/Alternate /DeviceRGB` converts to **exactly** the `/DeviceRGB`
  colour, asserted by identity and not by closeness, because it is the same space;
- `/Alternate /DeviceGray` and `/Alternate /DeviceCMYK` route to those arms, and a four-component
  profile's colour is CMYK *subtractive* — read as RGB it would be red;
- `/N` decides how wide a colour is: 1 is grey, 3 is RGB, 4 is CMYK, and an `/N 2` or an
  unreadable profile is three rather than something it is not;
- a profile with **no** `/Alternate` is refused, the note names the space and says why, and
  nothing is drawn in its place;
- `cs` carries the profile from the resource table to the conversion — the test that would fail
  if the information were dropped again at any point in between;
- the same space gives the same colour as a **fill, a stroke and text on one page**, which is the
  property that catches the three-call-site problem, and a page with three colours in one space
  gives three colours rather than one;
- an ordinary colour space renders exactly as before, marks, notes and all three squares' pixel
  values;
- a `Separation` is **still refused** and the page is not filled with a guess at what the tint
  looks like — *as of this change; D20 evaluates the tint transform, and what this one still
  covers is a `Separation` used as a **pattern's** shading colour, which is not a flat colour
  and is evaluated by the pattern, per pixel*;
- an image we cannot decode with an `/SMask` we also cannot decode produces a note naming
  **both**, and a `/Mask` whose codec is JBIG2 has that codec named too;
- an image with no `/Mask` and no `/SMask` produces only its own note; and
- a refusal that is **not** a codec — an impossible bit depth — reports its skipped mask too,
  which is what says the reporting is not living in the codec arms.

Plus one on the image path: an `[/ICCBased …]` image reads through its `/Alternate`, and where
`/N` and the alternate disagree the alternate wins, which is the case counting `/N` gets wrong.

---

## D20 — a separation was painted black where the file asked for a tint, and now its tint transform is evaluated

**Severity: it was the largest single colour-space gap in the corpus. Fixed for `/Separation` and
`/DeviceN`; the pages it freed are now limited by a different defect, named below.**

### What was wrong, and it was worse than a refusal

D19 added the `ICCBased` route: read the components through the profile's `/Alternate`. A
`Separation` has no such route, because its components are not a colour at all — one scalar per
colorant, a *tint* — and what the space says a tint means is its `/TintTransform`. Until now the
conversion for `"Separation" | "DeviceN"` was:

```rust
"Separation" | "DeviceN" => ink.and_then(|i| i.to_rgba(None)).or(Some(Rgba::BLACK)),
```

Two things are wrong with that line, and the second is the one that mattered.

The first is that it ends in `Rgba::BLACK`. A spot colour is the **one** space where black is a
plausible guess, which is exactly why it is the one where a guess is most likely to be believed:
a reader that paints every tint of every spot colour black looks correct on a page that only ever
uses tint 1. And `ink` was `None` at every call site in the product, so the `.or(Some(BLACK))` was
what actually ran.

The second is quieter and is where the pages were. `ColourSpace::name` is the **resource key** a
content stream used — `Cs8`, not `Separation`. The `match` above is on that name, so the arm was
**unreachable for every real file**; a separation fell through to `_ => None` and was reported. So
the fallback never fired either, and 16 of TAMReview's 23 pages drew only their running furniture.

### What it does now

The tint transform is a PDF function, and all four kinds already existed in
`crates/mangle-render/src/shading.rs` — `Sampled`, `Exponential`, `Stitching`, `Calculator`. No
second evaluator was written. The four kinds and `Function::parse` moved **down** into a new
`crates/mangle-content/src/function.rs`, because the dependency direction is
`content -> render/text` and the conversion that needs them lives in `mangle-content`:
`Colour::to_rgba` is in the graphics state, a shading's function is only how a shading is
defined, and a `/TintTransform` is an ordinary document object that two unrelated callers need.
`mangle-render` re-exports the module as `shading`, so a gradient's function is still reached the
same way.

What the change does, in the file's own order:

1. read `[/Separation /Black <alternate> <transform>]` and `[/DeviceN /A /B … <alt> <xf>]`
   once per page, in `Resources::read_at`, beside the ICC profiles, and carry the result on
   `ColourSpace` — because `cs`/`CS` is the only moment the resource table can be consulted;
2. take the tint, **clamped** to 0..1, and hand it to `/TintTransform`. A `/Separation` has one
   tint; a `/DeviceN` has one per colorant, read from `/Names` rather than assumed;
3. read the transform's output in `/Alternate`, which is usually a device name and is an
   `[/ICCBased …]` array in most real files — kept whole, so it goes through the same
   `through_alternate` an `ICCBased` fill colour does;
4. **refuse, by name,** where any of that is unavailable. Not black. Not grey.

Three things are refused rather than guessed, and the reasons are the same as everywhere else in
this file: a transform that is missing or unreadable, a transform that cannot answer at this
tint, and an output that does not fill the alternate's components — padding a one-value answer
out to three invents two thirds of an RGB colour. A `/DeviceGray` alternate is the common case and
needs none of that care: its single component *is* the grey level.

`/DeviceN` supports both shapes the specification allows: one function taking one input per
colorant, and one function taking a single input applied to each colorant independently. A
function whose input count is neither is refused — a three-input transform for a two-colorant
space has no reading, and picking the inputs that look right is how a colour becomes a
coincidence.

One latent bug came with it. `Function::inputs()` returned `domain.len() / 2`, and `domain` is
already one `[min, max]` pair per input — so it reported **zero** inputs for every function in
existence. No gradient noticed, because each kind reads `domain.first()` directly. Deciding how
many tints to hand a transform is exactly the question it answers, so a separation's tint
transform returned `None` on every tint until it was fixed.

### What it measured

Both against `mutool draw` at 150 DPI, all 23 pages of TAMReview and page 1 of sp800-88:

| file | pages | before | after |
|---|---|---|---|
| `pdfjs__TAMReview.pdf` median SSIM | 23 | 0.7692 | **0.8879** |
| pages below 0.95 | | 22 | 22 |
| worst page | | 0.6532 | 0.8210 |
| best page | | 0.9592 | 0.9840 |
| page 17 ink pixels | | 5 937 | **173 940** (oracle 245 598) |
| `gov__nist-sp800-88.pdf` page 1 SSIM | 1 | 0.81035 | **0.98501** |
| page 1 ink pixels | | 69 341 | **313 368** (oracle 316 364) |

**Not one page of either file reports `could not be converted` any more.** sp800-88's page 1 was
the corpus's one badly-wrong NIST page; at 0.985 it is now among the best pages of the file, so
its 0.96095 median can only have risen.

### Why TAMReview is still 22 pages below 0.95 — and it is not colour

`corpus/wild/report/W076-p17-diff.png` after the change shows the body text present and the
diff almost empty. What is left is described by the pages' only remaining note — the unembedded
`/TiRoARRN~1268702012` — and by measurement:

| page 17 | ours | oracle |
|---|---|---|
| glyphs the interpreter places | 2 661 | 3 960 (`mutool trace`) |
| of which `/EMMOLK+Cambria` | 2 388 | 3 687 |
| `/EMMONL+Cambria-Bold` | **199** | **199** |
| `/EMMOML+Calibri` | **5** | **5** |
| `Times-Roman` | **69** | **69** |

Three of the four faces match to the glyph, so the font layer is not failing: the loss is
**entirely** in the regular Cambria. It is not a coverage failure either — every code is short by
a near-constant 35%, which is what losing whole show operations looks like and not what missing
glyphs look like.

The page's text lives in one form XObject (`222 0 obj`, `/Name /ARUA`). Its `TJ` arrays are
kerned per character, with **no whitespace between the elements**:

```
[(cid)82.3(cid)77.5(cid)81.6(cid)5.3(cid)82(012)82.5(cid08 cid)86.3 …]TJ
```

That show carries **84 strings**. `crates/mangle-content/src/tokens.rs` has
`MAX_COLLECTION_DEPTH: usize = 32`, documented as a bound on how *deep* a bracketed collection
may nest — and `collect` uses it as a bound on how many *items* a collection may hold:

```rust
other => {
    if items.len() < MAX_COLLECTION_DEPTH {
        items.push(object_of(other));
    }
}
```

So every array of more than 32 elements is silently truncated, and a `TJ` array is the commonest
array in a text-heavy file. The first 32 items of that show are 16 strings and 16 kerns; the
remaining 68 are dropped, and the line is cut mid-word. **That is the whole of the remaining
0.8879.**

This is a content-stream lexer defect, unrelated to colour, and it is **not fixed here** — it is a
separate change with its own verification, and the bound is there for a reason (a hostile file
can open a bracket and never close it). It is named because it is now the largest single cause of
difference left in this corpus, and because nothing in the report above would be honest without
saying what the number is still limited by.

### What is pinned now

**Fifteen tests**, each confirmed to fail on the code before this change and to pass on it after —
nine in `state.rs` and five end to end in `pages.rs`, plus one that says the other colour spaces
did not move:

- a separation at tint 1 is **the transform's own value at 1**, asserted by identity against what
  the transform returns rather than against a number written beside the assertion;
- tint 0 is the transform's value at 0 and a mid tint is the transform's value there — the test
  names the number a straight line between the two ends would have produced, so a converter that
  interpolated would fail it;
- a tint outside 0..1 is **clamped**, and the test says which end each one landed on;
- the same separation gives the identical colour as a **fill, a stroke and text on one page**;
- a `/DeviceN` transform with one input per colorant yields **both** colorants from the one
  answer, and the test says that neither was defaulted to zero;
- a `/DeviceN` transform taking **one** input is applied to each colorant independently, with the
  answers concatenated;
- a transform taking the wrong number of inputs is **refused**, which is the case worth saying
  out loud: the multi-input form is supported, an input count that is neither one nor the colorant
  count is not, and nothing else is either;
- a `/DeviceGray` alternate reads the transform's own single value, and an `[/ICCBased …]`
  alternate goes through the profile's `/Alternate` to the same `/DeviceRGB` colour;
- a separation whose transform is **missing** or is not a function is reported **by name** — the
  space, its colorant, and `/TintTransform` — and nothing is drawn;
- the alternate-colour rendering fallback is still a fallback: it stands in where the file gave
  nothing to convert, and never in place of a transform that answered;
- and every other colour space — grey, RGB, CMYK, `CalRGB` — converts to exactly what it did, with
  `Indexed` and `Pattern` still gaps rather than quietly becoming tints of something.
