# Spec Map — Mouseless Linux workspace

> Initiative: kanata-fronted Mouseless for Ubuntu GNOME (Wayland), per
> `.agents/plan.md`. This map is the index of the module specs; each module id
> below maps 1:1 to `specs/SPEC-<id>.md`.
>
> Status snapshot: 2026-11-04 · `cargo test --workspace` = 184 tests green;
> Phase 1.3 overlay + Phase 2 custom free mode/action semantics implemented
> (headless Shell smoke and Phase-1.4 host e2e/visual acceptance passed).
> Phase-3 grid layout landed: 10 columns × 30 rows primary grid with
> two-character labels, 10×3 subgrid, and config-driven appearance (default
> `#d4de04` at 20% opacity; host visual acceptance pending).
> Phase-3 config engine (`mouseless-config`), presets, atomic apply/rollback,
> kanata render/check gate, daemon persistence, live appearance and monitor
> assignment, CLI `reload`/`status`, the `mouseless://` scheme handler, and the
> `mouseless-settings` GTK4/Adwaita app (including settings-page links) are
> implemented. The operator smoke checklist for the GUI remains manual.
> Packaging (installers, `sysusers`, runbook, platform checks) is complete;
> live free-mode movement/click behavior is confirmed. The final host
> service install waits on interactive sudo.

## Authority & document roles

| Artifact | Role |
|---|---|
| `.agents/plan.md` | Locked decisions, contracts, phase plan, §8 test matrix. **Source of truth.** |
| `specs/SPEC-*.md` (this suite) | Per-module acceptance criteria, frozen contracts, open questions. Traces to plan.md. |
| `ARCHITECTURE.md` | Crate map + data-flow summary. |
| `.agents/AGENTS.md`, `.agents/skills.md` | Agent roles, operating rules. |

Docs discipline (SKILL-09): a behavior change without a matching doc + test
update is incomplete. If a spec ever contradicts plan.md, plan.md wins and the
spec is re-negotiated with the human before code moves.

## Capability map

| Module id | Responsibility | Paths | Depends on |
|---|---|---|---|
| `core` | Pure grid math: cell→pixel, layouts, scale-aware geometry. Zero I/O. | `crates/mouseless-core` | — |
| `state` | Overlay state machine, free-mode physics, mouse-action semantics; `Clock` trait. | `crates/mouseless-state` | `core` |
| `config` | GTK-free schema v1, validation, presets, atomic store/rollback, kanata include render/check. | `crates/mouseless-config` | — |
| `ipc` | Versioned NDJSON wire protocol (daemon ↔ overlay ↔ clients) + socket auth contract. | `crates/mouseless-ipc` | — |
| `kbd` | `KeyboardBackend` + `CellGrab` (kanata TCP subscriber, evdev temp grab). | `crates/mouseless-kbd` | — |
| `mouse` | `MouseBackend` (uinput `mouseless-mouse`) + REL-delta math. | `crates/mouseless-mouse` | — |
| `overlay` | `OverlayBackend` + GNOME Shell extension (draw-only; geometry/cursor reports). | `crates/mouseless-overlay`, `gnome-extension` | `ipc`, `core` |
| `daemon` | `mouselessd`: single-flight orchestration, grab lifecycle, click path, kill switch. | `crates/mouseless-daemon` | all runtime modules |
| `cli` | `mouseless` verbs, deep-link parity, `--list-input-devices`. | `crates/mouseless-cli` | `kbd`, `ipc` |
| `settings` | GTK4/Adwaita settings app, presets, config lifecycle (validate-then-apply + rollback). | `crates/mouseless-gtk`, `ui` | `ipc`, `platform` |
| `platform` | OS specifics: kanata include, systemd units, udev, `.desktop`, distro matrix. | `platform` | — |

## Build order (dependency direction only; phases per plan §9)

```text
core, state, ipc, kbd, mouse, platform      (foundations — mostly scaffolded)
   → overlay                                (Phase 1.3)
   → daemon + cli                           (Phase 1.2 completion)
   → Phase 1.4 e2e MVP acceptance
   → state Phase 2 (physics wiring, action semantics) — implemented
   → settings Phase 3 (GTK, presets, config lifecycle)
   → Phase 4 (keyd/evdev_raw, libei, wlr/x11, AT-SPI labels)
```

## Module status snapshot

| Module id | Status | Primary gap |
|---|---|---|
| `core` | Implemented | Mixed-scale multi-monitor origin e2e (Phase 2.2, manual). |
| `state` | Implemented (Phase 2) | Live-host tuning of easing/auto-off; Phase-4 label mode. |
| `config` | Implemented (Phase 3) | — (GTK-free engine; the GUI is a separate crate). |
| `ipc` | Implemented (v1) | — (`Reload`/`Update`/`Status`/`GetConfig` client messages wired). |
| `kbd` | Implemented (Phase 2) | Live evdev grab/EBUSY smoke after the kanata config is applied. |
| `mouse` | Implemented | Hi-res wheel decision; click-accuracy verified only in e2e (Phase 1.4). |
| `overlay` | Implemented (1.3) | Mixed-scale multi-monitor origin e2e (Phase 2.2, manual). |
| `daemon` | Implemented (Phase 2 + config) | Live free-mode/drag/nudge/monitor/cursor-hide smoke. |
| `cli` | Implemented (Phase 3) | — (deep-link install is owned by `platform`). |
| `settings` | Implemented (Phase 3) | Manual operator smoke: edit → live apply, preset round-trip, daemon-down fallback. |
| `platform` | Packaged (host install pending) | Live `lyr-free` apply + service install (sudo), wider distro matrix. |

