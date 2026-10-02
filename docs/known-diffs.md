# Known differences from the oracles

FINISH.md G3.1: *"per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31,
and Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root
cause and evidence."*

**The Tier-B target is not met, and not narrowly.** Against `mutool draw` at 150 DPI over the
77-file wild corpus: 460 pages were compared, the median SSIM is **0.6929**, and **436 of 460
pages (94.8%) are below 0.95**. Thirty-five of the 77 files could not be opened at all. This
page records what the corpus found and the evidence for each claim; none of it is fixed yet.

The corpus is `corpus/wild/`, pinned by SHA-256 in `corpus/wild/MANIFEST.toml`, fetched with
`cargo xtask corpus fetch`, and measured by `crates/mangle-render/tests/wild_corpus.rs`.
That test asserts nothing: the numbers below come from `corpus/wild/report/SUMMARY.md` and
the per-file reports beside it, which are git-ignored and regenerated on every run. Oracle
versions: `mutool 1.28.5`, `pdftotext 26.08.0` (poppler), `qpdf` (system).

Reproduce any line:

```sh
cargo xtask corpus fetch
cargo test -p mangle-render --test wild_corpus -- --nocapture
```

---

## D1 — `render_page` renders a blank page for any Flate-compressed content stream

**Severity: critical. 440 of 460 measured pages.**

`render_page` hands the *raw* stream bytes to the content-stream interpreter instead of the
decoded ones. The interpreter then reports the compressed bytes as operator names.

Evidence, `pdfjs__alphatrans.pdf` page 1 — a plain classic-cross-reference PDF whose
`/Contents` is object 6, `<</Filter /FlateDecode /Length 412>>`:

- Our render: a blank white page. `marks: 0`.
- Our note, from `corpus/wild/report/pdfjs__alphatrans.md`:
  `operator \`x????N?0??y??RL=vL=K'\` at byte 0 is not in the table and was not executed`.
- The object 6 stream in the file begins `x µ Á N ã 0 ï y  RL = v  »  \ K '`. **The garbage in
  the note is that byte for byte.** `zlib.decompress` on those exact bytes yields
  `0.57 w 0 J 0 j [] 0 d 0 G 0 g\nBT /F1 12.00 Tf ET\n…`.
- The decoder is not broken: `manglepdf-cli extract pdfjs__alphatrans.pdf -page 0`, which
  goes through `Page::decoded_contents`, prints `Red: stroke=1, fill=1.` … `Powered by TCPDF`.
  So `decoded_contents` decodes and `render_page` does not use it.

Files where every measured page drew nothing: `freeculture.pdf` (344 of 350 pages, a
352-page typeset book), `TAMReview.pdf` (23 of 23), `nist-nistir7255.pdf` (60 of 62),
`bug1992868.pdf` (14 of 14), `issue12337.pdf` (14 of 14), `arxiv-1206.5537.pdf` (23 of 23),
`arxiv-1512.03385.pdf` (12 of 12), and 21 single-page files.

`freeculture.pdf` page 255 makes it plain: `W077-p255-ours.png` is empty paper,
`W077-p255-theirs.png` is a full comic page, and `W077-p255-diff.png` is therefore a
perfect black silhouette of every panel and every letter.

**Why the fixture suite missed it:** every hand-written fixture writes its content stream
unfiltered or through `/ASCIIHexDecode`, both of which need no inflate.

---

## D2 — a cross-reference stream with a PNG predictor is not decoded, so the file reports zero pages

**Severity: critical. 31 of the 34 files that would not open.**

When the reader cannot use the cross-reference stream at the `startxref` offset it falls
back to scanning for `N G obj` headers. That scan cannot see objects stored inside object
streams, so `/Pages` is unreachable and the document reports **0 pages** with the note
`dangling reference: the page tree root is missing`.

Correlation over the corpus is exact:

| cross-reference stream | PNG predictor | result |
|---|---|---|
| no | no | 39 opened |
| yes | **yes** | **30 failed** |
| yes | no | 3 opened |
| no | no | 3 failed — a 184-byte stub, a fuzzed file and an encrypted file, all legitimately broken |
| yes | yes | 1 opened — a hybrid file whose `startxref` points at a classic table with `/XRefStm` |

The one apparent exception (`pdfjs__issue20324.pdf`) is a hybrid-reference file read through
the classic-table path, not the predictor path.

Evidence, `gov__irs-fw4.pdf` — **live IRS Form W-4**, which `qpdf --check` reports as clean:

