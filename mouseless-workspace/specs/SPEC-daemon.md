# SPEC-daemon — Orchestration (`mouselessd`)

> Module: `daemon` · Paths: `crates/mouseless-daemon` · Depends on: `kbd`, `mouse`,
> `overlay`, `state`, `core`, `ipc` · Plan refs: §1.2, §5, §6, §7, §8 #2/#3/#6/#10 ·
> Matrix rows: #2, #3, #6, #10 (owner); supports #4, #5, #8, #9
> Status as of 2026-11-04: **Implemented (Phase 1.3 MVP + Phase 2 + Phase 3
> config)** — real kanata TCP, `EvdevCellGrab`, uinput mouse, GNOME socket server,
> grid/subgrid models, FSM, click path,
> multi-click/drag/nudge/monitor/cursor-hide actions, custom 8 ms free-mode loop,
> degrade modes, kill-switch co-exit, and persisted `config.yaml` apply
> (`Reload`/`Update`/`Status`/`GetConfig`) are wired. Remaining gaps: `tracing`
> (prints today), process-level kill-switch/e2e tests, and live-host visual smoke.

## Objective

`mouselessd` is thin: it wires backends together and owns only the single-flight
overlay machine, the grab lifecycle, the click/drag/nudge action path, the
free-mode tick, and the kill switch. All math is `core`, all decisions are
`state`, all I/O is in the backends.

## Scope

- In (Phase 1.2/1.4): TCP subscribe + resync, layer→show/hide, grab lifecycle,
  grid model to the extension, click execution, kill-switch co-exit, degraded
  modes, `SO_PEERCRED` server (all implemented); `tracing` remains.
- In (Phase 2): physics loop wiring, `ActionIntent` execution, multi-click
  cadence, drag/drop, subgrid nudges, continuous mode, multi-monitor switching,
  cursor hiding, free-mode engine selection flag. *(implemented 2026-10-29)*
- In (Phase 3): config apply (`Reload`/`Update`) with validate-then-apply (shared
  with `settings`). *(implemented 2026-11-04: daemon-owned persistence under
  `StateDirectory`, startup fallback, one-shot client replies)*
- Out: any backend logic (plan §1.2).

## Contracts

### `Daemon<K, M, O, G>` (generic, testable)

```rust
pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError>;
pub fn start_overlay(&mut self) -> Result<(), DaemonError>; // grab + show, or degrade
pub fn end_overlay(&mut self);                              // drop grab + hide + base
pub fn start_free_mode(&mut self) -> Result<(), DaemonError>; // grab, no overlay
pub fn end_free_mode(&mut self);                              // release buttons + base
pub fn on_overlay_lost(&mut self);                          // Shell crash/restart
pub fn poll_overlay_events(&mut self) -> Vec<OverlayEvent>;
pub fn poll_key_event(&mut self) -> Result<Option<KeyEvent>, DaemonError>;
pub fn handle_key_event_at(&mut self, ev: KeyEvent, now_ms: u64) -> Result<SessionStep, DaemonError>;
pub fn tick(&mut self, now_ms: u64) -> Result<(), DaemonError>;
pub fn tick_now(&mut self) -> Result<(), DaemonError>;
pub fn session_active(&self) -> bool;                      // overlay OR free OR grab
pub fn click_relative(&mut self, b: MouseButton, cur: CursorPos, target: (i32,i32)) -> Result<(), DaemonError>;
pub fn on_kanata_lost(&mut self) -> DaemonError;   // release + hide + signal co-exit
pub fn keyboard_mut(&mut self) -> &mut K;          // event loop polls the backend
```

- `lyr-mouseless` → show + acquire `CellGrab` when a helloed overlay and a
  layout exist; otherwise degrade without grabbing. `lyr-free` → custom
  free-mode grab without showing the overlay. `lyr-mouse` → release/teardown
  without forcing base (kanata chose its own fallback layer). Any other layer →
  hide + drop grab. All directions are idempotent.
