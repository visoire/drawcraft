# DrawCraft — Architecture Blueprint (`ARCHITECTURE.md`)

This document defines the technical architecture, frontend/backend/engine boundaries, and iPadOS Safari/WebAssembly engineering constraints for **DrawCraft**.

---

## 1. Architectural Philosophy

DrawCraft is built as a **single portable Pure-Rust Core (`L0–L5`)** surrounded by **thin, replaceable platform shells (`L6`)**, an **optional self-hosted sync backend**, and a **pluggable local/BYO AI & MCP/A2A layer**:

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  L6: Frontend Shells & Platform Bridges (Thin — zero business logic)       │
│  ┌──────────────────────────────┐  ┌──────────────────┐  ┌──────────────┐ │
│  │ apps/drawcraft-web (PRIO 1) │  │ apps/drawcraft  │  │ drawcraft-  │ │
│  │ WASM + PWA + iPadOS Safari   │  │ Desktop (Win/Mac/│  │ ffi (Phase 2)│ │
│  │ Apple Pencil + Magic Keyb.   │  │ Linux, egui/wgpu)│  │ iPadOS/Andr. │ │
│  └──────────────┬───────────────┘  └────────┬─────────┘  └──────┬───────┘ │
└─────────────────┼───────────────────────────┼───────────────────┼─────────┘
                  └───────────────────────────┼───────────────────┘
                                              ▼ (All mutations via Commands)
┌────────────────────────────────────────────────────────────────────────────┐
│  L5: drawcraft-engine (Headless Session + Command Registry + Permissions) │
│      Also driven directly by: apps/drawcraft-cli · Sync · MCP · A2A       │
├────────────────────────────────────────────────────────────────────────────┤
│  L4: drawcraft-sync (Offline-First Client) · drawcraft-storage (Tiered)  │
├────────────────────────────────────────────────────────────────────────────┤
│  L3: drawcraft-format (.dcraft atomic codec) · drawcraft-compose (CPU/PDF)│
├────────────────────────────────────────────────────────────────────────────┤
│  L2: drawcraft-ops (Undo/Redo) · drawcraft-paint (Ink/Shapes) · protocol │
├────────────────────────────────────────────────────────────────────────────┤
│  L1: drawcraft-doc (Library, Notebooks, Pages, Strokes[Arc], Study Items) │
├────────────────────────────────────────────────────────────────────────────┤
│  L0: drawcraft-geom → drawcraft-color → drawcraft-raster | tablet       │
└────────────────────────────────────────────────────────────────────────────┘
         │                                            │
         ▼ (Phase 2 — Optional)                       ▼ (Phase 4 — Optional)
