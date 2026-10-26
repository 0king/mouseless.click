# SPEC-kbd — Keyboard backend & temp-grab (`mouseless-kbd`)

> Module: `kbd` · Paths: `crates/mouseless-kbd` · Depends on: — ·
> Plan refs: §1.1, §2, §5, §7, §8 #1–3 · Matrix rows: #1, #2, #3 (with `daemon`)
> Status as of 2026-10-18: **Partial** — trait, kanata TCP subscriber, and evdev
> grab implemented; resync-on-reconnect wiring and EBUSY policy tests pending.

## Objective

Kanata stays authoritative for physical keyboards (grab, HRM, layers — SKILL-01).
This module provides (a) the swappable `KeyboardBackend` the daemon polls for
layer events, and (b) the RAII `CellGrab` that temporarily captures kanata's
virtual output while the overlay is visible so cell keys never leak to apps.

## Scope

- In: `KeyboardBackend` trait + `KanataSubscriber` (TCP), `EvdevCellGrab` +
  device discovery (`find_output_device`), `translate_key`, backoff schedule,
  `query_current_layer`.
- Out (Phase 4): `keyd`, `evdev_raw` backends. Out (never): permanent grabs,
  `cmd`, tap-hold reimplementation.

## Contracts

### `KeyboardBackend` (trait — pin)

```rust
fn next_layer_event(&mut self) -> Result<LayerEvent, Self::Error>; // blocking w/ timeout
fn set_layer(&mut self, name: &str) -> Result<(), Self::Error>;    // ChangeLayer
fn current_layer(&mut self) -> Result<String, Self::Error>;        // resync
fn reload(&mut self) -> Result<(), Self::Error>;                   // Reload*
```

Implementations MUST: reconnect with backoff, resync with
`RequestCurrentLayerName` on every (re)connect, and treat duplicate/missed
`LayerChange` as idempotent (plan §5).

### Kanata TCP protocol (localhost only)

| Direction | Frame | Notes |
|---|---|---|
| server → client | `{"LayerChange":{"new":"lyr-mouseless"}}` | Pushed on connect + every switch; parsed → `LayerEvent::LayerChanged`. |
| server → client | `{"CurrentLayerName":{"name":"lyr-base"}}` | Resync response; other pushes skipped. |
| client → server | `{"ChangeLayer":{"new":"…"}}` | `set_layer`. |
| client → server | `{"RequestCurrentLayerName":{}}` | Resync. |
| client → server | `{"RequestReload":{}}` | `reload`. |

Non-layer server lines (e.g. `ConfigFileReload`) are ignored. Read timeout is
500 ms so the daemon loop keeps a tick; timeout surfaces as
`KbdError::Unavailable` (caller keeps loop alive, stays connected).

Layer names that matter: `lyr-mouseless` = show; anything else = hide.

### Grab invariants (audits G6/G7, matrix #2)

1. **Fresh fd per overlay.** `acquire()` opens the device path by name every
   time; fds are never cached or reused (kernel per-client buffer would replay
   stale typing / `SYN_DROPPED`).
2. **No grab over a held key.** `EVIOCGKEY` snapshot on acquire; any key down →
   `KbdError::Busy`, caller retries. Key case: kanata is still forwarding a held
   physical modifier (e.g. `ShiftLeft` released after the layer switch); grabbing
   now would eat its key-up and leave the compositor stuck. Strategy: retry until
   clean (≤ 2 s in the daemon), never synthesize key-ups — rationale: kanata owns
   the physical path and will emit the real release.
3. **Drain on release.** `Drop` drains pending events, then `ungrab()`s
   best-effort, then closes the fd. No partial press/release pair survives.
4. **Drop always runs** — normal exit, error, panic unwind, and kill-switch
   co-exit. Kernel also releases on fd close, even on SIGKILL.
5. **Device discovery by stable name** `kanata-mouseless` (must match
   `platform/kanata/mouseless.kbd` `linux-output-device-name`). Never any
   `/dev/input/eventN` hardcode. Enumeration happens per acquire, which also
   covers hotplug/kanata-restart (an inotify watcher is only needed if a
   long-lived listener path is ever added — none exists).
6. **`EBUSY` is retried, never fatal** (plan §5 rule 5): daemon policy =
   40 × 50 ms attempts, then degrade to base layer with "typing unaffected".