- Duplicate/missed `LayerChange` MUST NOT desync (single-flight, one mutex or
  single-threaded ownership — current design is single-threaded, keep it).
- Kill switch (plan §7): TCP EOF/reset → release grab (RAII), best-effort
  `Hide` broadcast, `ExitCode 1` to let the supervisor restart the stack.

### Session event loops (production path, `main.rs`)

Single-threaded; `next_layer_event` uses a 500 ms idle / 2 ms session read
timeout. A session returns the layer that ended it, and the idle loop applies
that transition (so `lyr-mouse` is not overridden by a forced base).

```text
idle loop:
  poll overlay events (Disconnected -> on_overlay_lost)
  wait for LayerChange (500 ms)
  lyr-mouseless -> start_overlay_with_retry (EBUSY ≤ ~2 s) -> overlay loop
  lyr-free      -> start_free_with_retry    (EBUSY ≤ ~2 s) -> free loop
  otherwise     -> daemon.on_layer_event (base restore; lyr-mouse teardown only)

overlay loop (2 ms TCP read):
  overlay disconnect -> end
  layer != lyr-mouseless -> return that layer to the idle loop
  drain grabbed press/release events; handle_key_event
  tick_now every 8 ms (multi-click deadline)
  session_active()==false -> end; end_overlay on every exit path

free loop (2 ms TCP read):
  overlay disconnect -> keep running (relative motion), hide overlay only
  layer == lyr-free -> continue; other layer -> return it to the idle loop
  drain grabbed press/release events; handle_free_key_event
  tick_now every 8 ms -> physics move/scroll + auto-off
  Esc/auto-off/uinput failure -> end_free_mode (release held buttons, base)
```

### Action semantics (Phase 2)

- `ActionIntent` is the only state→backend boundary: `Move`, `Click {button,
  count}`, `DragBegin`, `DragTo`, `DragRelease`, `Wheel`, `CursorHide`.
- Multi-click: `count` is the logical ordinal (1..=3); the daemon emits one
  native down/up pair per click and lets the compositor coalesce. The
  overlay/grab stays alive for `multi_click_threshold_ms`; continuous mode
  resets to level 1 instead of ending.
- Drag: Alt on the final key begins/continues a drag; the final key without Alt
  moves and drops. Esc releases at the current cursor.
- Nudge: optional `hold_subgrid_for_nudge`; step = sub-cell size /
  `nudges_per_cell`; release clicks at the nudged target.
- Shift tap ≤ `MONITOR_SWITCH_TAP_MS` switches monitors by x order unless Shift
  was used as the right-click modifier.
- `hide_cursor_on_click` moves to the configured corner after the click.
- Movement (`Move`, `DragTo`, `CursorHide`, and legacy `click_relative`) is
  closed-loop: `move_to` refreshes the cursor, emits REL steps of at most
  `MAX_MOVE_STEP` (64 px), and re-queries after each step, settling within
  `MOVE_TOLERANCE_PX` (2 px). Each step carries only half of the remaining
  error (`MOVE_DAMPING_DIVISOR` 2, with a `MOVE_MIN_STEP` 2 px floor) because
  the compositor's pointer acceleration has an observed gain of up to ~2×: a
  64 px request moves ~128 px. Sending the full error overshoots, reverses,
  and alternates around the target — the live post-click "dance". Half-error
  steps are stable for gains up to ~2 and still land in one step at gain 2;
  the floor prevents a sub-unity gain from truncating the last few pixels to
  zero. A single large delta is also unsafe at monitor edges: the compositor
  clamps a delta that would cross an edge at that edge (the live
  cursor-trapped-at-x=1920 regression). If the first cursor reply still equals
  the pre-move position it may predate the motion, so the daemon waits up to
  `MOVE_SETTLE_TIMEOUT` (20 ms) for a change before computing the next delta.
  A move with no progress for `MAX_STALLED_STEPS` (8) queries, or one
  exceeding `MAX_MOVE_ITERATIONS` (256), returns `DaemonError::MoveStalled`;
  callers MUST NOT click at a guessed position.

