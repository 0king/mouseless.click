# Architecture (maps to plan §3)

Core depends on **traits**, never on concrete backends. New distro /
compositor / keyboard backend = new `impls/` module + `platform/` entries +
config value, never a core refactor.

| Crate | Role | Backends |
|---|---|---|
| `mouseless-core` | Pure grid math (cell→pixel, layouts, scale-aware) including the two-level `PrimaryGrid` (10 column keys × 30 row keys → two-character cell labels). Zero I/O. | — |
| `mouseless-state` | State machine (`Normal→Overlay→Subgrid→Click`), action intents (multi-click/drag/nudge), `FreeModeSession` physics; `Clock` trait (real/fake). I/O-free. | — |
| `mouseless-config` | GTK-free config engine: schema v1, semantic validation, presets, atomic store with last-good rollback, kanata include render/check. No I/O outside caller-supplied paths. | — |
| `mouseless-ipc` | Versioned JSON protocol (`schema_version`; negotiate/refuse), newline-delimited envelopes, one-shot `Reload`/`Update`/`Status` client frames + replies. | — |
| `mouseless-kbd` | `KeyboardBackend` (layer events, `set_layer`, resync, reload) + `CellGrab` RAII contract. | `impls/kanata.rs` (TCP); future `keyd`, `evdev_raw` |
| `mouseless-mouse` | `MouseBackend` (`move_rel`/`click`/`button_down`/`button_up`/`scroll`) + `compute_delta` (saturating). | `impls/uinput.rs` (identity); future `libei` |
| `mouseless-overlay` | `OverlayBackend` (`show`/`highlight`/`show_subgrid`/`hide`, reports), draw-only; daemon-side Unix-socket server. | `impls/gnome.rs` (server + JSON); GNOME extension is the client; future `wlr`, `x11` |
| `mouseless-daemon` | `mouselessd`: thin orchestration (single-flight machine, grab lifecycle, action execution, overlay/free session loops, 8 ms tick, kill-switch co-exit) + config persistence/apply. | — |
| `mouseless-cli` | `mouseless` verbs with deep-link parity + kanata TCP layer switching + `--list-input-devices` + daemon `reload`/`status`. | — |
| `mouseless-gtk` | Settings app; Phase-3 Adwaita UI over `mouseless-config` (pending GTK dev packages). | — |

## Data flow (plan §2.4)

```text
physical kbd → kanata (grab, HRM, tap-dance, lyr-mouse fallback)
                 │ virtual kbd "kanata-mouseless"
                 ├──→ compositor/apps (normal typing)
                 └──→ mouselessd (TEMP grab during overlay OR custom free mode)
mouselessd → uinput "mouseless-mouse" (REL deltas + BTN_*/WHEEL)
mouselessd ↔ kanata TCP 127.0.0.1:port (LayerChange / ChangeLayer)
mouselessd ↔ GNOME extension /run/mouseless/mouselessd.sock (JSON)
```

No `cmd` anywhere (audit W1). `EvdevCellGrab` owns a fresh fd per overlay/free
session with `EVIOCGKEY` + drain (audits G6/G7). Pointer movement is closed-loop:
the daemon emits bounded REL steps (≤64 px) carrying half of the remaining error
(the compositor's acceleration gain is up to ~2×), waits for the compositor
report to change after each step, and re-queries the cursor until it settles
within 2 px, so acceleration, monitor-edge clamping, and stale pre-motion
replies cannot strand a click or make it oscillate. The half-error law is a
deliberate response to the compositor's ~2× acceleration gain; see
`specs/SPEC-daemon.md` §Movement control loop for the trace, postmortem, and
latency budget. Socket auth via
`SO_PEERCRED` (audit G3); the daemon listens and the GNOME extension connects,
sends `Hello`/`Report`, and receives
`Show`/`HighlightCell`/`ShowSubgrid`/`Hide` plus state replay on reconnect.
The default primary model is 300 cells labelled with two characters (column
key + row key, e.g. `AQ`); selecting one by its two keys opens a 10×3 subgrid
inside it. The extension draws `#d4de04` at 80% transparency (20% opacity)
with `#ebe9e8` labels and scales the font to each cell's stage-unit size.

Phase 2 free mode uses a dedicated `lyr-free` kanata layer (`@ml-free-tap` on
ControlLeft); the daemon holds the same temporary grab and runs an 8 ms physics
tick. `lyr-mouse` remains authoritative when `free_mode_engine = kanata`.

Phase 1.3 extension layout: `gnome-extension/extension.js` (lifecycle +
dispatch), `ipc.js` (NDJSON client/reconnect), `render.js` (Clutter actor and
physical→stage conversion). Pack with `--extra-source=ipc.js
--extra-source=render.js`. GNOME 45–49 use `affectsInputRegion: false`; GNOME 50
removed that option and is handled by a guarded retry. `Clutter.Color` is
resolved with a `Cogl.Color` fallback for GNOME 47+.

## Test matrix (plan §8, scaffold coverage)

- #1 tap-vs-hold: `mouseless-state` fake-clock property shape (kanata config in Phase 1.1)
- #2 grab lifecycle: `EvdevCellGrab` Drop-release + daemon layer↔grab tests,
  including post-grab show-failure rollback
- #3 layer resync: `FakeKeyboard` missed/dupe `LayerChange` tests + subscriber
  reconnect test (connect-time push); explicit same-connection query pending
- #4 geometry goldens: `mouseless-core` 1080p/4K/vertical/scale tests, plus `PrimaryGrid` 10×30 tiling/label/lookup tests
- #5 REL deltas: `compute_delta` clamp + fake-mouse tests; closed-loop
  cross-monitor, acceleration-gain damping, stale-reply, and stalled-cursor
  regressions with a shared `CursorMouse`/`FakeOverlay` cursor
- #7 physics goldens: per-easing-curve convergence with `ManualClock`
- Phase 2 actions: daemon multi-click/continuous/drag/nudge/monitor/cursor-hide
  tests + state `actions.rs` unit tests
- Free mode: daemon move/wheel/button/auto-off/Esc tests + `lyr-free` config
  validation (`kanata --check`)
- #8 IPC: version negotiate/refuse, 1 MiB limit, checked-in malformed corpus,
  Hello timeout, and overlay-server malformed-frame tests
- #10 kill switch: `on_kanata_lost` grab-release + co-exit test
- CLI: fake-kanata TCP tests for show/enter-free-mode/toggle/status + exit codes
