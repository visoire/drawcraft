# DrawCraft — Visual & Interaction Design System (`DESIGN.md`)

Structured visual identity and tablet-first ergonomic specification for **DrawCraft** (clean-room, original design).

---

## 1. Visual Theme & Atmosphere

- **Calm Studio Canvas:** The page or PDF document is always the quiet visual centerpiece. Chrome recedes into low-contrast neutral surfaces with 1px hairlines.
- **Dual Studio Modes (`Light` default & `Dark`):** warm daylight, or neutral anthracite with no blue cast.
  - Default on first launch respects OS/iPadOS preference (or Dark Studio chrome with Pure White or Warm Cream starter sheets, configurable independently of UI theme).
  - Optional **PDF Dark-Mode Inversion** preserves legibility without harsh pure `#000000` glare.
- **Anti-AI-Slop Mandate:** No decorative gradients, no neon glows, no glassmorphism over canvas or text, no emojis in UI chrome, no competing second accent, no bloated dropdown toolbars.

---

## 2. Color Palette & Semantic Tokens

All colors live in `drawcraft-color` (`Tokens::light()` / `Tokens::dark()`). Never hardcode hex values inside widgets.

| Role | Purpose | Light Studio | Dark Studio | Contrast Requirement |
|---|---|---|---|---|
| `canvas_bg` | Desk backdrop behind pages/PDFs | `#EBECEF` | `#0E0E10` | Clear separation from paper sheet |
| `panel_bg` | Sidebars & recessed Level-1 tab bar | `#F5F5F7` | `#141416` | Flat + inner-edge hairline |
| `toolbar_bg` | Active Level-2 tool bar & home header | `#FFFFFF` | `#1C1C1E` | Pure calm surface + bottom hairline |
| `field_bg` | Modals, popovers, raised controls | `#FFFFFF` | `#262629` | Subtle elevation shadow on floating surfaces only |
| `pill_active` | Recessed segmented-switch track, active pill | `#ECECF0` | `#2C2C30` | Borderless track framing active pill |
| `accent_soft` | Selected sidebar row / active tool tint | `#E4EEFC` | `#2C2C30` (neutral, no blue cast) | Paired with `accent` / `text` |
| `border` | 1px hairlines & dividers | `#E5E5EA` | `#2A2A2E` | Crisp, never heavy gray |
| `text` | Primary labels & headings | `#1C1C1E` | `#F2F2F5` | ≥ 12:1 on surface |
| `text_muted` | Subtitles, metadata, hints | `#66666B` | `#8E8E93` | **≥ 4.5:1** WCAG AA on every surface |
| `accent` | Active tab underline, focus ring, single `+ New` CTA | `#0068F5` (system blue, darkened from `#007AFF` so white labels keep WCAG AA) | `#4D8BFF` | White or ink label ≥ 4.5:1 |
| `danger` | Destructive actions / errors | `#B42828` | `#F07A7A` | Paired with icon/text, never color-only |

### Starter Paper Sheet Tints
- **Pure White:** `#FFFFFF` (high contrast for printing & daylight)
- **Warm Cream:** `#FAF6EE` (reduced eye strain for long study sessions)
- **Dark Slate:** `#1E222A` (dark writing surface with automatic ink-color contrast adaptation)

---

## 3. Typography & Iconography

- **Fonts (Local SIL OFL only — zero CDN requests):**
  - UI Sans: `Inter` / `Geist` (clean, legible on Retina iPad displays)
  - Monospace / Numeric: `Geist Mono` with `tabular-nums` for page counters (`12 / 148`), zoom (`125%`), stroke widths (`1.5 pt`), and study timers.
- **Hierarchy:** Sentence case throughout; concise plain-English labels; inline non-jargon subtitles on settings cards explaining *what a setting adjusts* and *how it changes writing feel*.
- **Icons:** Exclusively locally vendored **Lucide 24×24 rounded outline SVGs** (`ISC`, recorded in `ATTRIBUTION.md`), tinted dynamically from active theme tokens. Never draw ad-hoc replacement icons by hand.

---

## 4. Layout & Tablet / iPadOS Ergonomics

