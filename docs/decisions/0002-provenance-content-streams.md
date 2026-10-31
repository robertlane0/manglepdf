# ADR-0002: Provenance-preserving content stream parsing

**Status:** accepted

## Context

`GOAL.md` §4.1 (laws 1 and 7) and §4.3 require surgical write-back: editing or deleting one page
object must patch only the byte ranges of the operators that produced that object, preserving all
other operators, whitespace, comments, and unknown operators verbatim.

## Decision

`mangle-content` parses content streams in two stages that preserve byte offsets end-to-end:

1. The tokeniser (`token::tokenize`) emits `Token`s paired with their exact `Span` (`start..end`
   byte offsets in the decoded content stream).
2. The operator parser and interpreter (`interpret`) group operand tokens with each operator,
   recording the full operation `Span` on every emitted mark (`Record`).
3. `mangle-edit::page_objects` groups marks into selectable `PageObject`s (text blocks/lines/runs,
   images, paths, shadings, forms) and retains the provenance spans of every run and operation.
4. `mangle-edit::surgery` applies non-overlapping byte-range `Patch`es to the decoded stream and
   verifies `q`/`Q`, `BT`/`ET`, and `BDC`/`EMC` balance before and after patching.

## Consequences

- In-place edits (text retyping, property steppers, recolouring, transforms, deletion, and z-order
  `Arrange`) patch only the targeted byte spans in the content stream.
- Every edit can be verified by re-running the interpreter on the patched stream and asserting that
  all untouched records remain identical.
