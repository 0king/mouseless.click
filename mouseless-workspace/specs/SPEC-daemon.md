# SPEC-daemon — Orchestration (`mouselessd`)

> Module: `daemon` · Paths: `crates/mouseless-daemon` · Depends on: `kbd`, `mouse`,
> `overlay`, `state`, `core`, `ipc` · Plan refs: §1.2, §5, §6, §7, §8 #2/#3/#6/#10 ·
> Matrix rows: #2, #3, #6, #10 (owner); supports #4, #5, #8, #9
> Status as of 2026-10-18: **Partial** — TCP subscriber + evdev grab + IPC broadcast
> loop live; production mouse/overlay still faked in `main.rs`; SO_PEERCRED,
> grid model, click path, tracing pending.

## Objective

`mouselessd` is thin: it wires backends together and owns only the single-flight
overlay machine, the grab lifecycle, the click path, and the kill switch. All
math is `core`, all decisions are `state`, all I/O is in the backends.

## Scope

- In (Phase 1.2/1.4): TCP subscribe + resync, layer→show/hide, grab lifecycle,
  grid model to the extension, click execution, kill-switch co-exit, degraded
  modes, `tracing`, `SO_PEERCRED` server.
- In (Phase 2): physics loop wiring, `ActionIntent` execution, continuous mode,
  multi-monitor switching, free-mode engine selection flag.
- In (Phase 3): config apply (`Reload`/`Update`) with validate-then-apply (shared
  with `settings`).
- Out: any backend logic (plan §1.2).

## Contracts

### `Daemon<K, M, O>` (generic, testable)

```rust
pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError>;
pub fn click_relative(&mut self, b: MouseButton, cur: CursorPos, target: (i32,i32)) -> Result<(), DaemonError>;
pub fn on_kanata_lost(&mut self) -> DaemonError;   // release + hide + signal co-exit
pub fn keyboard_mut(&mut self) -> &mut K;          // event loop polls the backend
```

- `lyr-mouseless` → show + acquire `CellGrab` (idempotent); any other layer →
  hide + drop grab.
- Duplicate/missed `LayerChange` MUST NOT desync (single-flight, one mutex or
  single-threaded ownership — current design is single-threaded, keep it).
- Kill switch (plan §7): TCP EOF/reset → release grab (RAII), best-effort
  `Hide` broadcast, `ExitCode 1` to let the supervisor restart the stack.

### Session event loop (production path, `main.rs`)

Per iteration (single thread, no async for MVP):

```text
1. ipc.poll_accept()                      → late-join replay on client change
2. reconcile daemon state with kanata     → resync queries after reconnect (#3)
3. poll keyboard (500 ms TCP read timeout) → layer events
4. on lyr-mouseless:  wait ≤ 500 ms for overlay Report; if absent → degrade
                      (no grab, set base, log loudly; typing unaffected)
                      else: grab (EBUSY policy ≤ 2 s), build GridModel via core,
                      broadcast Show, start session loop
5. session loop: poll grab key presses + IPC accept + layer changes
                 keys accumulate per `state` FSM; subgrid frames on cell select;
                 Space/subgrid-execute → click path; Esc/layer-exit → hide
6. click path: QueryCursor (≤ 100 ms) → target = core::subcell_center/cell_center
               → mouse::compute_delta → move_rel → click(button[, count])
               → hide (unless continuous mode, Phase 2.2)
7. release grab (Drop) on every exit path; set base only if the layer isn't
   already there (idempotent)
```

### Production wiring (no fakes on production paths)

`main.rs` must construct `UinputMouse` (degrade to a null-mouse with loud log if
`/dev/uinput` is denied) and the GNOME overlay client. `FakeMouse`/`FakeOverlay`
remain test-only. Current code violates this — Phase-1.2 completion item.

### Configuration (MVP)

