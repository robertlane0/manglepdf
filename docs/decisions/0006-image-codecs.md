# ADR-0006: General-purpose JPEG codecs and in-house PNG

**Status:** accepted

## Context

`GOAL.md` §2.3 permits general-purpose, non-PDF image codecs for JPEG and PNG while requiring
PDF-specific filters, predictors, `/Decode` arrays, and `/SMask` handling to be in-house.

## Decision

- Use `zune-jpeg` (pure safe Rust, no `unsafe`) for baseline and progressive `DCTDecode` JPEG
  decoding in `mangle-render`, with PDF-specific colour-space and Adobe APP14 CMYK/YCCK handling in
  our own image pipeline.
- Use `jpeg-encoder` (pure safe Rust, no `unsafe`) for page-to-JPEG export in `mangle-edit::export`.
- Implement PNG encoding and decoding in-house (`mangle-edit::export::encode_png` and image import)
  on top of `mangle-filters`'s in-house zlib/Flate and PNG row predictors (10–15).

## Consequences

- Embedded JPEG XObjects are kept byte-for-byte untouched on save and exported by copying the raw
  `DCTDecode` stream directly (`GOAL.md` §4.5).
- PNG alpha channels map directly to PDF `/SMask` soft-mask streams without intermediate conversion
  through a third-party image crate.
