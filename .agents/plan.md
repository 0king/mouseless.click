# Mouseless Linux: High-Level Architecture & Implementation Plan

> **Target OS:** Ubuntu GNOME (Wayland, GNOME Shell 50+)
> **Input Interception:** Kanata-fronted (kanata owns grabs, tap-hold/HRM, layers; TCP + `cmd` as required)
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
3. **Kanata TCP required; `cmd` as required (rare signals only).** TCP
   (`-p 127.0.0.1:7070`-style, localhost only) is mandatory for `LayerChange`
   subscription and `ChangeLayer` control. The `cmd` action (requires the
   `kanata_cmd_allowed` binary variant + `danger-enable-cmd yes`) is used ONLY for
   rare signals (overlay show/hide hooks), NEVER for the per-cell hot path
   (fork-per-keypress latency/jitter + sandbox limits). Per-cell keys travel via
   temporary evdev grab (zero fork, zero leak).
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
  binary (`cmd is not enabled for this kanata executable`); it needs the
  `kanata_cmd_allowed` release asset or a `--features cmd` rebuild.
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
  base. Trigger: `ShiftLeft` tap (tap-dance/tap-hold) → `layer-switch lyr-mouseless`.
  Rare-signal `cmd` hooks for show/hide only (no per-key `cmd`).
- `kanata.service`: append TCP flag, e.g. `-p 127.0.0.1:7070` (localhost only);
  add `--emergency-exit-code 1` (non-zero so `Restart=on-failure` restarts it);
  swap binary to `kanata_cmd_allowed` variant (verified checksum) iff `cmd` hooks
  are used; relax sandbox minimally for the helper path (`/usr/local/bin` exec
  allowed; helpers must not need `$HOME`/session D-Bus since they run as
  `kanata-svc` under `ProtectSystem=strict` + `ProtectHome=true` +
  `NoNewPrivileges=true`).

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

- **Overlay (GNOME extension):** fullscreen transparent actor in `Main.uiGroup`;
  reports monitor geometry + cursor position to the daemon (the daemon needs both:
  geometry for cell centers, cursor pos for REL-delta computation since Linux has no
  absolute warp); renders grid/labels/highlight at 60 FPS from daemon frames.
- **State (`mouseless-state`):** `Normal → Overlay → Subgrid → Click`, plus
  `FreeMode` (custom physics loop, `tokio` fixed-step tick) and drag handling.
  `free_mode_engine` flag selects custom vs kanata-native motion.
- **Settings (`mouseless-gtk`):** grid/appearance editor, monitor assignment,
  keybinding reference, presets (Standard, Quick-Select+Free-Mode), deep links
  (`mouseless://toggle-overlay` etc. via `.desktop` + `x-scheme-handler`), writes
  `config.yaml` + kanata include then triggers kanata `Reload` + daemon reload.
- **Contracts:**
  - kanata → daemon: TCP `LayerChange` subscription; daemon → kanata: `ChangeLayer`,
    `RequestCurrentLayerName/Info` (resync), `Reload*` (config apply).
  - daemon → extension: `Show(GridModel)`, `HighlightCell`, `ShowSubgrid`, `Hide`
    over Unix socket, versioned JSON framing.
  - GUI/CLI → daemon: `Reload`, `Update(ConfigData)`; CLI also speaks kanata TCP
    for layer commands.

---

## 5. Race-Avoidance Rules (normative — no boot-order dependency)

1. **Never grab at startup.** Exclusive grab of `kanata-mouseless` happens only
   while the overlay is visible, via an RAII `CellGrab` guard whose `Drop`
   implementation always releases the grab and best-effort notifies hide.
2. **No `eventN` hardcodes.** Discover the kanata output by stable
   `linux-output-device-name` + inotify rescan on hotplug/restart.
3. **TCP reconnect with backoff + resync.** Exponential-backoff reconnect and
   `RequestCurrentLayerName` on every (re)connect; duplicate/missed `LayerChange`
   events are idempotent (single-flight overlay state machine, one mutex,
   idempotent show/hide).
4. **`EBUSY` is retried, never fatal.** Grab attempts use timeout + retry; failure
   degrades to "overlay unavailable, typing unaffected".
5. **Supervision without coupling.** Daemon unit is `After=kanata.service` but
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
  kanata with a failure code (`Restart=on-failure` restarts it).
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
| 1 | Tap-vs-hold (`ShiftLeft` tap <200 ms vs `Shift+key`) | False overlays / leaked Shifts | Fake-clock property tests: tap/hold/chord/interrupt sequences |
| 2 | Temp-grab lifecycle | Stuck grab = dead keyboard | Fake-evdev tests: show/hide, error mid-overlay, panic, kill-switch → assert release on EVERY path |
| 3 | Layer↔daemon↔overlay sync | Ghost overlay / desync after kanata restart | Fake-TCP-server tests: missed/duplicate `LayerChange`, reconnect resync |
| 4 | Cell→pixel incl. scale (100/125/150/200%, 1080p/4K, vertical) | Clicks land off-target | Golden geometry tests per layout |
| 5 | REL-delta from extension cursor pos | Overflow/clamp/multi-monitor offset bugs | Unit tests incl. negative/large deltas, per-monitor offsets |
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
  `ShiftLeft`-tap trigger, `cmd_allowed` swap + `danger-enable-cmd` iff show/hide
  hooks are used; `--emergency-exit-code 1`; verify `LayerChange` latency.
- [ ] **1.2 Thin daemon:** TCP subscriber + resync, RAII temp-grab cell reader,
  `mouseless-core` grid math, uinput mouse deltas + `BTN_*`, overlay IPC server,
  `mouseless-cli` (`show/hide/toggle`, `--list-input-devices`).
- [ ] **1.3 GNOME overlay:** fullscreen Clutter actor, geometry + cursor-pos report,
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
   succeeds; kanata does not hold its own output).
2. TCP `LayerChange` round-trip latency kanata↔daemon (target <5 ms localhost).
3. `cmd_allowed` binary checksum + `cmd` show/hide latency (only if hooks used).
4. Extension cursor-pos + geometry report accuracy at 100/150/200% scale.
5. `apt install libgtk-4-dev libadwaita-1-dev` for the Rust GTK build.
6. Permissions path: `mouseless` group + udev `uinput` rule + `input` membership
   (same as upstream), scoped to temp-grab + uinput mouse (no permanent grab).
