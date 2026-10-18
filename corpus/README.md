# corpus/ — test PDFs

This repository tracks no PDF under `corpus/`. Real-world PDFs are often copyrighted and large, so everything here is git-ignored **except this file, any `SOURCES.md`, `MANIFEST.toml` and `.gitignore`, and `wild/`**, which is a git submodule: its own private repository, with every file pinned by SHA-256 in `wild/MANIFEST.toml`.

| Directory | Contents | Rules |
|---|---|---|
| `wild/` | Tier B (a git submodule): ≥ 40 diverse real-world PDFs (LaTeX papers, InDesign/Illustrator output, Office exports, scans, government forms, maps/CAD, slide decks, damaged files) | Every source set gets a `SOURCES.md` (origin, licence, date retrieved). Use only files whose licence allows local testing. Fetch them with `cargo xtask corpus fetch`; the manifest pins every hash. |
| `local/` | Anything the human drops in for ad-hoc testing | Never referenced by automated gates. |
| `heldout/` | Tier C: ≥ 3 PDFs the Builder has never seen (FINISH.md, Gate 9) | **Keep this directory empty until final acceptance.** The Builder must never open, list, or read these files. The human (or Verifier) adds them at the end — ideally they never exist on the Builder's machine before then. |

Suggested public sources (verify each licence before use): the PDF.js test corpus, the qpdf and poppler test files, the veraPDF corpus, the Isartor suite, the Ghent output suite, GovDocs1.

`wild/` is already populated: 77 files pinned in `wild/MANIFEST.toml` (60 from the PDF.js test
resources, 7 from PDFBox's, 8 US-government documents and 2 arXiv papers), with `wild/SOURCES.md`
recording the licence of each and — just as importantly — why most of the suggestions above were
**not** used: the qpdf and poppler test files are GPL, and veraPDF, Isartor, the Ghent suite and
GovDocs1 are not licensed for this use.

