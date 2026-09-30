# ManglePDF

A from-scratch desktop PDF viewer and editor written in safe Rust (edition 2024): open very complex or damaged PDFs, edit existing text, images and shapes in place, add new content as standards-compliant annotations, and flatten annotations on demand.

> **Status: pre-alpha.** This repository is being built autonomously against a written goal and an acceptance test — see [`GOAL.md`](GOAL.md) and [`FINISH.md`](FINISH.md). Nothing here is production-ready until the Gauntlet described in `FINISH.md` passes.

## Ground rules

- Rust 2024 edition; **no `unsafe` in first-party code** (`#![forbid(unsafe_code)]` everywhere).
- **No third-party PDF libraries.** Parsing, rendering, editing, writing, encryption and signing are implemented in this repository.
- Hand-authored SVG icons; permissively licensed bundled assets only.
- Clean-room: implemented from the specifications and observed behaviour, not by porting other PDF implementations.

## Layout

One workspace; dependencies flow one way and a lower crate never names a higher one.

| Path | What lives there |
|---|---|
| `crates/mangle-filters` | Flate, LZW, RunLength, ASCII85/Hex, predictors, CCITT |
| `crates/mangle-crypto` | RC4, AES, MD5, SHA, the standard security handler |
| `crates/mangle-syntax` | Object model, lexer, parser, cross-references, recovery, writer |
| `crates/mangle-doc` | Catalogue, page tree, name trees, outlines, labels, layers |
| `crates/mangle-font` | Font programs, encodings, CMaps, metrics, subsetting |
| `crates/mangle-content` | Content streams with byte provenance, the interpreter |
| `crates/mangle-render` | The rasterizer |
| `crates/mangle-text` | Extraction, reading order, search |
| `crates/mangle-edit` | Commands, undo, write-back, annotations, forms, redaction |
| `crates/mangle-ui` | The application |
| `tools/fixturegen` | Deterministic fixtures, with no dependency on the product |
| `xtask` | `cargo xtask policy`, fixtures, icons, the acceptance run |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for how they fit together and
[`docs/STATUS.md`](docs/STATUS.md) for what is actually built.

## Building

```sh
cargo build
cargo test --workspace
cargo xtask policy
```

See [`docs/DEV.md`](docs/DEV.md).

## Documentation

| | |
|---|---|
| [`PLAN.md`](PLAN.md) | What is being built now, and in what order |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crates, losslessness, bounded work |
| [`docs/STATUS.md`](docs/STATUS.md) | Milestone state and the honest gap list |
| [`docs/DEV.md`](docs/DEV.md) | The development loop |
| [`docs/TESTING.md`](docs/TESTING.md) | Test layers, oracles, tolerances |
| [`docs/PDF-QUIRKS.md`](docs/PDF-QUIRKS.md) | Facts about the format that bite |
| [`docs/DEPENDENCIES.md`](docs/DEPENDENCIES.md) | Every dependency and why it is not in-house |
| [`docs/ICONS.md`](docs/ICONS.md) | The icon set and how it is drawn |
| [`docs/decisions/`](docs/decisions/) | Architectural decision records |
| [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md) | Bundled assets and their licences |

## Trademarks

Adobe and Acrobat are trademarks of Adobe Inc. This project is independent and is not affiliated with or endorsed by Adobe.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

