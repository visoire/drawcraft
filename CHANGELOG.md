# Changelog

All notable changes to DrawCraft. Format follows [Keep a Changelog](https://keepachangelog.com/); versions follow [SemVer](https://semver.org/).

## [Unreleased]

## [1.0.0] - 2026-11-24

First stable release. Renamed from its working title; the primary file format is now `.dcraft` / `.dcraftpkg` (`DCFT` / `DCPK`), and `.scraft`, `.pcraft` and `.ncraft` files still open.

### Added
- Pure-Rust WebAssembly PWA (iPadOS Safari, Android tablets, desktop browsers), desktop shell and headless CLI over one engine.
- Ink: fountain pen, ball pen, brush, pencil, highlighter; eraser (whole stroke, precision, highlighter-only); lasso; shapes with draw-and-hold; ruler; laser pointer; stabilization presets; palm rejection and finger panning while the Pencil draws; side-button pen/eraser toggle.
- Notebooks: paper templates and tints, custom image / PDF page backgrounds, page sizes, add-page popover (before / after / last), duplicate / rotate / delete / bookmark pages, thumbnails.
- Content: text boxes, images, stickers, study tape, flashcards with spaced repetition.
- PDF import, annotation and export; PNG, SVG and `.dcraft` export; `.dcraftpkg` library backup.
- Library: nested folders, favorites for notebooks and folders, tags, search, grid and list, trash (30-day restore), delete folder with or without its notebooks.
- Import of PDF, Word (`.docx`), `.goodnotes` notebooks and backup archives (pages from embedded PDFs), pictures, and document-scan cleanup.
- OPFS / IndexedDB persistence with durable-write acknowledgment, Light and Dark themes (neutral graphite dark), local profiles bound to the signed-in account.
- Original icons, illustrations, stickers and tool icons; all assets recorded in `ATTRIBUTION.md`.

### Not included
Sync server and accounts UI, collaboration, AI features, audio recording, and conversion of other apps' handwriting strokes.