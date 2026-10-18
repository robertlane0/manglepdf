# Dependencies

The direct third-party dependencies, why each is here, and why it is not in-house.
`cargo xtask policy` fails on a banned crate, and on any crate in the graph that looks like PDF
provenance and has no entry in this file saying why it is not one.

The rule this document exists to enforce is in [`GOAL.md`](../GOAL.md) §2.3: parsing,
rendering, writing, editing, encrypting, signing and text extraction must be ours.
General-purpose codecs, font-file parsing, text shaping, windowing and threading may
come from outside.

## The short list of direct dependencies

| Crate | Why it is here | Why not in-house | Licence | Unsafe |
|---|---|---|---|---|
| `thiserror` | Typed errors with context: the error enums of `mangle-syntax`, `mangle-filters` and `mangle-crypto` (`mangle-doc` and `mangle-edit` write theirs by hand) | Deriving `Display`/`Error` instead of writing both | MIT OR Apache-2.0 | proc-macro only |
| `serde_json` | `xtask` reads `cargo metadata` with it | A correct JSON parser is not the product | MIT OR Apache-2.0 | internally `unsafe` |
| `log` / `env_logger` | Diagnostics | A logging facade is plumbing, not product | MIT / Apache-2.0 | minimal |
| `crossbeam-channel` | Sending tiles and results from render workers to the UI | The UI thread must never block on rendering | MIT OR Apache-2.0 | internally `unsafe` |
| `unicode-normalization` | NFKC for R6 password preparation (SASLprep) | Unicode normalization tables are large generated data | (MIT OR Apache-2.0) AND Unicode-3.0 | no |
| `ttf-parser` | Reading `glyf`/`CFF ` tables, metrics and layout from TrueType and OpenType files | Shaping is not PDF-shaped; the *PDF* part (encodings, CMaps, subsetting, `/W`, `ToUnicode`) is ours | MIT OR Apache-2.0 | no `unsafe` |
| `zune-jpeg` | Baseline and progressive JPEG **decode** for `DCTDecode` images | Permitted codec (ADR-0006). Writing JPEG twice is not worth it | MIT OR Apache-2.0 | no |
| `jpeg-encoder` | JPEG **encode** for image export | Same reasoning as above | MIT | no |
| `sha2` | SHA-256/384/512 | The RustCrypto implementation is audited; ours is used for MD5, SHA-1 and RC4 only because the PDF algorithms need them together | MIT OR Apache-2.0 | internally `unsafe` |
| `egui` / `eframe` | The GUI: windowing, input, layout, accessibility via AccessKit | ADR-0004. A toolkit that paints its own widgets gives us the mockup's look without fighting native styling | MIT OR Apache-2.0 | internally `unsafe` |
| `egui_extras` | Loaders for the image formats the UI displays outside the page canvas | Our page images never go through it | MIT OR Apache-2.0 | internally `unsafe` |
| `rfd` | The native file dialog (FINISH S0.1 requires the real dialog) | A native dialog is a platform service | MIT | internally `unsafe` |
| `arboard` | Clipboard, for copy and paste across applications | Same reason as `rfd` | MIT OR Apache-2.0 | internally `unsafe` |
| `dirs` | Per-user configuration and font directories | Platform paths | MIT OR Apache-2.0 | no |
| `tempfile` | Scratch directories in the tests of `mangle-font` and `xtask` | Correct temp-file semantics are fiddly and platform-specific | MIT OR Apache-2.0 | internally `unsafe` |
| `syn` / `proc-macro2` | `xtask` only: the AST scan that proves there is no `unsafe` | A regex cannot tell a keyword in a comment from a keyword in a program | MIT OR Apache-2.0 | `proc-macro2` only |

**Declared by `mangle-ui` and referenced by nothing:** `egui_extras`, `arboard`, `crossbeam-channel` and
`env_logger`. Their rows above say what they were added for, not what they do today. `image`, `tiff` and
`fax` reach the lockfile by two routes: `egui_extras` (`all_loaders` → `image` → `tiff` → `fax`) and
`arboard`, whose `image-data` feature `egui-winit` (eframe's windowing layer) enables. So `fax` needs
the provenance note below whichever of the two is removed.

`cargo xtask deps` prints the current table straight from `cargo metadata`.

## Deliberate non-dependencies

These are the ones it would be tempting to reach for, and what we do instead.

| Temptation | Instead |
|---|---|
| `lopdf`, `pdf`, `pdf-writer`, `printpdf`, `krilla` | `mangle-syntax`, `mangle-doc` and `mangle-edit` |
| `flate2` / `miniz_oxide` | `mangle-filters`. They abort on a corrupt stream; a damaged file must yield the part that decoded |
| `pdf-extract` | `mangle-text` |
| `mupdf`, `pdfium`, Ghostscript, poppler bindings | `mangle-render` |
| `pikepdf` / `qpdf` as a library | `mangle-crypto` and `mangle-syntax`. They are still used as **test oracles** in dev scripts, never linked and never called from product code |
| `image` for page images | `mangle-render` decodes PDF image data itself, because `/Decode`, `/SMask`, stencil masks and colour keys are PDF concepts |

## Provenance notes

`cargo xtask policy` scans the whole graph for a crate whose name, description,
keywords or repository indicates PDF provenance. One transitive crate trips that scan
and is recorded here:

| Crate | Why it is not a PDF dependency |
|---|---|
| `fax` | A Group 3/4 fax image codec, pulled in transitively by `tiff` → `image` → the GUI stack. It decodes the same bit patterns PDF's `CCITTFaxDecode` uses, but it knows nothing about PDF: no objects, no dictionaries, no files. The *PDF* half of that format — `/K`, `/BlackIs1`, `/EncodedByteAlign`, `/EndOfBlock`, `/DamagedRowsBeforeError` — is implemented in `mangle-filters` and tested against the T.4/T.6 code tables. |

## Adding a dependency

1. Say in this file what it is for and why writing it yourself was worse.
2. Run `cargo xtask policy`. It will tell you if the crate is banned, or looks like PDF
   provenance without an entry here.
3. Prefer a replacement under 300 lines. `GOAL.md` §2.2 says so explicitly.