### Movement control loop (post-click oscillation postmortem)

**Symptom (2026-10-31).** After a subcell click the cursor visibly danced
within ~50 px of the target before settling; some sessions ended with
`cell action failed (cursor stalled at ... moving to ...)`. It reproduced on
both monitors and with repeated clicks on the same cell.

**Investigation.** A live harness impersonated the daemon socket, created a
uinput pointer, and replayed the closed loop against the real GNOME extension
(`global.get_pointer()`). The extension's first reply after every `move_rel`
was fresh — there was no stale-frame lag. The trace showed the compositor
applying a speed-dependent gain of up to ~2×:

| from | sent | reported | actual |
|---|---|---|---|
| (2526,138) | (54,-3) | (2634,132) | (+108,-6) |
| (2634,132) | (-54,3) | (2526,138) | (-108,+6) |

**Root cause.** `move_to` computed `step = target - cursor` and assumed unity
gain. With a ~2× gain the applied step is ~2× the error, so the cursor lands
the same distance on the other side of the target; the next step flips sign
and the loop alternates forever. The stall detector eventually reports
`MoveStalled` after `MAX_MOVE_ITERATIONS` (256), or after a run of identical
reports. The earlier "stale `CursorReply`" hypothesis was wrong: waiting for a
changed report did not fix the dance because the reply was already current.

**Fix.** Send half of the remaining error:
`step = clamp(error / 2, ±MAX_MOVE_STEP)`, with a `MOVE_MIN_STEP` 2 px floor.
For a plant gain `g`, the error multiplier per step is `|1 - g/2|`:

- `g = 2` → lands in one step;
- `g = 1` → halves per step (geometric convergence);
- `g < 1` → shrinks by `1 - g/2` per step;
- `g` up to ~2 is stable; the observed ~2× gain is the design point.

The 2 px floor keeps a sub-unity gain from truncating a 1 px request to zero
and masquerading as a stall. `MAX_MOVE_STEP` stays 64 px so monitor-edge
clamping cannot trap a single event. Do **not** replace this with a fixed
per-step sleep, a stability window, or a full-error step: the first two were
tried and only added latency, and the last one is the bug.

**Performance.** The fast path is one `QueryCursor` per step and no
unconditional sleep. The 20 ms settle wait is only entered when the first
reply still equals the pre-move position (a genuine stale reply). A normal
in-monitor click is ~2–6 steps; the live harness measured ~5 ms per step
(≈10–30 ms), and a 1900 px cross-monitor move ~19 steps (≈100 ms). The app is
expected to feel instantaneous: keep the fast path allocation-free and
sleep-free, and do not add fixed frame pacing to "improve" accuracy.

**Regression coverage.** `move_to_does_not_oscillate_under_two_times_pointer_acceleration`
reproduces the 2× gain; `damped_step_halves_and_floors` pins the control law;
the cross-monitor, sub-unity gain, stale-reply, and stuck-cursor tests cover
the remaining matrix row #5 cases.

### Why every relative pointer is accelerated (and kanata is not exempt)

The pointer pipeline is layered: device → evdev → libinput (inside the
compositor) → GNOME Shell pointer → `global.get_pointer()`. libinput applies
the adaptive acceleration curve to **relative** devices, i.e. anything that
reports `REL_X`/`REL_Y`. Absolute devices (touchscreens, tablets) bypass it
because they already report a position.

Both virtual devices are relative pointers: `kanata-mouseless` (`EV=7`, `REL`
includes X/Y) and `mouseless-mouse` (`EV=7`, `REL=143`). They therefore share
the same acceleration curve. Kanata does not appear affected because it is used
as a *relative nudge*: the user watches the cursor and keeps steering.
Acceleration is perceived as speed, and there is no target pixel to miss. A
fixed number of kanata `movemouse` steps issued at different speeds would show
the same speed-dependent displacement.