### Key translation (pin)

`a s d f j k l ; → CellKey::Char`, `space → Space`, `esc → Escape`; every other
key while grabbed is consumed silently (returns `None`, batch keeps draining).
Codes: `KEY_A/S/D/F/J/K/L/SEMICOLON/SPACE/ESC`.

## Tech stack

Rust 2021; `evdev` 0.13 (grab), `serde_json` (kanata frames), `thiserror`.
No async — the trait is blocking-poll by design.

## Commands

```sh
cargo test -p mouseless-kbd
# device check on a live machine:
grep -A2 'Name="kanata-mouseless"' /proc/bus/input/devices
```

## Project structure

- `src/lib.rs` — trait, `LayerEvent`, `KbdError`, `CellGrab` scaffold contract.
- `src/tcp.rs` — `KanataSubscriber`, `parse_server_line`, `query_current_layer`,
  `backoff_for_attempt`.
- `src/grab.rs` — `EvdevCellGrab`, `find_output_device`, `translate_key`.
- `src/impls/kanata.rs` — `KanataTcpConfig`. `src/impls/fake.rs` — `FakeKeyboard`.

## Code style

- Errors carry context (`KbdError::Io(String)` with the failing op, path, or addr).
- Public constants (`KANATA_OUTPUT_NAME`) are the only place the device name
  string exists on the Rust side.
- Every invariant in the grab contract has a comment naming its audit id.

## Testing strategy

- #2: grab release on every path (Drop, explicit release, error mid-session via
  daemon tests, panic path in integration test). `EVIOCGKEY` pre-check and
  drain verified against a scripted fake evdev (Phase 1.2 follow-up: need a
  fake device layer under `grab.rs` to make this hermetic).
- #3: duplicate/missed `LayerChange` idempotent; reconnect + resync test using a
  fake TCP server that drops the connection mid-stream. *(partial: subscriber
  receives + resync query tested; add reconnect-resync loop test)*
- Backoff caps at 5 s. *(exists)*
- `translate_key` mapping incl. non-overlay keys. *(exists)*
- Device-absent degrades to `Unavailable`, never panics. *(exists)*

## Boundaries

- **Always:** resync after every reconnect; keep `KANATA_OUTPUT_NAME` in sync
  with the kanata include; return `Result` from every fallible op.
- **Ask first:** adding a new backend impl; changing grab retry policy; any
  change to the TCP frame set (must match kanata 1.12.0).
- **Never:** `cmd`/`danger-enable-cmd`/helpers (audit W1); permanent grabs; a
  second tap-hold engine; reusing an fd across overlay sessions; caching
  `eventN` paths.

## Success criteria

- [x] Trait + fake backend + kanata TCP subscriber with backoff; dupes safe.
- [x] `EvdevCellGrab` fresh fd, `EVIOCGKEY` pre-check, drain-on-drop, ungrab.
- [x] `translate_key` pinned mapping; device-absent → `Unavailable`.
- [x] Backoff 100 ms → 5 s cap.
- [ ] Daemon loop resyncs with `RequestCurrentLayerName` after every reconnect
      and reconciles overlay state (matrix #3, joint with `SPEC-daemon.md`).
- [ ] Hermetic fake-evdev tests proving: grab never opens twice concurrently;
      no event survives release; held-key path retries then degrades.
- [ ] Fake-TCP test: server drops mid-overlay → daemon sees EOF → `KanataLost`
      path (matrix #10, joint with daemon).

## Open questions

1. **Matrix #1 semantics (tap-vs-hold).** Plan §8 #1 asks for fake-clock
   property tests, but tap-hold is kanata's (SKILL-01). Proposed blocking test:
   (a) config assertion script checks `ml-tap` is `tap-hold-release 200 200`,
   `concurrent-tap-hold yes`, and `ShiftRight` untouched; (b) state-level shape
   test for duplicate/late `LayerChange`. Confirm with human.
2. **`ShiftLeft` under `concurrent-tap-hold`.** The include relies on the live
   `unicfg.kbd` setting `concurrent-tap-hold yes` (fast Shift+key = hold). The
   apply script does not enforce this flag; decide whether Phase 1.1 install
   must assert it (proposed: yes, assert in `--check` script).
3. **Kanata version pin.** Verified against 1.12.0. Document the supported
   range or commit to exact-version checks in the apply script.
