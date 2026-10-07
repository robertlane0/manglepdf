# Known differences from the oracles

FINISH.md G3.1: *"per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31,
and Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root
cause and evidence."*

**The Tier-B target is not met, and not narrowly.** Against `mutool draw` at 150 DPI over the
77-file wild corpus: **740 of 745 pages produced an SSIM** and the page-level median is
**0.96015**, and **165 of those 740 pages (22.3%) are below 0.95**. Seventy-three of the 77
files open; **four cannot**. This page records what the corpus found and the evidence for each
claim.

**The page distribution improved a great deal and the per-file median moved only a little.**
Against the previous full run: the pages below 0.95 went **254 → 165**, a **35% reduction**, and
the per-page median went **0.95795 → 0.96015**. Both movements are real. **The per-file median
went 0.9698 → 0.97375**, which is a much smaller move than the page count implies: the gains
landed in files that were already being counted as good, and a file's own worst page rarely came
back over the line. Neither movement is spread evenly, and saying so is the point of the rest of
this page: **the gain is concentrated in a handful of files**, and four files still account for
most of what is left.

The corpus is `corpus/wild/`, pinned by SHA-256 in `corpus/wild/MANIFEST.toml`, fetched with
`cargo xtask corpus fetch`, and measured by `crates/mangle-render/tests/wild_corpus.rs`.
That test asserts nothing: the numbers below come from `corpus/wild/report/SUMMARY.md` and
the per-file reports beside it, which are git-ignored and regenerated on every run. Oracle
versions: `mutool 1.28.5`, `pdftotext` (poppler), `qpdf` (system).

> **These figures are a re-run, not an estimate.** The block above is the whole 77-file corpus
> measured end to end: 77 files, **16646.23s**, **0 panicked**, **73 opened and 4 closed**,
> `test result: ok`. It replaces the previous run (16326.09s, 73 opened, 738 of 745 pages with an
> SSIM), which in turn replaced one made when 34 of the 77 files could not be opened at all — and
> every figure in that run, the 460-of-542 comparison count, the 0.6929 median and the
> 94.8%-below-0.95 figure an earlier version of this file carried, described a corpus with a third
> of it missing. **Two more pages were compared this time and the two size disagreements are
> gone**, which is where the difference between 738 and 740 comes from. **Of the 73 that open, 67
> read their structure as written; 6 needed recovery** (`bug1795263`, `bug1980958`,
> `GHOSTSCRIPT-698804-1-fuzzed`, `issue15590`, `issue15893_reduced`, `PDFBOX-3148-2-fuzzed`), and the
> run's own summary counts 9 of the 77 as having needed recovery once three of the four that did
> not open but left reader notes are included.