The daemon is an *absolute-position* client: it commands "put the cursor on
cell center (x, y), then click". Any gain ≠ 1 is therefore positional error.
The 64 px steps land in the fast part of the curve where the observed gain is
~2×; near the target the remaining error is small and the overshoot is most
visible, which is why the dance looked like a ~50 px wobble rather than a huge
miss.

Alternatives and why they are not used:

| Option | Why rejected |
|---|---|
| Global `accel-profile=flat` (GNOME) | Changes the user's normal mouse feel; violates the "do not override defaults without asking" rule. |
| Per-device `LIBINPUT_ATTR_POINTER_ACCEL` udev property | Private libinput API; not honored (confirmed on the reference host). |
| Absolute `EV_ABS` uinput device | Bypasses acceleration, but needs desktop-wide coordinate mapping across monitors/scales, may be treated as a tablet/touchscreen, and still needs closed-loop verification for rounding. Larger risk for no user-visible gain. |
| Fixed per-step sleeps / frame pacing | Adds latency without changing the gain; the reply was already fresh. |

Closed-loop half-error stepping is backend-agnostic: it works with the
compositor's real gain (up to ~2×) and with any future `MouseBackend`, and it
keeps the fast path sleep-free.

### Custom free-mode input path

`free_mode_engine = custom` (default) uses the dedicated `lyr-free` layer:
kanata emits the mapped letters, the daemon holds the same temporary grab as
the overlay, and `mouseless-state` advances physics on a fixed 8 ms tick.
Held buttons are tracked by `FreeModeSession` and released on every teardown.
`free_mode_engine = kanata` leaves `lyr-mouse` authoritative; `lyr-free` is then
ignored and the daemon restores base so letters cannot leak.

### Cell/action FSM (Phase 2)

| Input | State | Effect |
|---|---|---|
| `a s d f g h j k l ;` | primary grid, no pending column | remember that level-1 column (no visual change yet; the full 10×30 grid is already visible) |
| any level-2 key from `qwertyuiopasdfghjkl;zxcvbnm,./` | primary grid, pending column | select `(column, row)`, render its 10×3 subgrid |
| any level-2 key | subgrid | click that sub-cell's center (or press-move, release-click with `hold_subgrid_for_nudge`) |
| Space | selected / none | click the level-1 cell center / the configured initial action target |
| Arrow keys | nudge-held | move by sub-cell size / `nudges_per_cell`, clamped to the monitor |
| Shift tap ≤200 ms | any | switch monitor by x order, unless Shift was used as a modifier |
| Shift + final key | subgrid | right click instead of left |
| Alt + final key | subgrid | begin or continue a drag |
| final key without Alt | dragging | move to the target and drop |
| Esc | any | release a drag at the current cursor, exit, release grab, hide, base |
| layer change | any | exit (kanata already left the layer) |

Keys while grabbed are consumed by evdev and never reach kanata/compositor, so
there is no leaked keystroke path. Every level-1 key also appears in the
level-2 key set, so once a column is pending the next key is always resolved as
a row first (e.g. `a` then `s` is the cell labelled `AS`); to abandon a pending
column the user presses Esc. The subgrid uses the 30-key level-2 set, arranged
10×3 in keyboard-row order. Multi-click keeps the session alive for the
threshold; continuous mode resets to level 1 instead of ending.

### Production wiring (no fakes on production paths)

`main.rs` constructs `KanataSubscriber`, `UinputMouse` wrapped in `ProdMouse`
(degrade to a loud no-op if `/dev/uinput` is denied), `GnomeOverlay` wrapped in
`ProdOverlay` (degrade if the socket cannot bind), and `EvdevCellGrab::acquire`
through `Daemon::with_grab`. `FakeMouse`/`FakeOverlay`/`FakeCellGrab` remain
test-only. `ProdOverlay::Disabled` reports `is_connected() == false`, so the
daemon degrades instead of grabbing with nowhere to draw.

