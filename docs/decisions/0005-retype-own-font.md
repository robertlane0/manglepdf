# ADR-0005: Retype in the run's own font, refusing everything else by name

**Status:** accepted

## Context

GOAL.md §4.4 asks for clicking a headline, retyping it, and saving a file every other
viewer opens perfectly. The font ladder it specifies has five rungs — the run's own
font, the same family in the document, a bundled or system font with an embedded
subset, the closest metric match, per-script fallback — each with a user-visible notice
from the third rung on.

Only the first rung is built. The question this records is what "in the run's own
font" exactly means, and where it stops.

## Decision

A retype (`Change::Retype`) replaces the shown strings' bytes and nothing else: one
patch per string operand over its own token span, or one patch over the whole operation
where the operation collapses. The codes come from the run's own `/Encoding` read
backwards (glyph name through the AGL, name to code through the encoding), and a
character with no code is a refusal, never a substitution.

Four rules decide the shape, and each is about keeping something the file owns:

1. **A character at its own code point is written there when that code reads back as
   the same glyph.** `space` lives at both 32 and 160 in WinAnsi, so the reverse lookup
   refuses to choose — but 32 reads back as a space, which makes it the answer rather
   than a guess.
2. **Kerns stay at their indices.** A kerned `TJ` retypes only at the same glyph count,
   split at the old resolved lengths (escapes resolved by the lexer's own reader, so an
   escaped paren never moves a boundary). Zero-kern arrays collapse to one `Tj`, which
   draws pixel-identically.
3. **Runs keep their counts.** A multi-run object flows the text across runs at the old
   boundaries, each run encoded in its own font. One run takes any length; several
   split, or the totals refuse.
4. **The operation decides.** `Tj`, `'`, and `"` replace their string; anything else —
   composite text, nested arrays, truncated collections — is a named refusal.

Checking is by the bytes through the interpreter (`verify_retype` compares raw-string
multisets, so regrouping cannot hide a change), and the corpus end-to-end asserts the
page's decoded text contains the replacement.

## Consequences

**Good.** The first rung is total within its scope: one page each of 73 corpus files
holds 151 plain-`Tj` runs, 56 degenerate-`TJ` ones, 340 kerned-`TJ` ones, and 512
multi-run objects, and all four shapes retype. The headless loop (CLI `--retype` →
incremental save → reopen → verify → `qpdf --check` → `mutool` extraction) is the
Gauntlet's S1 shape minus the subset embedding S1.2 needs.

**Bad.** Same-count and same-font are narrow: "Explore" → "Discover" (seven glyphs to
eight, needing a subset the font does not carry) refuses, as does anything in a
composite font. Both are later rungs, not defects in this one.

**Neutral.** The refusal taxonomy (`GlyphNotEncodable`, `GlyphCountMismatch`,
`CompositeText`, `NotPlainShow`, `NoEncoding`, `NoSuchFont`, `NoFontContext`) is the
surface the fallback ladder will match against: each refusal already says which rung
would answer it.

## Alternatives

**Reflowing and re-encoding in one step.** Tempting, because most real retypes change
the length, but reflow needs line metrics the object model does not yet carry, and an
edit that moved glyphs it was not asked to move would break the locality the whole
module is built on. Length changes stay inside kern-free runs until reflow exists.

**Guessing a code for a missing glyph (`.notdef`, a lookalike).** Refused everywhere
else in this project for the same reason: a plausible wrong glyph is worse than a
named refusal, and the notice the charter requires has to say *which* font stood in.
