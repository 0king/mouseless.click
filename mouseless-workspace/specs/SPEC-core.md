# SPEC-core — Pure grid math (`mouseless-core`)

> Module: `core` · Paths: `crates/mouseless-core` · Depends on: — ·
> Plan refs: §1.8, §4 (geometry), §8 #4 · Matrix row: #4
> Status as of 2026-10-18: **Implemented** for Phase-1 scope; Phase-2 helpers pending.

## Objective

Single source of truth for where a click lands. The daemon turns a selected
cell into a pixel target with this crate; the overlay renders the same grid.
Any divergence between click targets and rendered cells is a correctness bug —
so all geometry is computed here, never in JS or the daemon.

Zero I/O by construction: monitor layout and cursor position are parameters.

## Scope

- In: cell bounds/centers for an arbitrary grid on one monitor, sub-cell
  nesting, scale conversion helpers, validation of grid configs.
- In (Phase 2.2): monitor ordering + switch helpers for multi-monitor switching.
- Out: rendering (owned by `overlay`), config parsing (owned by `settings`),
  cursor math from REL deltas (owned by `mouse`).

## Contracts

| Item | Contract |
|---|---|
| `Point` / `Rect` | f64, global physical pixels (compositor layout space, origin top-left). |
| `Monitor { x, y, w, h, scale }` | x/y = monitor origin in global physical px; w/h physical; `scale` = fractional scale factor (1.0/1.25/1.5/2.0). |
| `GridConfig { cols, rows, keys }` | Row-major; `validate()` requires non-zero dims and `keys.len() == cols * rows`. |
| `cell_bounds(config, monitor, cell)` | Cells tile the monitor exactly; last column/row absorbs float rounding (tested to 1e-6). |
| `cell_center(...)` | `cell_bounds().center()`; this is the click target for "click at cell center". |
| `subcell_bounds(parent, sub, cell)` | Nests a subgrid in a parent rect; click target for subgrid selection. |
| `logical_to_physical(logical, scale)` | `logical * scale`. Physical = logical × scale is the pinned direction. |

Invariants:

1. All public math is pure and deterministic — same inputs, same output.
2. No function performs I/O or reads globals; failures are `CoreError` values.
3. Grid keys are the *labels*; key→cell mapping is the caller's job (daemon FSM),
   because the same key list is reused for level-2 defaults upstream.

## Tech stack

Rust 2021; `serde` (wire reuse of `Monitor`/`Point`), `thiserror`. No other deps.

## Commands

```sh
cargo test -p mouseless-core
cargo test -p mouseless-core golden_1080p_cell_centers
```

## Project structure

`crates/mouseless-core/src/lib.rs` — types, math, inline tests. Keep it one
file until Phase-2 monitor helpers land, then split `monitor.rs` if needed.

## Code style

- Every public fn has rustdoc stating the pixel space it operates in.
- Debug-assert cell indices; release builds assume validated configs.
- Golden tests name the exact layout (resolution, scale, orientation, offset).

## Testing strategy

- Golden geometry for 1080p@1.0, 4K@2.0, 150% conversion, vertical with offset. *(exist)*
- Exact-tiling property: max(x+w) == monitor right, max(y+h) == monitor bottom. *(exists, 7×5 case)*
- Subgrid nesting center check. *(exists)*
- Phase 2: monitor switch ordering tests (left/right by x, wraparound behavior to be confirmed — OQ-2).

## Boundaries

- **Always:** keep functions pure; add goldens for every new layout class.
- **Ask first:** changing pixel-space definitions (physical vs logical) or
  rounding strategy — both ripple into `overlay` and spike §10.4.
- **Never:** add I/O, config parsing, or rendering concerns; hardcode a
  resolution or monitor count into the math.

## Success criteria

- [x] `cell_center` goldens for 1080p, 4K@200%, vertical+offset.
- [x] Tiles sum exactly to monitor bounds (float remainder absorbed in last column/row).
- [x] `GridConfig::validate` rejects empty/mismatched configs.
- [x] Subgrid center nests correctly inside parent cells.
- [ ] A click target computed here lands within tolerance in the Phase-1.4 e2e
      test at all four scale factors (blocked by `overlay`/`daemon`).
- [ ] Phase 2.2: `select_monitor(direction)` helper with x-order semantics
      (upstream: tap ShiftLeft/ShiftRight switches monitors left/right).

## Open questions

1. **Fractional-scale coordinate space (spike §10.4).** Is the extension's
   reported cursor position in the same physical space as `Monitor.w/h`? If GNOME
   reports stage/logical coordinates, the conversion point (extension vs daemon)
   must be pinned at ONE boundary. Current assumption: extension reports
   physical px (`logical × scale`), daemon math is physical throughout.
2. **Monitor switch ordering.** Upstream orders monitors by x position for
   left/right switching. Behavior for vertically stacked monitors sharing an x
   (or wrapped-around switching) needs a decision before Phase 2.2.
3. **Grid margins.** MVP tiles the full monitor. If upstream-style inset/padding
   is wanted, it must be added here (and rendered by the extension) — not faked
   in either side.