### Configuration (Phase 3)

`mouselessd` loads its last-applied schema-v1 document from
`$STATE_DIRECTORY/config.yaml` (`/var/lib/mouseless` by default;
`--state-dir`/`MOUSELESS_STATE_DIR` override for dev), falling back to
`config.last-good.yaml` and then built-in defaults, and logs which fallback it
used. `DaemonConfig::apply_config` maps the document onto the runtime: grid
columns/rows and subgrid dimensions, `grid_action_level` (`2` vs `subgrid`),
`default_action` (`click` vs `move`), `free_mode_after_move`, dynamic
overlay/free/kanata layer names, appearance colors/opacity, monitor assignment,
thresholds, and physics parameters. Appearance is resolved to RGB triples and
`grid_opacity` in the daemon and travels with every `GridModel`; the extension
never parses config. Monitor assignment is `auto` (cursor monitor, primary
fallback), `single` (primary), or `custom` (first present connector from
`custom_monitors`, primary fallback). Built-in defaults equal
`Config::default()`; `--kanata`/`--socket` still override transport addresses
(CLI > env > stored config > defaults).

Clients speak one-shot `Reload`/`Update`/`Status`/`GetConfig` frames over the
overlay socket (`SPEC-ipc.md`). `Update` validates, persists atomically, then
swaps the runtime; any failure leaves the previous config live and returns
`Error`. `GetConfig` is read-only and returns the effective daemon-owned
document (live file → last-good → defaults); the settings GUI reads it so its
edits start from the daemon's values rather than a shadow copy. Client peers
bypass the overlay Hello budget and never receive overlay broadcasts. Applying a
live config change tears down any active session so the next show uses the new
geometry.

### Error handling & observability (plan §6, SKILL-05)

- `Result` everywhere; zero `unwrap`/`expect` in daemon/kbd hot paths. The
  `CellGrab`/`EvdevCellGrab` and kanata subscriber paths were cleaned up; tests
  may still `unwrap`. A CI `deny` lint is still pending.
- Timeouts on every external call: TCP connect 2 s, grab `EBUSY` retry ≤ ~2 s,
  `QueryCursor` 100 ms; session TCP read timeout 2 ms (500 ms idle); fixed
  8 ms physics/multi-click tick with a 1 ms sleep cap.
- `tracing` with correlation IDs is **not yet implemented**; the binary still
  uses `println!/eprintln!`. Tracked as the main observability gap.
- `MOUSELESS_DEBUG_LAYOUT=1` logs every overlay `Report`/`CursorReply`
  (monitor geometry + cursor position) for multi-monitor diagnosis; off by
  default.
- `MOUSELESS_DEBUG_MOVE=1` logs each closed-loop step (`from`/`sent`/
  `reported`) for movement diagnosis; off by default.
- Degraded modes: overlay missing → no grab; uinput missing → no clicks, overlay
  still tracks; extension gone → keyboard/mouse safe, retry; show failure after
  grab → grab released + base restored.

## Tech stack

Rust 2021; `serde`/`serde_json`, `thiserror`. Sync single-thread loop for MVP;
Phase-2 physics runs on the loop's fixed 8 ms tick (OQ-3, resolved).

## Commands

```sh
cargo test -p mouseless-daemon
# dev run against a live kanata:
MOUSELESS_SOCKET=/tmp/mouseless.sock cargo run -p mouseless-daemon -- --kanata 127.0.0.1:7070
# e2e skeleton (Phase 1.4): fake kanata server + fake extension over the socket
```

## Project structure

- `src/lib.rs` — `Daemon`, `DaemonConfig`, `DaemonError`, grid/subgrid model
  builders, click path, unit tests with fakes.
