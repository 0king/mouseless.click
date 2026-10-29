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

`mouseless-gtk` is a stub until `apt install libgtk-4-dev libadwaita-1-dev`
(plan §10.5); the real `Adw.PreferencesWindow` app lands in Phase 3. The
settings/CLI show/hide verbs and the Phase-1.4 end-to-end acceptance remain
open; see `specs/SPEC-MAP.md` for module status.
