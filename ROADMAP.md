# Roadmap

DrawCraft is a pure-Rust notebook and handwriting studio. Phases keep scope honest: later phases are not pulled forward.

## Phase 0 - Foundation (done)
Cargo workspace (`edition = "2024"`, `unsafe_code = "forbid"`, panic-deny lints), layer rules, `xtask` automation (`ci`, `layers`, `panic-hunt`, `web`, `web-build`, `dev`, `branding`).

## Phase 1 - Web / WASM PWA (v1.0.0, shipped)
Installable offline PWA for iPadOS Safari, Android tablets and desktop browsers, plus a thin desktop shell and a headless CLI.
- Ink engine (five pens, eraser modes, lasso, shapes, ruler, laser), pressure / tilt, stabilization, palm rejection.
- Notebooks with templates, custom backgrounds, text, images, stickers, study tape and flashcards.
- PDF import, annotation and export; PNG / SVG / `.dcraft` export; `.dcraftpkg` backups.
- Library: folders, favorites, tags, search, grid / list, trash.
- Import of PDF, Word, notebook-app backups and `.goodnotes` files (pages only), document-scan cleanup.
- OPFS / IndexedDB persistence with durable-write acknowledgment; Light and Dark themes.

## Next
- Floating tool-options bar, lasso filters, eraser sub-modes, page-sidebar outline and bookmark tabs, scroll direction, toolbar customization.
- Real hardware verification on iPad with Apple Pencil and Magic Keyboard.

## Phase 2 - Self-hosted sync and native shells
Sync server (`apps/drawcraft-server`), account UI, native iPadOS / Android shells through `drawcraft-ffi`.

## Phase 3 - Hybrid canvas and capture
Infinite whiteboard mode, math tools, camera scanner with deskew and OCR.

## Phase 4 - Grounded AI
Local-first AI research, MCP and agent integrations with strict read / write roots.