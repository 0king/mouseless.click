# Changelog

All notable changes to Mouseless are recorded here.

The project uses [Semantic Versioning](https://semver.org/). The single source
of truth is [`VERSION`](VERSION); the Cargo workspace inherits it, and
`scripts/check-version.sh` verifies the CLI/daemon/settings binaries and the
GNOME extension metadata agree with it.

## [Unreleased]

### Added

- Dual light/dark themes: `appearance.theme` (`dark` default, `light`) plus
  independent light-theme grid/label/highlight colors and opacity.
- `Tab` toggles the active theme in-session while an overlay is active and
  re-renders the live grid without ending the session.

## [0.1.0] - 2026-11-05

First tracked release.

### Added

- Two-level primary grid: 10 column keys × 30 row keys, then a 10×3 subgrid
  for the selected cell.
- GNOME Shell 45–50 draw-only overlay extension with versioned NDJSON IPC,
  monitor/cursor reporting, and reconnect replay.
- `mouselessd` daemon: kanata TCP layer subscription, temporary evdev grab,
  uinput pointer output, free-mode physics, monitor switching, and the
  kill-switch co-exit path.
- `mouseless` CLI: show/hide/toggle, layer and free-mode commands, live
  `status`, config reload, deep links, and `--list-input-devices`.
- `mouseless-settings` GTK4/Adwaita app: grid/appearance/behavior/free-mode
  editors, keybinding reference, presets, live preview, status page, and
  validated auto-save with Restart App.
- `mouseless.toml` application configuration for paths, endpoints, timeouts,
  device identity, deep-link scheme, GTK app id, and appearance fallbacks,
  installed as `/etc/mouseless/mouseless.toml`.
- `--version`/`-V` on the CLI, daemon, and settings binaries.

### Fixed

- The overlay now dismisses on a single Esc tap and while the cursor moves;
  the grabbed evdev fd is non-blocking so the session loop keeps servicing
  kanata events, auto-hide timers, and the action tick.
- Selecting a column highlights it by fading the labels of every other
  column, and the emphasis is replayed after a Shell reconnect.
- Keys outside the 30-character cell vocabulary dismiss the overlay instead
  of being silently swallowed by the temporary grab.
- Settings auto-save persists valid edits without applying them live; the
  daemon remains the only writer of `config.yaml`.

[0.1.0]: https://github.com/0king/mouseless.click/releases/tag/v0.1.0
