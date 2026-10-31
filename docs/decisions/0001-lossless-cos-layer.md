# ADR-0001: Lossless COS layer and copy-on-write overlay

**Status:** accepted

## Context

`GOAL.md` §4.1 (law 1, Locality) and `FINISH.md` S1.17 require that editing an object leaves every
untouched object and operator byte-for-byte identical, and that an incremental save appends a new
revision while leaving the original file bytes as an exact prefix.

## Decision

`mangle-syntax` represents a PDF document as:

1. The original file bytes, kept intact.
2. A cross-reference map (`XRefTable`) built from classic tables, cross-reference streams
   (`/XRefStm`, `/Prev`), or the fallback recovery scanner.
3. Lazy object resolution by offset or object-stream index, preserving raw stream bytes, original
   `/Filter` chains, and dictionary key order (`IndexMap`-style ordered entries in `PdfDict`).
4. An object-level copy-on-write overlay (`BTreeMap<ObjId, Object>`) holding only modified or newly
   allocated objects (`ObjId` allocated monotonically above `/Size`).

Incremental saves (`write_incremental`) serialize only the overlay and a new cross-reference
section chained via `/Prev`. Full saves (`write_full`) walk reachable objects from `/Root`, `/Info`
and `/Encrypt`, emitting untouched objects verbatim where possible.

## Consequences

- Untouched streams (such as JPEG/JPX/CCITT image XObjects and unedited content streams) are never
  re-encoded on save.
- `Arc`-shared document snapshots (`mangle-edit::History`) are cheap because cloning a `Document`
  clones the overlay map and shares the underlying file bytes.
- Structural repair on open sets a `repaired` flag on the document so the caller can surface a
  banner and force a full rewrite on save (`GOAL.md` §5.4).