## Blocking test matrix → owners (plan §8, SKILL-07)

| # | Critical area | Owner spec(s) | Blocking test |
|---|---|---|---|
| 1 | Tap-vs-hold (`ShiftLeft` tap < 200 ms) | `kbd` + `state` (+ `platform` config assert) | Kanata-config assertion + fake-clock shape — see `SPEC-kbd.md` OQ-1. |
| 2 | Temp-grab lifecycle | `kbd` + `daemon` | Fake-evdev: release on every path (incl. panic/kill); `EVIOCGKEY` pre-check; drain; fresh fd per overlay. |
| 3 | Layer ↔ daemon ↔ overlay sync | `kbd` + `daemon` | Fake-TCP: missed/duplicate `LayerChange`; reconnect resync. |
| 4 | Cell→pixel incl. scale | `core` | Goldens: 100/125/150/200%, 1080p/4K, vertical, offsets. |
| 5 | REL-delta + closed-loop movement | `mouse` + `daemon` | Clamp/offset unit tests; cross-monitor, acceleration-gain damping, stale-reply, and stalled-cursor regressions. |
| 6 | Loop prevention | `platform` + `kbd`/`mouse` | Both startup orders; exclusion assertions (output name, mouse name). |
| 7 | Free-mode tick determinism | `state` | Fixed-step goldens per easing curve (`loose/natural/snappy/instant`). |
| 8 | IPC schema + malformed frames | `ipc` + `daemon` | Version negotiation; fuzz corpus; never panic. |
| 9 | Config reload (YAML + kanata include) | `settings` + `platform` | Validate-then-apply + rollback; kanata `Reload` failure path. |
| 10 | Kill-switch path | `daemon` + `kbd` | Fake-TCP drop mid-overlay → grab release + hide + exit code. |

## Shared conventions

### Commands (workspace level)

```sh
cargo test --workspace                 # unit + fake-backend tests (hermetic)
cargo test -p mouseless-core           # single module
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo build --release --workspace
```

Dev run without system install:

```sh
MOUSELESS_SOCKET=/tmp/mouseless.sock cargo run -p mouseless-daemon -- --kanata 127.0.0.1:7070
```

System install/verify commands live in `SPEC-platform.md`.

### Project structure

- `crates/` — Rust workspace: traits + thin orchestration; concrete backends in `impls/`.
- `gnome-extension/` — draw-only Shell overlay (JS ESM, Shell 45–50).
- `platform/` — kanata include, systemd units, udev rules, `.desktop` (Phase 3).
- `ui/` — GTK4 Blueprint definitions (Phase 3).
- `specs/` — this suite.

### Code style

- Rustdoc on every public item; module docs cite plan § / audit ids (G1, W2, C1, …).
- `thiserror` error enums per crate; `anyhow` only at process boundaries (binaries).
- Every backend trait ships a `Fake*` impl under `impls/` for hermetic tests.
- `serde` derives on wire/config types; no `unwrap`/`expect` in daemon hot paths (CI deny).
- Comments explain *why* with a traceable reference (test name, audit id, plan §).

### Testing strategy

- Inline `#[cfg(test)] mod tests` per module; fakes for all I/O.
- Unit tests are hermetic: no real `/dev/input`, no system sockets (ephemeral
  TCP listeners are allowed for kanata-protocol tests).
- The §8 matrix above is blocking per SKILL-07: red = no merge.

### Boundaries (workspace-wide)

- **Always:** run `cargo test` + `clippy` before commit; update spec/plan/ARCHITECTURE
  on behavior change; add a fake when adding a backend trait; cite audits in comments.
- **Ask first:** bumping `schema_version` or changing `SOCKET_PATH`; changing the
  socket auth model; adding workspace dependencies; changing kanata `defcfg` or
  unit-file contracts; changing `platform/` install paths.
- **Never:** any `cmd` path (audit W1); permanent keyboard grabs; the
  hardware-priority pipeline (plan §1.4); `/dev/input/eventN` hardcodes;
  `unwrap`/`expect` in daemon hot paths; fake backends on production paths;
  a second input engine (SKILL-01); skipping blocking tests.

## Module index

| Spec | One-liner |
|---|---|
| [`SPEC-core.md`](SPEC-core.md) | Grid math: cell↔pixel, subgrids, scale, monitors. |
| [`SPEC-state.md`](SPEC-state.md) | Machine + physics + action semantics; `Clock`. |
| [`SPEC-ipc.md`](SPEC-ipc.md) | Wire spec (audit G2): envelope, messages, auth, lifecycle. |
| [`SPEC-kbd.md`](SPEC-kbd.md) | Keyboard backend + temp-grab contract (G6/G7). |
| [`SPEC-mouse.md`](SPEC-mouse.md) | uinput mouse + delta math + loop prevention (G1). |
| [`SPEC-overlay.md`](SPEC-overlay.md) | GNOME extension + overlay backend (audits C2). |
| [`SPEC-daemon.md`](SPEC-daemon.md) | Orchestration, click path, kill switch (W2). |
| [`SPEC-cli.md`](SPEC-cli.md) | Verbs + deep-link parity. |
| [`SPEC-settings.md`](SPEC-settings.md) | GTK4 settings, presets, config lifecycle. |
| [`SPEC-platform.md`](SPEC-platform.md) | udev/systemd/kanata/desktop OS layer. |
