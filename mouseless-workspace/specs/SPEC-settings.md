# SPEC-settings — GTK4 settings & config lifecycle (`mouseless-gtk`, `ui/`)

> Module: `settings` · Paths: `crates/mouseless-config`, `crates/mouseless-gtk`,
> `ui`, config files · Depends on: `ipc`, `platform` · Plan refs: §1.6, §4,
> §8 #9, §10.5 · Matrix row: #9 (owner, with `platform`)
> Status as of 2026-11-04: **Config engine implemented** in the GTK-free
> `mouseless-config` crate (schema v1, validation, presets, atomic
> apply/rollback, kanata include renderer + check gate). The daemon is the
> persistence writer: `Reload`/`Update`/`Status` over `mouseless-ipc`, config
> under `/var/lib/mouseless/config.yaml` (`StateDirectory=mouseless`), and
> `mouseless reload`/`status` exercise the same path. The GTK app and UI are the
> remaining Phase-3 slice and still need `libgtk-4-dev libadwaita-1-dev` (OQ-1).

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

### Config file (`config.yaml`, schema v1) — **frozen 2026-11-04**

- Path: `${XDG_CONFIG_HOME:-~/.config}/mouseless/config.yaml` is the user-facing
  document the GUI edits. The **authoritative runtime copy** is
  `/var/lib/mouseless/config.yaml`, written only by `mouselessd` (systemd
  `StateDirectory=mouseless`, mode `0750`) so it survives restarts and is
  readable without the user's session. See OQ-2.
- Top-level `config_version: 1` is required. Unknown keys are rejected on load
  (fail loudly, fall back to last-good). Missing sections/fields take the
  documented defaults, so a minimal file is just `config_version: 1`.
- Implementation: `mouseless-config` (no GTK dependency) owns the types,
  validation, presets, atomic store, and kanata include renderer.

```yaml
config_version: 1

grid:
  # Level-1 keys, left-to-right. Subset of "asdfghjkl;" (unique).
  columns: "asdfghjkl;"
  # Level-2 keys, top-to-bottom. Subset of the 30-key QWERTY block (unique).
  rows: "qwertyuiopasdfghjkl;zxcvbnm,./"
  subgrid:
    columns: 10
    rows: 3
    keys: "qwertyuiopasdfghjkl;zxcvbnm,./"
  monitor_assignment: auto        # auto | single | custom
  custom_monitors: []             # connector/output names; required for custom

behavior:
  initial_action_location: screen-center   # screen-center | system-cursor
  grid_action_level: subgrid               # subgrid | 2
  default_action: click                    # click | move
  free_mode_after_move: false
  multi_click_threshold_ms: 500
  tap_threshold_ms: 200
  nudges_per_cell: 4
  hold_subgrid_for_nudge: false
  continuous_mode: false
  hold_for_right_button: true
  hold_for_drag: true

free_mode:
  engine: custom                  # custom | kanata
  base_px_per_tick: 10.0
  speed_multiplier: 2.0
  easing: natural                 # loose | natural | snappy | instant
  auto_off_secs: 10
  wheel:
    base_lines_per_tick: 1.0
    speed_multiplier: 2.0
    easing: natural
    step_size: 3
    step_size_large: 10

appearance:
  grid_color: "#d4de04"
  grid_opacity: 0.2               # 0.0..=1.0 (0.2 = 80% transparent)
  label_color: "#ebe9e8"
  highlight_color: "#ffb22c"
  hide_cursor_on_click: false
  cursor_hide_corner: top-right   # top-left | top-right | bottom-left | bottom-right

keybindings:
  overlay_layer: lyr-mouseless
  free_mode_layer: lyr-free
  kanata_free_mode_layer: lyr-mouse
  exit_key: esc
  action_key: space
  monitor_switch_key: shift
  drag_modifier: alt
```

Semantic validation (all reported as field-tagged issues, shown 1:1 in the GUI):

- `config_version` must equal 1.
- Column keys are unique and drawn from `asdfghjkl;`; row keys are unique and
  drawn from the QWERTY block; neither may use `esc`/`space`.
- Subgrid dimensions are non-zero, `columns * rows == keys.len()`, keys are
  unique, and none is `esc`/`space`.
- Colors are `#rrggbb`; `grid_opacity` is finite in `0.0..=1.0`.
- Thresholds/ranges: tap 50–1000 ms, multi-click ≤ 5000 ms, nudges 1–16,
  auto-off ≤ 3600 s, positive finite speeds/multipliers, wheel steps 1–100.
- `custom_monitors` is non-empty iff `monitor_assignment: custom`; names are
  non-empty and unique.
- Layer names are non-empty and distinct; `exit_key`/`action_key` do not
  collide with any cell key (MVP conflict rule, OQ-4).

### Presets (schema v1)

Built-ins are read-only; user presets are files under
`~/.config/mouseless/presets/<slug>.yaml` with the same shape as the
`behavior:` section plus a `name:`:

```yaml
name: Quick-Select + Free-Mode
behavior:
  grid_action_level: 2
  default_action: move
  free_mode_after_move: true
```

Applying a preset replaces `behavior:` and preserves every other section.
Import merges one preset file; export writes one preset file. Built-ins:
`standard` (subgrid/click/false) and `quick-select-free-mode` (2/move/true).

### Lifecycle (matrix #9)

