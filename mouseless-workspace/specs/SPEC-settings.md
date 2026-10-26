# SPEC-settings — GTK4 settings & config lifecycle (`mouseless-gtk`, `ui/`)

> Module: `settings` · Paths: `crates/mouseless-gtk`, `ui`, config files ·
> Depends on: `ipc`, `platform` · Plan refs: §1.6, §4, §8 #9, §10.5 ·
> Matrix row: #9 (owner, with `platform`)
> Status as of 2026-10-18: **Stub** — binary prints a Phase-3 notice; `ui/` has
> only a README. Build prereq missing on this machine (OQ-0).

## Objective

A native GNOME settings app (`Adw.PreferencesWindow`, Rust `gtk4` +
`libadwaita`) that edits grid/appearance/monitor settings, offers presets, and
owns the config lifecycle: validate-then-apply with rollback so a bad config
can never brick input (plan §6, matrix #9).

## Scope

- In (Phase 3): grid config editor (level 1/2/subgrid cols, rows, keys with
  conflict hints), appearance, monitor assignment (`auto` / `single` / `custom`
  per upstream), free-mode parameters, `free_mode_engine = custom | kanata`,
  presets engine with YAML import/export (Standard, Quick-Select+Free-Mode),
  keybinding reference, deep links, status integration, kill-switch chord
  documentation.
- Out: in-overlay config editor (Tab, Phase 4); label mode settings (Phase 4).

## Contracts

### Config file (`config.yaml`, schema v1) — to be created in Phase 3

- Path: `${XDG_CONFIG_HOME:-~/.config}/mouseless/config.yaml` (service-side copy
  under `/etc/mouseless/` only if a daemon running without a user session needs
  it — OQ-2).
- Top-level `config_version: 1`. Unknown keys rejected on load (fail loudly,
  fall back to last-good).
- Sections (MVP): `grid:` (list of grid configs: `level1{cols,rows,keys}`,
  `level2{cols,rows,keys}` or `subgrid: true`, `monitor_assignment:
  auto|single|custom`, `custom_monitors: [names]`), `behavior:` (initial action
  location, grid action level, default action, free mode after move, hide
  cursor on click + location, multi-click threshold, tap threshold),
  `free_mode:` (base move speed, multiplier, easing, auto-off, wheel params,
  engine), `appearance:` (colors/opacity), `keybindings:` (reference data and
  edited bindings that map onto kanata layers).

### Lifecycle (matrix #9)

```text
edit in GUI
  → serialize candidate config (temp file)
  → validate (schema + semantic: keys cover cells, no binding conflicts, values in range)
  → for kanata-affecting changes: regenerate mouseless include → `kanata --check`
  → apply atomically (rename); keep last-good copy
  → trigger reloads: kanata `Reload*` over TCP; daemon `Reload`/`Update` client frame
  → on ANY failure: restore last-good + surface the error in the GUI (toast/banner)
```

Never write the live config in place; never leave a partially-applied state.

### Presets (pinned from upstream)

| Preset | Values |
|---|---|
| Standard | grid action level = `subgrid`, default action = `click`, free mode after move = `false` |
| Quick-Select + Free-Mode | grid action level = `2`, default action = `move`, free mode after move = `true` |

Preset files are the same YAML shape as the corresponding config sections
(import merges, export writes the section). Built-ins are read-only; user
presets are files under `~/.config/mouseless/presets/`.

### Deep links

`mouseless://` scheme handled by a `.desktop` entry (owned by `platform`).
Settings opens the relevant page for verbs that target the GUI; overlay verbs
route to `cli` (parity table in `SPEC-cli.md`). `gnome-settings`-style single
instance via `Gio.Application` flags.

### Status integration

Show: kanata TCP state (layer), daemon reachable, overlay client connected,
uinput device present — sourced from the same queries as `mouseless status`.

## Tech stack

Rust `gtk4` + `libadwaita` (CRATES, when buildable); UI definitions in
Blueprint (`.blp`) under `ui/`. Build prereq: `apt install libgtk-4-dev
libadwaita-1-dev` (plan §10.5). Keep the config engine in a separate module
(`src/config/`) with **no GTK dependency** so matrix #9 runs in plain cargo
tests.

## Commands

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev   # prerequisite
cargo test -p mouseless-gtk                      # config engine tests (no GTK needed)
cargo run -p mouseless-gtk                       # requires the -dev packages
```

## Project structure

- `crates/mouseless-gtk/src/main.rs` — app entry (stub until deps exist).
- `crates/mouseless-gtk/src/config/` — schema, load/save, validate, rollback (pure).
- `ui/*.blp` — window/pages (Phase 3), edited with GNOME Builder or blueprint-compiler.
- `platform/` — `.desktop` files + scheme handler (Phase 3).

## Code style

- Config engine: total functions, typed errors, no GTK types (testable headless).
- GTK layer: bindings/preferences glue only; no business logic.
- Every validation failure produces a message shown 1:1 in the GUI.

## Testing strategy (#9, blocking)

- Schema validate/reject fixtures; rollback restores last-good on write/reload
  failure (temp dirs, fake kanata TCP server).
- Kanata include regeneration: golden output for a given preset; `kanata --check`
  failure path (fake binary) → rollback.
- GUI smoke: manual checklist (Phase 3): edit → apply → live update; import/export
  preset round-trip; deep link opens correct page; kill-switch chord visible.

## Boundaries

- **Always:** validate-then-apply + rollback; last-good copy retained; parity
  with CLI/deep links; keep the config engine GTK-free.
- **Ask first:** config schema changes (bump `config_version`); writing anything
  outside the user config dir; privilege model for kanata include writes (OQ-3).
- **Never:** write the live config in place; apply without validation; require
  root for user-level settings; introduce a second UI toolkit (Slint is deleted).

## Success criteria

- [ ] `config.yaml` schema v1 frozen + documented (this file) before code.
- [ ] Config engine: load/validate/save/rollback with tests; no GTK dep.
- [ ] Presets Standard + Quick-Select+Free-Mode import/export round-trip.
- [ ] Kanata include regeneration + `--check` gate + rollback on failure.
- [ ] Deep links open the right page; parity with `SPEC-cli.md` table.
- [ ] Kill-switch chord (`LCtrl+Space+Esc`) documented in the GUI.
- [ ] Manual smoke checklist green on Ubuntu 26.04.

## Open questions

1. **Build prerequisite.** `libgtk-4-dev`/`libadwaita-1-dev` are not installed
   on this machine yet (plan §10.5/§10.6). Phase 3 cannot start until OQ-0 is
   resolved on the dev box.
2. **Config ownership.** User config (`~/.config/mouseless/`) vs system config
   for a system-level daemon: does the daemon read the invoking user's config
   (no — it has no session), or does the GUI push the config to the daemon via
   `Update(ConfigData)` over the socket? Proposed: GUI is the sole writer; it
   validates locally and pushes `Update` to the daemon; daemon persists the
   last-applied copy under `/var/lib/mouseless/` for restart.
3. **Kanata include writes.** Regenerating `mouseless.kbd` and reloading kanata
   needs write access to `/etc/kanata/` + a service restart/reload. Options:
   (a) GUI writes via a narrow privileged helper/sudo prompt; (b) daemon (user
   `mouselessd`) owns the include and the GUI only sends `Update` (proposed —
   keeps one writer). Decide before Phase 3 implementation; cross-check
   `SPEC-platform.md` OQ-2.
4. **Keybinding conflict hints.** MVP scope question: which conflicts are
   checked (cell keys vs HRM keys vs modifiers)? Proposed: cell keys vs cell
   keys + reserved keys (`esc`, `space`) only; deeper analysis deferred.
