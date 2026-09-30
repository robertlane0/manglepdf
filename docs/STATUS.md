# Status

Where the work actually is. Updated whenever a milestone moves.

## Milestones

| M | Deliverable | State |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen, window shell, icon pipeline | **in progress** — workspace, lints and the policy gate are done; fixturegen, the window shell and the icon set are not |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector | **partly done** — everything except the page tree (`mangle-doc`), the save path and the round-trip tests |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | not started |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | not started |
| **M4** | Page objects, select/move/scale/recolour, undo/redo, first save→reopen | not started |
| **M5**–**M12** | Text, annotations, flatten, forms, organize, redact, signatures, export, UI polish, gauntlet | not started |

## What exists

- **Workspace** — one crate graph, edition 2024, `resolver = "3"`, pinned toolchain,
  shared lints, `forbid(unsafe_code)` everywhere.
- **`cargo xtask policy`** — Gate 0. An AST scan (not a regex) for `unsafe`, an audit of
  the dependency graph against the banned and PDF-provenance lists, a process-spawning
  audit, doc and licence presence, an icon lint, and the fmt/clippy/test/release run.
- **`mangle-filters`** — in-house inflate and deflate, LZW, RunLength, ASCII85, ASCIIHex,
  PNG and TIFF predictors, CCITT G3-1D/2D and G4. Every decoder returns partial output
  with a note rather than failing.
- **`mangle-crypto`** — RC4, AES-CBC (128/192/256), MD5, SHA-1, SHA-2. The standard
  security handler for revisions 2, 3, 4, 5 and 6, with the R5/R6 key derivation and
  permissions. Checked against dictionaries produced by an independent implementation.
- **`mangle-syntax`** — the object model, lexer, parser, cross-reference tables and
  streams with `/Prev` and hybrid `/XRefStm` chains, object streams, recovery by
  scanning, standard-security decryption, a copy-on-write document overlay, and the
  full and incremental writers.
- **Docs** — this set, plus the decision records under `docs/decisions/`.

## What is missing, in the order it blocks

1. **`mangle-doc`** is an empty crate. The page tree with attribute inheritance, name
   trees, outlines, destinations and page labels are the next real piece of work, and
   the Inspector and the Organizer both sit on them.
2. **No save path.** `Document` has an overlay and there is a writer, but nothing
   connects them. Until it does, the first round-trip test cannot be written.
3. **No round-trip tests.** Every test is a unit test on one function. Nothing yet opens
   a file, writes it, reopens it and compares.
4. **`fixturegen` is empty**, so there is no Tier-A corpus and `fixtures/MANIFEST.toml`
   does not exist.
5. **The window shell does not exist.** `mangle-ui` is an empty crate.
6. **No icons.** `assets/icons/` does not exist.

## Known gaps in the finished layers

Recorded so they are not mistaken for done:

- The writer re-serialises every reachable object; it does **not** yet copy untouched
  objects verbatim. That is the single largest correctness gap against Charter §4.1.
- Crypt filters (`/StmF`, `/StrF`, `/CF`, `/Identity`) are parsed but not consulted when
  the decryptor is built, so a `/V 4` file using AESV2 without `/SubFilter` decrypts
  with the wrong algorithm.
- Objects inside object streams are not decrypted.
- Incremental updates cannot express a deletion, and do not carry `/ID` forward.
- JBIG2 and JPX are not implemented.
- The inflate bomb ceiling is a ratio cap only. That is provably safe for deflate (a
  match cannot exceed roughly 1032:1) but it is not a defence against a stream that is
  merely *large*.