`DaemonConfig` grows to carry: kanata addr, socket path, **grid config** (MVP
default: 4×2 level-1 cells keyed `a s d f / j k l ;`, subgrid 4×2 with the same
keys, target = sub-cell center on key 2, cell center on Space — confirm OQ-2),
free-mode params, feature flags (`free_mode_engine`). Until Phase-3
`config.yaml` exists, defaults live in code + `--flags` + env overrides
(`MOUSELESS_SOCKET` already exists). Config precedence: CLI > env > defaults.

### Error handling & observability (plan §6, SKILL-05)

- `Result` everywhere; zero `unwrap`/`expect` in hot paths (CI deny). Current
  `.expect("show is infallible…")` calls get replaced by infallible signatures
  (`show()` returns `()` — impossible states already unrepresentable).
- Timeouts on every external call: TCP connect 2 s, socket write, grab acquire
  2 s, `QueryCursor` 100 ms.
- `tracing` with correlation IDs: grab acquire/release, layer transitions, TCP
  reconnects, IPC mismatches, uinput errors. Replace all `println!/eprintln!`.
- Degraded modes: overlay missing → no grab; uinput missing → no clicks, overlay
  still tracks; extension gone → keyboard/mouse safe, retry.

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

- `src/lib.rs` — `Daemon`, `DaemonConfig`, `DaemonError`, unit tests with fakes.
- `src/main.rs` — argument parsing, backend construction, event loop.
- `src/ipc_server.rs` — broadcast server; `SO_PEERCRED` check lands here
  (per `SPEC-ipc.md` auth predicate).

## Code style

- The generic `Daemon` never names a concrete backend type.
- Session loop state is explicit (`enum SessionState` when it grows past the
  current inline flow); no hidden globals.
- Log lines: `tracing::info!(grab_id, layer, "…")` style; no bare prints.

## Testing strategy

- #2: grab lifecycle across show/hide/error/panic/kill paths — fake keyboard,
  fake overlay, fake mouse; assert release on every path. *(partial exists:
  layer_switch_drives_grab_lifecycle, kanata_loss_releases_and_signals_coexit)*
- #3: fake TCP server: missed/duplicate `LayerChange`, reconnect → resync →
  state reconcile. *(needs subscriber-level fake usage)*
- #6: both startup orders (kanata first / daemon first) with fake backends;
  exclusion assertions live in `platform` tests. *(with platform)*
- #10: fake TCP drop mid-overlay → assert grab released + Hide emitted + exit code.
  *(lib-level exists; add process-level test via the fake kanata server)*
- Click path: `click_relative` moves by `compute_delta` then clicks then hides. *(exists)*
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
- [x] `click_relative` = delta + click + hide.
- [x] `on_kanata_lost` releases grab, hides (best-effort), signals co-exit.
- [ ] Production `main.rs` uses real mouse + overlay backends; fakes are test-only.
- [ ] Overlay-absent degrade path: no grab, base layer restored, loud log.
      (Design delta: current code grabs even with no overlay client — change it.)
- [ ] Session loop: cell keys accumulate; subgrid; Space/exec → click; Esc → exit.
- [ ] `SO_PEERCRED` server check (audit G3) wired from `ipc_server.rs`.
- [ ] Resync after every TCP reconnect + state reconcile (matrix #3).
- [ ] `tracing` replaces prints; grab/layer/reconnect events carry IDs.
- [ ] Zero `unwrap`/`expect` in hot paths; CI lint added.
- [ ] Phase-1.4 e2e test with fake kanata server + fake extension passes,
      including "zero leaked keystrokes".

## Open questions

1. **Auth model** — see `SPEC-ipc.md` OQ-1 (uid vs group membership; plan §4
   wording is ambiguous).
2. **MVP grid default** — proposed 4×2 both levels, keys `a s d f j k l ;`,
   click target = sub-cell center on the second keypress; confirm before
   Phase 1.4 e2e.
3. **Threading/tick** — MVP single-thread with 500 ms TCP read timeouts and
   10 ms session sleep; Phase-2 physics wants a fixed 8 ms tick. Decide then:
   dedicated tick thread vs tokio timer (plan allows either, audit G5: do not
   async-color the physics core).
4. **Initial monitor choice for `Show`** — proposed: monitor under cursor at
   show time (Phase 3 exposes a setting).
