# ADR-0003: In-house Flate inflate and deflate

**Status:** accepted

## Context

`GOAL.md` §2.3 requires that `FlateDecode` tolerate truncated or corrupt streams and return partial
decoded output with a diagnostic note rather than aborting. Standard crates (`flate2`,
`miniz_oxide`) treat truncation or checksum mismatches at the end of a damaged PDF stream as hard
errors unless carefully wrapped, and Flate is small enough to own completely in safe Rust.

## Decision

Implement RFC 1950 (zlib) and RFC 1951 (DEFLATE inflate and deflate) in-house in `mangle-filters`:

- `inflate` decodes uncompressed, fixed-Huffman, and dynamic-Huffman blocks with an explicit output
  cap (decompression-bomb guard), returning all bytes decoded prior to a truncation or corrupt
  symbol along with a `FilterWarning`.
- `deflate` / `zlib_compress` writes deterministic compressed streams used when re-encoding modified
  content streams, embedded font subsets, and PNG exports.

## Consequences

- Truncated or damaged `FlateDecode` streams in broken and wild-corpus PDFs yield partial content
  that the interpreter and renderer can still draw (`GOAL.md` §2.6).
- Zero external dependency for `FlateDecode` or PNG compression; verified against Python's `zlib`
  in interop tests.
