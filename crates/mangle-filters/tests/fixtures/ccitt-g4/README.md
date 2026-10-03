# CCITT G.4 ground truth

Every `.ccf` file here is a CCITT stream **libtiff encoded** next to the raster **libtiff
decoded back from it**. `crates/mangle-filters/tests/ccitt_libtiff.rs` reads the two and asks
whether our decoder produces the second from the first, byte for byte.

Nothing in this repository wrote either half, which is the whole point. A test derived from
ITU-T T.4 could not have found the bug these files were built for: the decoder read every
changing element in two-dimensional mode as an offset from the *reference* line's `b1`, where
T.4 measures it from the *coding* line's `a0`, and it lost 272 of a page's 287 rows while every
specification-derived assertion still passed.

## Format

Little-endian throughout.

| offset | type    | meaning                                     |
|--------|---------|---------------------------------------------|
| 0      | 8 bytes | `MANGLEG4`                                  |
| 8      | u16     | format version (1)                          |
| 10     | u16     | variant: 0 = G.4, 1 = G.3 2D, 2 = G.3 1D   |
| 12     | u32     | `/Columns`                                  |
| 16     | u32     | `/Rows`                                     |
| 20     | u32     | compressed byte count                       |
| 24     | u32     | expected raster byte count                  |
| 28     | bytes   | the stream libtiff encoded                  |
| …      | bytes   | the raster libtiff decoded, 1 bit per sample, 1 = black, each row padded to a byte |

## Regenerating

```sh
scripts/make-ccitt-fixtures.py crates/mangle-filters/tests/fixtures/ccitt-g4
```

Needs `tiffcp` and `tiffinfo` (libtiff) and the wild corpus for the one case taken from a
real PDF. The script writes the raster as an uncompressed bilevel TIFF, has `tiffcp -c g4` (or
`-c g3:1d`) encode it, checks with `tiffinfo` that the compression really is the one asked
for, then has `tiffcp -c none` decode that same stream back and refuses to write a fixture
unless the samples came back identical to the raster it started from.

Two details it gets right, both of which corrupt a stream if missed:

* **one strip per image.** A strip's data is a whole number of bytes, so a file with a strip
  boundary in the middle of a page has its codes padded with fill bits there. Concatenating
  its strips splices those fill bits into the stream, and a PDF has no strips to splice. The
  encoder is therefore always asked for `-r <rows>`.
* **the same stream in and out.** The raster libtiff gives back is decoded from the identical
  bytes the test will decode, by wrapping them in a TIFF whose single strip is that stream.

## What is not here, and why

**Group 3 2D.** libtiff is an oracle for G.4 and Group 3 1D and not for Group 3 2D. Its
decoder rejects every first line built to T.4 — including an all-white one — with

```text
Fax3Decode2D: Warning, Line length mismatch at line 0 of strip 0 (got 33, expected 32).
```

and its encoder writes that first line as a vertical zero, where T.4 requires the horizontal
pair that codes the two runs. Its encoder and decoder agree with each other and with neither
T.4 nor any other implementation, so a fixture frozen from them would freeze the disagreement
rather than the format. `mangle-filters` therefore checks its Group 3 2D path against
hand-written bit strings in `src/ccitt.rs`, with every code named in the comment above it.

**`pdfjs__issue13372.pdf`.** Its CCITT stream is 82 KB and its raster 62 KB, which is more
than every other fixture here together, and it is the same 344-by-287-or-larger shape as the
corpus case that *is* here. The corpus harness measures that document; this directory is for
keeping the decoder honest without the corpus.

## The corpus case

`pdfbox-multitiff-image12.ccf` is object 12 of `corpus/wild/pdfbox__multitiff.pdf`, 223 bytes
of it, kept as a regression repro. Its expected raster came from libtiff the same way as every
other case: the PDF's own stream was wrapped in a G.4 TIFF and decoded by `tiffcp`.