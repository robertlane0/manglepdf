# corpus/ — test PDFs (git-ignored)

Everything in this directory **except this file and any `SOURCES.md`** is git-ignored. Real-world PDFs are often copyrighted and large, and must never be committed.

| Directory | Contents | Rules |
|---|---|---|
| `wild/` | Tier B: ≥ 40 diverse real-world PDFs (LaTeX papers, InDesign/Illustrator output, Office exports, scans, government forms, maps/CAD, slide decks, damaged files) | Every source set gets a `SOURCES.md` (origin, licence, date retrieved). Use only files whose licence allows local testing. Fetch them with `cargo xtask corpus fetch`; the manifest pins every hash. |
| `local/` | Anything the human drops in for ad-hoc testing | Never referenced by automated gates. |
| `heldout/` | Tier C: ≥ 3 PDFs the Builder has never seen (FINISH.md, Gate 9) | **Keep this directory empty until final acceptance.** The Builder must never open, list, or read these files. The human (or Verifier) adds them at the end — ideally they never exist on the Builder's machine before then. |

Suggested public sources (verify each licence before use): the PDF.js test corpus, the qpdf and poppler test files, the veraPDF corpus, the Isartor suite, the Ghent output suite, GovDocs1.

`wild/` is already populated from the first two of those: 77 files pinned in
`wild/MANIFEST.toml`, with `wild/SOURCES.md` recording the licence of each and — just as
importantly — which of the suggestions above were **not** used and why.

