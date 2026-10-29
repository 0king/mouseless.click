# SPEC-state — State machine & physics (`mouseless-state`)

> Module: `state` · Paths: `crates/mouseless-state` · Depends on: `core` ·
> Plan refs: §1.7, §2.2, §4, §8 #1/#7 · Matrix rows: #1 (shape), #7
> Status as of 2026-10-29: **Implemented (Phase 2)** — machine, physics,
> actions, auto-off, and the free-mode key/intent contract are done; the daemon
> executes intents and drives the fixed 8 ms tick.

## Objective

The deterministic brain of the overlay: which phase we are in, how a key
sequence becomes a mouse action, and how free-mode motion evolves tick by tick.
I/O-free by construction — time arrives via `Clock`, geometry via `core` types,
backends are called by the daemon, never from here.

## Scope

- In (done): `OverlayMachine` (`Normal→Overlay→Subgrid→Click`), idempotent
  show/hide, `FreeMode` tick (base px/tick, `S/D/F` boosts, `A` decel, easing
  curves), 2-D movement/wheel carry, auto-off, `ManualClock`.
- In (done, Phase 2): multi-click cadence, drag & drop lifecycle, nudges,
  continuous mode, cursor hiding, `ActionIntent` for the daemon, free-mode key
  state (`FreeModeSession`).
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

### FreeMode physics (current)

- `advance(target: Vec2, boosts, decel) -> Vec2` (2-D) and
  `tick_axis(target, boosts, decel)` (scalar helper): `target = base ×
  mult^boosts`, `target /= mult` when `A` held; `vel += (target − vel) ×
  easing.factor()`.
- `move_delta(keys)`/`wheel_delta(keys)` convert held key state into whole
  pixel/detent deltas and carry fractional remainders.
- Easing factors pinned: `Loose 0.1`, `Natural 0.2` (default), `Snappy 0.3`,
  `Instant 0.9`.
- Fixed-step callers: tests and production tick at 8 ms (~120 Hz); the daemon
  owns the cadence (`FREE_MODE_TICK`, SPEC-daemon OQ-3).

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

### Phase-2 semantics (rules from upstream docs, pinned for implementation)

| Behavior | Rule |
|---|---|
| Multi-click cadence | Final key pressed again within `multi_click_threshold_ms` → count 2, then 3; reset after. Default 500 ms; `count` is the logical ordinal and the daemon emits one native pair per click. |
| Drag & drop | Alt held on the final key → `DragBegin`; subsequent final keys → `DragTo`; final key without Alt → move + `DragRelease`; `Esc` → `DragRelease` at the current cursor. |
| Modifier clicks | Shift held on the final key → right button (`hold_for_right_button`, default on). Physical modifiers stay held on the seat; the intent records only the button. |
| Nudges | Opt-in `hold_subgrid_for_nudge`: holding a subgrid key moves to its center; arrow hotkeys move by `cell_size / nudges_per_cell` (default 4); release executes the click. |
| Continuous mode | After a click, reset to level 1 instead of hiding until `Esc`/layer exit. |
| Cursor hiding | After a click with `hide_cursor_on_click`, emit `CursorHide{corner}` (default `TopRight`); the daemon warps by delta. |
| Auto-off | Free mode with no action for `auto_off_secs` (default 10) exits; 0 disables. |
| Wheel physics | Same easing/boost/decel machinery on wheel targets; defaults `base_lines_per_tick 1.0`, `step_size 3`, `step_size_large 10`. |

## Tech stack

Rust 2021; `serde`, `thiserror`, `mouseless-core`. No async color — the physics
core stays sync (audit G5).

## Commands

```sh
cargo test -p mouseless-state
cargo test -p mouseless-state physics_converges_per_curve_with_fake_clock
```

## Project structure

One crate, split by concern:
- `src/machine.rs` — overlay phase machine.
- `src/physics.rs` — free-mode movement/wheel physics, key state, auto-off.
- `src/actions.rs` — buttons, multi-click counter, drag/nudge state, intents.
- `src/lib.rs` — re-exports, `Clock`, `FreeModeEngine`.

## Code style

- Transition methods return `Result<(), StateError>`; invalid events are
  non-fatal and never panic.
- Every timing rule takes `&impl Clock` (or a passed-in `now_ms`) — no
  `SystemTime::now()` outside `SystemClock`.
- Golden tests state the tick rate and expected convergence tolerance.

## Testing strategy

- #7: per-curve convergence goldens at a fixed 8 ms step. *(exist)*
- Boost/decel target scaling. *(exists)*
- Show/hide idempotency + full flow + invalid-event non-fatality. *(exist)*
- Phase 2: auto-off boundaries; multi-click threshold boundaries; drag
  begin/continue/release; nudge step math; continuous mode; `ActionIntent`
  shapes for each flow. *(exist in `machine.rs`/`physics.rs`/`actions.rs`)*
- #1 (shape): pending decision — see `SPEC-kbd.md` OQ-1; tap-hold itself is
  kanata's job (SKILL-01), so any state-level test can only cover the
  *consumer* behavior (late/early `LayerChange`, idempotent show).

## Boundaries

- **Always:** injectable time for anything time-dependent; idempotent
  show/hide; non-fatal invalid events.
- **Ask first:** adding a new phase or intent variant (ripples into daemon);
  changing tick rate assumptions; freezing config defaults.
- **Never:** I/O, backend calls, `std::thread::sleep`, or production clocks in
  logic under test; reimplementing kanata's tap-hold.

## Success criteria

- [x] Duplicate show/hide never change phase; invalid events non-fatal.
- [x] Physics converges per easing curve with `ManualClock`.
- [x] Boosts ×mult^n and `A` decel ÷mult.
- [x] Auto-off test: idle for `auto_off_secs` → free mode exits; action resets timer.
- [x] Selection FSM: keys accumulate; `Space` with no selection uses initial
      action location; `Space` after a cell clicks its center; subgrid key
      executes at sub-cell; `Esc` exits via `ActionIntent`/phase change.
- [x] Multi-click: two/three presses within threshold → count 2/3; reset after timeout.
- [x] Drag lifecycle intents for begin/continue/release + `Esc` cancel.
- [x] Continuous mode keeps `Click` from hiding the overlay.
- [x] All Phase-2 flows tested with explicit `now_ms`/`ManualClock` only.

## Open questions

1. **Matrix #1 (tap-vs-hold).** Plan §8 #1 says "fake-clock property tests",
   but tap-hold lives in kanata (SKILL-01). Proposed reading: blocking test =
   kanata-config assertion (`tap-hold-release 200 200`) + state-level shape test
   for late/early/duplicate `LayerChange`. Needs human confirmation.
2. **Defaults — resolved (2026-10-29):** `multi_click_threshold_ms 500`,
   `nudges_per_cell 4`, initial action location `screen_center`, default action
   `click`, `hold_subgrid_for_nudge false`, `continuous_mode false`,
   `hide_cursor_on_click false`, `cursor_hide_corner top_right`, wheel
   `base_lines_per_tick 1.0` / `step_size 3` / `step_size_large 10`.
3. **Production tick rate — resolved (2026-10-29):** the daemon uses a fixed
   8 ms tick (`FREE_MODE_TICK`); 1 ms sleep cap bounds jitter.
4. **Custom free-mode input path — resolved (2026-10-29):** a dedicated
   `lyr-free` kanata layer plus the daemon's temporary grab while free mode is
   active (extends plan §5 rule 1 wording). `lyr-mouse` remains authoritative
   when `free_mode_engine = kanata`. Implemented in `kbd`, `platform/kanata`,
   and `daemon`.