```
$ manglepdf-cli info corpus/wild/gov__irs-fw4.pdf
pages       0
structure   repaired (3 note(s))
            - no cross-reference section at offset 208493
            - 112 objects were located by scanning
            - the catalogue was found by scanning
```

`startxref` is `208493`, and the bytes there are
`3594 0 obj <</Length 44/Type/XRef/Root 3540 0 R/… /Index[3540 1 3563 1 3589 2 3592 3]/W[1 3 1]/DecodeParms<</Columns 5/Predictor 12>>/Filter/FlateDecode>>`.
A well-formed cross-reference stream that qpdf reads without a warning.

The decisive experiment — rewrite the *same content* with the streams expanded and nothing
else changed:

```
$ qpdf --object-streams=disable irs-fw4.pdf irs-fw4-nostream.pdf
$ manglepdf-cli info irs-fw4-nostream.pdf
pages       5
structure   as written
```

Same for `pdfjs__annotation-highlight.pdf` (0 → 1 page) and `gov__nist-fips197.pdf`
(0 → 52 pages).

Also affected: **IRS Form 1040**, **NIST FIPS 197** (52 pp), **NIST SP 800-88** (44 pp),
NIST IR 7657, both USGS 1:25000 topo sheets, all six Microsoft Word 2013 exports carrying
annotations, both JPEG 2000 files, every Adobe InDesign output in the corpus, and
`bug1885505.pdf` (a pdfeTeX paper).

Bisection already run on `pdfjs__bug1885505.pdf`, which reports
`no cross-reference section at offset 116` over a valid stream: removing `/DecodeParms`,
removing `/ID`, removing `/Index`, changing `/W`, correcting `/Length` by ±3 bytes, and
rewriting every bare CR as LF each left the failure in place. The predictor itself has not
been isolated by direct experiment yet.

---

## D3 — the page canvas gains a spurious row whenever the size in points times the scale should be a whole number

**Severity: high, and small. 80 pages across 17 files became impossible to compare.**

`render_page` computes `(points * scale).ceil()` with the division hoisted into `scale`.
In IEEE-754:

```
792.0 * (150.0/72.0) == 1650.0000000000002   ->  ceil == 1651
792.0 * 150.0 / 72.0 == 1650.0               ->  ceil == 1650
561.6 * (150.0/72.0) == 1170.0000000000002   ->  ceil == 1171
```

So US Letter (612 × 792 pt) comes out **1651 rows tall where `mutool` produces 1650**, and
`compare()` refuses to compare images of different sizes. Every such page is recorded in the
report as `size disagreement: we rendered 1275x1651, mutool rendered 1275x1650`.

That is 80 pages, including **all 23 pages of an arXiv paper** and **all 12 pages of a second
arXiv paper** — which is to say the corpus currently measures almost no LaTeX at all.

Note the existing fixtures did not catch this either: they are 200 × 200 pt and similar, where
`200 × (150/72) = 416.666…`, and both renderers round up to 417.

---

## D4 — an uncompressed cross-reference stream is not read

**Severity: high. One file, and a whole class.**

`pdfjs__bug1539074.pdf`, a pdfTeX-1.40.17 page, whose only cross-reference section is
`<</Type /XRef /Index [0 18] /Size 18 /W [1 2 1] /Length 72>>` — **no `/Filter` at all**, so
the entries are raw bytes.

```
$ qpdf --check bug1539074.pdf
No syntax or stream encoding errors found
$ manglepdf-cli info bug1539074.pdf
pages       0
            - no cross-reference section at offset 5715
```

---

## D5 — a page whose image comes from a multi-page TIFF is drawn as a full-page raster

**Severity: high. 0.36 SSIM, 54% of pixels wrong.**

`pdfbox__multitiff.pdf`, all three pages: 0.3715 / 0.3685 / 0.3591, `max delta 255`, and
1 174 623 of 2 176 714 pixels above tolerance on page 1.

- `W035-p1-ours.png` — the whole page is a black-and-white striped raster.
- `W035-p1-theirs.png` — a white page with the numeral "1" on it, and nothing else.
- `W035-p1-diff.png` — black everywhere except a white silhouette of that "1".

`marks: 1` on our side: we draw exactly one thing, the raster, stretched over the page.
`mutool` draws no image and puts a page number on the page. So the page-boundary handling for
a multi-page TIFF is wrong, and the page label is missing.

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
finds faults being called a corpus. Where pages *did* draw something:

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

The D1 blank-page finding is almost certainly masking everything downstream of it. Until D1 is
fixed, the render numbers for a Flate-compressed document describe an empty page, and any
of the nine files above could be hiding a real defect behind it.