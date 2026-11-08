# Architecture (maps to plan §3)

Core depends on **traits**, never on concrete backends. New distro /
compositor / keyboard backend = new `impls/` module + `platform/` entries +
config value, never a core refactor.

| Crate | Role | Backends |
|---|---|---|
| `mouseless-core` | Pure grid math (cell→pixel, layouts, scale-aware) including the two-level `PrimaryGrid` (10 column keys × 30 row keys → two-character cell labels). Zero I/O. | — |
| `mouseless-state` | State machine (`Normal→Overlay→Subgrid→Click`), action intents (multi-click/drag/nudge), `FreeModeSession` physics; `Clock` trait (real/fake). I/O-free. | — |
| `mouseless-config` | GTK-free config engine: schema v1, semantic validation, presets, atomic store with last-good rollback, kanata include render/check, and the `mouseless.toml` application-config loader. No I/O outside caller-supplied paths (the loader path is supplied by each binary). | — |
| `mouseless-ipc` | Versioned JSON protocol (`schema_version`; negotiate/refuse), newline-delimited envelopes, one-shot `Reload`/`Update`/`Status`/`GetConfig` client frames + replies. | — |
| `mouseless-kbd` | `KeyboardBackend` (layer events, `set_layer`, resync, reload) + `CellGrab` RAII contract. | `impls/kanata.rs` (TCP); future `keyd`, `evdev_raw` |
| `mouseless-mouse` | `MouseBackend` (`move_rel`/`click`/`button_down`/`button_up`/`scroll`) + `compute_delta` (saturating). | `impls/uinput.rs` (identity); future `libei` |
| `mouseless-overlay` | `OverlayBackend` (`show`/`highlight`/`show_subgrid`/`hide`, reports), draw-only; daemon-side Unix-socket server. | `impls/gnome.rs` (server + JSON); GNOME extension is the client; future `wlr`, `x11` |
| `mouseless-daemon` | `mouselessd`: thin orchestration (single-flight machine, grab lifecycle, action execution, overlay/free session loops, 8 ms tick, kill-switch co-exit) + config persistence/apply. | — |
| `mouseless-cli` | `mouseless` verbs with deep-link parity + kanata TCP layer switching + `--list-input-devices` + daemon `reload`/`status` + `settings` forwarding. | — |
| `mouseless-gtk` | `mouseless-settings`: GTK4/Adwaita preferences window (programmatic Rust) over `mouseless-config`; reads `GetConfig`, auto-saves with `Save`, applies on Restart App with `Update`, never writes the live file. | — |

## Data flow (plan §2.4)

```text
physical kbd → kanata (grab, HRM, tap-dance, lyr-mouse fallback)
                 │ virtual kbd "kanata-mouseless"
                 ├──→ compositor/apps (normal typing)
                  └──→ mouselessd (TEMP grab during overlay OR precision mode)
mouselessd → uinput "mouseless-mouse" (REL deltas + BTN_*/WHEEL)
mouselessd ↔ kanata TCP 127.0.0.1:port (LayerChange / ChangeLayer)
mouselessd ↔ GNOME extension /run/mouseless/mouselessd.sock (JSON)
mouseless-settings → mouselessd (GetConfig/Save/Update over the same socket;
                    Save auto-persists without changing the running app,
                    Update on Restart App persists and applies; the daemon
                    remains the only writer of the live config)
```

No `cmd` anywhere (audit W1). `EvdevCellGrab` owns a fresh fd per overlay session with `EVIOCGKEY` + drain (audits G6/G7) and is non-blocking, so one
thread can interleave key reads, kanata TCP layer events, and the action tick
(a blocking read is what made Esc need two taps and postponed auto-hide).
Pointer movement is closed-loop:
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
`Show`/`HighlightCell`/`HighlightColumn`/`ShowSubgrid`/`Hide` plus state replay
on reconnect. `HighlightColumn` fades every label outside the pending column
(the first cell key), using the same alpha-110 label treatment as the muted
subgrid context; `Show`/`ShowSubgrid`/`Hide` clear it. The default primary model
is 300 cells labelled with two characters (column key + row key, e.g. `AQ`);
selecting one by its two keys opens a 10×3 subgrid inside it. The daemon resolves the user's appearance config (defaults:
`#d4de04` at 80% transparency, 20% opacity, `#ebe9e8` labels) and sends it with
every model; the extension scales the font to each cell's stage-unit size.

## Application config (`mouseless.toml`)

Binary-level constants are loaded once at startup by `mouselessd`, `mouseless`,
and `mouseless-settings` (precedence: `--config`, `MOUSELESS_CONFIG`,
`/etc/mouseless/mouseless.toml`, built-in defaults). The file covers paths,
the kanata endpoint, loop timeouts, device identity, the base layer, the
deep-link scheme, the GTK app id, and appearance fallbacks. `config.yaml`
(users' grid/behavior/appearance) stays the daemon-owned settings
document and wins over the TOML appearance fallback. The GNOME extension cannot
parse TOML; `platform/install-extension.sh` generates its `config.js` (socket
path plus legacy-payload color fallback) from the same file. The file is read
once per process; edits require a restart.

Phase 3 precision layer uses `lyr-mouse-precision` for 1 px `i/j/k/l` movement
with `c`/`v` speed toggles and `esc`/`capslock` exit. Entered after a `move`
when `behavior.free_mode_after_move` is true, or via the optional
`keybindings.precision_trigger`. The daemon does not grab in precision mode;
unmapped keys type into the focused app. Kanata's `lyr-mouse` fallback layer is
never grabbed either.

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
- #7 physics goldens: removed with free-mode physics (SPEC-state);
- Phase 2 actions: daemon multi-click/continuous/drag/nudge/monitor/cursor-hide
  tests + state `actions.rs` unit tests
- Precision layer: daemon `teardown(false)` + `kbd.set_layer(lyr-mouse-precision)`
  tests + `after_move` enter/skip tests
- #8 IPC: version negotiate/refuse, 1 MiB limit, checked-in malformed corpus,
  Hello timeout, and overlay-server malformed-frame tests
- #10 kill switch: `on_kanata_lost` grab-release + co-exit test
- CLI: fake-kanata TCP tests for show/toggle/status + exit codes