┌───────────────────────────┐            ┌──────────────────────────────────┐
│ Backend: drawcraft-server│            │ AI / Research / Agent Layer      │
│ Self-hosted Rust (axum +  │            │ Hybrid FTS+Vector Index,         │
│ redb/SQLite/Postgres),    │            │ Grounded Citations, Local/BYO    │
│ Multi-Account Delta Sync  │            │ LLMs, TTS Audio Overviews,       │
│ (Zero Cloud Lock-In)      │            │ MCP Server/Client & A2A Protocol │
└───────────────────────────┘            └──────────────────────────────────┘
```

---

## 2. Strict Crate Layering (`L0–L6`)

Enforced automatically by `cargo xtask layers`:

| Layer | Crates | Responsibilities & Hard Rules |
|---|---|---|
| **L0** | `drawcraft-geom`, `drawcraft-color`, `drawcraft-raster`, `drawcraft-tablet` | Pure math (`Point2D`, `Rect`, transforms, Catmull-Rom/Bezier, `NaN`/`Inf` guards), sRGB/OKLCH/contrast & PDF dark-mode inversion, pixel buffers, and `StylusSample` input normalization + palm-rejection state machine. `drawcraft-tablet` has **zero workspace deps** and is the only core crate permitted localized `unsafe` for OS stylus hooks. |
| **L1** | `drawcraft-doc` | Pure serializable domain model: `Library`, `Folder`, `Notebook`, `Page` (paginated sizes + infinite whiteboard flag, paper template, protected PDF background ref), `Layer`, `Stroke` (`Arc<[StrokeSample]>` for cheap clone/undo), `TextBox`, `ImageItem`, `ShapeItem`, `StudyTapeItem`, `FlashcardItem`, `AudioMarker`, and schema version (`schema_version`). No I/O, no UI. |
| **L2** | `drawcraft-ops`, `drawcraft-paint`, `drawcraft-sync-protocol` | Document-bound Undo/Redo command deltas (survives page-canvas recycling); Ink simulation (fountain, ball, brush, pencil, highlighter, stroke/partial/highlighter-only eraser), 4 stabilization presets (`ArithmeticBuffer10`, `ModeledSpring`, `CurvedCubicSpline`, `SimpleDirect` + 1€ adaptive filter), kinematic tip prediction, Draw & Hold shape snapping; wire-level sync delta types. |
| **L3** | `drawcraft-format`, `drawcraft-compose` | `.dcraft` package codec (one folder/package per notebook: `manifest.json` + per-page files + assets; atomic write changed pages first, manifest last; unreadable files quarantined as `.corrupt` rather than overwritten); `.dcraftpkg` full library backup; `.pdf`/`.png`/`.svg` export. `drawcraft-compose`: deterministic CPU reference compositor, Pure-Rust PDF rendering (`hayro`), and **visible-page ±1 virtualization** so 500+ page textbooks stay within tight tablet memory caps. |
| **L4** | `drawcraft-storage`, `drawcraft-sync` | Portable tiered storage model inspired by public web-runtime storage guidance: hot LRU mirror + async persistence queue with two-step revision-tied flush acknowledgments (`accepted` vs `persisted`) and explicit memory budget caps; offline-first delta sync client. |
| **L5** | `drawcraft-engine` | Headless `Session`, central `CommandRegistry` (typed JSON-serializable commands with validation and structured errors), query/projection APIs, and permission checks (`can()`). Every UI action, CLI script, sync replay, MCP tool, or A2A task dispatches here. |
| **L6** | `drawcraft-ui`, `drawcraft-ffi`, `apps/*` | Thin UI shells (`egui`/`wgpu` for Desktop & Web/WASM) and narrow FFI bridge (`drawcraft-ffi`, Phase 2) for native iPadOS (SwiftUI/PencilKit/Metal) & Android (Kotlin/Stylus). Zero business logic in `L6`. |

---

## 3. Priority #1 Deep Dive: Full Web Version (WASM/PWA) & iPadOS Safari Support

Phase 1 delivers a **production-grade WebAssembly PWA (`apps/drawcraft-web`)** designed first and foremost to feel native on **iPadOS Safari**, while also working seamlessly on desktop browsers (Chrome, Firefox, Edge, Safari).

### 3.1 Apple Pencil & Touch Ergonomics in iPadOS Safari
- **W3C `PointerEvent` pipeline (`apps/drawcraft-web` stylus glue):**
  - Capture `pointerType` (`"pen"` | `"touch"` | `"mouse"`), `pressure`, `tiltX`, `tiltY`, `twist`, `azimuthAngle`/`altitudeAngle` (where Safari exposes them), `buttons` (barrel button / inverted eraser), and high-frequency `getCoalescedEvents()` + `getPredictedEvents()` for low-latency 120Hz ProMotion stroke rendering.
  - Enforce CSS `touch-action: none`, `-webkit-user-select: none`, and `-webkit-touch-callout: none` on the canvas surface so Safari never hijacks ink strokes for page scroll, rubber-banding, or text selection popups.
  - **Palm rejection:** Dedicated `PencilOnly` draw mode (default when stylus is detected: `"pen"` draws, single-finger `"touch"` pans/scrolls, two-finger pinch zooms, two-finger tap undoes, three-finger tap redoes) plus toggleable `PencilAndFinger` mode.
  - **Safari-specific quirks to guard:** Avoid double-tap browser zoom, prevent pull-to-refresh on the workspace container, handle Safari viewport resize (`visualViewport`) smoothly when the on-screen keyboard or Stage Manager resizes the window.

### 3.2 Magic Keyboard & Trackpad Support on iPadOS
- Full keyboard shortcut map (`Cmd` on iPadOS/macOS, `Ctrl` on Windows/Linux):
  - `Cmd+Z` / `Cmd+Shift+Z` (Undo / Redo), `Cmd+K` (Command Palette / Quick Search), `Cmd+F` (Find in Notebook), `Cmd+0` / `Cmd+9` (Fit Width / Fit Page), `Cmd+N` (New Notebook/Quick Note), `P` (Page Navigator), `S` (Settings), `1–6` (Tool slots), `Escape` (Close modal/popover / exit selection).
- Real keyboard focus management (no keyboard traps, visible `:focus-visible` ring, ARIA live announcements).
- Trackpad two-finger scroll & pinch-to-zoom (`wheel` with `ctrlKey` for pinch zoom on Safari/macOS/iPadOS).

### 3.3 Safari Memory Caps & Offline Persistence (RFC-0023 Aligned)
- **Strict memory budgets:** iPadOS Safari terminates tabs that spike RAM. `drawcraft-compose` + `drawcraft-storage` MUST enforce explicit byte caps:
  - Only render & cache **visible pages ±1** (evict distant page bitmaps/PDF tiles via LRU).
  - Never buffer an entire multi-hundred-MB PDF or library into the WASM heap at once.
- **Two-step offline persistence:**
  - Persist notebooks and preferences locally in **OPFS (Origin Private File System)** with **IndexedDB** fallback (for older Safari versions) — never rely solely on 5 MB `localStorage` for notebook data!
  - Revision-tied flush acknowledgments: UI shows "Saved" only after OPFS/IndexedDB confirms persistence; handle `QuotaExceededError` gracefully with a clear user warning and export fallback.
- **PWA offline shell:** Service Worker caches the WASM bundle, static assets, Lucide SVGs, and SIL-OFL fonts so DrawCraft launches and works 100% offline on an iPad in airplane mode.
- **Zero credentials in browser tab:** Sync tokens/provider keys never leak into client-exposed storage or unauthenticated endpoints.

---

## 4. Clean Frontend / Backend / AI Boundaries

### Frontend vs. Backend Separation
- **Offline-first by default:** DrawCraft requires **zero backend** to run on Web, Desktop, or Tablet.
- **Optional Self-Hosted Sync Backend (Phase 2 — `apps/drawcraft-server`):**
  - Pure Rust (`axum` + `redb`/SQLite/Postgres), deployable as a single binary, system-tray app, or Docker container.
  - Design goals: single schema contract shared with clients, cursor pagination, workspace/account row isolation, optimistic zero-spinner client reads, zero-config quickstart.
  - Exposes a versioned HTTP/WebSocket sync & storage gateway (`read_range`, `stat`, delta push/pull) so large attachments stream by byte range.

### AI, Research, MCP & A2A Layer (Phase 4 — Strictly Decoupled)
- **Provider-agnostic & local-first:** Trait-based `LlmProvider`, `EmbeddingProvider`, and `TtsProvider` supporting **Ollama**, **llama.cpp**, **LM Studio**, and external OpenAI/Anthropic/Google-compatible APIs (BYO-key, per-task model routing).
- **Grounded Citation Engine:** Every source chunk (PDF page + bounding box/character span, Markdown heading, audio timestamp) carries a stable `CitationAnchor` so AI answers link directly back to the exact page and highlighted passage.
- **MCP (Model Context Protocol):**
  - **DrawCraft as MCP Server (`drawcraft-cli mcp` / streamable HTTP on `drawcraft-server`):** Exposes safe, permission-checked tools (`list_notebooks`, `search_notes`, `read_page`, `create_flashcards`, `append_summary`) routed through `drawcraft-engine`. Requires explicit `--automation-read-root` and `--automation-write-root`.
  - **DrawCraft as MCP Client:** Connects to external student/research MCP servers (e.g., Zotero, PubMed, local databases).
- **A2A (Agent-to-Agent Protocol):**
  - Clean agent card + task lifecycle contract allowing DrawCraft's planner agent to delegate specialized research, extraction, verification, or study-guide subtasks to external or local worker agents with explicit user approval and activity audit logs.
