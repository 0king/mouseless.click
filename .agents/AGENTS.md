# Agents & Architecture Roles for Mouseless Development

This document defines the specialized agent roles, domain boundaries, contracts,
and collaboration workflows for **Mouseless Linux** (kanata-fronted edition).
Authoritative architecture: [plan.md](file:///.agents/plan.md).
Operational instructions: [skills.md](file:///.agents/skills.md).

---

## 1. Agent Overview & Division of Responsibilities

```
+─────────────────────────────────────────────────────────────────────────────+
|                         COORDINATION / LEAD AGENT                           |
|      Maintains architecture alignment, feature parity, and overall roadmap  |
|      Owns trait boundaries, backend-swap readiness, and the test matrix     |
+─────────────────────────────────────────────────────────────────────────────+
        │                   │                   │                    │
        ▼                   ▼                   ▼                    ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
| Kanata &     |    |    State     |    |   Overlay    |    | GTK UI &     |
| Keyboard     |    |  & Physics   |    |  & Wayland   |    | Config/QA    |
| Backend      |    |              |    |              |    |              |
└──────────────┘    └──────────────┘    └──────────────┘    └──────────────┘
```

The old "Input Interception" agent (custom evdev keyboard engine) is dissolved:
kanata owns interception. The old "Slint UI" agent is replaced by the GTK agent.

---

## 2. Specialized Roles

### 2.1 Kanata & Keyboard Backend Agent (`mouseless-kbd`, `platform/`)
- **Domain:** Kanata configuration, TCP protocol, `cmd` policy, device identity.
- **Responsibilities:**
  - Own `unicfg.kbd` layers: base/HRM preservation, `lyr-mouseless` (clean no-HRM),
    `ShiftLeft`-tap trigger, `esc` exit path; `lyr-mouse` maintained until post-MVP
    removal (behind `free_mode_engine` flag).
  - Own `defcfg`: `linux-output-device-name "kanata-mouseless"`,
    `linux-dev-names-exclude ("mouseless-mouse")`, `keyboard-only` detect mode.
  - Own service surface: TCP `-p 127.0.0.1:port` (localhost only),
    `--emergency-exit-code 1`, `cmd_allowed` binary swap (checksummed) +
    `danger-enable-cmd yes` ONLY for rare show/hide signals — never per-key `cmd`.
    Document sandbox consequences (`ProtectSystem=strict`, `ProtectHome=true`,
    `NoNewPrivileges=true`: helpers run as `kanata-svc`, no `$HOME`/session D-Bus).
  - Own the `KeyboardBackend` trait surface so kanata is swappable
    (`impls/kanata.rs` today; `impls/keyd.rs`, `impls/evdev_raw.rs` tomorrow).
  - Own race-avoidance: stable output-device naming, no `eventN` hardcodes, inotify
    rescan, TCP backoff + `RequestCurrentLayerName` resync, `EBUSY` retry policy.
- **Non-goals:** No permanent grabs, no uinput keyboard passthrough, no tap-hold
  reimplementation, no hardware-priority pipeline (deleted — see plan §1.4).

### 2.2 State Machine & Physics Agent (`mouseless-state`, `mouseless-core`)
- **Domain:** Pure logic: grid geometry, state transitions, free-mode motion.
- **Responsibilities:**
  - Own transitions: `Normal → Overlay → Subgrid → Click`, `FreeMode`, drag flows;
    single-flight overlay state (one mutex, idempotent show/hide).
  - Own `mouseless-core` grid math (cell boundaries, centers, monitor mapping,
    scale-aware: 100/125/150/200%, 1080p/4K, vertical monitors).
  - Own custom free-mode physics (kept per directive): base px/tick, `S/D/F`
    accel, `A` decel, easing curves (`loose`, `natural`, `snappy`, `instant`),
    wheel physics + step sizes, inactivity auto-off — all on a fixed-step tick
    with an injectable `Clock` (real/fake) for deterministic tests.
  - Own mouse-action semantics: clicks, multi-click cadence, modifier clicks,
    drag/drop, cursor hiding.
- **Key rule:** Crates stay I/O-free; geometry inputs (cursor pos, monitor layout)
  arrive as parameters from the daemon, never fetched directly.

### 2.3 Wayland & Overlay Agent (`gnome-extension`, `mouseless-overlay`)
- **Domain:** Compositor integration, rendering. Draw-only — never input capture.
- **Responsibilities:**
  - Own the GNOME Shell extension: fullscreen transparent Clutter actor in
    `Main.uiGroup`; monitor geometry discovery; fractional-scale correctness;
    grid/label/highlight rendering at 60 FPS; idempotent show/hide; Shell-restart
    resilience.
  - Own cursor-position + geometry reporting to the daemon (required: Linux has no
    absolute warp; the daemon emits REL deltas computed from these reports).
  - Own the `OverlayBackend` trait (`impls/gnome.rs` today; `impls/wlr.rs`,
    `impls/x11.rs` later) and the versioned Unix-socket JSON IPC framing.
  - Wrap every IPC/socket handler defensively: a malformed frame or disconnect
    must never crash the Shell (error to logs, toast where appropriate).
- **Non-goals:** No `EVIOCGRAB`, no uinput, no key capture, no physics loop in the
  compositor thread.

### 2.4 GTK UI, Config & QA Agent (`mouseless-gtk`, `mouseless-ipc`, `mouseless-cli`, tests)
- **Domain:** Settings, configuration lifecycle, IPC/CLI, and overall reliability.
- **Responsibilities:**
  - **GTK settings (Rust `gtk4` + `libadwaita`, `Adw.PreferencesWindow`):** grid/
    appearance editor, monitor assignment, keybinding reference + conflict hints,
    presets engine (YAML import/export: Standard, Quick-Select+Free-Mode),
    deep links (`mouseless://` via `.desktop` + `x-scheme-handler`), status
    integration. Document the `libgtk-4-dev libadwaita-1-dev` build prerequisite.
  - **Config lifecycle:** validate-then-apply + rollback for `config.yaml` and the
    kanata include; kanata `Reload*` over TCP; surfaced errors on failure.
  - **IPC & CLI:** versioned JSON protocol (`schema_version`, negotiate/refuse);
    CLI (`show/hide/toggle`, layer commands, `--list-input-devices`); deep-link
    parity with CLI.
  - **QA ownership:** the 10-row regression matrix (plan §8), `deny unwrap` lint
    for daemon hot paths, fuzz corpus for the socket parser, CI gating (no merge
    with red critical-area tests), kill-switch test, release checklist.

---

## 3. Inter-Agent Communication & Contracts

1. **Kanata → Daemon (TCP):** `LayerChange` subscription (localhost only).
   Daemon → Kanata: `ChangeLayer`, `RequestCurrentLayerName/Info` (resync),
   `Reload*` (config apply). Reconnect = backoff + resync; events idempotent.
2. **Kanata → Daemon (`cmd`, rare only):** show/hide hooks via
   `kanata_cmd_allowed` + `danger-enable-cmd`; helper runs as `kanata-svc` and
   speaks over socket — never touches session D-Bus/display. Per-key `cmd`
   forbidden.
3. **Daemon → Kanata output (evdev, temp-grab only):** RAII `CellGrab` held solely
   while overlay is visible; `Drop` always releases. `EBUSY` → retry with timeout.
4. **Daemon → Mouse (uinput):** `move_rel(dx, dy)`, `click(btn)`, `scroll()`
   on `mouseless-mouse` (kanata-excluded). Deltas computed from extension reports.
5. **Daemon ↔ Overlay (Unix socket, versioned JSON):**
   `Show(GridModel)` / `HighlightCell` / `ShowSubgrid` / `Hide` + geometry/cursor
   reports upstream. Mismatch → negotiate or refuse with clear error.
6. **Config → Daemon/Kanata:** `Reload`, `Update(ConfigData)`; validate-then-apply
   + rollback on either side.
7. **Kill switch:** kanata EOF/reset → daemon releases grab, best-effort hide,
   co-exits failure code (supervisor restarts stack in order).

---

## 4. Trait-Boundary & Modularity Rules (all agents, enforced in review)

1. No `kanata`/`evdev`/`gtk`/`clutter` imports outside `impls/` or the owning crate.
2. New distro/compositor/backend = new `impls/` module + `platform/` entries +
   config value. If a change touches core to add a backend, the abstraction is wrong.
3. `platform/` owns OS specifics (udev, units, `.desktop`, Shell API targets).
4. Critical-area tests (plan §8) are blocking: red means no merge.

---

## 5. Work Phases & Deliverables

| Phase | Lead Domain | Deliverable |
|---|---|---|
| **Phase 1** | Kanata + Daemon | TCP + stable output name + excludes; temp-grab cell reader; uinput mouse; CLI |
| **Phase 1** | Overlay | Draw-only Clutter overlay + geometry/cursor reports |
| **Phase 1** | Integration | E2E MVP: tap Shift → overlay → 2 keys → click, zero leaks; tests #1–#6, #8 green |
| **Phase 2** | State & Physics | Custom free-mode loop behind flag; nudging, drag, multi-click, modifiers |
| **Phase 3** | GTK & Config | Settings app, presets, deep links; reload + rollback (test #9) |
| **Phase 4** | Backends | New `KeyboardBackend`/`OverlayBackend` impls; AT-SPI label mode (experimental) |
| Always | QA | 10-row matrix green; kill-switch test; `deny unwrap`; fuzz corpus current |
