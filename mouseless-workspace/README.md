# Mouseless Linux workspace

Kanata-fronted Mouseless for Ubuntu GNOME (Wayland). Architecture source of
truth: `../.agents/plan.md`; roles: `../.agents/AGENTS.md`; operating rules:
`../.agents/skills.md`. See `ARCHITECTURE.md` for the crate map.

## Layout

- `crates/` — Rust workspace (traits + thin orchestration; backends in `impls/`)
- `gnome-extension/` — draw-only Shell overlay (JS ESM, Shell 45–50)
- `platform/` — udev rules, systemd units, kanata include
- `ui/` — GTK4 Blueprint definitions (Phase 3)

## Build

```sh
cargo test   # all crates, incl. fake-backend regression tests (plan §8)
```

`mouseless-gtk` is a stub until `apt install libgtk-4-dev libadwaita-1-dev`
(plan §10.5); the real `Adw.PreferencesWindow` app lands in Phase 3.