```text
edit in GUI
  → serialize candidate config (temp file)
  → validate (schema + semantic: keys cover cells, no binding conflicts, values in range)
  → for kanata-affecting changes: render the include → `kanata --check` against a
    staged copy (never writes /etc)
  → push `Update` to the daemon over the Unix socket
  → daemon validates again, persists atomically under /var/lib/mouseless
    (last-good backup), then swaps the live runtime config
  → on ANY failure: the daemon replies `Error`, keeps the previous config live,
    and the GUI surfaces the message (toast/banner)
```

Never write the live config in place; never leave a partially-applied state.

Implemented split:

| Layer | Owner | Responsibility |
|---|---|---|
| `mouseless-config` | shared crate | schema, validation, presets, atomic store, kanata render/check |
| `mouselessd` | daemon | sole persistence writer (`StateDirectory`), runtime apply, `Ack`/`Error` |
| `mouseless-gtk` | GUI (pending) | edit, local validate, push `Update`, show status/errors |
| `mouseless` CLI | CLI | `reload`, `status` against the same protocol |

`ConfigStore::apply` performs validate → staged temp → check gate → last-good
backup → atomic rename; `save` is the daemon path (already-validated document,
no kanata gate). Running the kanata gate inside the daemon would require it to
read the root-owned main config, so include installation stays with
`platform/install-system.sh` (OQ-3).

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
libadwaita-1-dev` (plan §10.5). The config engine lives in the separate,
GTK-free `mouseless-config` crate so matrix #9 runs in plain cargo tests.

## Commands

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev   # prerequisite for the GTK app
cargo test -p mouseless-config                   # config engine (no GTK needed)
cargo test -p mouseless-ipc -p mouseless-cli     # protocol + reload/status
cargo run -p mouseless-gtk                       # requires the -dev packages
```

## Project structure

- `crates/mouseless-config/` — schema, validation, presets, atomic store,
  kanata renderer/checker (pure, implemented).
- `crates/mouseless-gtk/src/main.rs` — app entry (stub until deps exist).
- `ui/*.blp` — window/pages (Phase 3), edited with GNOME Builder or blueprint-compiler.
- `platform/` — `.desktop` files + scheme handler (Phase 3).

## Code style

- Config engine: total functions, typed errors, no GTK types (testable headless).
- GTK layer: bindings/preferences glue only; no business logic.
- Every validation failure produces a message shown 1:1 in the GUI.

## Testing strategy (#9, blocking)

- Schema validate/reject fixtures; rollback restores last-good on write/reload
  failure (temp dirs, fake kanata TCP server). *(exists in
  `crates/mouseless-config/src/store.rs`)*
- Kanata include regeneration: golden output for a given preset; `kanata --check`
  failure path (fake binary) → rollback. *(exists;
  `render_include` golden test + fake runner + real `kanata --check` when the
  binary is present)*
- Daemon apply/persist/status path exercised by `mouseless-cli` fake-daemon
  tests. *(exists)*
- GUI smoke: manual checklist (Phase 3): edit → apply → live update; import/export
  preset round-trip; deep link opens correct page; kill-switch chord visible.

## Boundaries

- **Always:** validate-then-apply + rollback; last-good copy retained; parity
  with CLI/deep links; keep the config engine GTK-free.
- **Ask first:** config schema changes (bump `config_version`); changing who owns
  persistence (`/var/lib/mouseless` is daemon-only); privilege model for kanata
  include installs (OQ-3).
- **Never:** write the live config in place; apply without validation; require
  root for user-level settings; introduce a second UI toolkit (Slint is deleted).

## Success criteria

- [x] `config.yaml` schema v1 frozen + documented (this file) before code.
- [x] Config engine: load/validate/save/rollback with tests; no GTK dep.
- [x] Presets Standard + Quick-Select+Free-Mode import/export round-trip.
- [x] Kanata include regeneration + `--check` gate + rollback on failure.
- [x] Daemon persists the last-applied config (`StateDirectory=mouseless`) and
      serves `Reload`/`Update`/`Status`; CLI `reload`/`status` parity.
- [ ] GTK app (Adwaita window/pages) over the engine — blocked on OQ-1.
- [ ] Deep links open the right page; parity with `SPEC-cli.md` table.
- [ ] Kill-switch chord (`LCtrl+Space+Esc`) documented in the GUI.
- [ ] Manual smoke checklist green on Ubuntu 26.04.

## Open questions

1. **Build prerequisite.** `libgtk-4-dev`/`libadwaita-1-dev` are not installed
   on this machine yet (plan §10.5/§10.6). The config engine, IPC, daemon, and
   CLI are complete and tested headless; only the Adwaita app/UI and deep-link
   `.desktop` files wait on the packages.
2. **Config ownership — resolved (2026-11-04).** The GUI validates the
   user-facing document, then pushes `Update(ConfigData)` over the socket; the
   daemon is the **sole persistence writer** and stores the last-applied copy at
   `/var/lib/mouseless/config.yaml` (`StateDirectory=mouseless`, mode `0750`).
   The daemon has no session, so it never reads `~/.config`. On start it loads
   `ConfigStore::effective()` (live → last-good → defaults) and logs the
   fallback reason.
3. **Kanata include writes — resolved (2026-11-04).** The config engine renders
   the include and can run `kanata --check` against a staged copy without
   privileges (`kanata::KanataCheck`), but installing `/etc/kanata/mouseless.kbd`
   and reloading kanata stay with the root `platform/install-system.sh` runbook.
   The daemon never writes `/etc`; `Update` only affects runtime behavior and
   the daemon copy.
4. **Keybinding conflict hints.** MVP scope question: which conflicts are
   checked (cell keys vs HRM keys vs modifiers)? Proposed: cell keys vs cell
   keys + reserved keys (`esc`, `space`) only; deeper analysis deferred.