- `src/main.rs` — argument parsing, production backend construction
  (`KanataSubscriber`, `ProdMouse`, `ProdOverlay`, `EvdevCellGrab`), idle and
  session loops, EBUSY retry, kill-switch co-exit.
- The socket server lives in `mouseless-overlay::impls::gnome`; the old
  `src/ipc_server.rs` was removed when the overlay backend took ownership of
  accept/auth/replay (per `SPEC-ipc.md` lifecycle).

## Code style

- The generic `Daemon` never names a concrete backend type.
- Session loop state is explicit (`enum SessionState` when it grows past the
  current inline flow); no hidden globals.
- Log lines: `tracing::info!(grab_id, layer, "…")` style; no bare prints.

## Testing strategy

- #2: grab lifecycle across show/hide/error/panic/kill paths — fake keyboard,
  fake overlay, fake mouse; assert release on every path. *(exists:
  `layer_switch_drives_grab_lifecycle`, `show_failure_releases_grab_and_restores_base`,
  `kanata_loss_releases_and_signals_coexit`, `overlay_loss_releases_grab_and_hides`)*
- #3: fake TCP server: missed/duplicate `LayerChange`, reconnect → resync →
  state reconcile. *(duplicate events and reconnect via kanata's connect-time
  `LayerChange` are covered; an explicit `RequestCurrentLayerName` on the same
  connection is not wired)*
- #6: both startup orders (kanata first / daemon first) with fake backends;
  exclusion assertions live in `platform` tests. *(with platform)*
- #10: fake TCP drop mid-overlay → assert grab released + Hide emitted + exit code.
  *(lib-level exists; process-level test via the fake kanata server pending)*
- Click path: `click_relative` delegates to closed-loop `move_to`, then clicks
  and hides; Space clicks the level-1 center, a second key the sub-cell center. *(exists)*
- Movement regressions (live dual-monitor layout): a large one-shot delta that
  clamps at the external monitor edge still reaches a built-in target, pointer
  acceleration with a sub-unity gain converges within tolerance, a ~2×
  acceleration gain does not oscillate around the target, a stale
  `CursorReply` answered before the compositor applies the motion does not
  reverse the loop, and a stuck cursor returns `MoveStalled` without clicking.
  Fakes share one cursor so `move_rel` changes what `QueryCursor` reports, and
  can push the pre-move position as a one-shot stale reply. *(exists:
  `move_to_crosses_to_builtin_when_large_deltas_are_clamped`,
  `move_to_converges_on_external_monitor_under_pointer_acceleration`,
  `move_to_does_not_oscillate_under_two_times_pointer_acceleration`,
  `move_to_waits_out_a_stale_cursor_reply`,
  `move_to_reports_stalled_cursor_instead_of_clicking`,
  `damped_step_halves_and_floors`)*
- Degraded modes: no overlay, no layout, failed show, uinput absent. *(lib-level
  covered; uinput-absent wrapper is production-only)*
- Phase 1.4 e2e: tap Shift → overlay → 2 keys → click at sub-cell center,
  zero leaked keystrokes (assert no cell key reaches the fake compositor side).
- Phase 2 action tests (`crates/mouseless-daemon/src/lib.rs`): multi-click
  cadence + tick expiry, continuous reset, drag begin/continue/drop, Esc drag
  cancel, Shift monitor switch, Shift-as-right-click does not switch, nudge
  math + release click, cursor-hide corner, free-mode move/wheel/buttons,
  auto-off button release, Esc exit, `lyr-mouse` ignored without base restore.
- Phase 2 state tests (`crates/mouseless-state`): easing convergence, 2-D
  movement, wheel carry, auto-off boundaries, click counter, drag state,
  nudge steps, overlay machine idempotency.

## Boundaries

- **Always:** keep `Daemon` generic; release grab on every path; broadcast
  replay on late join; timeouts on external calls; update `SPEC-ipc.md` if the
  wire changes.
