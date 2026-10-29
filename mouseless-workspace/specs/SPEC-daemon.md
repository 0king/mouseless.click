# SPEC-daemon — Orchestration (`mouselessd`)

> Module: `daemon` · Paths: `crates/mouseless-daemon` · Depends on: `kbd`, `mouse`,
> `overlay`, `state`, `core`, `ipc` · Plan refs: §1.2, §5, §6, §7, §8 #2/#3/#6/#10 ·
> Matrix rows: #2, #3, #6, #10 (owner); supports #4, #5, #8, #9
> Status as of 2026-10-29: **Implemented (Phase 1.3 MVP)** — real kanata TCP,
> `EvdevCellGrab`, uinput mouse, GNOME socket server, grid/subgrid models, FSM,
> click path, degrade modes, and kill-switch co-exit are wired. Remaining gaps:
> `tracing` (prints today), explicit reconnect-resync verification (matrix #3),
> `mouseless-cli` show/hide/toggle verbs, and Phase-1.4 process-level e2e.

## Objective

`mouselessd` is thin: it wires backends together and owns only the single-flight
overlay machine, the grab lifecycle, the click path, and the kill switch. All
math is `core`, all decisions are `state`, all I/O is in the backends.

## Scope

- In (Phase 1.2/1.4): TCP subscribe + resync, layer→show/hide, grab lifecycle,
  grid model to the extension, click execution, kill-switch co-exit, degraded
  modes, `SO_PEERCRED` server (all implemented); `tracing` remains.
- In (Phase 2): physics loop wiring, `ActionIntent` execution, continuous mode,
  multi-monitor switching, free-mode engine selection flag.
- In (Phase 3): config apply (`Reload`/`Update`) with validate-then-apply (shared
  with `settings`).
- Out: any backend logic (plan §1.2).

## Contracts

### `Daemon<K, M, O, G>` (generic, testable)

```rust
pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError>;
pub fn start_overlay(&mut self) -> Result<(), DaemonError>; // grab + show, or degrade
pub fn end_overlay(&mut self);                              // drop grab + hide + base
pub fn on_overlay_lost(&mut self);                          // Shell crash/restart
pub fn poll_overlay_events(&mut self) -> Vec<OverlayEvent>;
pub fn handle_cell_key(&mut self, key: CellKey) -> Result<SessionStep, DaemonError>;
pub fn click_relative(&mut self, b: MouseButton, cur: CursorPos, target: (i32,i32)) -> Result<(), DaemonError>;
pub fn on_kanata_lost(&mut self) -> DaemonError;   // release + hide + signal co-exit
pub fn keyboard_mut(&mut self) -> &mut K;          // event loop polls the backend
```

- `lyr-mouseless` → show + acquire `CellGrab` when a helloed overlay and a
  layout exist; otherwise degrade without grabbing. Any other layer → hide +
  drop grab. Both directions are idempotent.
- Duplicate/missed `LayerChange` MUST NOT desync (single-flight, one mutex or
  single-threaded ownership — current design is single-threaded, keep it).
- Kill switch (plan §7): TCP EOF/reset → release grab (RAII), best-effort
  `Hide` broadcast, `ExitCode 1` to let the supervisor restart the stack.

### Session event loop (production path, `main.rs`)

Per iteration (single thread, no async for MVP):

```text
1. overlay.poll_events()                  → accept/Hello/Report/Disconnected
2. poll keyboard (500 ms TCP read timeout) → layer events
3. on lyr-mouseless: start_overlay:
     - require is_connected() + last_layout(); else degrade
       (no grab, set base, log loudly; typing unaffected)
     - pick monitor under cursor (fallback primary), build GridModel via core
     - acquire EvdevCellGrab (EBUSY retried by main ≤ ~2 s)
     - show; if show fails, release the grab + degrade (no orphan grab)
4. session loop (2 ms TCP read timeout):
     - overlay disconnect → end session
     - kanata layer != lyr-mouseless → end session
     - poll grab key presses, apply the FSM (below)
5. click path: QueryCursor (≤ 100 ms) → fall back to last Report
               → target = core::subcell_center/cell_center
               → mouse::compute_delta → move_rel → click(button)
               → end_overlay (drop grab, hide, base layer)
6. every exit path: grab Drop drains + ungrabs; hide is best-effort; set base
   idempotently. Idle read timeout returns to 500 ms when the session ends.
```

### MVP cell FSM (Phase 1.3, OQ-2 resolved)

| Input | State | Effect |
|---|---|---|
| `a s d f j k l ;` | level 1 | select that cell, render its 4×2 subgrid |
| same keys | subgrid | click that sub-cell's center |
| Space | selected cell | click the level-1 cell center |
| Esc | any | exit, release grab, hide, restore base |
| layer change | any | exit (kanata already left the layer) |

Keys while grabbed are consumed by evdev and never reach kanata/compositor, so
there is no leaked keystroke path. The subgrid uses the same key set by default.

### Production wiring (no fakes on production paths)

`main.rs` constructs `KanataSubscriber`, `UinputMouse` wrapped in `ProdMouse`
(degrade to a loud no-op if `/dev/uinput` is denied), `GnomeOverlay` wrapped in
`ProdOverlay` (degrade if the socket cannot bind), and `EvdevCellGrab::acquire`
through `Daemon::with_grab`. `FakeMouse`/`FakeOverlay`/`FakeCellGrab` remain
test-only. `ProdOverlay::Disabled` reports `is_connected() == false`, so the
daemon degrades instead of grabbing with nowhere to draw.

### Configuration (MVP)

`DaemonConfig` carries: kanata addr, socket path, **grid config** (MVP default:
4×2 level-1 cells keyed `a s d f / j k l ;`, subgrid 4×2 with the same keys,
target = sub-cell center on the second key, cell center on Space), and backend
names. Free-mode params/feature flags arrive with Phase 2. Until Phase-3
`config.yaml` exists, defaults live in code + `--kanata`/`--socket` + the
`MOUSELESS_SOCKET` env override. Config precedence: CLI > env > defaults.

### Error handling & observability (plan §6, SKILL-05)

- `Result` everywhere; zero `unwrap`/`expect` in daemon/kbd hot paths. The
  `CellGrab`/`EvdevCellGrab` and kanata subscriber paths were cleaned up; tests
  may still `unwrap`. A CI `deny` lint is still pending.
- Timeouts on every external call: TCP connect 2 s, grab `EBUSY` retry ≤ ~2 s,
  `QueryCursor` 100 ms; session TCP read timeout 2 ms (500 ms idle).
- `tracing` with correlation IDs is **not yet implemented**; the binary still
  uses `println!/eprintln!`. Tracked as the main observability gap.
- Degraded modes: overlay missing → no grab; uinput missing → no clicks, overlay
  still tracks; extension gone → keyboard/mouse safe, retry; show failure after
  grab → grab released + base restored.

## Tech stack

Rust 2021; `serde`/`serde_json`, `thiserror`. Sync single-thread loop for MVP;
Phase-2 physics may use a dedicated tick (thread or tokio task) — see OQ-3.

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
- Click path: `click_relative` refreshes cursor, moves by `compute_delta`, clicks,
  then hides; Space clicks the level-1 center, a second key the sub-cell center. *(exists)*
- Degraded modes: no overlay, no layout, failed show, uinput absent. *(lib-level
  covered; uinput-absent wrapper is production-only)*
- Phase 1.4 e2e: tap Shift → overlay → 2 keys → click at sub-cell center,
  zero leaked keystrokes (assert no cell key reaches the fake compositor side).

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
- [x] `click_relative` = cursor refresh + delta + click + hide.
- [x] `on_kanata_lost` releases grab, hides (best-effort), signals co-exit.
- [x] Production `main.rs` uses real mouse + overlay backends; fakes are test-only.
- [x] Overlay-absent/no-layout degrade: no grab, base layer restored, loud log.
- [x] Post-grab show failure releases the grab and restores base.
- [x] Session loop: first key opens subgrid; second key clicks sub-cell center;
      Space clicks cell center; Esc exits.
- [x] `SO_PEERCRED` server check (audit G3) wired in the GNOME overlay backend.
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
2. **MVP grid default — resolved (2026-10-29).** 4×2 both levels, keys
   `a s d f j k l ;`; second key clicks the sub-cell center; Space clicks the
   level-1 cell center; Esc exits.
3. **Threading/tick** — MVP single-thread with 500 ms idle / 2 ms session TCP
   read timeouts and a 1 ms session sleep; Phase-2 physics wants a fixed 8 ms
   tick. Decide then: dedicated tick thread vs tokio timer (plan allows either,
   audit G5: do not async-color the physics core).
4. **Initial monitor choice for `Show` — implemented.** Monitor under the last
   reported cursor, falling back to primary (Phase 3 exposes a setting).
