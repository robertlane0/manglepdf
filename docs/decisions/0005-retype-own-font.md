# ADR-0005: Text retyping and the font resolution ladder

**Status:** accepted

## Context

GOAL.md §4.4 asks for clicking a headline, retyping it, and saving a file every other
viewer opens perfectly. The font ladder it specifies has five rungs:

1. The run's own font if its encoding and font program contain every needed glyph.
2. Same family among the document's other fonts.
3. Same family from bundled/system fonts (embed a subset).
4. Closest metric/style match.
5. Per-script fallback chain.

Whenever rungs (3)–(5) apply, a non-blocking notice names the font and reason.

## Decision

Retyping is built as three surgical `Change` variants that walk rungs (1)–(3) in order:

1. **Rung 1 — `Change::Retype` (own font).** Replaces the shown strings' bytes and nothing
   else: one patch per string operand over its own token span, or one patch over the whole
   operation where the operation collapses. Codes come from the run's `/Encoding` read
   backwards (glyph name through the AGL, name to code through the encoding), preferring
   identity code points when a glyph name appears at multiple codes (such as `space` at
   32 and 160 in WinAnsi).
2. **Rung 2 — `Change::RetypeFace` (same-family document font).** When the run's own font
   lacks a character, `Editor::apply_family` probes the page's (or form's) resource table
   for another simple font sharing the same canonical family. If one covers every character
   of the replacement, each run's operation is wrapped in `q /<Font> <size> Tf … Q` and
   encoded in that font's `/Encoding` without embedding new font objects.
3. **Rung 3 — `Change::RetypeIn` (embedded TrueType subset).** When no document font covers
   the text, `mangle-font::subset_truetype` subsets a bundled metric-compatible TrueType
   face (`assets/fonts/`), `mangle-edit::embed_truetype` builds the five-object `/Type0` +
   `/CIDFontType2` + `/FontDescriptor` + `/ToUnicode` + `/FontFile2` hierarchy, and
   `Editor::install_truetype` stages it into the page's resources for the next save while
   returning a user-facing notice.

Four rules govern all three rungs:

- **Kerns stay at their indices.** A kerned `TJ` retypes only at the same glyph count,
  split at the old resolved lengths. Zero-kern arrays collapse to one `Tj`.
- **Runs keep their counts.** A multi-run object flows the text across runs at the old
  boundaries, each run encoded in its target font and wrapped at its own size.
- **Type 3 bitmap fonts refuse direct retyping by name** (`Refusal::BitmapFont`) and offer
  explicit replacement (`Editor::replace_with_live_text`) through an embedded subset.
- **Checking is through the interpreter** (`verify_with`), comparing the decoded byte
  multiset and ensuring every untouched record on the page stays byte-identical.

## Consequences

- Rungs (1)–(3) and Type 3 replacement work end-to-end across single-run `Tj`, degenerate
  and kerned `TJ`, and multi-run objects, verified with `qpdf --check` and `mutool` text
  extraction.
- Length changes inside kerned `TJ` arrays or multi-run lines require paragraph/line reflow,
  which is tracked separately in M5.
