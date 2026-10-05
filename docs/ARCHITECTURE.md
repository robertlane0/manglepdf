# Architecture

One sentence: **the file is the document, and everything we do is an edit to it.**

## The spine

```
open → parse (lossless) → render → select → edit → save → reopen elsewhere
```

Everything else hangs off that spine. The spine is built first and deeply, because
where the architecture is decided is where it is expensive to change.

## Crates

Dependencies flow one way only. A lower crate never names a higher one.

| Crate | Responsibility |
|---|---|
| `mangle-filters` | Stream filters: Flate, LZW, RunLength, ASCII85/Hex, predictors, CCITT. No PDF knowledge — no dictionaries, no objects. |
| `mangle-crypto` | RC4, AES, MD5, SHA-1/2, the standard security handler, permission bits. |
| `mangle-syntax` | The object model, lexer, parser, cross-references, object streams, recovery, and the writer. Lossless by construction. |
| `mangle-doc` | Catalog, page tree with inheritance, name trees, outlines, destinations, page labels, optional-content groups, attachments, XMP. |
| `mangle-font` | Font programs, encodings, CMaps, glyph names, metrics, subsetting, substitution. |
| `mangle-content` | Content streams: tokens with byte spans, the interpreter, the graphics state, the page-object model, and **PDF functions** — a function dictionary is an ordinary document object, and both a gradient and a separation's tint transform are one. |
| `mangle-render` | Rasterization: colour, paths, images, shadings, patterns, transparency, tiles. Re-exports `mangle_content::function` as `shading::` because a gradient's function is how a gradient is defined. |
| `mangle-text` | Text extraction, reading order, search. |
| `mangle-edit` | Commands, undo, surgical write-back, annotations, forms, redaction, flatten, page operations, optimize. |
| `mangle-ui` | The application. |
| `mangle-cli` | The headless surface the tests and the acceptance harness drive. |
| `mangle-testkit` | Shared fixtures, comparisons and tolerances for the layers above. |
| `tools/fixturegen` | Deterministic fixture generation. **Depends on no `mangle-*` crate on purpose.** |
| `xtask` | Gate checks, fixtures, icons, the gauntlet. |

## The page placement

One function, `Placement::fit`, turns a page's box, a canvas size, a scale and a
`/Rotate` into the matrix every mark is drawn through, in that order: rotate in page
space, normalise the corners, then fit and flip. It is one function because the failure
modes are invisible rather than loud — a page drawn upside down is a page drawn
plausible.

## The gradient and the image

Two things on a page are sampled backwards rather than forwards, and it is the same
technique both times: each pixel is mapped through the *inverse* of the page's
transformation into the space the thing is defined in, and its value is read from there.

For an image that means asking the unit square which sample of the picture a pixel wants.
For a shading it means asking the gradient's two endpoints for the parameter at that point.
In both cases the forward alternative — walking the thing's own space and filling as it goes
— works only while the thing is axis-aligned, and a page with a diagonal banner or a
photograph set at an angle is ordinary rather than exotic. Paying one matrix multiply per
pixel buys having no special case at all.

The same discipline explains the clip, and its history. Clip bounds are recorded by the
interpreter with the CTM already applied, so the renderer applies only the page placement.
This is worth writing down because the two spaces coincide under an identity CTM, which is
every fixture in the repository, so a missing or duplicated transform is invisible exactly
where a test is most likely to be written; a page under a scaled CTM is now a regression
test of its own.

## The text model

Text is the one place where two different scales are legitimately in play at once, and
almost every bug in a text renderer comes from applying one of them twice or not at all.

The text matrix is measured in ems and carries no font size. That sounds like a detail and
is not: it is what lets a glyph be half a unit wide and still come out ten points wide on
the page, because the size arrives afterwards, when the glyph is drawn. Character spacing
and word spacing are the exception the specification makes — it states both in *unscaled*
text-space units — and they are therefore added as they stand, while the glyph's own width
carries the size. Treating them the same as the width, or the same as each other, is the bug.

The model here is pinned against `mutool` over several hundred cases rather than against a
reading of the specification, and that is deliberate. The first attempt at getting this right
was derived from the specification by someone confident, and it matched twenty-nine cases
out of four hundred. Where an independent implementation can be asked, it should be asked.

## Losslessness

The organising principle of the syntax layer is that an object is kept exactly as it
was found: raw stream bytes, the original `/Filter` chain, dictionary key order, name
spelling. Resolution is lazy and pull-based from the file bytes through the
cross-reference. An edit replaces one object in a copy-on-write overlay; the bytes of
everything else are never rewritten.

Three rules make that work in practice:

1. **Never re-serialise what you did not change.** A full save copies untouched objects
   verbatim; only rewritten objects are emitted from the model.
2. **Object numbers are stable.** New objects are allocated after `/Size`. Only an
   explicit optimize may renumber.
3. **Repairs are visible.** When the cross-reference is rebuilt the caller is told, and
   saving forces a full rewrite rather than pretending the file was fine.

## Bounded work

Every recursion, loop and allocation driven by file data has a limit: nesting depth,
element counts, decompression ratio and absolute size, reference resolution depth, page
tree depth, and a read budget. Cycles are detected rather than followed. A library
never panics on untrusted input; it returns a typed error, or a partial result with a
note saying what was lost.

## Testing

`cargo xtask policy` enforces the mechanical rules (no `unsafe`, no banned dependency,
no failed test, every ignored test named in the report, every fixture independent).
`docs/TESTING.md` covers the rest.
