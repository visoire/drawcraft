# Contributing to DrawCraft

Thanks for helping. DrawCraft is a pure-Rust notebook and handwriting studio (`MIT OR Apache-2.0`).

## Ground rules
- Pure Rust, `edition = "2024"`, `#![forbid(unsafe_code)]` (only `drawcraft-tablet` / `drawcraft-ffi` may use `unsafe`, each block with a `// SAFETY:` comment).
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` or `unreachable!` in non-test code.
- Every document, library or tool change goes through a `drawcraft-engine` command.
- Never copy code from `GPL`, `LGPL` or `AGPL` projects, and never copy icons, artwork, strings or templates from proprietary products. Do not name third-party products in code or UI text.
- New assets (icons, fonts, images) must be recorded in [`ATTRIBUTION.md`](ATTRIBUTION.md) in the same change.
- UI follows [`DESIGN.md`](DESIGN.md): neutral Light and Dark themes, contrast of at least 4.5:1, touch targets of at least 44 px, no gradients, glows or emoji in the chrome.
- Do not weaken or `#[ignore]` tests to get CI green.

## Workflow
```
cargo xtask ci          # fmt --check, clippy (native + wasm32), layers, panic-hunt, all tests
cargo xtask web-build   # release WASM bundle in apps/drawcraft-web/dist
cargo xtask web         # local dev server (needs `trunk`)
```
Add a line under `[Unreleased]` in [`CHANGELOG.md`](CHANGELOG.md) for every user-visible change.