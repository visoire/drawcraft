# CLAUDE.md

Entry file for AI coding agents working in this repository. Read [`AGENTS.md`](AGENTS.md) first; it is authoritative.

- Layout and layer rules: [`ARCHITECTURE.md`](ARCHITECTURE.md), enforced by `cargo xtask layers`.
- UI rules and tokens: [`DESIGN.md`](DESIGN.md).
- Scope and phases: [`ROADMAP.md`](ROADMAP.md).
- Run `cargo xtask ci` after a change and report only checks that actually ran.
- Read only the files you need (use line ranges on large files); never scan `target/`.