- **Ask first:** threading model change; adding config fields that affect kanata
  (`free_mode_engine`); changing layer names (`lyr-mouseless`, `lyr-base`).
- **Never:** backend logic in `Daemon`; fakes on production paths; `unwrap` in
  hot paths; permanent grabs; blocking the session loop on unbounded I/O.

## Success criteria

- [x] Single-flight show/hide with grab acquire/release; dupes safe.
- [x] `click_relative` = closed-loop move + click + hide; `MoveStalled` when the
      cursor never reaches the target.
- [x] `move_to` damps to half-error and lands within 2 px under ~2× pointer
      acceleration without oscillating; fast path has no per-step sleep.
- [x] `on_kanata_lost` releases grab, hides (best-effort), signals co-exit.
- [x] Production `main.rs` uses real mouse + overlay backends; fakes are test-only.
- [x] Overlay-absent/no-layout degrade: no grab, base layer restored, loud log.
- [x] Post-grab show failure releases the grab and restores base.
- [x] Session loop: first key opens subgrid; second key clicks sub-cell center;
      Space clicks cell center; Esc exits.
- [x] Phase 2 actions: multi-click/continuous, drag/drop, nudges, modifier
      clicks, monitor switching, cursor hiding.
- [x] Custom free mode: `lyr-free` + temporary grab + fixed 8 ms physics tick,
      held-button release on every teardown, `lyr-mouse` fallback preserved.
- [x] `mouseless-cli` show/hide/toggle/enter-free-mode/exit-free-mode speak
      kanata TCP; fake-TCP tests in `crates/mouseless-cli/tests/kanata_verbs.rs`.
- [x] `SO_PEERCRED` server check (audit G3) wired in the GNOME overlay backend.
- [x] Persisted config: startup loads live → last-good → defaults from
      `StateDirectory`, `apply_config` maps schema v1 to runtime, one-shot
      `Reload`/`Update`/`Status` replies, failed apply keeps the old config live.
- [ ] Explicit `RequestCurrentLayerName` resync on the same connection after
      reconnect (matrix #3). Kanata 1.12 pushes `LayerChange` on connect and
      `reconnect_resyncs_from_connect_time_layer_push` covers that behavior;
      the explicit query remains a hardening item.
- [ ] `tracing` replaces prints; grab/layer/reconnect events carry IDs.
- [x] Zero `unwrap`/`expect` in daemon/kbd hot paths.
- [ ] CI `deny(unwrap_used, expect_used)` lint for hot-path crates.
- [ ] Phase-1.4 e2e test with fake kanata server + fake extension passes,
      including "zero leaked keystrokes".

## Open questions

1. **Auth model — resolved.** See `SPEC-ipc.md` OQ-1: uid or `mouseless`
   group membership.
2. **Grid default — resolved (2026-11-03).** Two-level primary grid:
   10 columns (`a s d f g h j k l ;`) × 30 rows
   (`qwertyuiopasdfghjkl;zxcvbnm,./`), each cell labelled with both characters
   (e.g. `AQ`, `SQ`, `AW`). Selection is two keys (column, row); the 10×3
   subgrid then takes one key to click its sub-cell center. Space clicks the
   selected cell center, or the configured initial target before a full cell
   selection. Esc exits. Changed from the 2026-10-29 4×2 MVP to match the
   mouseless.click v1.0 column layout and make sub-cells much smaller than the
   level-1 cells.
3. **Threading/tick — resolved (2026-10-29).** MVP stays single-threaded with
   500 ms idle / 2 ms session TCP read timeouts and a fixed 8 ms tick
   (`FREE_MODE_TICK`) driven from the session loop; the 1 ms sleep cap bounds
   jitter without busy-spinning. The physics core stays sync/uncolored (audit
   G5). Revisit a dedicated thread/tokio timer only if measured jitter affects
   feel.
4. **Initial monitor choice for `Show` — implemented.** Monitor under the last
   reported cursor, falling back to primary (Phase 3 exposes a setting).
