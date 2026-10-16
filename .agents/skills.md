# Skills & Operating Instructions for Mouseless Development

Applies to every agent working in this repo. Architecture source of truth:
[plan.md](file:///.agents/plan.md). Role boundaries: [AGENTS.md](file:///.agents/AGENTS.md).
Reference behavior: [mouseless.click docs](file:///.misc/extracted_docs.txt).

---

## SKILL-01 — Kanata-first input (no competing grabber)

- Kanata owns physical keyboards, tap-hold/HRM, base layers, and `lyr-mouse`.
  Never add a permanent keyboard grab, a second tap-hold engine, or a uinput
  keyboard passthrough.
- The ONLY evdev capture is the temporary overlay grab of the stable
  `kanata-mouseless` output device (`linux-output-device-name`), held solely
  while the overlay is visible via the RAII `CellGrab` guard. Guard owns its fd:
  open fresh per overlay, close on `Drop`; `EVIOCGKEY` snapshot on acquire, drain
  on release (audits G6/G7).
- Never hardcode `/dev/input/eventN`. Discover by device name + inotify rescan.
- The hardware-priority pipeline (grab-physical-first) is DELETED. Do not
  reintroduce it under any name. Rationale in plan §1.4.

## SKILL-02 — Kanata TCP + `cmd` policy

- TCP (`-p 127.0.0.1:port`, localhost only) is mandatory AND sufficient:
  subscribe `LayerChange`, send `ChangeLayer`, resync with
  `RequestCurrentLayerName/Info`, apply config with `Reload*`. Reconnect =
  exponential backoff + resync; all events idempotent.
- `cmd` is REMOVED (audit W1). No `kanata_cmd_allowed`, no `danger-enable-cmd`,
  no helpers. `LayerChange` is the sole show/hide signal. Per-key `cmd` was
  already forbidden; now ALL `cmd` is forbidden.
- Spawned helpers: N/A (no helpers). Daemon↔extension socket is
  `/run/mouseless/mouselessd.sock` (`RuntimeDirectory=mouseless`) with
  `SO_PEERCRED` check (audit G3).
- `setmouse`/`SetMouse` do NOT work on Linux. Absolute clicks always go through
  our uinput mouse using extension-supplied cursor-pos + geometry, with flat
  pointer accel udev rule for `mouseless-mouse` (audit G1).

## SKILL-03 — Loop prevention & race avoidance

- Every virtual device we create is named `mouseless-*` with vendor `0x736e` and
  MUST be covered by kanata `linux-dev-names-exclude`. Verify both directions:
  we ignore kanata's needs, kanata ignores our mouse.
- No boot-order dependency: daemon unit is `After=kanata.service`, never
  `Requires=` for startup. Lazy-wait TCP + device with backoff; survive kanata
  restarts transparently (kill-switch co-exit in SKILL-06 is the only exception).
- Single-flight overlay state machine (one mutex, idempotent show/hide).
  `EBUSY` on grab = retry with timeout, never crash, degrade to "overlay
  unavailable, typing unaffected".

## SKILL-04 — Trait boundaries (backend-swappable code)

- Core/state crates stay I/O-free. No `kanata`/`evdev`/`gtk`/`clutter` imports
  outside `impls/` or the owning crate. New distro/compositor/backend = new
  `impls/` module + `platform/` entries + config value (`keyboard_backend`,
  `overlay_backend`). If core must change to add a backend, stop and fix the
  abstraction first.
- IPC frames carry `schema_version`. Mismatched peers negotiate or refuse loudly.
- `platform/` owns udev rules, systemd units, `.desktop` deep-links, Shell API
  targets. Crates take these as parameters.

## SKILL-05 — Robust error handling

- `Result` with context on every fallible op (`thiserror` + `anyhow` at
  boundaries). No `unwrap`/`expect` in daemon hot paths (CI denies it).
  Extension IPC/socket handlers never throw into the Shell (log + degrade).
- Timeouts on every external call (TCP, socket, grab). Structured `tracing`
  logs with correlation IDs for grabs, layer transitions, reconnects, IPC
  mismatches, uinput errors.
- Validate-then-apply + automatic rollback for `config.yaml` AND the kanata
  include. A bad config must never brick input; surface failures in the GUI.

## SKILL-06 — Kill switch

- Ship `--emergency-exit-code 1` so `LCtrl+Space+Esc` exits kanata (Linux raises
  `SIGTERM` directly — audit W2). Supervisor needs `Restart=always` or
  `Restart=on-failure` + `RestartForceExitStatus=SIGTERM`. Never put
  `forceful-exit-code` in `defcfg` (invalid). Daemon rule: kanata TCP EOF/reset → release grab
  (RAII), best-effort overlay-hide, co-exit failure code. No second kill chord.
- Document the chord in the GUI. Test it with a fake TCP drop mid-overlay
  (assert: grab released + hide emitted + exit code).

## SKILL-07 — Regression coverage (blocking)

Before merging anything touching these areas, the corresponding tests must be
green (matrix in plan §8): (1) tap-vs-hold fake-clock properties, (2) grab
release on every path incl. panic/kill, (3) layer resync incl. missed/dupe
events, (4) golden geometry incl. 125/150/200% + vertical, (5) REL-delta
overflow/clamp/offsets, (6) both startup orders + exclusion assertions,
(7) fixed-step physics goldens per easing curve, (8) IPC version + fuzz corpus,
(9) config rollback + `Reload`-failure, (10) kill-switch co-exit.

## SKILL-08 — GTK4/Adwaita settings (Rust)

- Rust `gtk4` + `libadwaita` (`Adw.PreferencesWindow`). Slint is deleted; do not
  add a second toolkit. Build requires `libgtk-4-dev libadwaita-1-dev`.
- Presets (Standard, Quick-Select+Free-Mode), YAML import/export, deep-link/CLI
  parity (`mouseless://` ↔ CLI verbs), `free_mode_engine = custom | kanata`
  (default `custom`), kanata `Reload` trigger after config apply.

## SKILL-09 — Docs discipline

- `plan.md` = decisions + contracts + test matrix. `AGENTS.md` = who owns what.
  This file = how to work. Keep all three consistent on every change; a behavior
  change without a matching doc/test update is incomplete.
- `lyr-mouse` removal stays post-MVP. wlroots/X11/label-mode stay Phase 4.
  Grid-click MVP with zero leaked keystrokes is the current definition of done.
