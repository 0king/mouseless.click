# SPEC-state — State machine & physics (`mouseless-state`)

> Module: `state` · Paths: `crates/mouseless-state` · Depends on: `core` ·
> Plan refs: §1.7, §2.2, §4, §8 #1/#7 · Matrix rows: #1 (shape), #7
> Status as of 2026-10-18: **Partial** — machine + physics core done; Phase-2 action
> semantics, auto-off logic, and the cell-selection FSM pending.

## Objective

The deterministic brain of the overlay: which phase we are in, how a key
sequence becomes a mouse action, and how free-mode motion evolves tick by tick.
I/O-free by construction — time arrives via `Clock`, geometry via `core` types,
backends are called by the daemon, never from here.

## Scope

- In (done): `OverlayMachine` (`Normal→Overlay→Subgrid→Click`), idempotent
  show/hide, `FreeMode` tick (base px/tick, `S/D/F` boosts, `A` decel, easing
  curves), `ManualClock`.
- In (Phase 2): auto-off timer, wheel physics, multi-click cadence, drag &
  drop lifecycle, modifier clicks, nudges, continuous mode, cursor hiding,
  selection FSM (key accumulation), action intents for the daemon.
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

- `tick(target_vel, boosts, decel) -> vel`: `target = base × mult^boosts`,
  `target /= mult` when `A` held; `vel += (target − vel) × easing.factor()`.
- Easing factors pinned: `Loose 0.1`, `Natural 0.2` (default), `Snappy 0.3`,
  `Instant 0.9`.
- Fixed-step callers: tests tick at 8 ms (~120 Hz); production tick rate is the
  daemon's choice (OQ-3).

### ActionIntent (planned — introduced with Phase 2, boundary rule)

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
| Multi-click cadence | Final key pressed again within `multi_click_threshold_ms` → count 2, then 3; reset after. Default TBD (OQ-2). |
| Drag & drop | Modifier held on final key → `DragBegin`; subsequent actions → `DragTo`; `Esc` → `DragRelease` at system cursor (cancel drops there). |
| Modifier clicks | If a non-hold-for modifier is held during a mouse action, the native mod+click must occur (physical modifiers are already held on the seat; intent only records the click). |
| Nudges | With a subgrid key held, nudge hotkeys move by `cell_size / nudges_per_cell`; release executes the click. |
| Continuous mode | After a click, stay visible (do not transition to Normal) until `Esc`/layer exit. |
| Cursor hiding | After a click with `hide_cursor_on_click`, emit `CursorHide{corner}` (Warp via delta to configured corner — executed by daemon). |
| Auto-off | Free mode with no action for `auto_off_secs` exits free mode; 0 disables. |
| Wheel physics | Same easing/boost/decel machinery on wheel targets; step/fast/step-large sizes configurable (defaults TBD). |

## Tech stack

Rust 2021; `serde`, `thiserror`, `mouseless-core`. No async color — the physics
core stays sync (audit G5).

## Commands

```sh
cargo test -p mouseless-state
cargo test -p mouseless-state physics_converges_per_curve_with_fake_clock
```

## Project structure

One crate, currently `lib.rs` only. When Phase-2 semantics land, split into
`machine.rs`, `physics.rs`, `actions.rs` once `lib.rs` exceeds ~400 lines.

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
- Phase 2: auto-off with `ManualClock`; multi-click threshold boundaries;
  drag sequence matrix; nudge step math; continuous mode; `ActionIntent`
  shapes for each flow.
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
- [ ] Auto-off test: idle for `auto_off_secs` → free mode exits; action resets timer.
- [ ] Selection FSM: keys accumulate; `Space` with no selection uses initial
      action location; `Space` after a cell clicks its center; subgrid key
      executes at sub-cell; `Esc` exits via `ActionIntent`/phase change.
- [ ] Multi-click: two/three presses within threshold → count 2/3; reset after timeout.
- [ ] Drag lifecycle intents for begin/continue/release + `Esc` cancel.
- [ ] Continuous mode keeps `Click` from hiding the overlay.
- [ ] All Phase-2 flows tested with `ManualClock` only.

## Open questions

1. **Matrix #1 (tap-vs-hold).** Plan §8 #1 says "fake-clock property tests",
   but tap-hold lives in kanata (SKILL-01). Proposed reading: blocking test =
   kanata-config assertion (`tap-hold-release 200 200`) + state-level shape test
   for late/early/duplicate `LayerChange`. Needs human confirmation.
2. **Defaults to freeze:** `multi_click_threshold_ms` (proposed 500),
   `nudges_per_cell` (proposed 4), initial action location (proposed
   `screen_center`), default action (proposed `click`), wheel step sizes.
3. **Production tick rate:** tests assume 8 ms (~120 Hz). Confirm the daemon's
   loop uses a fixed 8 ms tick (or document the jitter tolerance).
4. **Custom free-mode input path.** With `free_mode_engine = custom`, the daemon
   needs movement keys (I/K/J/L, wheel keys) per tick — either a temp grab while
   free mode is active (extends plan §5 rule 1 wording) or kanata `lyr-mouse`
   stays authoritative for MVP. This is the largest unresolved Phase-2 design
   point; it touches `kbd`, `platform` (kanata layer) and `daemon`. Decide before
   Phase 2.1 implementation.
