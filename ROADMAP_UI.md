# UI Quality Roadmap

Scope: visual/UX polish only — no functional changes. This is separate from
`ROADMAP.md` (feature/correctness gaps). Grounded in the current
implementation (`src/ui/theme.rs`, `src/ui/widgets.rs`, `src/ui/panels/*`),
not a rewrite.

## Current state (as implemented)

- One hardcoded dark palette (`theme.rs`), no light mode, no user theming.
- Every frame is flat: `flat()` in `widgets.rs` is just `Frame::new().fill(bg)`
  — no rounding anywhere, 1px solid `SEP_COLOR` (`#282828`) strokes for every
  border and separator.
- All text — buttons, headers, code, data — uses `FontId::monospace` via `m()`.
  No proportional font for UI chrome; no type scale (font sizes are
  ad-hoc `f32` literals passed at each call site, mostly `12.0`).
- Hover/press feedback is an instant flat color swap (`BG_HOVER`), no
  transition.
- No icons. Collapse state uses unicode `▾`/`▸` (`sec_hdr()`); every panel
  header is plain text.
- `ACCENT` green (`#00cc44`) is reused for both decorative accent and
  semantic "good/active" meaning (register highlighting, buttons, line
  highlight uses a separate `BG_LINE_HL`) — no single source of truth for
  status color semantics.
- No spacing scale — `ui.add_space(...)` and `min_size(Vec2::new(...))` calls
  are hardcoded per widget.

None of this is broken; it reads as "functional terminal tool," which is a
legitimate aesthetic for a debugger. The roadmap below tightens it rather
than restyling it into something else.

## Phase 1 — Design tokens (foundation, do first)

Everything after this phase depends on it; skipping it means every later
change is another round of magic numbers.

- Add a spacing scale (e.g. `SPACE_XS/S/M/L = 4/8/12/16`) to `theme.rs` and
  replace raw `ui.add_space(N)` / `Vec2::new(0.0, 22.0)` literals across
  `src/ui/panels/*`.
- Add a type scale (`FONT_SM = 11.0`, `FONT_MD = 12.0`, `FONT_LG = 14.0`)
  instead of inlining `12.0` at each `m(...)` call site.
- Split color roles from color values: keep `ACCENT`/`RED`/`BLUE` as raw
  palette, add semantic aliases (`STATUS_ACTIVE`, `STATUS_ERROR`,
  `STATUS_INFO`) that point at them. Panels reference the semantic name, not
  the raw color — lets you audit "what does green mean here" in one place.
- Add a corner-radius constant (`egui::CornerRadius`) and decide: sharp
  corners everywhere (current, consistent with the terminal look) or a small
  uniform radius (e.g. 3px) on buttons/panels. Pick one, apply everywhere —
  the current inconsistency is that it's *implicitly* sharp by omission, not
  a deliberate choice.

## Phase 2 — Component consistency

- `tbtn()` and `sec_hdr()` (`widgets.rs`) are the only shared components;
  everything else (breakpoint rows, watch rows, register rows) builds its own
  layout inline per panel. Extract a small shared row/badge component set
  (e.g. `status_dot(color)`, `kv_row(label, value)`) so breakpoints.rs,
  watchpoints.rs, catchpoints.rs, and watch.rs stop re-deriving the same
  "label + colored value" layout.
- Give hover/press a transition via `ctx.animate_bool`/`animate_value`
  instead of the instant `BG_HOVER` swap — cheap, egui-native, and it's the
  single biggest "feels dated" fix for the cost.
- Disabled-state styling: confirm buttons gated by `*_enabled()` predicates
  (`attach_enabled`, `remote_connect_enabled`, etc.) actually render visibly
  disabled (dimmed), not just non-interactive. Currently `tbtn`/egui defaults
  handle this implicitly — verify it's legible, not just functional.

## Phase 3 — Visual hierarchy

- Audit every use of `ACCENT` green vs `TXT_HL`/`TXT_CYAN`/`TXT_YELLOW` for
  semantic collisions (e.g. does "changed value" and "active breakpoint" ever
  both render green in the same view?). Fix collisions found using the Phase
  1 semantic aliases.
- `BG_LINE_HL` (current-line highlight) and hover/selection background are
  visually close (`#182b18` vs `#222222`+accent) — confirm they stay
  distinguishable when both apply to the same row (e.g. hovering the current
  line).
- Panel headers (`sec_hdr`) are currently the same weight/size as body text
  plus a triangle icon — consider a subtle weight or background differentiation
  so panel boundaries read at a glance without relying on `hl()` separator
  lines alone.

## Phase 4 — Iconography (optional, evaluate cost/benefit first)

- Panels are text-only (`Breakpoints`, `Watch`, `Stack`, `Registers`, ...).
  A small icon set (via an icon font like `egui-phosphor`, or embedded SVG)
  for panel tabs and status (breakpoint dot, watchpoint eye, catchpoint flag)
  would improve scanability, but adds a font/asset dependency — weigh against
  the "terminal tool" aesthetic, which arguably benefits from staying
  text/monospace-only. Decide explicitly rather than by omission.

## Phase 5 — Empty/error states

- Check each panel's behavior with zero entries (no breakpoints, no
  watchpoints, no threads yet) — currently likely just an empty scroll area.
  A one-line muted placeholder (`TXT_DIM`, "No breakpoints set") is a small
  change with outsized perceived-polish payoff.
- Error surfaces already exist (`edit_errors`, `watchpoint_errors`,
  `catchpoint_errors`, `attach_error`, `remote_connect_error`) — audit that
  they render with consistent placement/styling across panels rather than
  each panel inventing its own inline error layout.

## Explicitly out of scope

- Light theme / user-configurable themes — not requested, meaningfully more
  work than the polish above, revisit only if asked.
- Animations beyond hover/press transitions (no purpose in a debugger UI).
- Any change to panel layout/information architecture — this roadmap is
  strictly visual, not structural.