### 4.1 Two-level top chrome & floating canvas pills
- **Navigation bar (48 px, `chrome.rs`):** calm recessed `#F5F5F7` / `#16171B` strip with Library, Pages (`P`), Find (`Cmd+F`) on the left; elevated active document tabs + `+` in the middle; save state, Share, Flashcards, Focus mode and `⋯` overflow menu (Add page, Clear page, Page format, Dark PDF pages, Left-handed layout, Draw with finger, Theme, Profile & Sync, Settings) on the right.
- **Minimalist tool strip (`toolbar.rs`):** pure-white (`#FFFFFF` / `#1C1D22`) single-row strip with a borderless recessed tool track and cobalt-tinted active pill, three inline color dots + three stroke-size dots, and only Add Page + Zoom% fit pill on the right. A second tap on the active pen, eraser or laser opens its settings popover. The floating left color dock is hidden on touch/iPadOS screens so the canvas edge stays clear for palm rest.
- **Floating canvas pills (`canvas.rs`):** unobtrusive floating `[ Undo | Redo ]` pill in the top-left corner of the canvas (mirrored to top-right in left-handed mode) and a quiet `Page X / N` pill in the bottom-right corner (hidden in focus mode or modals), plus layered physical paper drop shadows.
- **Minimalist library (`home.rs`, `library_folders.rs`):** calm two-surface layout (`#F5F5F7` sidebar with Flashcards shortcut + pure `#FFFFFF` notebook shelf), single cobalt `+ New` focal CTA consolidating New Notebook / Import PDF / New Folder / Open `.dcraft` / Backup Packages, clean borderless pill search, and serene book-cover cards with only Star + `⋯` actions.
- **Controls are ghosts:** zero resting borders on icon buttons, soft pill on hover, sky-blue `accent_soft` active pill, 10/14 px concentric radii, ≥ 44×44 px touch targets on iPadOS.
- **Type:** Inter for text, Geist SemiBold for headings, Geist Mono for numbers (local SIL-OFL fonts, see `ATTRIBUTION.md`).

### 4.1a Legacy 3-zone description (superseded)
- **Left Zone:** Library Back button, compact Notebook/Tab switcher, Page Navigator toggle (`P`).
- **Center Zone:** Writing tools (Pen, Highlighter, Eraser, Lasso, Shapes, Text, Image, Study Tape, Ruler, Laser Pointer), 3 quick thickness dots, editable quick color swatches + compact HSV/Hex popover. **No crowded preset dropdowns in the toolbar.**
- **Right Zone:** Undo / Redo, Zoom indicator (`Fit Width` / `Fit Page`), Export / Share modal, Settings drawer toggle (`S`).
- **Left-Handed Mode:** Mirrors primary tool/page drawers cleanly so the palm never triggers accidental toolbar taps.

### 4.2 Side Panels & Unified Modals
- **Left Sidebar (`P`):** Visual book-style page thumbnails + PDF Table of Contents (collapsible chapters) + Bookmarks + quick page reorder/duplicate/delete.
- **Right Settings Drawer (`S`):** Two focused tabs:
  - **`Pen`:** 4 curated 1-click stabilization presets (`ArithmeticBuffer10`, `ModeledSpring`, `CurvedCubicSpline`, `SimpleDirect`) with a **live interactive stroke preview** (no overwhelming raw-parameter slider walls).
  - **`Page & Stylus`:** Paper template & tint, page size/orientation, Pencil-Only vs Finger mode, barrel-button / double-tap actions, left-handed layout, Light/Dark theme.
- **Unified Centered Modals:** `New Notebook` (Details & Cover → Size & Orientation → Paper & Templates), `Notebook Settings`, `Export`, and `Quick Search / Command Palette (Cmd+K)`.

### 4.3 Touch, Apple Pencil & Magic Keyboard Rules
- **Hit Targets:** Minimum **44×44 px** on every interactive button, swatch, thumbnail action, and modal close control.
- **Concentric Radii:** Outer radius minus padding = inner radius (e.g., 16px card with 6px padding → 10px inner control).
- **Motion:** Fast, purposeful easing (`150–250ms`), transform-only, strictly disabled when `prefers-reduced-motion` is enabled.
- **Keyboard & Accessibility:** Every interactive widget supports `default`, `hover` *(fine pointers only)*, `focus-visible`, `active`, `disabled`, and `error` states; zero keyboard traps; full Magic Keyboard shortcut parity.

### 2026-11-24 redesign notes (supersede older wording above where they differ)
- **Dark is graphite, never blue:** every Dark surface has saturation 0 (`#0E0E10` desk, `#141416` sidebar/tab strip, `#1C1C1E` toolbar & cards, `#262629` raised, `#2C2C30` active pill, `#2A2A2E` hairlines). `apps/drawcraft-web/index.html` (`theme-color`, loading screen) uses the same values.
- **Library:** no icon boxes behind sidebar rows; sidebar has Documents, Favorites, Marketplace only (no Shared). Hiding the sidebar leaves no replacement "Browse" button. The actionbar is `[ + New ] [ grid/sort ] | [ cloud ]` on one 28-30 px baseline. Notebook cards are borderless: cover and paper on the desk, title + page count below, then icon-only sync state, an icon-only folder marker (path as tooltip), a star and a small `v` menu. Folder pages show `Library › Parent` above the folder title.
- **Dialogs:** title and a quiet close only (no badge icon, no subtitle); secondary buttons are ghost; New Notebook is a flat 600 px dialog (cover / paper cards, title field, Size and Color rows split by a hairline, template shelf). Folder `Color` and `Icon` grids share one 9 x 37 px lattice so switching tabs never shifts anything.
- **Editor:** the active notebook tab is marked with a 2 px accent underline instead of an outlined box; selected icon buttons use a tint only (no accent outline).
- **Empty states:** original 1.9 px round-cap line illustrations for Documents, Favorites, Marketplace and Trash (`crates/ui/assets/illustrations/`), no halo circles.