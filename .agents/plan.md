# Mouseless Linux: High-Level Architecture & Implementation Plan

> **Target OS:** Ubuntu GNOME (Wayland, GNOME Shell 45+ ESM, primary test target 50+ / Ubuntu 26.04)
> **Input Interception:** Kanata-fronted (kanata owns grabs, tap-hold/HRM, layers; TCP only, no `cmd`)
> **Custom daemon:** Thin privileged `mouselessd` (temp-grab cell capture, grid math, uinput mouse, free-mode physics, overlay IPC, TCP client)
> **GUI Framework:** GTK4 + libadwaita, in Rust (`gtk4` + `libadwaita` crates, `Adw.PreferencesWindow`)
> **Overlay Subsystem:** GNOME Shell Extension (Clutter canvas in `Main.uiGroup`, draw-only)
> **Kanata Integration:** Coexistence with Home Row Mods and Kanata daemon; no boot-order dependency; no hardware-priority pipeline
> **Extensibility:** Trait-based backends — same core extends to other distros/compositors; kanata swappable behind `KeyboardBackend`
> **Reference:** [mouseless.click documentation](file:///.misc/extracted_docs.txt)

---

## 1. Locked Architectural Decisions

1. **Kanata-fronted input (kanata owns keyboards).** Kanata keeps its exclusive grabs
   on physical keyboards and owns tap-hold/HRM detection, base layers, and the
   existing `lyr-mouse` free-mode layer. We do NOT write a competing evdev keyboard
   engine (no permanent grab, no custom tap-hold, no uinput keyboard passthrough).
2. **Thin `mouselessd`.** Our Rust daemon does only what kanata cannot on Linux:
   temporary cell-key capture during overlay, grid math, absolute-positioned clicks
   via its own uinput **mouse**, custom free-mode physics loop, overlay IPC server,
   kanata TCP client. Permanent keyboard capture is forbidden.
3. **Kanata TCP required; `cmd` REMOVED (audit W1).** TCP
    (`-p 127.0.0.1:7070`-style, localhost only) is mandatory AND sufficient for
    `LayerChange` subscription and `ChangeLayer` control. `LayerChange` IS the
    overlay show/hide signal — no separate `cmd` hook is needed. We do NOT use
    the `cmd` action, so we do NOT need the `kanata_cmd_allowed` binary variant,
    do NOT set `danger-enable-cmd yes`, fork no helper per signal, and avoid the
    `kanata-svc` sandbox socket-path blocker entirely (audit C3 resolved by
    deletion). Per-cell keys travel via temporary evdev grab (zero fork, zero leak).
4. **Hardware-priority pipeline DELETED.** The old "Alternative Pipeline" (mouseless
   grabs physical hardware first, kanata consumes our virtual keyboard) is removed
   and must not be reintroduced. Rationale: boot-order race with `kanata.service`
   (kanata has a ~2 s device-settle delay with documented yield/re-grab pathologies
   when two grabbers fight), kanata loses physical device identity (names/phys
   change breaks `linux-dev-*` rules and hotplug logic), every normal keystroke
   traverses two daemons, and a `mouselessd` crash means a dead keyboard. The
   temp-grab-during-overlay design achieves the same 0 ms cell path with none of
   these costs.
5. **Overlay = GNOME extension, draw-only.** Fullscreen transparent Clutter actor in
   `Main.uiGroup`. A normal Wayland window (GTK/Slint/Qt) cannot do this on Mutter:
   `xdg-shell` steals focus and breaks typing; Mutter does not support
   `wlr-layer-shell`. Input stays at evdev/kanata; the extension never captures keys.
6. **Settings = GTK4/libadwaita in Rust** (replaces Slint). Native GNOME look,
   `Adw.PreferencesWindow`, same Cargo workspace (`gtk4` + `libadwaita` crates).
   Build prerequisite: `apt install libgtk-4-dev libadwaita-1-dev` (runtimes
   GTK 4.22 / Adwaita 1.9.1 already present; `-dev` packages missing as of 2026-10-16).
7. **Custom free-mode physics KEPT** (`mouseless-state`). Base px/tick, `S/D/F`
   accel, `A` decel, easing curves (`loose/natural/snappy/instant`), wheel physics,
   auto-off timer. Coexists with kanata's `lyr-mouse` via the
   `free_mode_engine = custom | kanata` flag (default `custom`). Kanata's mouse
   layer stays as fallback preset; its removal is explicitly post-MVP.
8. **Modular, backend-swappable code.** Core depends on traits, never on
   kanata/GNOME/evdev directly (see §3). Adding a distro, compositor, or keyboard
   backend = new `impls/` module + config value, not a refactor.
9. **Kill switch.** Kanata's emergency-exit chord (`LCtrl+Space+Esc`) kills the app
   too (see §7).

---

## 2. Kanata & Home Row Mods Integration

### 2.1 Why kanata-fronted (verified against installed kanata 1.12.0)

- Kanata's TCP protocol emits `LayerChange`/layer-names/fakekey actions but **no
  per-keypress events** — kanata alone cannot tell the daemon which cell was pressed.
- The action that could (`cmd mouseless-cell a`) is **compiled out** of the stock
  binary and is deliberately NOT used (audit W1): per-key `cmd` would fork per
  press and `show/hide` via `cmd` is redundant with `LayerChange`. Stock kanata
  binary suffices; no `kanata_cmd_allowed` swap, no `danger-enable-cmd`.
- Kanata **cannot absolute-position the mouse on Linux** (`setmouse does not work
  in Linux yet`, TCP `SetMouse` likewise). Grid "click at cell center" is impossible
  from kanata alone — hence our uinput mouse helper.
- Kanata supports what we need otherwise: `linux-dev-names-exclude/include`,
  `linux-output-device-name`, `linux-device-detect-mode`, full TCP
  (`ChangeLayer`, `ActOnFakeKey`, `Reload*`, `RequestCurrentLayerName/Info`).

### 2.2 Clean-layer ≠ suppression (why the temp grab exists)

A clean no-HRM overlay layer fixes **latency** (no 200 ms tap-hold on
`a/s/d/f/j/k/l/;/spc`), not **suppression**: `a → a` in `lyr-mouseless` still
reaches the focused app (2 stray characters per click). On evdev a key is stopped
only if kanata swallows it or something downstream grabs it. Per-key `cmd` could
swallow, but forks per press. Therefore: kanata's overlay layer stays plain
letters (zero suppression work for kanata); **suppression happens downstream via
temporary exclusive grab** of kanata's virtual output, held only while the overlay
is visible (see §5).

### 2.3 Required kanata changes

- `defcfg`: add `linux-output-device-name "kanata-mouseless"`,
  `linux-dev-names-exclude ("mouseless-mouse")`, keep `keyboard-only` detect mode.
- New layer `lyr-mouseless`: plain `a s d f j k l ;`, plain `spc`, `esc` → back to
  base. Trigger: `ShiftLeft` tap (tap-dance/tap-hold, ~200 ms) → `layer-switch lyr-mouseless`.
  Decision (audit C1, user-accepted): keep `ShiftLeft` to match upstream
  (`ShiftLeft` tap on Windows/Linux). Known cost: fast `ShiftLeft+letter` (<200 ms)
  can misfire as tap-then-letter instead of capital. Accepted mitigation for MVP:
  `ShiftRight` remains fully usable for capitals; tap/hold/chord/interrupt matrix
  in test #1 must stay green. Double-tap / `Shift+Mod` tap / CapsLock alternatives
  considered and deferred.
- No `cmd` hooks of any kind (audit W1). No `kanata_cmd_allowed`, no
  `danger-enable-cmd`, no `/usr/local/bin` helpers, no sandbox relaxation.
- `kanata.service`: append TCP flag, e.g. `-p 127.0.0.1:7070` (localhost only);
  keep `--emergency-exit-code 1` for macOS/Windows parity BUT on Linux kanata
  raises `SIGTERM` directly (ignores the exit code — audit W2). Since
  `Restart=on-failure` alone does not restart on `SIGTERM`, set either
  `Restart=always` or `Restart=on-failure` + `RestartForceExitStatus=SIGTERM`.
  Do NOT add `forceful-exit-code` to `defcfg` (invalid option — config parse error).

### 2.4 Data flow

```
physical kbd → kanata (grab, HRM, tap-dance, lyr-mouse free-mode)
                 │ virtual kbd "kanata-mouseless"
                 ├──→ compositor/apps (normal typing; kanata handles everything)
                 └──→ mouselessd (TEMP grab only during overlay → eat cell keys)
mouselessd → uinput mouse "mouseless-mouse" (REL deltas + BTN_*/WHEEL;
             kanata ignores it via linux-dev-names-exclude)
mouselessd ↔ kanata TCP 127.0.0.1:port (subscribe LayerChange, send ChangeLayer)
mouselessd ↔ GNOME extension, Unix-socket JSON (show/hide, geometry + cursor pos)
GTK4 settings → writes config.yaml + kanata include → triggers kanata Reload
```

---

## 3. Modular Component Architecture (backend-swappable)

Core crates depend on **traits**, never on concrete backends. Concrete integrations
live in `impls/` behind Cargo feature flags. Platform specifics (device paths, group
names, unit files, extension API versions) live in `platform/`, never hardcoded.

```
mouseless-workspace/
├── Cargo.toml
├── crates/
│   ├── mouseless-core/        # PURE grid math, zero I/O (cell→pixel, layouts)
│   ├── mouseless-state/       # State machine + free-mode physics; Clock trait (real/fake)
│   ├── mouseless-kbd/         # trait KeyboardBackend { subscribe_layers(), set_layer(),
│   │   │                       grab_cells(), release() }
│   │   └── impls/kanata.rs    # TCP + temp-grab; future: impls/keyd.rs, impls/evdev_raw.rs
│   ├── mouseless-mouse/       # trait MouseBackend { move_rel(), click(), scroll() }
│   │   └── impls/uinput.rs    # future: impls/libei.rs (portal-friendly Wayland)
│   ├── mouseless-overlay/     # trait OverlayBackend { show(), hide(), render() }
│   │   └── impls/gnome.rs     # Unix-socket JSON; future: impls/wlr.rs, impls/x11.rs
│   ├── mouseless-ipc/         # Versioned JSON protocol (schema_version; negotiate/reject)
│   ├── mouseless-daemon/      # mouselessd: thin orchestration only
│   ├── mouseless-gtk/         # Settings app (Rust gtk4 + libadwaita)
│   └── mouseless-cli/         # show/hide/toggle, ChangeLayer sender, --list-input-devices
├── gnome-extension/           # Draw-only overlay (no input capture, no uinput)
├── platform/                  # udev rules, systemd units, .desktop deep-links, distro matrix
└── ui/                        # GTK4 UI definitions (GtkBuilder/Blueprint, Adwaita styles)
```

Trait-boundary rules (enforced in review + CI):

1. No `kanata`/`evdev`/`gtk`/`clutter` imports outside `impls/` or the owning crate.
2. Backend selection is config (`keyboard_backend: kanata`, `overlay_backend: gnome`),
   never a refactor.
3. IPC carries `schema_version`; mismatched peers negotiate or refuse with a clear error.
4. `platform/` owns all OS/compositor specifics; crates take them as parameters.

---

## 4. Overlay, State, Settings, IPC Contracts

- **Overlay (GNOME extension):** fullscreen transparent actor in `Main.uiGroup`
  via `Main.layoutManager.addTopChrome(actor, { affectsInputRegion: false,
  trackFullscreen: true })`; GNOME 45+ ESM syntax, `metadata.json`
  `"shell-version": ["45", "46", "47", "48", "49", "50"]` (audit C2);
  reports monitor geometry + cursor position to the daemon (the daemon needs both:
  geometry for cell centers, cursor pos for REL-delta computation since Linux has no
  absolute warp); renders grid/labels/highlight at 60 FPS from daemon frames.
- **State (`mouseless-state`):** `Normal → Overlay → Subgrid → Click`, plus
  `FreeMode` (custom physics loop, fixed-step tick with injectable `Clock` —
  tokio timer for daemon I/O, dedicated thread or tokio task for the tick; do not
  async-color the physics core) and drag handling.
  `free_mode_engine` flag selects custom vs kanata-native motion.
- **Mouse (`mouseless-mouse` → uinput `mouseless-mouse`):** REL deltas computed
  from extension reports. Requires flat pointer acceleration for accuracy
  (audit G1): ship udev rule `ENV{LIBINPUT_ATTR_POINTER_ACCEL}="flat"` (or
  equivalent `ACCEL_PROFILE=flat`) scoped to `mouseless-mouse` in `platform/`.
  Without this, libinput accel distorts large deltas and clicks land off-target.
- **Settings (`mouseless-gtk`):** grid/appearance editor, monitor assignment,
  keybinding reference, presets (Standard, Quick-Select+Free-Mode), deep links
  (`mouseless://toggle-overlay` etc. via `.desktop` + `x-scheme-handler`), writes
  `config.yaml` + kanata include then triggers kanata `Reload` + daemon reload.
- **Contracts:**
  - kanata → daemon: TCP `LayerChange` subscription; daemon → kanata: `ChangeLayer`,
    `RequestCurrentLayerName/Info` (resync), `Reload*` (config apply).
  - daemon → extension: `Show(GridModel)`, `HighlightCell`, `ShowSubgrid`, `Hide`
    over Unix socket at `/run/mouseless/mouselessd.sock` (`RuntimeDirectory=mouseless`
    on the daemon unit; mode `0770`, group `mouseless`), versioned JSON framing,
    `SO_PEERCRED` peer check that the client is the daemon UID (audit G3).
    Full 1-page wire spec (envelope, message schemas, error format, hello/negotiate
    → stream → close, `schema_version` negotiation, keepalive/reconnect) is a
    Phase-1 pre-coding deliverable (audit G2).
  - GUI/CLI → daemon: `Reload`, `Update(ConfigData)`; CLI also speaks kanata TCP
    for layer commands.
- **Upstream feature mapping (audit G4, MVP scope):** in-scope for MVP/Phase 2:
  grid click, subgrid, continuous mode (Phase 2.2), multi-monitor switching.
  Deliberately deferred to Phase 4: label mode (AT-SPI, experimental), overlay
  lock / pass-through (beta upstream), in-overlay config editor (Tab), global
  mouse buttons, hold-for-free-mode, cycle mouse action/button/count. No ambiguity:
  anything not listed here is out of scope until Phase 4.

---

## 5. Race-Avoidance Rules (normative — no boot-order dependency)

1. **Never grab at startup.** Exclusive grab of `kanata-mouseless` happens only
   while the overlay is visible, via an RAII `CellGrab` guard whose `Drop`
   implementation always releases the grab and best-effort notifies hide.
   Guard owns its fd: open `/dev/input/eventX` fresh on each overlay invocation,
   close on `Drop` (audit G7 — never reuse an fd across overlays or the kernel
   per-client buffer replays stale typing / `SYN_DROPPED`). On acquire, snapshot
   `EVIOCGKEY` and if a key is already down, wait for release or synthesize the
   matching key-up to the compositor before grabbing; on release, drain the fd
   before close so no partial press/release pair leaks (audit G6 — stuck/phantom
   keys). Kernel guarantees grab release on fd close, even on SIGKILL.
2. **LayerChange→grab race ACCEPTED (audit W3, user-accepted).** Daemon learns
   about `lyr-mouseless` over TCP after kanata switches; a cell key pressed
   within ~5 ms of the switch can beat the grab. Accepted for MVP as rare in
   practice (users don't strike the cell key within 5 ms of tapping Shift).
   No speculative grab, no kanata-side key swallowing for MVP; revisit only if
   telemetry/tests show real misfires.
3. **No `eventN` hardcodes.** Discover the kanata output by stable
   `linux-output-device-name` + inotify rescan on hotplug/restart.
4. **TCP reconnect with backoff + resync.** Exponential-backoff reconnect and
   `RequestCurrentLayerName` on every (re)connect; duplicate/missed `LayerChange`
   events are idempotent (single-flight overlay state machine, one mutex,
   idempotent show/hide).
5. **`EBUSY` is retried, never fatal.** Grab attempts use timeout + retry; failure
   degrades to "overlay unavailable, typing unaffected".
6. **Supervision without coupling.** Daemon unit is `After=kanata.service` but
   explicitly NOT `Requires=`/`BindsTo=` for startup; it waits lazily for TCP +
   device with backoff and survives kanata restarts transparently (except the
   kill-switch path, §7, where co-exit is intentional).

---

## 6. Error-Handling Policy (robust by default)

- Every fallible operation returns `Result` with context (`thiserror` types +
  `anyhow` context at boundaries). **Zero `unwrap`/`expect` in daemon hot paths**
  (enforced by CI lint); extension wraps all IPC/socket handlers in try/catch with
  error toasts to logs, never a Shell crash.
- Structured logging (`tracing`): grab acquire/release, layer transitions, TCP
  reconnects, IPC version mismatches, uinput errors — all with correlation IDs.
- Degraded modes: overlay unavailable ⇒ kanata layers keep working; extension
  disconnected ⇒ daemon keeps keyboard/mouse safe and retries; bad config ⇒
  validate-then-apply with automatic rollback to last-good (both `config.yaml` and
  kanata include; kanata `Reload` failure surfaces in GUI).
- Timeouts on every external call (TCP connect/RPC, socket write, grab acquire);
  no unbounded blocking on the input path.

---

## 7. Kill Switch (kanata emergency-exit kills the app too)

- Configure kanata `--emergency-exit-code 1` so the `LCtrl+Space+Esc` chord exits
  kanata (audit W2: on Linux kanata raises `SIGTERM` directly and ignores the code;
  the flag is kept for macOS/Windows parity). Supervisor restart requires
  `Restart=always` OR `Restart=on-failure` + `RestartForceExitStatus=SIGTERM` in
  `kanata.service` — `Restart=on-failure` alone does not restart on `SIGTERM`.
- `mouselessd` treats kanata TCP EOF/reset as kanata-death: RAII releases any held
  grab, sends overlay-hide (idempotent, best-effort), then exits via the same
  failure path so the supervisor restarts the stack in order.
- The chord is documented in the settings GUI and in-plan (no second kill chord to
  maintain). Covered by a dedicated integration test (fake TCP server drops
  mid-overlay → assert grab released + hide emitted + exit code).

---

## 8. Critical Sections & Regression-Test Plan (must-ship with MVP)

| # | Critical area | Why it regresses | Test (automated, blocking CI) |
|---|---|---|---|
| 1 | Tap-vs-hold (`ShiftLeft` tap <200 ms vs `Shift+key`) | False overlays / leaked Shifts (`ShiftLeft` retained per C1 decision; `ShiftRight` fallback for capitals) | Fake-clock property tests: tap/hold/chord/interrupt sequences |
| 2 | Temp-grab lifecycle | Stuck grab = dead keyboard; stuck/phantom keys, stale fd buffer (G6/G7) | Fake-evdev tests: show/hide, error mid-overlay, panic, kill-switch → assert release on EVERY path; `EVIOCGKEY` pre-check + drain-on-release; fresh fd per overlay |
| 3 | Layer↔daemon↔overlay sync | Ghost overlay / desync after kanata restart | Fake-TCP-server tests: missed/duplicate `LayerChange`, reconnect resync |
| 4 | Cell→pixel incl. scale (100/125/150/200%, 1080p/4K, vertical) | Clicks land off-target | Golden geometry tests per layout |
| 5 | REL-delta from extension cursor pos | Overflow/clamp/multi-monitor offset bugs; libinput accel distortion (G1) | Unit tests incl. negative/large deltas, per-monitor offsets; flat-accel udev rule assertion |
| 6 | Loop prevention (exclude `mouseless-mouse`, stable output name) | Event feedback loop resurgence | Startup-ordering tests (both orders) + exclusion assertions |
| 7 | Free-mode tick determinism | Easing/accel drift | Fixed-step golden tests per curve (`loose/natural/snappy/instant`) |
| 8 | IPC schema + malformed frames | Daemon/extension crash on bad JSON | Version-negotiation + fuzz/corpus tests on the socket parser |
| 9 | Config reload (YAML + kanata include) | Bad config bricks input | Validate-then-apply + rollback tests; kanata `Reload`-failure path |
| 10 | Kill-switch path | Grab held across co-exit | Fake-TCP-drop mid-overlay → assert release + hide + exit code |

---

## 9. Phase-by-Phase Implementation Plan

### Phase 1: Kanata-fronted grid MVP (Priority 1)
- [ ] **1.1 Kanata base:** TCP port, `linux-output-device-name "kanata-mouseless"`,
  `linux-dev-names-exclude ("mouseless-mouse")`, `lyr-mouseless` (clean no-HRM),
  `ShiftLeft`-tap trigger (retained per C1 decision), NO `cmd` (audit W1);
  `--emergency-exit-code 1` + `Restart=always` (or `RestartForceExitStatus=SIGTERM`)
  per audit W2; verify `LayerChange` latency.
- [ ] **1.2 Thin daemon:** TCP subscriber + resync, RAII temp-grab cell reader
  (fresh fd per overlay, `EVIOCGKEY` + drain per G6/G7),
  `mouseless-core` grid math, uinput mouse deltas + `BTN_*` (flat-accel udev rule
  per G1), overlay IPC server (`/run/mouseless/mouselessd.sock` + `SO_PEERCRED`
  per G3, 1-page wire spec per G2),
  `mouseless-cli` (`show/hide/toggle`, `--list-input-devices`).
- [ ] **1.3 GNOME overlay:** fullscreen Clutter actor via `addTopChrome`
  (`affectsInputRegion: false`), 45+ ESM, geometry + cursor-pos report,
  grid/label/highlight render, idempotent show/hide, Shell-restart resilience.
- [ ] **1.4 End-to-end MVP:** tap Shift → overlay → 2 keys → left-click at cell
  center, zero leaked keystrokes; tests #1–#6, #8 green.

### Phase 2: Custom free-mode physics + precision (Priority 2)
- [ ] **2.1** `mouseless-state` free-mode loop (fixed-step tick, accel/easing/wheel,
  auto-off) behind `free_mode_engine=custom`; kanata `lyr-mouse` kept as fallback.
- [ ] **2.2** Subgrid nudging, drag & drop, multi-click cadence, modifier clicks,
  multi-monitor switching, continuous mode, cursor hiding; tests #4, #5, #7 green.

### Phase 3: GTK4 Settings GUI & Presets (Priority 3)
- [ ] **3.1** `mouseless-gtk` (`Adw.PreferencesWindow`): live preview, keybinding
  reference with conflict hints, monitor assignment, presets engine (YAML
  import/export), deep links (`mouseless://`), tray/status integration; test #9.

### Phase 4: Multi-distro / multi-backend extensibility (Priority 4)
- [ ] **4.1** New `KeyboardBackend` impl (e.g. keyd) and/or `MouseBackend`
  (`libei`/portal) behind config + feature flags.
- [ ] **4.2** `OverlayBackend` for wlroots (`wlr-layer-shell`) and X11 fallback.
- [ ] **4.3** Label mode (AT-SPI, experimental, GNOME/X11 first).

---

## 10. Open Verification Steps (pre-build spikes, ≤2 days)

1. Temp-grab `kanata-mouseless` output while kanata runs (assert `EVIOCGRAB`
   succeeds; kanata does not hold its own output; verify `EVIOCGKEY` snapshot +
   fresh-fd-per-grab per G6/G7).
2. TCP `LayerChange` round-trip latency kanata↔daemon (target <5 ms localhost).
   Race window accepted per W3 — no extra action unless misfires observed.
3. Unix socket path + auth (replaces old `cmd_allowed` check, removed per W1):
   `/run/mouseless/mouselessd.sock` via `RuntimeDirectory=mouseless`, mode 0770,
   `SO_PEERCRED` check; write 1-page IPC wire spec first (G2/G3).
4. Extension cursor-pos + geometry report accuracy at 100/150/200% scale.
5. Flat-accel udev rule for `mouseless-mouse` (G1): verify large REL deltas land
   on cell center with no libinput distortion.
6. `apt install libgtk-4-dev libadwaita-1-dev` for the Rust GTK build.
7. Permissions path: `mouseless` group + udev `uinput` rule + `input` membership
   (same as upstream), scoped to temp-grab + uinput mouse (no permanent grab).
