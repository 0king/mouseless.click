# Mouseless Linux workspace

Kanata-fronted Mouseless for Ubuntu GNOME (Wayland). Architecture source of
truth: `../.agents/plan.md`; roles: `../.agents/AGENTS.md`; operating rules:
`../.agents/skills.md`. See `ARCHITECTURE.md` for the crate map.

## Layout

- `crates/` — Rust workspace (traits + thin orchestration; backends in `impls/`)
- `gnome-extension/` — draw-only Shell overlay (JS ESM, Shell 45–50)
- `platform/` — udev rules, systemd units, kanata include
- `ui/` — GTK4 Blueprint definitions (Phase 3)
- `specs/` — module specs; index: `specs/SPEC-MAP.md` (acceptance criteria + open questions)

## Build

```sh
cargo test --workspace        # fake-backend regression tests (plan §8)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace
```

## GNOME overlay extension

The Shell 45–50 extension is a draw-only client; `mouselessd` owns the socket
server. It is split into `extension.js`, `ipc.js`, and `render.js`, so the pack
command must include the extras:

```sh
gnome-extensions pack gnome-extension --force -o /tmp \
  --extra-source=ipc.js --extra-source=render.js
gnome-extensions install --force /tmp/mouseless@sonuscape.example.shell-extension.zip
```

Headless protocol/lifecycle smoke test (private D-Bus session, no real desktop):

```sh
gnome-extension/tests/headless-smoke.sh
```

Dev run against a private socket:

```sh
MOUSELESS_SOCKET=/tmp/mouseless.sock cargo run -p mouseless-daemon -- \
  --kanata 127.0.0.1:7070
```

## CLI

`mouseless` switches layers through kanata TCP (`--kanata HOST:PORT` or
`MOUSELESS_KANATA`, default `127.0.0.1:7070`):

```sh
cargo run -p mouseless-cli -- show|hide|toggle
cargo run -p mouseless-cli -- enter-free-mode|exit-free-mode
cargo run -p mouseless-cli -- status
cargo run -p mouseless-cli -- --list-input-devices
```

Usage errors exit `2`; kanata/daemon operational failures exit `1`.

## Phase status

Phases 1.2/1.3/1.4 and Phase 2 (custom free-mode physics, multi-click,
drag/drop, nudges, modifier clicks, monitor switching, cursor hiding) are
implemented and covered by 99 hermetic tests; the Phase-1.4 host e2e and
visual/fractional acceptance passed on 2026-10-29. The live host still needs
the Phase-2 kanata include applied (`platform/kanata/UNICFG-EDITS.md`, requires
sudo) before the free-mode smoke matrix. `mouseless-gtk` is a stub until
`apt install libgtk-4-dev libadwaita-1-dev` (plan §10.5); the real
`Adw.PreferencesWindow` app lands in Phase 3. See `specs/SPEC-MAP.md` for
per-module status.
