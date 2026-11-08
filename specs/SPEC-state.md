# SPEC-state — State machine (`mouseless-state`)

> Module: `state` · Paths: `crates/mouseless-state` · Depends on: `core` ·
> Plan refs: §1.7, §2.2, §4 #1/#7 · Matrix rows: #1 (shape), #7
> Status as of 2026-11-08: **Implemented (Phase 3)** — machine, actions,
> and auto-off done; custom free-mode physics removed. The daemon executes
> intents and drives the fixed tick cadence (`daemon.tick_ms`).

## Objective

The deterministic brain of the overlay: which phase we are in, how a key
sequence becomes a mouse action. I/O-free by construction — time arrives via
`Clock`, geometry via `core` types, backends are called by the daemon, never
from here.

## Scope

- In (done): `OverlayMachine` (`Normal→Overlay→Subgrid→Click`), idempotent
  show/hide, `ManualClock`.
- In (done, Phase 3): multi-click cadence, drag & drop lifecycle, nudges,
  continuous mode, cursor hiding, `ActionIntent` for the daemon.
- Out (Phase 4, plan §4 G4): cycle mouse action/button/count, repeat last
  action, undo last key, hold-for-free-mode, global mouse buttons, label mode.

## Contracts

### Clock

`Clock::now_ms()`; production `SystemClock`, tests `ManualClock{ now, advance }`.
Every time-dependent behavior must be testable with `ManualClock` (matrix #1/#7).

### OverlayMachine transitions (current, exact)

| from | `show` | `hide` | `open_subgrid` | `click` |
|---|---|---|---|---|
| `Normal` | → Overlay | no-op `Ok` | `Err(InvalidEvent)` | `Err(InvalidEvent)` |
| `Overlay` | no-op `Ok` | → Normal | → Subgrid | → Click |
| `Subgrid` | no-op `Ok` | → Normal | `Err` | → Click |
| `Click` | → Overlay | → Normal | `Err` | `Err` |

Invalid events return `Err` and never change phase — duplicate/missed
`LayerChange` cannot desync the machine (plan §5).

### ActionIntent (implemented — state→daemon boundary)

State emits intents; the daemon executes them via backends. State never calls
`mouse`/`overlay`/`kbd`.

```rust
pub enum ActionIntent {
    Click { button: Button, count: u8 },     // count 1..=3, cadence per threshold
    Move  { to: core::Point },               // physical px (from core::cell_center)
    DragBegin { button: Button },
    DragTo { to: core::Point },
    DragRelease,
    Wheel { dx: i32, dy: i32 },              // detents
    CursorHide { corner: Corner },
}
```

`Button` mirrors the five buttons (state owns a dependency-free copy; daemon
maps to `mouseless_mouse::MouseButton`).

### Precision layer (entered from the daemon)

When `behavior.free_mode_after_move` is true, a `move` action ends the overlay
and switches kanata to `lyr-mouse-precision` (1 px `i/j/k/l`, `c`/`v` speed,
`esc`/`capslock` exit). The daemon does not grab in this layer; unmapped keys
type into the focused app.

## Tech stack

Rust 2021; `serde`, `thiserror`, `mouseless-core`. No async color — the core
stays sync (audit G5).

## Commands

```sh
cargo test -p mouseless-state
```

## Project structure

One crate, split by concern:
- `src/machine.rs` — overlay phase machine.
- `src/actions.rs` — buttons, multi-click counter, drag/nudge state, intents,
  and `Vec2` geometry.
- `src/lib.rs` — re-exports, `Clock`.

## Code style

- Transition methods return `Result<(), StateError>`; invalid events are
  non-fatal and never panic.
- Every timing rule takes `&impl Clock` (or a passed-in `now_ms`) — no
  `SystemTime::now()` outside `SystemClock`.
- Golden tests state the tick rate and expected convergence tolerance.

## Testing strategy

- Show/hide idempotency + full flow + invalid-event non-fatality. *(exist)*
- Multi-click threshold boundaries; drag begin/continue/release; nudge step
  math; continuous mode; `ActionIntent` shapes for each flow. *(exist in
  `machine.rs`/`actions.rs`)*
- #1 (shape): pending decision — see `SPEC-kbd.md` OQ-1; tap-hold itself is
  kanata's job, so any state-level test can only cover the *consumer* behavior
  (late/early `LayerChange`, idempotent show).

## Boundaries

- **Always:** injectable time for anything time-dependent; idempotent
  show/hide; non-fatal invalid events.
- **Ask first:** adding a new phase or intent variant (ripples into daemon);
  changing tick rate assumptions; freezing config defaults.
- **Never:** I/O, backend calls, `std::thread::sleep`, or production clocks in
  logic under test; reimplementing kanata's tap-hold.

## Success criteria

- [x] Duplicate show/hide never change phase; invalid events non-fatal.
- [x] Selection FSM: keys accumulate; `Space` with no selection uses initial
      action location; `Space` after a cell clicks its center; subgrid key
      executes at sub-cell; `Esc` exits via `ActionIntent`/phase change.
- [x] Multi-click: two/three presses within threshold → count 2/3; reset after timeout.
- [x] Drag lifecycle intents for begin/continue/release + `Esc` cancel.
- [x] Continuous mode keeps `Click` from hiding the overlay.
- [x] All flows tested with explicit `now_ms`/`ManualClock` only.

## Open questions

1. **Matrix #1 (tap-vs-hold).** Plan §8 #1 says "fake-clock property tests",
   but tap-hold lives in kanata. Proposed reading: kanata-config assertion
   (`tap-hold-release 300 300`) + state-level shape test for late/early/
   duplicate `LayerChange`. Needs human confirmation.
2. **Defaults — resolved (2026-11-08):** `multi_click_threshold_ms 500`,
   `nudges_per_cell 4`, initial action location `screen_center`, default action
   `click`, `hold_subgrid_for_nudge false`, `continuous_mode false`,
   `hide_cursor_on_click false`, `cursor_hide_corner top_right`.
   Modifier policy on sub-cell selection: `Shift` toggles the default action
   (click→move-only, move→left-click); `Ctrl` performs a right-click.
   Precision layer: `lyr-mouse-precision`, no dedicated trigger by default.