**The four that do not open are the same four as last time**, unchanged in kind and unchanged in
reason — `issue19484_1`, `bug1020226`, `REDHAT-1531897-0` and `issue21579`, each itemised
[below](#the-four-files-that-still-do-not-open). **73 opened in this run and 73 in the previous
one**: the same four files, the same four reasons, and no file has changed sides since D2.

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
| wall clock | **16646.23s** |
| panicked | **0** |
| pages measured | **745** |
| pages with an SSIM | **740** |
| page-level median SSIM | **0.96015** |
| page-level mean SSIM | **0.94274** |
| size disagreements | **0** |
| pages left uncompared on purpose | 2 |
| pages the oracle could not render | 3 |

The page-level median and mean are not stated in the run's summary — they are computed from
`corpus/wild/report/pages.tsv` over the **740** rows that carry an SSIM. Nothing else in this
section is derived; the rest is quoted from the run.

### Per file, by its worst page

Over the **68** files that have at least one page with an SSIM. 73 files open and 745 pages are
measured; five of the opened files produced no SSIM at all (`usgs-topo-cnmi-1`, `usgs-topo-cnmi-3`,
`GHOSTSCRIPT-698804-1-fuzzed`, `issue15590`, `issue15893_reduced` — the reasons are itemised
[below](#the-five-pages-with-no-ssim-and-why-each-is-missing)), so 73 − 5 = **68**:

| worst-page SSIM | files |
|---|---|
| ≥ 0.99 | 22 |
| ≥ 0.95 | 23 |
| ≥ 0.90 | 6 |
| ≥ 0.80 | 13 |
| < 0.80 | **4** |

**Median 0.97375, mean 0.93588.** The mean is 0.038 below the median because those four files are
not a little way under 0.80 — the best of the four is 0.7564 and the worst is 0.5716, and one of
them is a single page. **The median here moved only from 0.96980**, which is worth reading
alongside the 35% reduction in the pages below 0.95: most of the files that improved were files
whose worst page was already at or above 0.95, and a file is scored on its worst page, so a run of
good pages inside it changes little this table can see.

### Per page

Over the **740** pages that carry an SSIM:

| | pages | share |
|---|---|---|
| below 0.95 | **165** | **22.3%** |
| below 0.90 | 87 | 11.8% |
| below 0.80 | 50 | 6.8% |
| below 0.50 | **0** | 0% |

**Zero pages below 0.50** is the one unambiguous good number in the run, and it is worth saying
what it means: no page is blank where the oracle has a page, and none is grossly wrong. Every
one of the 165 is a page that draws *something* and draws it incompletely, in the wrong place, or
in the wrong colour.

### A median overstates fidelity while a cluster is missing whole pages of content

This is the caveat that decides whether the numbers above should be believed, so it belongs next
to them rather than in a footnote. **A page rendered as bare paper scores about 0.79 against the
real page**, which is high enough to sit near the median of a corpus and low enough to be
invisible in it. A page where a whole cluster is missing — every figure, every shaded box —
therefore moves the median far less than the fix that restores it does. The 0.95795 → 0.96015
movement is a real 35% reduction in the pages below 0.95, and it is also the kind of number that
would barely register if a single file had gone from 30 wrong pages to 1.

**The count of pages below 0.95 is the number the gate is written against**, and it is the number
to watch. The median is the number that flatters.

### The five pages with no SSIM, and why each is missing

**This run is the first to measure the whole corpus after D26**, so the two `freeculture` size
disagreements are gone and those pages now carry a number — `freeculture` is **352 of 352** pages
compared, where the previous run could only compare 350.

The 745/740 split matters as much as the SSIM does, because a page that produced no number is a
page that was never measured and must never read as one that passed.

| page | why there is no SSIM |
|---|---|
| `gov__usgs-topo-cnmi-1.pdf` 1 | 25119300 oracle pixels, above the 16777216 this harness will compare. Rendered at 150 DPI and **left uncompared on purpose** — the bound is the harness's own memory ceiling, not a property of the file. |
| `gov__usgs-topo-cnmi-3.pdf` 1 | 25127777 oracle pixels, same bound. |
| `pdfjs__issue15590.pdf` 1 | mutool produced no page 1; **it could not render it** |
| `pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf` 1 | mutool produced no page 1; **it could not render it** |
| `pdfjs__issue15893_reduced.pdf` 1 | mutool produced no page 1; **it could not render it** |

**Both size disagreements were ours, and both were the same defect: we were one pixel taller than
`mutool` on both pages.** It looked like D3's shape, and D3's fix did not cover it. D3's epsilon
was sized for *arithmetic* error — `1650.0000000000002` from `792.0 * (150.0 / 72.0)` — and
`freeculture` fails by something three orders of magnitude larger: its `/CropBox` is written to
eight decimals, so the height is `734.400024` where `734.4` was meant, which at 150 DPI is
`1530.00005` pixels, and a `1e-9` tolerance cannot see it. **Representation error, not
arithmetic error** — and a tolerance sized for the first is useless against the second. The
epsilon is now `1e-6` relative, measured against `mutool` rather than guessed. See
[D26](#d26--a-cropbox-written-to-eight-decimals-made-the-buffer-a-pixel-taller-and-the-tolerance-meant-to-prevent-that-was-a-thousand-times-too-tight).

The three pages `mutool` cannot render are not differences at all: there is no second opinion to
have. They are recorded because a page the *oracle* refuses is a page nobody has checked.

### Where the 165 are

**They are not scattered, and that is the most useful thing in this section**, because it says
what is still worth doing and what is not. `fips197` is listed at the 25 it was measured at, with
the one page that has since been fixed noted against it, so that the table reads against the run
it belongs to:

| pages | file | cause |
|---|---|---|
| **61** | `gov__nist-nistir7255.pdf` | JPEG 2000 — **out of scope**, see [D22](#d22--jpeg-2000-is-not-decoded-and-the-decision-to-leave-it-that-way-was-measured-rather-than-assumed) |
| **24** | `gov__nist-fips197.pdf` | **all font substitution** — [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect). The 25th page, the one `ICCBased` gap, is **fixed** — [D30](#d30--an-icc-profile-names-its-own-colour-space-and-fips197-omitted-alternate-rather-than-failing-to-declare-one) reads the space the profile's own header declares, and page 1 went 0.7564 → 0.9932 |
| **22** | `pdfjs__TAMReview.pdf` | median 0.9240; an unembedded font, see D21 and D24 |
| **14** | `gov__nist-nistir7657.pdf` | **all font substitution** — [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect) |
| 12 | `issue12337` 3, `highlights` 3, `bug1992868` 2, `comments` 2, `ZapfDingbats` 2 | font substitution, as in [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect) — *not* tiling, see [D25](#d25--a-tiling-pattern-was-refused-rather-than-tiled) |
| **10** | `pdfjs__freeculture.pdf` | 9 in the previous run; its worst page is the corpus's at 0.5716 |
| 7 | `gov__nist-sp800-88.pdf` | was **41** before the ICCBased fix |
| 3 | `gov__arxiv-1512.03385.pdf` | |
| 11 | 11 other files, 1 page each | |

**62 of the 165 are the two JPEG 2000 files** — `nistir7255` 61 and `S2` 1 — which are now
formally out of scope, so the **addressable remainder is about 103 pages**. And those 103 are not
spread across the corpus either: **four files account for 70 of them**, `fips197` 24,
`TAMReview` 22, `nistir7657` 14 and `freeculture` 10. Everything else is one or two pages each
across a dozen files, which is the shape of long-tail noise rather than a defect with a name.
**`fips197` was 25 in the run these figures come from** and is 24 as of
[D30](#d30--an-icc-profile-names-its-own-colour-space-and-fips197-omitted-alternate-rather-than-failing-to-declare-one),
which is the whole of that file's real gap; a full corpus run has not been repeated since, so every
other number here still belongs to the run that produced it.

**And of the 103, about 38 are this project's own font-substitution policy** — `fips197` 24 and
`nistir7657` 14 — which is a decision rather than a defect and is measured in
[D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect).
**So the largest remaining cause of a page below 0.95 is not a missing feature.**

The largest clusters, and what each one actually is:

| id | file | what is missing | what the renderer says |
|---|---|---|---|
| W034 | `gov__nist-nistir7255.pdf` | a **JPEG 2000** decoder (`JPXDecode`) | `a JPEG 2000 image was found but no decoder exists yet`, twice, on **all 66 pages**. Both images the page draws are JPX, and one carries a `JBIG2Decode` `/Mask`. **Refused by name and formally out of scope — see [D22](#d22--jpeg-2000-is-not-decoded-and-the-decision-to-leave-it-that-way-was-measured-rather-than-assumed), where a written decoder is recorded as having six defects and not being shipped.** |
| W074 | `gov__nist-fips197.pdf` | **nothing — the last real gap is fixed** | 51 of its 52 pages carry one note and one only: the unembedded-font one. **The page that needed an `ICCBased` profile with no `/Alternate` no longer does** — [D30](#d30--an-icc-profile-names-its-own-colour-space-and-fips197-omitted-alternate-rather-than-failing-to-declare-one) reads the space the profile's own header declares, page 1 carries no note at all and scores **0.9932** where it was 0.7564, and the file's worst page is 0.8432. All 24 pages still below 0.95 are the face, not the layout. [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect) measures the rest. |
| W076 | `pdfjs__TAMReview.pdf` | ~~**`Separation`/`DeviceN` tint-transform evaluation`~~ | **PARTLY FIXED — see [D20](#d20--a-separation-was-painted-black-where-the-file-asked-for-a-tint-and-now-its-tint-transform-is-evaluated) and [D21](#d21--a-collection-holds-how-many-items-which-is-a-different-question-from-how-deep-it-nests).** `Cs8` is `[/Separation /Black <ICCBased sRGB> <FunctionType 0, 255 samples>]`, and it is the colour of the entire article body. **The transform is evaluated now: no page of the file reports `could not be converted`, and the six pages that drew 5,937 ink pixels draw 100k–174k. The file's median SSIM went from 0.7692 to 0.8879, then to 0.92395 once the `TJ` truncation was fixed, and this run measures 0.9240.** The 22 pages still below 0.95 are limited by an unembedded font — see D21 and [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect) — and not by colour or by truncation. |
| W073 | `gov__nist-nistir7657.pdf` | **nothing — this is not a gap** | 46 pages carry only the unembedded-font note. The file has **49 images and no JPX or JBIG2 at all**, so it is not the codec story. See [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect). |
| ~~W075~~ | ~~`gov__nist-sp800-88.pdf`~~ | ~~**`ICCBased` colour conversion`~~ | **FIXED — see [D19](#d19--an-iccbased-colour-was-refused-where-the-file-named-the-answer-and-a-mask-never-read-was-never-reported).** `CS0` is `[/ICCBased …]` over an sRGB profile carrying `/Alternate /DeviceRGB`, and that alternate is now what the colour is read through. **43 of the file's 44 pages went from zero ink pixels to ink and its median SSIM from 0.7741 to 0.96095; this run measures the median at 0.9648 with 7 pages below 0.95, where there were 41.** |
| W038 | `pdfjs__S2.pdf` | a **JPEG 2000** decoder, again | `a JPEG 2000 image was found but no decoder exists yet` six times, plus one naming `/Im7`. 13 JPX images carry the whole figure: we draw 31987 ink pixels against the oracle's 838668, and **99.8% of what we do draw is in the right place**. Its single page is the corpus's second-worst at 0.6386. |

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

Gate 3.1 asks three things of this corpus. **All three are missed. None of them is met, and none
of them is close.**

> *"At 150 DPI: per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31, and
> Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root cause and
> evidence."*

Each of the three, in the gate's own order, with the figure it is measured against:

- **per-page ≥ 0.95 — NOT MET.** **165 of the 740 pages are below it**, 22.3%, so 77.7% clear the
  floor. It is a corpus-wide average rather than a per-page promise, and this is the figure it
  produces.
- **median ≥ 0.985 — NOT MET on Tier B.** The Tier-B page median is **0.96015**, which is **0.025
  short**. The gate's median is taken across F12–F22, F31 *and* Tier B together, and this run
  measures only the last of those, so 0.96015 is not by itself the gate's number — but 740 of the
  pages in that median are Tier-B pages, and Tier B's own median is 0.960.
- **≤ 3% of pages below 0.95 — NOT MET.** **22.3% against a 3% bound**: at 740 pages the gate
  allows **22** pages below 0.95 and there are **165**, a factor of **7.5**.

**What a reader needs in order to judge how far away that is.** The third is the one that decides
the gate, and **62 of the 165 are the two JPEG 2000 files that are now formally out of scope**
([D22](#d22--jpeg-2000-is-not-decoded-and-the-decision-to-leave-it-that-way-was-measured-rather-than-assumed)).
That leaves an **addressable remainder of about 103 pages**, and those 103 are **concentrated in a
handful of files rather than spread evenly across the corpus** — four files hold 70 of them
(`fips197` 24, `TAMReview` 22, `nistir7657` 14, `freeculture` 10), and everything else is one or two
pages across a dozen files. So the honest reading of where this stands is not "a fifth of the
corpus is wrong" and not "two thirds of a bad number has been fixed"; it is **four files decide the
gate, and two of them ([D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect))
are now diagnosed and turn out not to need a codec or a colour space at all.**

**The comparison with the run before this one, stated as plainly as it can be stated.** Pages below
0.95 **167 → 165**, and against the run before that **254 → 165**, a **35% reduction** — 89 pages
came back over the line. Per-page median **0.96010 → 0.96015**. **Per-file median (worst page)
0.9698 → 0.97375**: it did move, but by an order of magnitude less than the page count did, and
the reason is the shape of the measure rather than anything about the fixes — a file is scored on
its worst page, and the gains landed in files whose worst page was already above 0.95, so pages
got better *inside* files this measure cannot see. The gain is also not spread evenly —
`gov__nist-sp800-88.pdf` alone accounts for 34 of the 89 pages that came back over the line
(41 below 0.95 before the ICCBased fix, 7 now). A reader who takes "35% fewer bad pages" as a
general improvement in fidelity across the corpus would be reading it wrong: it is one file's worth
of improvement plus a second one's, and the long tail is unmoved.

**And the median is the number that flatters.** A page rendered as bare paper scores about 0.79
against the real page, so while any cluster is missing whole pages of content the median moves far
less than the fix that restores it does. **The count of pages below 0.95 is the number the gate is
written against and the number to watch.**

### The four files that still do not open

All four are reported, none is guessed at, and each is listed here so the count of 73 opened is
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
[re-run](#the-run-in-full) has **zero pages below 0.50**. Blank pages are not gone, but the one
cause that produced them here — every mark painted in a colour this could not convert — was named
and then removed, and `gov__nist-sp800-88.pdf`, which was entirely blank for that reason, is now
at a median of 0.9648 with 7 of its 44 pages below 0.95. What is left is [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect):
no page in the corpus is blank, and every remaining difference is a page that draws its text in
someone else's glyphs.

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
of the files that open — 73 in the [current run](#the-run-in-full) and 73 in the run this table
was measured from.

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
[current run](#the-run-in-full) confirms: `pdfbox__multitiff.pdf` pages 1–3 measure 0.9975,
0.9977 and 0.9984, a file median of 0.9977 and no page of it below 0.95.** The three-page file is
new to the numbers here — the table below was measured on page 1 alone.

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

**Drawn at 0.83738 by that change, and at 0.92923 by [D10](#d10--the-type-2-function-adds-c1-where-the-specification-adds-c1--c0) since.**
A `/PatternType 2` pattern used as a fill colour — including the
colour of an image mask — evaluates the shading per pixel where the fill lands, and this
page's gradient is in the right place at the right strength: our ink over the mask's rectangle
is 1.96% and the oracle's is 11.87%, both measured the same way at 150 DPI. The difference
between those two numbers is not a missing region — it is that the oracle's colour is
**dithered**, so half its pixels are saturated ink and half are paper, where ours is a smooth
average of the same two. What was still missing from this page was the type-2 function's own
rule, which put it at **0.92923** once that one line was corrected — see
[D10](#d10--the-type-2-function-adds-c1-where-the-specification-adds-c1--c0), now fixed. It was
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
the code as it stood, and the current figure — **0.96015 over 740 pages** — is at the
[top of this file](#the-result-against-gate-31). The 422 of 542 in this entry's severity line
counts the same way: it is what the corpus looked like then, not what it looks like now.

**The 67 pages that name a font no file embeds are no longer blank either** — the standard
fourteen now draw from bundled metric-compatible faces, and that half of D7 is closed. What
replaced the blankness is subtler and is worth naming precisely, because it is now the largest
single cause of pages below 0.95 that this project has any control over: **a substituted face is
a different set of outlines from the original, and a different set of outlines at the same widths
is a page that scores 0.93 where it should score 0.99.**
[D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect)
measures that on the two files that carry it most heavily.

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
and the [current run](#the-run-in-full) puts it at **0.9292** with 411430 of 2176200 pixels above
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
policy — and which [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect)
measures precisely. **The current run puts this file's median at 0.9648 with 7 of its 44 pages
below 0.95, against a corpus-wide page median of 0.96015** — so the file is now above the corpus
median, and the pages it still loses are the ones it loses on glyph shapes.

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
`Cs8`, not by turning it into RGB. The 22 pages still below 0.95 were limited by a `TJ` array
truncation in the content lexer, which D20 named and
[D21](#d21--a-collection-holds-how-many-items-which-is-a-different-question-from-how-deep-it-nests)
fixed: the file's median is now **0.92395**.

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
may nest — and `collect` used it as a bound on how many *items* a collection may hold:

```rust
other => {
    if items.len() < MAX_COLLECTION_DEPTH {
        items.push(object_of(other));
    }
}
```

So every array of more than 32 elements was silently truncated, and a `TJ` array is the commonest
array in a text-heavy file. The first 32 items of that show are 16 strings and 16 kerns; the
remaining 68 were dropped, and the line was cut mid-word. **That was the whole of the remaining
0.8879.**

**Fixed, as [D21](#d21--a-collection-holds-how-many-items-which-is-a-different-question-from-how-deep-it-nests).**
The two bounds are separate now, and the file's median went from 0.8879 to **0.92395**. The 22
pages are still below 0.95, and what limits them now is stated in D21: the unembedded
`/TiRoARRN~1268702012`, which is D6's subject and the project's stated policy.

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
---

## D21 — a collection holds how many items, which is a different question from how deep it nests

**Severity: it was truncating every array in every text-heavy file in the corpus, silently. Fixed,
and both bounds are still there.**

### What was wrong

D20 named it and did not fix it: `MAX_COLLECTION_DEPTH` in `crates/mangle-content/src/tokens.rs`
is documented as a bound on how *deep* a bracketed collection may nest, and `collect` used it as a
bound on how many *items* one may hold. One constant, two questions, and the second question is
the one text files ask constantly. **Every array of more than 32 elements lost everything past the
thirty-second, with no note** — and a `TJ` array kerned per character is the commonest array in a
text-heavy file, so it was the commonest way to lose a page.

A second thing came with it and was worse than a silent drop: a nested bracket did not nest. The old
`collect` pushed `object_of(token)` for an `[` or `<<`, which is an **empty** array or dictionary,
and then kept reading the inner items as if they belonged to the *outer* collection. So
`<< /OCGs [/OC1 /OC2] >>` arrived as a two-entry dictionary whose `OCGs` was an array containing
nothing and whose pairings were off by two. A membership list inside a marked-content property
list is the deepest thing a real content stream contains, and it did not survive.

### What it does now

**Two constants, and neither substitutes for the other.** `MAX_COLLECTION_DEPTH` stays at 32 and
still means nesting; `MAX_COLLECTION_ITEMS` is new at **8 192** and means width. `collect` recurses,
so a nested collection is a nested collection now, and the depth bound is the thing that stops a file
that opens brackets and never closes them.

**The item bound is 8 192, and the arithmetic is in the constant's own doc comment.** In short: a
full line of 12-point text in a 600-point measure is about a hundred characters at a mean advance of
half an em, and a typesetter kerns between each pair, so it is `2 × 100 − 1 = 199` items. The
measure the format allows is 14 400 points: 4 799 items at 12-point type, 7 199 at 8-point. Both
fit. What the bound refuses is one show operation carrying a line thinner than about 7 points
stretched the full width of a page, which is not a document.

**What a hostile file pays, also in that comment:** one array at the bound is 8 192 `Object`s at 104
bytes each on a 64-bit target — 852 KB. Filling it costs at least two bytes per item in the file, so
16 KB of hostile stream buys that 852 KB, a 52× amplification **that the constant does not set**: it
is the object size over the two bytes an item needs, so a file that wants more objects opens more
arrays, and every collection on the page is bounded separately. Raising or lowering this number
moves the per-array ceiling, not that ratio.

**Nothing is truncated in silence.** `ContentStream` carries `notes()`, `collect` writes one note
per affected collection naming what it cut, where, and how far it got — *the array at byte 4 holds
more than the 8192 items allowed, so it was cut short there and the rest of it was not read* — and
`run_with_state` seeds `PageContent::notes` from it, so a renderer's report carries it. The depth
bound reports the same way. **A dictionary goes through the same arm**, because `collect` is one
function for both, which is why a 100-entry dictionary was losing half its entries to the same bug.

**Reading to the depth bound and then skipping, once.** Past 32 levels `collect` walks the rest of
the collection with `skip_collection` in a single pass rather than letting each level skip its own
remainder, which would re-walk the file once per level: a megabyte of unclosed brackets would cost
thirty-two passes over it instead of one.

### Depth protection still holds, and it was tested rather than asserted

A hundred thousand `[` and not one `]`: the parse finishes, holds **31 items** — one empty collection
at each level above the innermost — and reports **one** note naming the nesting, not thirty-two.

That test is the one that catches the wrong fix, and the wrong fix was measured rather than argued
about. Setting `MAX_COLLECTION_DEPTH = 1024` and changing nothing else makes the test fail. Setting
it to 200 000 — "just make the number big enough" — does not make the test fail slowly, it **aborts
the process**: *thread has overflowed its stack / fatal runtime error: stack overflow*, on a 2 MiB
stack as well as an 8 MiB one. A crash is not a note, which is the whole reason the depth bound is
a number and not an afterthought. Both expectations in that test are written out in the plain rather
than taken from the constant, because a test whose expectation comes from the thing it is testing
follows the constant up; the same care went into the item-bound tests, which is why they fail on the
old code instead of agreeing with it.

### What it measured

Against `mutool draw` at 150 DPI, all 23 pages of TAMReview and page 1 of sp800-88, the same harness
and the same oracle as every table above:

| file | pages | before | after |
|---|---|---|---|
| `pdfjs__TAMReview.pdf` median SSIM | 23 | 0.88794 | **0.92395** |
| pages below 0.95 | | 22 | 22 |
| worst page | | 0.82103 (p17) | **0.89465** (p17) |
| best page | | 0.98404 (p21) | **0.98568** (p21) |
| page 17 ink pixels | | 173 940 | **252 275** (oracle 245 598) |
| `gov__nist-sp800-88.pdf` page 1 SSIM | 1 | 0.98501 | **0.98501** (unchanged) |

Every one of the 23 pages moved up, page 17 most of all, and **not one page of either file reports a
bound being hit** — so nothing in the corpus needed the new 8 192, which is the answer to whether it
is set generously enough for the files that exist.

**The 22 pages are still below 0.95, and what is left is not this.** The pages' only remaining note
is the unembedded `/TiRoARRN~1268702012`, which stands in for `Times-Roman` with a metric-compatible
face: D6's subject, and the project's stated policy. Page 17 now draws *slightly more* ink than the
oracle (252 275 against 245 598) where it used to draw 71 per cent of it, and that overshoot is what
a substituted face's wider advances look like.

**The current run measures this file's median at 0.9240 with 22 pages below 0.95, a worst page of
0.8947 and no page of it below 0.89** — so nothing regressed here and nothing moved much either.
[D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect)
measures what that remaining gap is on the two files that carry it most heavily, and it is glyph
outlines rather than anything structural.

**The three files that reported `pdftotext could not read this file` still do, and that note is not
ours.** It is written by `wild_corpus.rs` when the *oracle* cannot read the original file, so no
change to this project can move it. Checked directly, and all three are poppler refusing the bytes:
`issue15590` — *No valid XRef size in trailer / Page count in top-level pages object is wrong type*;
`bug1980958` — *Couldn't find trailer dictionary*, on a 219-byte truncated file; `issue15893_reduced` —
*Incorrect password*. **No separate win here, and the reason is worth recording**: a missing feature
in our own reader shows up as a *difference* from the oracle, never as the oracle being unable to
read the file.

### What is pinned now

**Nine tests**, each confirmed to fail on the code before this change and to pass on it after — eight
in `tokens.rs` and one in `interp.rs`:

- an array of 100 elements yields **100 elements**, each the value at its own index;
- a `TJ` of **84 strings kerned per character**, written with no whitespace between the elements the
  way a typesetter writes one, yields **167 items** — asserted against the whole expected vector, so
  both the count and the bytes of every string and every kern are checked, and it says which corpus
  page the number comes from;
- a dictionary of **100 entries** yields 100 entries with their own values;
- **100 000 unclosed `[`** finish, hold 31 items, and report one note naming the nesting at 32 — the
  test that catches the wrong fix, with both expectations written out rather than derived from the
  constant, as described above;
- an array **500 over** the bound holds exactly the bound, keeps the **first** items rather than the
  last, and is reported **by name** with the count it reached;
- an array of **exactly** the bound is accepted with no note, and **one item more** is cut back to the
  bound and reported — the off-by-one on both sides at once;
- a document nested to **exactly** the depth bound and holding one item at each level parses whole,
  with every level read and the item at the bottom of them intact — depth and item count are two
  questions and the bound for the second must not touch the first;
- an `/OCMD` dictionary's membership list arrives **as an array of its two names**, rather than as its
  parts mixed into the enclosing dictionary's pairings — the defect the old non-recursive `collect`
  had, named here so it cannot come back quietly;
- a truncated array's note reaches **`PageContent::notes`** — the test that says the reporting is not
  living in the lexer where nobody reads it; and
- `MAX_COLLECTION_ITEMS > MAX_COLLECTION_DEPTH` is asserted in three of them, because the whole defect
  was two bounds being one number and no test that ever said so.
## D22 — JPEG 2000 is not decoded, and the decision to leave it that way was measured rather than assumed

**Status: refused by name, deliberately.** The renderer reports
`a JPEG 2000 image was found but no decoder exists yet` and draws nothing, which is the pre-existing
behaviour and the honest one. This entry records why the decoder that was written is not in the tree,
because "we tried it and here is what was wrong" is worth more to the next person than silence.

### What was attempted

A JPEG 2000 decoder (ISO/IEC 15444-1) of **4,320 lines** across eight modules — markers and JP2 boxes,
the MQ arithmetic coder, tier-1 code-block decoding, tier-2 packet headers and tag trees, the inverse
DWT and the component transform — was written from the specification. It had **never been compiled**:
it was not declared in `lib.rs`, and 20 compile errors were the first thing that happened to it.

The way to find out whether such a decoder is right is not a self-consistency test, because a decoder
that agrees with itself can still be wrong everywhere. It is comparison against an implementation that
is not us. `opj_decompress` (OpenJPEG 2.5.4 — the library `mutool` itself uses) is installed, and
`opj_compress` generates codestreams with individual features switched on, so "does this decode" becomes
a bisect over features rather than an opinion.

### Six defects, each confirmed against the oracle

| # | Defect | Effect |
|---|---|---|
| 1 | The 5/3 low step read its right-hand neighbour **two samples away**, and at `i == 0` both neighbours must be `Y(1)` | every line wrong from sample 2 on |
| 2 | The 9/7's four lifting steps ran in **reverse order and on the wrong subbands** | every irreversible picture wrong |
| 3 | `main_header_end` returned the first marker after SOC — always `SIZ` — never walking the main header's segments | **no codestream could get past it** |
| 4 | The `SOD` marker was parsed as packet bits: `read_sot` returns the offset *of* `SOD`, and the body began there | every packet after the first desynchronised |
| 5 | `CodeBlock::codeword` was declared, documented as "copied out of the packet bodies as the packets go past", and **never written by any code** | every code-block failed the empty guard; **every tile decoded to a uniform 128** |

Defect 5 is the one worth remembering: it produced no error, no note, and a plausible-looking page of
correct size. It is the same failure shape as everything else in this file — a gap that is invisible is
a wrong answer rather than a bug report.

### Where it stopped, on the simplest possible input

With 1–5 fixed, on a one-component image, no component transform, **zero** decomposition levels (so no
wavelet at all) and one quality layer:

* the **irreversible 9/7** produced `numbps = 0` and all-zero coefficients — the code-block's
  magnitude-bit-plane count came out zero, which is Annex B.10.7 and is also what the packet header's
  length bits hang off;
* the **reversible 5/3** produced magnitudes of **±258 with all 1024 coefficients nonzero** on an image
  the oracle decodes to values mostly between 0 and 112 — the tier-1 MQ decoder assembling magnitude
  and sign bit planes wrongly.

That is the MQ-coded tier-1 pass, the most intricate part of the standard: three context-modelled
passes, run-length mode, vertical causal context formation and sign coding.

### The decision

**Not shipped.** A JPEG 2000 decoder that decodes and emits wrong pixels is worse than one that
refuses, because wrong pixels are believed — and the corpus cannot see the difference, since it scores
a grey page against a photo and calls the page "roughly right". The five fixes were each verified
against OpenJPEG; what remained could not be certified in the time available, and a module nobody has
proved correct is a liability rather than progress. The alternative `PLAN.md` offers for this item — an
explicit scope statement — is what this entry is.

### What the next attempt should start from

`opj_decompress` and `opj_compress` are the oracle and the lever. Three traps cost real time:

* `opj_compress -n` counts **resolutions**, not decomposition levels, so `-n 1` means `levels = 0` and
  no wavelet at all — which makes it the cleanest possible isolate of tier 1;
* `-I` is irreversible 9/7 and `-J` reversible 5/3, while Annex A.6.2's `transformation` byte is
  **0 for 9/7 and 1 for 5/3** — the opposite of the flag letters, which is an easy way to test the
  wrong filter and conclude the filter is broken; and
* a JP2 `jp2c` box has its length four bytes *before* the tag and its contents eight bytes after the
  box begins, so mis-slicing it silently decodes the wrong bytes.

Start at Annex B.10.7's `numbps`, then the tier-1 passes.

## D23 — a page that lost every one of its content streams drew as paper and said nothing

**Fixed.** `Page::content_streams` resolved each entry of a `/Contents` array through two chained
`filter_map`s: an indirect reference that did not resolve was dropped, and so was an object that was
not a stream. Neither loss was reported.

The consequence was worse than the wrong text. `gov__nist-sp800-88.pdf` page 1 has an eight-entry
`/Contents` array and **not one of them resolves to a stream this project can read**, so the page
came back empty — and with no note at all. A page that silently produces nothing is the one failure
shape this project cannot see from the outside: the corpus scores a blank page against a real one and
calls it "roughly right", and no test failed. There was nothing to fail.

A page that genuinely has no content and a page whose content was all thrown away now look the same
on the page and completely different in the notes, which is the only place the difference can live.

**What changed.** `content_streams` is now a thin wrapper over a new `content_parts`, which returns
the streams *and* one note per entry it could not use. `decoded_contents_full` starts from those
notes, so the losses reach the renderer. Two faults are named separately, because they are different
faults and a reader fixing the file needs to know which one they have:

* an **indirect reference that does not resolve** — `content stream 1 of 3 is an indirect reference
  that does not resolve, so it was not drawn`;
* an **object that is not a stream** — `content stream 2 of 3 is a number rather than a stream, so
  it was not drawn`.

**Two tests, both confirmed to fail on the code before the change** and to pass on it after:

* an array of three in which all three are lost draws **zero marks and three notes**, naming each
  entry by its position and by which fault it was — the assertion is on the note text, not on the
  absence of a panic, because the old code's symptom was *silence*;
* an array of two in which the first is an unresolvable reference **still draws the second**, so
  turning on the reporting did not turn into dropping the page.

The second test is the one that keeps this honest. Reporting every loss is only an improvement if the
survivors are unaffected, and "we found out what we lost" is otherwise an easy way to lose more.

---

## D24 — the two largest addressable clusters were both unembedded fonts, which is this project's own substitution policy and not a defect

**Diagnosis only. Nothing is fixed here, and nothing should be read as fixed.** `gov__nist-fips197.pdf`
(52 pages, 25 below 0.95, worst page 0.7564, median 0.9500) and `gov__nist-nistir7657.pdf` (48 pages,
14 below 0.95, worst page 0.9311, median 0.9627) were the two largest addressable clusters in the
[current run](#the-run-in-full) and neither had ever been looked at. Between them they are 39 of
the ~105 addressable pages, and **38 of those 39 are font substitution rather than a defect**. Both
are diagnosed here, and the answer is the same for both and is not what either cluster's size
suggested.

> **Since this was written, the one page of the 39 that was a defect is fixed.** The 38 are still
> font substitution and are still policy; the 39th — `fips197` page 1, the `ICCBased` profile with
> no `/Alternate` — is [D30](#d30--an-icc-profile-names-its-own-colour-space-and-fips197-omitted-alternate-rather-than-failing-to-declare-one),
> and it is no longer below 0.95. The measurements below are kept as they were taken, because the
> point of this entry is what the file looked like before anyone looked at it; the file is now 24
> pages below 0.95 with a worst page of 0.8432.

### The short answer

**Neither file needs a codec or a colour space. Both are limited by unembedded fonts, and the
difference is that the page is laid out correctly and drawn in the wrong outlines.** On
`nistir7657`'s worst page our ink is **285 305 against the oracle's 284 710 — 0.2% apart** — with
7.9% of pixels above tolerance, which is only possible if the ink is all in the right places and
the *shapes* are wrong. On `fips197`'s worst page the ink is **5 859 against 323 535**, and that
one page is a different thing entirely: a real gap, named below. That is D6's subject — a
metric-compatible face standing in for a font the document did not embed — which this project
treats as its stated policy, so **neither cluster is a missing feature to build and neither is a
bug in what we draw**. What is new here is the measurement of how much that policy costs, which
until now was recorded as a note on the page and nowhere else.

### `gov__nist-fips197.pdf` (W074) — 52 pages, 25 below 0.95, worst 0.7564

**What the file actually uses.** `mutool show … grep -E "Filter|Subtype|ImageMask|ColorSpace|SMask|
BitsPerComponent|Width|Height"`, and `qpdf --qdf --object-streams=disable` for the counts:

| what | count |
|---|---|
| `/Subtype /Image` | **2** — a single logo, drawn once, on page 1 |
| `/BitsPerComponent` | 2, both **8** |
| `/SMask` | 2 |
| `/ImageMask` | **0** |
| `/Filter /FlateDecode` | every content stream and every font program; **no `DCTDecode`, no `JPXDecode`, no `JBIG2Decode`** |
| `/ICCBased` | **2**, and this matters — see below |

So there is no image codec in this file at all. **The only raster content is a 988×155 logo with a
soft mask, on one page.** Every other page is text, and every text font is unembedded.

**The oracle's trace of the worst page, page 1** (`mutool trace gov__nist-fips197.pdf 1`): 89
`fill_text` and 168 `fill_path` operations, of which **257 carry `colorspace="ICCBased(Gray,
Gray Gamma 2.2)"`** and 20 carry `ICCBased(RGB, sRGB IEC61966-2.1)`, plus one `clip_image_mask` and
one `fill_image` — the logo. Fonts used: `XGQZLO+Calibri`, `HQMCVU+Calibri-Bold`,
`KFYULO+TimesNewRomanPSMT`, `COXYDE+Calibri-Italic`, `RFMPFA+Cambria` — five fonts, **all embedded**,
because they are the ones the withdrawal notice is typeset in.

**Our renderer on page 1** — `marks: 397`, **our ink 5 859**, the oracle's **323 535**. Two notes,
in full (`corpus/wild/report/gov__nist-fips197.md`, page 1):

```
the colour text is painted in the `ICCBased` space `CS0`, whose profile names no `/Alternate` to read it through could not be converted, so no text on the page in that colour was drawn
a fill colour in the `ICCBased` space `CS0`, whose profile names no `/Alternate` to read it through could not be converted, so the shape was not drawn
```

**Those two notes are the answer to page 1, and they are a second defect, not the font.** The page's
withdrawal table is drawn in `CS0`, one of the file's two `[/ICCBased <profile>]` colour spaces —
`/CS0 241 0 R` over profile 4190 (`/N 1`, a gray profile whose TRC `mutool` reports as
`Gray Gamma 2.2`) and `/CS1 242 0 R` over 4192 (`/N 3`, `IEC sRGB`). **Neither profile names an
`/Alternate`**: `/Alternate` occurs **zero** times in the whole decompressed file. D19's route —
read the colour through the profile's `/Alternate` — has nothing to read here, so 387 of the page's
397 marks were refused and only the logo was drawn. `mutool` reads the profile itself and draws the
table.

**Page 1 is therefore one real gap: an `ICCBased` profile with no `/Alternate` cannot be converted,
and this project has no ICC transform.** That is a missing feature and it is a real one. It is also
exactly **one page** of this file's 25 — which is worth saying plainly, because the page is the
file's worst at 0.7564 and it is tempting to attribute all 25 pages to it.

**The other 24 pages are the font, and here is what the font costs.** Pages 41–50 all score
0.843–0.858 and all carry the same single note:

```
the font `/TT0` is not embedded, so a metric-compatible face stands in for `/CourierNew,Bold`: the glyphs are that face's, not the original's
the font `/T1_0` is not embedded, so a metric-compatible face stands in for `/Times-Roman`: the glyphs are that face's, not the original's
```

Those ten pages are a Courier hex dump: `round[12].k_sch a4970a331a78dc09c418c271e3a41d5d` and so on,
50 lines of monospace, and they are the worst cluster in the file after page 1. Measured on page 45:
**our ink 255 857 against the oracle's 225 589**, with **204 720 of 2 103 750 pixels (9.73%) above
tolerance**. Not one pixel of it is missing or displaced — the diff image
`corpus/wild/report/W074-p45-diff.png` shows both renderings' glyphs in register, each glyph's
outline drawn twice in slightly different places, which is what two different `r` shapes at the same
advance look like.

**The advance is identical and the shape is not, and here is the measurement that says so.** The
Courier advance is 600/1000 of an em at every size, so a page laid out in Courier is laid out the
same way whichever face draws it — and it is: the two renderings' glyph starts on page 45's first
line agree to within a pixel for the whole line (`151, 163, …` against `152, 163, …`). The *height*
of one glyph is where they part. On page 45 the text matrix is `trm="9.96 0 0 9.96"`, so an em is
20.75 device pixels, and the `[` glyph measures:

| | rows the `[` occupies | as a fraction of an em |
|---|---|---|
| `mutool` | 16 | 0.771 |
| ours | 21 | 1.012 |
| Liberation Mono Bold `[`, from the `glyf` table | 1909/2048 | **0.932** |
| Nimbus Mono PS Bold `[`, from its `OS/2` | — | — |

Checked on a controlled probe rather than on the corpus page, because the corpus page has nine
substituted faces on it and the probe has one: a page naming `/CourierNew,Bold` at 100 pt, rendered
by both. **`B` stands 138 device pixels above the baseline for us and 118 for `mutool`**, against an
em of 208.33 — **0.662 against 0.566**. Liberation Mono Bold's `OS/2` says `capHeight` 1349 over
2048, which is **0.659**: our figure matches Liberation to three decimal places. Nimbus Mono PS Bold's
says 565 over 1000, which is **0.565**: theirs matches Nimbus to three decimal places.

**So the cause is named exactly: `mutool` resolves `CourierNew,Bold` to Nimbus Mono PS Bold (the URW
clone, which is what a reader that ships the standard fourteen has) and this project substitutes
Liberation Mono Bold.** Both are metric-compatible with Courier and **their advances are
identical** — every glyph the two faces share has the same advance to the unit, so a line breaks in
the same place either way — which is why the layout is identical and the glyphs are not. **Widths
are not the difference; the mismatch is entirely in the outlines, and that is precisely why it is
invisible to any metric-based check.** A 0.85 SSIM on a Courier hex dump is not a defect in drawing
text; it is two correct faces being different faces.

### `gov__nist-nistir7657.pdf` (W073) — 48 pages, 14 below 0.95, worst 0.9311

**What the file actually uses.** Same two commands, and the answer rules out the codec story
immediately:

| what | count |
|---|---|
| `/Subtype /Image` | **49** |
| `/Filter /DCTDecode` (JPEG) | **9** |
| `/Filter /FlateDecode` | 256, content streams and fonts and 40 images |
| `/BitsPerComponent` | 37 at **8**, 12 at **1** |
| `/SMask` | **11** |
| `/ImageMask` | **0** |
| `/JPXDecode`, `/JBIG2Decode` | **0 and 0** |
| `/Indexed` | **2** (`[/Indexed <ICCBased> 1 …]`, both with inline palettes) |
| `/ICCBased` | 1, `/Separation` and `/DeviceN` 0 |

**No JPEG 2000 and no JBIG2 anywhere in this file**, which is the first thing to establish: the 49
images are ordinary Flate and DCT, both of which this project decodes, and the two `/Indexed` spaces
are the only colour-space construct in it.

**The oracle's trace of the worst page, page 4** (`mutool trace gov__nist-nistir7657.pdf 4`): ten
`fill_text` and three `fill_path`, **all in `DeviceGray`**, plus two `ignore_text` blocks, and no
image at all on the page. Fonts: `TimesNewRomanPSMT`, `TimesNewRomanPS-BoldMT`,
`TimesNewRomanPS-ItalicMT`. So the worst page of this file is **plain gray text**, and there is
nothing on it for a codec or a colour space to be wrong about.

**Our renderer on page 4** — `marks: 97`, **our ink 285 305 against the oracle's 284 710**, with
**165 999 of 2 103 750 pixels (7.89%) above tolerance**. Three notes, in full:

```
the font `/TT0` is not embedded, so a metric-compatible face stands in for `/TimesNewRomanPSMT`: the glyphs are that face's, not the original's
the font `/TT1` is not embedded, so a metric-compatible face stands in for `/TimesNewRomanPS-BoldMT`: the glyphs are that face's, not the original's
the font `/TT2` is not embedded, so a metric-compatible face stands in for `/TimesNewRomanPS-ItalicMT`: the glyphs are that face's, not the original's
```

**Ink within 0.2% of the oracle's and 7.9% of pixels wrong, which can only mean the ink is in the
right places and the shapes are wrong.** The diff image `corpus/wild/report/W073-p4-diff.png` shows
exactly that: every line of the preface present, in position, with every glyph drawn twice in
slightly different outlines. The page-21 diff (`W073-p21-diff.png`) is the same shape, and page 21
measures **our ink 252 910 against the oracle's 253 434** — 0.2% apart again. **Two pages, two
different amounts of text, the same 0.2%.** That is the signature of a substitution difference and of
nothing else.

### Verdict: missing feature, or bug?

**Both files, the diagnosis is the same, and it splits:**

| what | verdict | evidence |
|---|---|---|
| 38 of the 39 pages (`fips197` 24, `nistir7657` 14) | **neither — a deliberate policy, now measured** | the pages' only notes are the unembedded-font ones; ink matches the oracle within 0.2% to 13%; glyph *starts* agree to a pixel while glyph *heights* differ by exactly the ratio between Liberation's and Nimbus's `capHeight` (0.659 against 0.565, measured 0.662 against 0.566 on a one-font probe) |
| `fips197` page 1, 1 page | **a missing feature** — an `ICCBased` profile with no `/Alternate` | two notes say `whose profile names no /Alternate to read it through could not be converted`; `/Alternate` occurs **zero** times in the file; 387 of 397 marks refused; our ink 5 859 against the oracle's 323 535 |

**So: one page of the two clusters needs a feature this project does not have, and 38 pages need
nothing at all.** What the 38 pages need is a decision about which metric-compatible face to
substitute, and that decision already exists as policy — D6's subject, stated in `docs/STATUS.md`,
with the Liberation pairing measured against two independent renderers. **Nothing here argues for
changing it.** The Arial/Times New Roman/Courier New originals are proprietary and absent, so the
substitution cannot be removed; and Nimbus would be a different substitution, not the right one, with
the same trade in the other direction.

**What is worth recording is the size of the policy's cost, because it is now the largest single
contributor to pages below 0.95 that this project controls.** The 38 pages here are better than a
third of the ~105 addressable pages, and they are not addressable by anything except a font
decision. The honest summary of where the corpus stands is that its remaining shortfall is **mostly
not code**: 62 pages are JPEG 2000 and formally out of scope, about 38 are a font-substitution
policy, and the largest genuinely missing feature left is **one page of one file** — an ICC profile
with no alternate to read it through.

### What this changes about what to build next

Three things follow, and only the first is a build:

1. **An `ICCBased` profile with no `/Alternate` is a real gap**, and it is a small one to close
   correctly: a matrix/TRC profile could be evaluated for the `/N 1` and `/N 3` cases, which is what
   `fips197`'s `Gray Gamma 2.2` and `sRGB` profiles are. It is one page of the corpus today.
2. **The remaining shortfall is not a queue.** Of the ~105 addressable pages, 38 are the font policy,
   22 are `TAMReview`'s same cause, 9 are `freeculture`'s two-cluster case, and the rest is one or two
   pages per file across a dozen files. **The thing that would move the gate's third number most is
   not a codec.**
3. **`Indexed` is still a gap** and it is worth one more look before it is dismissed, because
   `nistir7657` declares it twice with inline palettes and this project does not read it. It does not
   affect that file's worst page, which is plain `DeviceGray` text, and its 49 images decode — so the
   gap is real and the cluster is not it.

## D25 — a tiling pattern was refused rather than tiled

**Fixed.** A `/PatternType 1` tiling pattern used as a fill or stroke colour was refused **by
name** — `a fill in the PatternType 1 tiling pattern `/P0` was found and not drawn` — and nothing
was drawn in its place. The refusal was the right *shape*: one cell of a tiling is a texture that
looks plausible and is wrong everywhere, so drawing one cell stretched was never an option.

### The correction: this has **no** corpus effect, and an earlier claim that it had 12 pages was wrong

**Nine corpus files *declare* tiling patterns** — 50 of them in `bug1992868`, `issue12337`,
`comments` and `highlights`, and 301 in `bug1795263`. An earlier draft of this entry concluded from
that count that the missing loop cost **12 pages below 0.95 across four files**, and named text
highlights as the reason. **Both halves of that were wrong.**

Tracing **every page** of all nine files with `mutool trace` finds **zero** `colorspace="Pattern"`
operators and **zero** `sh` operators. Not one corpus file selects a pattern at all: they declare
them in a resource dictionary and never use them. A measured four-file run confirms it from the
other end — after the loop landed, `bug1992868`, `issue12337` and `highlights` are at **0.8667** and
`comments` at **0.8645**, which is what they scored **before** it, to every digit.

**The mistake is worth recording because it is the same one this file keeps finding.** The count
came from `mutool show <file> grep PatternType`, which reads the *object structure* and reports
declarations. It cannot see inside a content stream at all — the same grep for ` re` and ` rg`
returns **zero** on a PDF full of rectangles and colour operators. And the first attempt to confirm
it by tracing matched `sh` inside `glyph`, so every glyph line looked like a hit. **A declared
resource is not a used one**, and neither an object-dump grep nor a substring match on a trace is
evidence that a page does anything.

So the tiling loop is justified as a **named M2/M3 deliverable**, on spec-completeness grounds and
because the four files that declare patterns are exactly the kind of document a reader will open.
It is **not** justified by a corpus figure, and none is claimed. What is still unexplained in those
four files is real and **undiagnosed**: `bug1992868`'s page 3 sits at **0.8667** against a file
median of 0.9633, with 103 marks and **no note of any kind** — nothing is refused, so the page is
being drawn and is coming out different.

### How it works now

The cell is rendered **once** into an image one cell wide, and the fill sampler repeats it. That
ordering is the whole of the affordability argument: a cell is a texture, and a page may paint one
across a thousand shapes, so running the cell's content stream per pixel would cost more than the
rest of the page put together.

The cell is drawn by `paint_records` — **the same marks-to-pixels loop the page itself uses**,
pulled out of `render_page` for the purpose. A mark inside a cell is therefore not a second
implementation that agrees with the page today and drifts from it tomorrow, which is the failure
mode a "just render it again here" shortcut invites.

### Three rules that fail *silently* when they are wrong

Each is stated in the code because none of them produces an error, a note, or a crash — they produce
a texture:

- **A zero `/XStep` or `/YStep` is a value, not an absence.** It means the cell is **not repeated
  in that direction** and is drawn once, anchored at the pattern-space origin. Reading it as "no
  pitch" divides by it and paints nothing at all. The two axes are independent, so
  `/XStep 0 /YStep 20` is a column of cells and not an error.
- **A negative step is legal** and is kept with its sign; the cells run the other way rather than
  being normalised away.
- **The cell is drawn with pattern space's y inverted**, because pattern space counts up and an
  image counts down, and the sampler measures from the box's *top* to match. A cell drawn one way
  and read back the other comes out mirrored — and a mirrored texture still looks like a texture.

### `/PaintType 2` is now implemented, and it needed the space a pattern replaced

It was refused by name here, on the reasoning that the cell paints "in the colour in force when the
pattern is used" and a `Pattern` colour space has already replaced that colour. **That reasoning was
half right, and the half that was wrong is the interesting part.**

`cs` replaces the colour **space** and leaves the **components** alone. So after
`1 0 0 rg /Pattern cs /P0 scn` the components are still `[1, 0, 0]` — exactly the colour the cell
should paint in. What was missing was not the colour but the **space those components belong to**,
which `cs` had discarded. `Colour` now carries it as `under`, set only when `cs` selects a `Pattern`
space, and `to_rgba` routes a pattern's components through it.

Three things that were wrong on the way, each caught by the test rather than by reading:

- **The detection looked in the wrong table.** `cs` names a resource in `/ColorSpace`, so the check
  is against the colour-space table for the name `/Pattern` — not against the *pattern* table for a
  pattern called `Pattern`, which is what the first attempt did, found nothing, and silently left
  the previous space unrecorded.
- **The recursion did not terminate usefully.** The conversion clones the colour and swaps its
  space, and `under` came along for the ride, so the new arm fired on its own output and recursed
  to the depth bound — returning `None`, which made an uncoloured pattern look exactly like a page
  that had set no colour at all. `under` is cleared on the resolved colour.
- **`scn` was clearing it.** Naming the pattern rebuilt the colour's space and wiped the record the
  `cs` had just made.

**A test premise that was simply false.** The first draft asserted that a page which sets *no*
colour leaves the cell with nothing to paint in. It does not: the graphics state begins black, so a
colour is always in force and the cell correctly paints black. The case that genuinely has nothing
to convert is a stream that names a pattern **without ever selecting the `Pattern` colour space**,
and that is what the test now covers — reported by name, since there is no space to read the
components in.

**Corpus effect: none measured, and none claimed.** No corpus file uses a tiling pattern at all (see
the correction at the top of this entry), so this cannot be justified by a figure. It is a named
part of the specification, and the pattern-fill path is now complete for both paint types.

### Tests

Five in `crates/mangle-render/tests/pages.rs`, replacing the two that pinned the old refusal. The
first two replace a refusal assertion, and the one worth reading twice is the repetition test:

- **the cell repeats** — a 20-unit cell painting only its lower-left 10, at a pitch of 20, so red
  runs alternate with paper five times across a 100-unit fill. **Counting runs is the point**: a
  renderer that stretched one cell over the shape is identical at any single pixel and wrong
  everywhere else. The cell is deliberately *smaller* than the pitch, because a cell that filled
  its own pitch could not tell the two apart at all — which is a mistake this test's first
  version made, and which it now says so about in the comment;
- **a zero step draws the cell once**, so one red run along a row rather than five, with the paper
  that follows it asserted as well;
- **`/BBox` clips** — a cell painting twice its own box leaves the gap between cells as paper,
  which is what a clipped tiling looks like and is not what an unclipped one looks like;
- **a `/PaintType 2` cell with no colour space behind it is refused by name** — see above —
  while a normal uncoloured cell paints in the colour the page set;
- **a pattern that is not a stream is refused by name**, and
- **a shading pattern still draws its ramp** to the same values as before — the test that catches a
  change to the shared plumbing the tiling path now runs through.

Two defects were found and fixed while building it, both in code this feature introduced rather
than inherited:

- the cell's content was drawn with the page's placement composed in **as well as** the cell's own
  transform, placing it twice; and
- the cell was drawn without y inverted while the sampler measured y from the bottom, so every
  cell came out vertically mirrored.

**The corpus effect is measured, and it is zero.** See the correction at the top: a targeted
four-file run put `bug1992868`, `issue12337` and `highlights` at 0.8667 and `comments` at 0.8645,
identical to their pre-feature figures, because no corpus file selects a pattern. The feature is
kept because it is a named deliverable and because it is tested, not because it moved a number.

## D26 — a `/CropBox` written to eight decimals made the buffer a pixel taller, and the tolerance meant to prevent that was a thousand times too tight

**Fixed.** `pdfjs__freeculture.pdf` pages 1 and 2 were reported as **size disagreements** and so
produced **no measurement at all** — `we rendered 1020x1531, mutool rendered 1020x1530`, and
`915x901` against `915x900`. A size disagreement is not a slightly worse picture: the harness
*refuses* the comparison rather than scoring it, so a buffer one pixel off **deletes the
measurement** instead of degrading it.

### The cause was not floating point, which is what the code was guarding against

The buffer size comes from `pixels_for`, which snaps a value within `WHOLE_PIXEL_EPSILON` of a
whole pixel to that whole pixel and then ceils. The epsilon existed for a real defect:
`792.0 * (150.0 / 72.0)` is `1650.0000000000002` in binary floating point, `ceil` makes that 1651,
and a US-Letter page came out a pixel too tall — which had cost **80 corpus pages** their
measurements. It was set to `1e-9`, which is about 4.5 million ulps and comfortably absorbs
arithmetic error.

But these two pages are not failing by an ulp. The file writes
`/CropBox [86.399997 248.40001 525.6 680.4]` — **eight significant decimals** — so the height is
`734.400024` where `734.4` was meant, and at 150 DPI that is **`1530.00005`** pixels. That is five
hundred times further from a whole pixel than one ulp, and `1e-9` cannot see it. `ceil` then
rounded **up**, and the buffer was one pixel too tall.

**The lesson is the difference between arithmetic error and representation error.** A tolerance
sized for the first is three orders of magnitude too small for the second, and it fails in the
way that looks like a rendering difference rather than a sizing one.

### The new value is measured, not chosen

`mutool` snaps a page size within about `1e-6` relative of a whole pixel and ceils beyond it.
Feeding it single-page PDFs whose height at 150 DPI lands `0.001`, `0.005` and `0.05` pixels above
a whole `1530` gives heights **`1530`, `1531`, `1531`** — so its threshold sits between the first
two, which is 1.3e-6 to 3.3e-6 relative.

That threshold is not an arbitrary tolerance on `mutool`'s part. It is what any reader has to
use: a producer writes a box as a decimal, and a decimal is not the number it was derived from.
`1e-6` relative is the measured value rounded to one significant figure.

`mutool` is **neither** pure `ceil` **nor** pure `round`, which is worth recording because both
were the obvious guesses and both are wrong:

| pixels | `ceil` | `round` | `mutool` |
|---|---|---|---|
| 1530.000000 | 1530 | 1530 | **1530** |
| 1530.000050 | 1531 | 1530 | **1530** |
| 1530.050000 | 1531 | 1530 | **1531** |
| 1650.000000 | 1651 | 1650 | **1650** |

`round` alone would give a US-Letter page 1650 correctly but would also drop the last third of a
pixel off a 100-point page, and `ceil` alone is the 80-page defect above. Snap-then-ceil is the
only one of the three that matches.

### Effect

Two corpus pages went from **no measurement** to a real one, and **none started disagreeing** —
widening the tolerance can only move a size *towards* the oracle's, since the cases it affects
are ones where `ceil` was adding a pixel. Both pages now render at exactly the oracle's size:
**1020x1530** and **915x900**.

### Tests

- **`a_page_size_a_hair_above_a_whole_pixel_is_snapped_but_a_real_fraction_is_not`** pins both
  bands, because the failure in each direction is different: too narrow and the page is a pixel
  too big and the comparison is *refused*, too wide and a genuinely-fractional page loses its last
  pixel of canvas. It asserts `734.400024 → 1530` (snapped), `734.4 → 1530` (exact),
  `734.424 → 1531` and `734.43 → 1531` (real fractions, ceiled). **Confirmed to fail on the old
  `1e-9`** with `but the buffer is 1531 rows. Its exact size is 1530.00005 pixels`, which is the
  defect stated as a number.
- A fractional-height page builder was added beside the integer one, since a producer writing a
  `/MediaBox` as a decimal does not round it and the integer helper could not express the case at
  all — which is why this defect could not have been written as a test before.

## D27 — annotations were not drawn at all, which is most of a page on a document that uses them

**Fixed.** `mangle-render` **never called `Page::annots`**. A page's annotations were parsed by
the document layer, available, and ignored by the rasteriser. On a document whose ink is
annotations, that is not a slightly worse picture: it is a blank page.

The gap is wide. **Twenty-five of the 77 corpus files carry annotations**, `pdfjs__freeculture.pdf`
alone has **168**, and `gov__irs-f1040.pdf` has appearance streams throughout. The pdfjs annotation
regression files exist to test exactly this and were scoring well below what they should.

### What is drawn, and the one thing that is not

An annotation's ink is an **appearance stream**: a form XObject whose own `/BBox` maps onto the
annotation's `/Rect`. ISO 32000-1 §12.5.5 gives the composition in a fixed order — the box onto
the rectangle, then the form's `/Matrix`, then `/Transform` — and it is applied in that order,
because every other order puts the annotation somewhere plausible and wrong. `/AP /N` is used and
`/AP /R` is not, since `/R` is what a rubber stamp shows while it is being dragged.

The appearance is drawn by `paint_records`, **the same marks-to-pixels loop the page's own content
goes through**, with the annotation's own `/Resources`. A file whose annotations name their own
fonts therefore gets those fonts, which is the rule forms already follow.

**What is not synthesised is an appearance the annotation does not carry.** A `/Highlight` with no
`/AP` is a coloured rectangle *by convention*, and inventing one would put colour on the page the
file never asked for; it is reported by name instead. A `/Link` or `/Popup` has no appearance by
design and is **silent** — a note per link on every page would be noise that teaches a reader to
skip notes. `/F` bits 2 (Hidden) and 6 (NoView) are honoured, because both mean the file says that
ink is not on the displayed page.

### Measured, before and after

| file | before | after |
|---|---|---|
| `pdfjs__annotation-link-text-popup.pdf` | — | **0.9997** |
| `pdfjs__annotation-squiggly.pdf` | — | **0.9986** |
| `pdfjs__annotation-underline.pdf` | — | **0.9985** |
| `pdfjs__annotation-strikeout.pdf` | — | **0.9985** |
| `pdfjs__annotation-highlight.pdf` | — | **0.9976** |
| `pdfjs__annotation-text-widget.pdf` | 0.9517 | **0.9840** |
| `pdfjs__annotation-square-circle.pdf` | — | **0.9760** |
| `pdfjs__bug1992868.pdf` (14 pp) | 0.8667 | 0.8667 |
| `pdfjs__issue12337.pdf` (14 pp) | 0.8667 | 0.8667 |
| `pdfjs__highlights.pdf` (14 pp) | 0.8667 | 0.8667 |
| `pdfjs__comments.pdf` (14 pp) | 0.8645 | **0.8591** |

**The four 14-page papers did not move, and `comments` went down by 0.005.** That is reported
rather than buried. Their worst pages are 0.859–0.867 and the diff images show every line of text
as a doubled grey ghost, which is the [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect)
font-substitution signature and nothing to do with annotations: those pages are text-bound. The
`comments` page-1 movement is not explained; its ink above tolerance fell from 22.8% to 7.0%, so
most of the page improved and something small got worse.

### The residual, named

**Blend modes are parsed into the graphics state and never consumed by the renderer.** `ExtGState`
records `/BM` as `blend_mode` and nothing reads it, so a highlight paints opaque. This matters
*here* specifically because **`/BM /Multiply` is the normal way a highlight is written** — it is
what makes a highlight darken the text under it rather than hide it — and the corpus appearances in
`pdfjs__highlights.pdf` use exactly that (`/R0 gs` with `/BM /Multiply` over a yellow fill). This is
the next thing to fix on this path.

One corpus appearance is **genuinely damaged**: `pdfjs__highlights.pdf` object 667 is a Flate
stream that Python's `zlib` also refuses with *incomplete or truncated stream*. We report it and
draw the part that decoded; that is the right behaviour and is not a defect here.

**One loose end, recorded rather than guessed at.** The same damaged stream also produces a
trailing `a JPEG image could not be decoded: Illegal start bytes:A5DA`, and that note is **not**
explained by the part of the stream that decodes: the recoverable 648 bytes are pure path drawing
with **no `Do` operator in them at all**, so nothing in them reaches an image. Either a second
nested form reaches one, or the note is attached to the wrong annotation. This was not chased to a
conclusion — it affects one annotation on one page of one file, and the file is damaged in a way
`mutool` also has to cope with. **It is an open question, not a finding.**

### A bug found on the way, in our own code

`render_page` **returned early when `/Contents` was empty**, before drawing annotations. A page
whose only ink is its annotations has no content stream at all — which is legal, and is how a form
made only of stamps is written — so every annotation on such a page was dropped, silently. The
early return is gone: the content is drawn if there is any, and the annotations either way.

### Tests

Five, all in `page.rs`, and **three confirmed to fail with the `paint_annotations` call removed**:

- **the appearance is drawn, and the `/BBox` maps onto the `/Rect`** — the appearance's box is the
  unit square and the rectangle is 200 units wide, so a renderer that mapped the box onto the page
  instead would put a one-pixel mark somewhere else or nowhere. The "outside it" assertion is part
  of the test rather than an afterthought;
- **Hidden and NoView draw nothing**, and draw nothing *without a note*, because that is the file
  saying what it meant;
- **an annotation with no `/AP` is reported by name and nothing is invented** for it;
- **an appearance whose filter this project does not implement is reported, not read as
  operators**; and
- **a `/Link` with no appearance is silent**, pinning the line between "missing" and "by design".

## D28 — blend modes were parsed into the graphics state and then ignored

**Implemented, and it works — but only after a second fix, which is the interesting part.**
`ExtGState` has always read `/BM` into
`GraphicsState::blend_mode`, `Record` has always carried it to the renderer, and **nothing ever
read it**. Every mark composited source-over regardless of what the page asked for. Four modes are
now implemented — `Normal`, `Multiply`, `Screen`, `Darken`, `Lighten` — with the separable blend
function and §11.3.5.2's general compositing formula.

`Multiply` is the one that mattered here, because **`/BM /Multiply` is the normal way a highlight
is written**: it is what makes a highlight darken the text under it rather than hide it, and
`pdfjs__highlights.pdf`'s appearances use exactly that (`/R0 gs` with `/BM /Multiply` over a yellow
fill).

### The first attempt reached almost nothing, and why

The first version threaded the blend mode into `fill::polygon` — the path a **pattern** or
**shading** fill takes — and left `Device::fill_polygon` alone. That is the path a **flat colour**
takes, which is nearly every mark on every page: `draw_mark` calls `device.fill_polygon` directly
for an ordinary colour and never goes near `fill::polygon` at all. So the blend was implemented,
unit-tested, and reached only the rare marks.

**Measured across the eleven annotation-bearing corpus files, that version changed nothing to four
decimal places.** Every figure was identical, and the honest conclusion at the time was "no
improvement is claimed". The test that would have caught it was written, failed, and was
`#[ignore]`d with the reason recorded — which is what left a breadcrumb worth having.

### The defect, and the fix

`Device::fill_polygon`, `Device::stroke_polygon`, `composite` and `composite_masked` all composited
with plain source-over. A blend mode therefore could not reach an ordinary fill or stroke at all.
All four now take the mark's blend mode, and the ignored test passes and is no longer ignored.

### Measured, after the fix

| file | annotations only | annotations + blend |
|---|---|---|
| `pdfjs__comments.pdf` worst | 0.8591 | **0.8672** |
| `pdfjs__comments.pdf` median | 0.9615 | **0.9633** |
| `bug1992868`, `issue12337`, `highlights` | 0.8667 | 0.8667 |
| the seven single-page annotation files | — | unchanged |

**`comments` is the only file that moved**, and it moved past where it was before annotations
existed at all (0.8645). The other three did not, and the reason is
[D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect):
their worst pages are 0.8667 because every line of text is a doubled grey ghost from substituting
Liberation for Nimbus, and no amount of correct blending changes the glyph outlines.

So: **one file, 0.008 of worst-page SSIM.** That is a small number for the work, and it is reported
as what it is rather than rounded up. The feature is kept because `/BM` is part of the
specification, because a page that asked for `Multiply` and got `Normal` was getting wrong colours,
and because the defect it exposed — an ordinary fill unable to honour a graphics-state setting — was
a real one that would have bitten every other graphics-state property routed the same way.

The remaining modes — `Difference`, `Exclusion`, `Hue`, `Saturation`, `Colour`, `Luminosity` and
the rest — are **refused by name**, with a note saying the mark was composited as `Normal`
instead. (This entry first said *twelve*. Table 136 names sixteen, five of them are read, and
sixteen minus five is **eleven**. The count was wrong and the decision was not.) That is a deliberate choice over substituting `Normal` silently: a page whose `Luminosity`
highlight came out as an opaque one looks entirely plausible and has the wrong colours on it, which
is the failure this project keeps finding. A name in a note is worth more than a plausible page.

### What is verified and what is not

Verified: `over_blend`'s arithmetic, against exact values rather than a reading of the code —
`Multiply` of 128 by 128 is `64.25` and rounds to **64**, while `Screen` of the same two is `191.75`
and rounds to **192**. (Those are not the round numbers a reader would guess, which is why they are
written down; the first draft of the test expected 191 and was wrong, not the code.) Also verified:
that every blend leaves a pixel alone where the backdrop is **transparent**, since there is nothing
behind it to blend with; that `over` and `over_blend(.., Normal)` are the same function and cannot
drift; and that `record.blend_mode` really does arrive as `"Multiply"` by the time the renderer
resolves it, which was traced directly.

**Verified end to end.** `a_multiply_blend_mode_from_an_ext_gstate_reaches_the_pixel` renders the
same page twice, with and without `/BM /Multiply`, and asserts that the multiply version is
**darker** — the two are compared against each other rather than against a remembered number, so
the test still bites if the plumbing breaks again. It was the test that found the flat-fill defect
in the first place, and it is no longer ignored.

## D29 — a `/ShadingType 7` drift shading was refused with the mesh types, though it is not a mesh

**Fixed.** Shading type 7 (`Dr`) was refused by name alongside types 1, 4, 5 and 6. That grouping
was wrong: **types 4–6 are the mesh types and type 7 is not one.** A drift shading is a type 3
radial — the same six coordinates and two circles — with one addition: a `/D` function applied to
the parameter, so the colour is `CF(D(t))` rather than `CF(t)`.

It is its own variant rather than a flag on `Radial` for a reason worth stating: the parameter is
transformed **before** the colour function sees it, while the coverage a caller derives from the
raw parameter must **not** be. A flag would invite both to move together.

**A `/D` that cannot be read is a refusal, not a plain radial.** Dropping the drift would paint a
gradient the file did not ask for, and it would look entirely plausible — the same reasoning that
keeps an `ICCBased` profile with no `/Alternate` reported rather than approximated.

### Corpus effect

**One file, one page**: `pdfjs__bug1703683_page2_reduced.pdf` is the only corpus file using an
exotic shading, and it uses type 7. That is much smaller than the specification's size suggests, and
it was worth measuring before building rather than after: types 4–6 remain refused and are named as
such, which is a different decision from refusing 7 as though it were one of them.

### Tests

- **the drift reaches the colour** — `/D` doubles the parameter, which is *not* the identity, so
  the **midpoint** is the probe: at half the radius the drifted parameter is already past 1 and the
  colour is white, where a type 7 painted without its drift would be mid-grey. A single-pixel check
  at the outer edge would pass either way, which is why the probe is where the two differ.
- **a type 7 with no `/D` is refused.**
- **types 1, 4, 5 and 6 are still refused** — adding 7 must not have widened what is accepted.


## D30 — the four papers that declare tiling patterns turned out to be font substitution, not tiling

**Diagnosis, recorded because the earlier entry called them undiagnosed and they are not.**

`pdfjs__bug1992868.pdf`, `pdfjs__issue12337.pdf`, `pdfjs__highlights.pdf` and `pdfjs__comments.pdf`
were the four corpus files carrying the tiling cluster, and [D25](#d25--a-tiling-pattern-was-refused-rather-than-tiled)
established that none of them *uses* a pattern. What was left was 12 pages below 0.95 with no
explanation, and the honest answer took a look rather than a number.

The diff images settle it. Every line of body text appears as a **doubled grey ghost** — the same
registration signature as [D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect),
where substituting Liberation for Nimbus puts the same glyphs at slightly different widths and
outlines. Nothing is missing, nothing is displaced, no image is absent, and no colour is wrong.

`comments.pdf` page 1 is the clearest case, and it is worth describing because it looks alarming
and is not. It shows large **solid bars** over the text, which read at first glance like
annotations we failed to draw. They are not: they are the *difference* between a yellow highlight
drawn at one width and the same highlight drawn at another, and drawing annotations ([D27](#d27--annotations-were-not-drawn-at-all-which-is-most-of-a-page-on-a-document-that-uses-them))
is what turned those bars from 22.8% of the page's differing pixels into 7.0%.

**So these 12 pages are the font-substitution policy, and the addressable total for it rises to
roughly 70 pages** — which makes it by some distance the largest thing left that is not a missing
codec.

**The methodological note is the point.** Every wrong conclusion in this session's cluster analysis
came from counting *something present* rather than looking at *what the page did*: `/PatternType 1`
declarations read as pattern use, `mutool show … grep` read as content-stream evidence when it
cannot see inside one, and a substring match for `sh` read as shading evidence when it matches
`glyph`. A heatmap costs nothing and settles in one look what three greps got wrong.

## D31 — a Type 3 font was silently answered with a substituted face, which is a wrong answer rather than a gap

**Fixed by refusal, not by support.** A Type 3 font's glyphs are **content streams**, not
outlines, so it has no `/FontDescriptor` and nothing to embed. That much is well known. The
defect is in what the search did with it.

The search for a font program ended at `substitute_for`, and that function asks exactly one
question: *what is the `/BaseFont` called?* A Type 3 font is free to be called anything at all —
`/BaseFont` is an advisory hint on a Type 3 font, not a claim about where its glyphs come from —
so the question got answered. Both pdfjs regression files in the corpus name theirs **`/Helvetica`**,
which has a metric-compatible face bundled for it. The substitution therefore **succeeded**.

Measured, by reverting the fix and re-running the test that now guards it:

```
a Type 3 font is refused: FontProgram { bytes: 410820, units_per_em: 2048,
                                       encoding: "StandardEncoding", substituted: true }
```

**410,820 bytes of Liberation Sans outlines**, and the document's own glyph procedures thrown
away. The page came out looking like ordinary Helvetica text — near enough that nothing on the
page would look wrong, and the substitution notice was the only trace. This is the failure mode
this project exists to prevent, and it was reached by the most defensible-looking code in the
font path: the substitution helper's own doc comment argues at length *why* substituting is
right, and for a simple font it is. The missing half of that argument is that the substitution is
keyed on a name, and **a name is only evidence when the file has no better one to give.**

### The rule this adds

**A substitution is only legitimate where the name is the whole of what the file said.** Where a
font carries something better — an embedded program, or, as here, glyphs of its own — the name is
a hint and answering from it discards the better evidence. So the refusal goes in *before* the
substitution, and names the kind of font, so that a reader is not sent looking for an embedding
that was never supposed to be there.

### What it costs the corpus: **nothing**, measured rather than asserted

**This section previously claimed the refusal costs three files their text, and that was wrong.**
The claim came from reading the code path rather than the corpus, so it was measured: both
arrangements were run over the three affected files and the reports compared.

**All 25 pages are byte-identical.** `pages.tsv` — every per-page SSIM, RMS and ink count — is the
same with the substitution in place and with the refusal in place. The only differences in either
report are the elapsed-seconds columns (`659.7 → 656.4`, `24.4 → 24.3`), which is what
established that the second run had really re-run rather than silently reusing the first.

**Why the hazard is real but cost nothing here** is worth stating precisely, because the two facts
sit oddly together:

- `gov__arxiv-1206.5537.pdf`'s Type 3 font is `/BaseFont /SDLSQD+CMMI12`, and there is no bundled
  stand-in for `CMMI12` — so it answered `None` and drew nothing even before the fix. The
  `/Times-Roman` substitution the report *does* name is a different, ordinary font on the page.
- `pdfjs__ContentStreamCycleType3insideType3.pdf`'s page has exactly one font resource,
  `/FType3A`, and its dictionary carries **no `/BaseFont` at all**. `substitute_for` asks what the
  `/BaseFont` is called and answers `None` when there is no `/BaseFont` — so nothing was ever
  substituted for the font the page actually draws with. The file's single `/BaseFont /Helvetica`
  sits in a font dictionary that is not on that page's resource path.

So the guard removes a reachable hazard without moving a single measured pixel. That is a better
outcome than the one this entry first claimed, and it is worth being explicit that **the claim was
wrong before it was right**: the reasoning was sound and the evidence was absent, which is the
failure mode this document exists to catch.

The two pdfjs files are about a **content stream that recurses into itself**, so the depth bound
that case needs is a second question from the one this entry answers.

### Test

`a_type3_font_is_refused_rather_than_substituted` — the font is named `/Helvetica` on purpose.
A name with no bundled stand-in would pass without this fix, so the substitution path is only
reached at all if the name is one that *would* substitute. Verified by removing the guard: the
test fails with the `substituted: true` value above.

### The one abbreviation that is refused on purpose, and why

`/L` used to be read as `Lighten`. **It is now refused**, because PDF 32000-1 Table 136 abbreviates
`Lighten` *and* `Luminosity` with the same letter, and Adobe resolves it to `Luminosity` — which is
one of the modes this project does not implement. So the old reading painted the wrong function on
a page that meant the other one, and the result was a plausible picture with the wrong colours on
it.

The reason it survived so long is worth recording: **the file's own comment already said `/L` names
`Luminosity`**, one screen above the line that mapped `"L"` to `Lighten`, and a test pinned the
wrong answer. Documentation and test both agreed with each other and both disagreed with the code.
Neither is evidence about what a *document* means, which is the only question here.

That is the third time this session that a claim contradicted the code near it — after the tiling
comment that said its own loop was unimplemented, and the `STATUS.md` line that said Type 3 pages
were reported rather than drawn. **A comment near code is a statement of intent, not evidence of
behaviour, and reading it as evidence is how a wrong answer survives review.**

---

## D30 — an ICC profile names its own colour space, and `fips197` omitted `/Alternate` rather than failing to declare one

**Fixed.** `gov__nist-fips197.pdf` page 1 painted its title and a shape in `[/ICCBased …]`, and both
were refused: `the colour text is painted in the ICCBased space CS0, whose profile names no
/Alternate to read it through could not be converted`. The page drew 397 marks where the oracle
draws far more, and scored **0.7564** — the worst page of the file and, at the time, the second
worst in the corpus.

The refusal was itself defensible: no profile is applied, so inventing a colour out of unconverted
components would be a wrong answer wearing a plausible hat. **What was wrong was the premise.** The
file had not declined to say what its components are. An ICC profile carries the space its
components are in at **bytes 16 to 20** of its header — `GRAY `, `RGB `, `CMYK` — and that field is
part of the format, present in every profile whatever wrote it. `CS0` in this file names a profile
whose header says `GRAY`.

`/Alternate` and that header field are the same statement made twice: the space a reader that
cannot apply the profile should interpret the components in. Where both are present there is nothing
to decide, and `/Alternate` still wins where they disagree, because it is the one the file wrote for
this purpose. A profile that says *neither* is still refused, which is the case the original refusal
was for and the test that pinned it still stands.

**The offset is 16, not 12, and that is the whole trap.** Bytes 12 to 16 are the profile's *device
class* — `mntr` for a display profile — which is four plausible letters that are not a colour space
at all. Reading the wrong field produces a string that looks like an answer. There is a test
pinning that.

### The compressed part, which is where the first attempt was wrong

The first version of this read `Stream::raw` directly, on the reasoning that a profile is never
filtered. That reasoning is wrong and the corpus says so in one line: **`fips197` stores both of its
profiles behind a `/FlateDecode`**, so the stored bytes at offset 16 are four bytes of zlib. The
result was a refusal that read exactly like a missing feature and would have been very easy to
accept as "this file is out of scope" — the same wrong conclusion reached by a different route.
The profile is now decoded before its header is read. `a_compressed_profile_is_decoded_before_its_header_is_read`
pins it, and that test fails if the decode is removed.

Two corpus measurements are the difference between the two readings, which is why the fixture is a
compressed stream and not a bare header:

| `fips197` page 1 | before | after |
|---|---|---|
| SSIM | 0.7564 | **0.9932** |
| notes on the page | 2 refusals | **none** |
| file's worst page | 0.7564 | **0.8432** |
| file's pages below 0.95 | 25 | **24** |

**The file's page count barely moved and the file is meaningfully better.** That is the shape of
this fix rather than an accident: 24 of the 25 pages below 0.95 are this project's own
font-substitution policy ([D24](#d24--the-two-largest-addressable-clusters-were-both-unembedded-fonts-which-is-this-projects-own-substitution-policy-and-not-a-defect))
and will stay there, so one real gap closing shows up as `25 → 24` in a count and as a page going
from unreadable to 0.9932 in fact. **A cluster that is mostly policy makes its own progress
invisible**, which is the strongest argument in this file for reading a named page rather than a
count.
