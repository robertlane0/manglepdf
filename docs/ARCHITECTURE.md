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
| `mangle-content` | Content streams: tokens with byte spans, the interpreter, the graphics state, the page-object model. |
| `mangle-render` | Rasterization: colour, paths, images, shadings, patterns, transparency, tiles. |
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

A clip's bounds travel from the interpreter in *user* space and are transformed by the
mark's own matrix before they mean anything in pixels. That is the same discipline as the
rest of the placement, and it is worth stating because the two spaces coincide at scale one
— which is exactly the scale a hand-written fixture is most likely to use, and exactly the
scale at which a missing transform is invisible.

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
no ignored test, every fixture independent). `docs/TESTING.md` covers the rest.
