# Architecture (maps to plan §3)

Core depends on **traits**, never on concrete backends. New distro /
compositor / keyboard backend = new `impls/` module + `platform/` entries +
config value, never a core refactor.

| Crate | Role | Backends |
|---|---|---|
| `mouseless-core` | Pure grid math (cell→pixel, layouts, scale-aware). Zero I/O. | — |
| `mouseless-state` | State machine (`Normal→Overlay→Subgrid→Click`, `FreeMode`) + physics; `Clock` trait (real/fake). I/O-free. | — |
| `mouseless-ipc` | Versioned JSON protocol (`schema_version`; negotiate/refuse), newline-delimited envelopes. | — |
| `mouseless-kbd` | `KeyboardBackend` (layer events, `set_layer`, resync, reload) + `CellGrab` RAII contract. | `impls/kanata.rs` (TCP); future `keyd`, `evdev_raw` |
| `mouseless-mouse` | `MouseBackend` (`move_rel`/`click`/`scroll`) + `compute_delta` (saturating). | `impls/uinput.rs` (identity); future `libei` |
| `mouseless-overlay` | `OverlayBackend` (`show`/`highlight`/`show_subgrid`/`hide`, reports), draw-only; daemon-side Unix-socket server. | `impls/gnome.rs` (server + JSON); GNOME extension is the client; future `wlr`, `x11` |
| `mouseless-daemon` | `mouselessd`: thin orchestration (single-flight machine + grab lifecycle + click path + kill-switch co-exit). | — |
| `mouseless-cli` | `mouseless` verbs with deep-link parity + `--list-input-devices`. | — |
| `mouseless-gtk` | Settings stub → Phase-3 `Adw.PreferencesWindow`. | — |

## Data flow (plan §2.4)

```text
physical kbd → kanata (grab, HRM, tap-dance, lyr-mouse free-mode)
                 │ virtual kbd "kanata-mouseless"
                 ├──→ compositor/apps (normal typing)
                 └──→ mouselessd (TEMP grab only during overlay)
mouselessd → uinput "mouseless-mouse" (REL deltas + BTN_*/WHEEL)
mouselessd ↔ kanata TCP 127.0.0.1:port (LayerChange / ChangeLayer)
mouselessd ↔ GNOME extension /run/mouseless/mouselessd.sock (JSON)
```

No `cmd` anywhere (audit W1). `EvdevCellGrab` owns a fresh fd per overlay with
`EVIOCGKEY` + drain (audits G6/G7). Flat pointer accel for `mouseless-mouse`
(audit G1). Socket auth via `SO_PEERCRED` (audit G3); the daemon listens and
the GNOME extension connects, sends `Hello`/`Report`, and receives
`Show`/`HighlightCell`/`ShowSubgrid`/`Hide` plus state replay on reconnect.

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
- #4 geometry goldens: `mouseless-core` 1080p/4K/vertical/scale tests
- #5 REL deltas: `compute_delta` clamp + fake-mouse tests
- #7 physics goldens: per-easing-curve convergence with `ManualClock`
- #8 IPC: version negotiate/refuse, 1 MiB limit, checked-in malformed corpus,
  Hello timeout, and overlay-server malformed-frame tests
- #10 kill switch: `on_kanata_lost` grab-release + co-exit test
