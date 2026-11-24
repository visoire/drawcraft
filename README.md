# DrawCraft

A notebook and handwriting studio written in pure Rust. It runs in the browser as an installable offline web app (WebAssembly), tuned for iPadOS Safari with Apple Pencil and Magic Keyboard, and also on Android tablets and desktop browsers. A native desktop shell and a headless CLI share the same engine.

License: `MIT OR Apache-2.0`. You may use, copy, modify and redistribute it freely under either license.

## Features (v1.0.0)
- **Ink:** fountain pen, ball pen, brush, pencil, highlighter; pressure and tilt; stabilization presets; draw-and-hold shapes; whole-stroke and precision eraser; lasso; ruler; laser pointer.
- **Pages:** paper templates and tints, custom image or PDF backgrounds, page sizes and orientation, add / duplicate / rotate / delete / bookmark pages, page thumbnails.
- **Content:** text boxes, images, shapes, stickers, study tape (tap to reveal), flashcards with spaced repetition.
- **PDF:** import as a protected base layer, annotate, export with vector ink; PNG, SVG and `.dcraft` export.
- **Library:** nested folders, favorites (notebooks and folders), tags, search, grid and list, trash with 30-day restore, backups as `.dcraftpkg`.
- **Import:** PDF, Word (`.docx`), `.goodnotes` notebooks and backup `.zip` files (pages from embedded PDFs; handwriting strokes of the source app are not converted), pictures, document-scan cleanup.
- **Apple Pencil and keyboard:** palm rejection, finger pans while the Pencil draws, side-button pen/eraser toggle, `Cmd` shortcuts, 44 px touch targets.
- **Offline first:** OPFS storage with IndexedDB fallback, saved only after a durable write; light and dark themes; zero telemetry.
- Earlier `.scraft`, `.pcraft` and `.ncraft` files still open.

## Quick start
You need a recent stable Rust toolchain (the repo pins it in `rust-toolchain.toml`, including the `wasm32-unknown-unknown` target) and [`trunk`](https://trunkrs.dev) (`cargo install --locked trunk`).

```sh
cargo xtask web          # dev server at http://localhost:8080
cargo xtask dev          # same, served on your LAN so an iPad can open it
cargo xtask web-build    # release bundle in apps/drawcraft-web/dist
cargo xtask ci           # fmt, clippy (native + wasm32), layers, panic-hunt, tests
```
Desktop shell and headless CLI:
```sh
cargo run -p drawcraft                    # native desktop shell
cargo run -p drawcraft-cli -- commands    # list every engine command
```

## Architecture
Dependencies only flow downward, enforced by `cargo xtask layers`:

| Layer | Crates |
|---|---|
| L0 Foundation | `drawcraft-geom`, `-color`, `-raster`, `-tablet` |
| L1 Document | `drawcraft-doc` |
| L2 Operations | `drawcraft-ops`, `-paint`, `-sync-protocol` |
| L3 Format and compositor | `drawcraft-format`, `-compose` |
| L4 Runtime | `drawcraft-storage`, `-sync` |
| L5 Engine | `drawcraft-engine` (headless session and command registry; every change is a command) |
| L6 Shells | `drawcraft-ui` (egui), `apps/drawcraft-web`, `apps/drawcraft`, `apps/drawcraft-cli` |

See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`DESIGN.md`](DESIGN.md) and [`ROADMAP.md`](ROADMAP.md).

## Status
v1.0.0 is a first stable release of the single-user, offline product. Not included yet: a sync server and accounts UI, collaboration, AI features, audio recording, and decoding of other apps' handwriting strokes. Apple Pencil Pro squeeze is not exposed by Safari; the side-button path is supported. Physical iPad behavior is best-effort and was not covered by automated tests.

## Contributing and security
See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md). Third-party licenses and asset provenance: [`ATTRIBUTION.md`](ATTRIBUTION.md).