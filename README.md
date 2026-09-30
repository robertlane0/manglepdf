# ManglePDF

A from-scratch desktop PDF viewer and editor written in safe Rust (edition 2024): open very complex or damaged PDFs, edit existing text, images and shapes in place, add new content as standards-compliant annotations, and flatten annotations on demand.

> **Status: pre-alpha.** This repository is being built autonomously against a written goal and an acceptance test — see [`GOAL.md`](GOAL.md) and [`FINISH.md`](FINISH.md). Nothing here is production-ready until the Gauntlet described in `FINISH.md` passes.

## Ground rules

- Rust 2024 edition; **no `unsafe` in first-party code** (`#![forbid(unsafe_code)]` everywhere).
- **No third-party PDF libraries.** Parsing, rendering, editing, writing, encryption and signing are implemented in this repository.
- Hand-authored SVG icons; permissively licensed bundled assets only.
- Clean-room: implemented from the specifications and observed behaviour, not by porting other PDF implementations.

## Layout

Filled in by `docs/ARCHITECTURE.md` once milestone M0 lands.

## Building

See `docs/DEV.md` (created during M0).

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

