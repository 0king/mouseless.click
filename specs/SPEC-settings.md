# SPEC-settings — GTK4 settings & config lifecycle (`mouseless-gtk`, `ui/`)

> Module: `settings` · Paths: `crates/mouseless-config`, `crates/mouseless-gtk`,
> `ui`, config files · Depends on: `ipc`, `platform` · Plan refs: §1.6, §4,
> §8 #9, §10.5 · Matrix row: #9 (owner, with `platform`)
> Status as of 2026-11-04: **Implemented.** The GTK-free `mouseless-config`
> crate owns schema v1, validation, presets, atomic apply/rollback, and the
> kanata include renderer + check gate. The daemon is the persistence writer:
> `Reload`/`Save`/`Update`/`Status`/`GetConfig` over `mouseless-ipc`, config under
> `/var/lib/mouseless/config.yaml` (`StateDirectory=mouseless`), and
> `mouseless reload`/`status` exercise the same path. Grid geometry, behaviour,
> free-mode, appearance, and monitor assignment are applied live; appearance
> travels in each `GridModel`, while `Report` carries the optional connector
> names used by `custom`. `mouseless-settings` (GTK4/Adwaita, built
> programmatically in `crates/mouseless-gtk`) edits the daemon-owned document,
> validates every change locally, and auto-saves it with `Save` (persist only);
> the running app keeps its previous values until “Restart App” sends `Update`.
> `mouseless://settings/<page>` deep links open the matching page.

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

- Path: the **authoritative runtime copy** is
  `/var/lib/mouseless/config.yaml`, written only by `mouselessd` (systemd
  `StateDirectory=mouseless`, mode `0750`) so it survives restarts and is
  readable without the user's session. The settings app never reads or writes a
  separate user-level document: it fetches the saved document with `GetConfig`,
  auto-saves edits with `Save` (persist only), and applies them to the running
  app with `Update` when the user presses Restart App. `ConfigStore::user_default()`
  still resolves `${XDG_CONFIG_HOME:-~/.config}/mouseless/config.yaml` for
  library users and locates user presets at `~/.config/mouseless/presets/`, but
  it is not part of the running daemon's path. See OQ-2.
- Top-level `config_version: 1` is required. Unknown keys are rejected on load
  (fail loudly, fall back to last-good). Missing sections/fields take the
  documented defaults, so a minimal file is just `config_version: 1`.
- Implementation: `mouseless-config` (no GTK dependency) owns the types,
  validation, presets, atomic store, and kanata include renderer.

```yaml
config_version: 1

grid:
  # Level-1 keys, left-to-right. Any 5-100 unique ASCII keys; space and the
  # full string "esc" are reserved. "asdfghjkl;" is only the default.
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
  tap_threshold_ms: 300
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
  theme: dark                         # dark | light
  grid_color: "#d4de04"               # dark theme grid fill
  grid_opacity: 0.2                   # 0.0..=1.0 (0.2 = 80% transparent)
  label_color: "#ebe9e8"              # dark theme cell labels
  highlight_color: "#ffb22c"
  light_grid_color: "#c9aa03"         # light theme grid fill
  light_label_color: "#000000"        # light theme cell labels
  light_highlight_color: "#ffb22c"    # light theme highlight (defaults to dark)
  light_grid_opacity: 0.2             # light theme grid opacity
  hide_cursor_on_click: false
  cursor_hide_corner: top-right       # top-left | top-right | bottom-left | bottom-right

keybindings:
  overlay_layer: lyr-mouseless
  free_mode_layer: lyr-free
  kanata_free_mode_layer: lyr-mouse
  exit_key: esc
  action_key: space
  monitor_switch_key: ctrl
  drag_modifier: alt
```

The `theme` field selects which color set the daemon renders (`dark` or `light`; default `dark`). Each theme has its own grid/label/highlight colors and opacity, all editable in the Appearance page. While an overlay session is active, pressing `Tab` toggles the theme in-session and re-renders the live grid with the new colors; the session stays alive and `Tab` is not persisted to `config.yaml` (the default theme is set here and applied on the next daemon update/restart).

Semantic validation (all reported as field-tagged issues, shown 1:1 in the GUI):

- `config_version` must equal 1.
- Column keys are 5–100 unique ASCII characters; `space` and the full string
  `esc` are reserved. Row keys are unique and drawn from the QWERTY block
  (which contains neither `esc` nor `space`).
- Subgrid dimensions are non-zero, `columns * rows == keys.len()`, keys are
  unique, and none is `esc`/`space`.
- Colors are `#rrggbb`; `grid_opacity` is finite in `0.0..=1.0`.
- Thresholds/ranges: tap 50–1000 ms, multi-click ≤ 5000 ms, nudges 1–16,
  auto-off ≤ 3600 s, positive finite speeds/multipliers, wheel steps 1–100.
- `custom_monitors` is non-empty iff `monitor_assignment: custom`; names are
  non-empty and unique.
- Layer names are non-empty and distinct; `exit_key`/`action_key` do not
  collide with any cell key (MVP conflict rule, OQ-4).

Runtime monitor selection: `auto` uses the monitor under the cursor and falls
back to the primary; `single` always uses the compositor-reported primary;
`custom` uses the first connector in `custom_monitors` that the extension
reports, then falls back to the primary when none is present. The extension
reports connector names via `Report`; a compositor that cannot provide them
therefore degrades `custom` to the primary instead of failing the session.

### Application config (`mouseless.toml`) — added 2026-11-04

`config.yaml` remains the user-editable document. A separate TOML file holds
application-level constants that are not user settings:

- Path: `/etc/mouseless/mouseless.toml` (0644), installed by
  `platform/install-system.sh`. `mouselessd`, `mouseless`, and
  `mouseless-settings` accept `--config PATH` and honor `MOUSELESS_CONFIG`.
  Precedence: `--config` → `MOUSELESS_CONFIG` → installed path → built-in
  defaults. A missing installed file falls back to defaults; any file that
  exists but is invalid, and any file named explicitly through
  `--config`/`MOUSELESS_CONFIG` that is missing or invalid, is reported and the
  binary exits `2` rather than silently applying a different document.
- Sections: `[paths]` (state dir, `config.yaml` name, socket, kanata main
  config, `/dev/uinput`, input listing), `[kanata]` (address,
  connect/query timeouts, include name), `[daemon]` (loop timeouts and
  closed-loop movement tuning, debug flags), `[devices]` (kanata output/mouse
  names and vendor id), `[layers]` (base layer), `[deep_link]`
  (scheme, settings binary), `[ui]` (GTK app id), `[appearance]` (fallbacks),
  `[ipc]` (client timeout).
- Validation (`mouseless-config::appconfig`): non-empty paths/names, relative
  `config_file` without `..`, `HOST:PORT` with a non-zero port, positive
  timeouts/limits, `#rrggbb` colors, opacity in `0.0..=1.0`, a valid URI
  scheme, and `deny_unknown_fields` for typo detection.
- `[appearance]` is a fallback only: an existing `config.yaml` appearance
  section wins. It seeds the overlay when no settings document exists and the
  GTK form/preview initial values.
- The GNOME extension cannot parse TOML; `install-extension.sh` generates
  `config.js` (socket path + appearance fallback) from this file before packing.
- The file is read once at process start; edits apply after a restart. The
  daemon remains the only writer of `config.yaml`.

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
  → serialize candidate config
  → validate (schema + semantic: keys cover cells, no binding conflicts, values in range)
  → debounce (flushed when the window closes), then push `Save` to the daemon
    (persist only)
  → daemon validates again and persists atomically under /var/lib/mouseless
    (last-good backup); the running app keeps its previous values
  → invalid draft: nothing persisted, the footer shows the first issue inline
    and closing the window offers “Keep Editing” or “Discard and Close”
  → click “Restart App” (or Ctrl+Enter):
      → render the include for kanata-affecting changes → `kanata --check`
        against a staged copy (never writes /etc)
      → push `Update` to the daemon
      → daemon validates again, persists, then swaps the live runtime config
  → on ANY restart failure: the daemon replies `Error`, keeps the previous
    config live, and the GUI surfaces the message (dialog/toast)
```

Never write the live config in place; never leave a partially-applied state.
Auto-save and live apply are deliberately separate so editing is non-destructive.

Implemented split:

| Layer | Owner | Responsibility |
|---|---|---|
| `mouseless-config` | shared crate | schema, validation, presets, atomic store, kanata render/check |
| `mouselessd` | daemon | sole persistence writer (`StateDirectory`), runtime apply, `Ack`/`Error`/`Config`; `Save` persists only, `Update` persists + applies |
| `mouseless-gtk` | GUI | edit, local validate, debounced `Save`, kanata check + `Update` on Restart App, show status/errors |
| `mouseless` CLI | CLI | `reload`, `status`, `settings` routing against the same protocol |

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

`mouseless://settings` and `mouseless://settings/<page>` route to the GUI.
`mouseless-url-handler.desktop` calls `mouseless %u`; the CLI parses the link
as `Verb::Settings(page)` and spawns `mouseless-settings` with it (the
`MOUSELESS_SETTINGS` environment variable overrides the binary path for
development). The app is a single-instance `AdwApplication`: a link that
arrives while it is running activates the existing window and switches to the
requested page instead of starting a second process.

Pages: `grid` (default, also `mouseless://settings/grid`), `appearance`,
`behavior`, `free-mode`, `keybindings`, `presets`, `status`. Overlay and
free-mode verbs keep routing to the CLI (parity table in `SPEC-cli.md`).
`mouseless-settings.desktop` in the application menu opens the default page.
Unknown pages are usage errors: the CLI exits `2` without side effects. A bad
argument forwarded to an already-running instance is rejected by the receiving
process with exit `2`; the open window keeps its current page.

### Status integration

Show: kanata TCP state (layer), daemon reachable, overlay client connected,
uinput device present — sourced from the same queries as `mouseless status`.

## Tech stack

Rust `gtk4` 0.11 (crate `gtk4`, aliased to `gtk`) and `libadwaita` 0.9
(`v1_5`), all inside `crates/mouseless-gtk`. The widget tree is typed Rust
built in code, not Blueprint: `blueprint-compiler` is not a system package on
every host and the `gtk-blueprint` crate is GPL-3.0, which the workspace does
not accept; `ui/README.md` records the deviation. Build prerequisite:
`apt install libgtk-4-dev libadwaita-1-dev` (plan §10.5). The config engine
lives in the separate, GTK-free `mouseless-config` crate so matrix #9 runs in
plain cargo tests.

## Commands

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev   # prerequisite for the GTK app
cargo test -p mouseless-config                   # config engine (no GTK needed)
cargo test -p mouseless-ipc -p mouseless-cli     # protocol + reload/status/settings routing
cargo test -p mouseless-gtk                      # toolkit-free helpers under the UI
cargo run -p mouseless-gtk -- mouseless://settings/appearance
mouseless settings appearance                    # installed CLI opens the same page
```

## Project structure

- `crates/mouseless-config/` — schema, validation, presets, atomic store,
  kanata renderer/checker (pure, headless).
- `crates/mouseless-gtk/src/main.rs` — single-instance app, command-line page
  routing, help/version.
- `crates/mouseless-gtk/src/pages.rs` — the seven pages, action groups, preset
  rows, status widgets.
- `crates/mouseless-gtk/src/form.rs` — editable Adwaita rows, `to_config`,
  `load`, field hints.
- `crates/mouseless-gtk/src/preview.rs` — Cairo subgrid preview.
- `crates/mouseless-gtk/src/state.rs` — load/auto-save/restart/revert/presets/status.
- `crates/mouseless-gtk/src/model.rs` — toolkit-free helpers + unit tests.
- `ui/README.md` — why the UI is programmatic and how the files fit.
- `platform/desktop/mouseless-url-handler.desktop`, `mouseless-settings.desktop`
  — scheme handler and app launcher.

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
- Appearance/monitor wiring: legacy `GridModel` payloads default appearance,
  `Report` accepts missing connectors, and daemon tests cover `auto`/`single`/
  `custom` selection plus configured appearance on both grid models. *(exists)*
- GUI smoke: manual checklist (Phase 3): edit → auto-save → Restart App → live
  update; import/export preset round-trip; deep link opens correct page;
  kill-switch chord visible. Automated coverage today: `mouseless-gtk` unit
  tests for paths, key-conflict hints, colour/opacity formatting, the kanata
  gate, status formatting, and the save/restart wire paths (`Save` and
  `Update` → Ack/Error/unreachable against a fake daemon); CLI tests for
  settings deep-link routing/argument forwarding; daemon tests for
  `save_candidate` persistence without runtime apply; plus a live launch smoke
  on every settings page (clean stderr, window stays up).
  Smoke status 2026-11-04: live `GetConfig` → `Update` → invalid `Update`
  round-trip against a private daemon passed (the invalid update left the
  previous config live), the GUI launched with the daemon up and down, and
  `mouseless settings <page>` forwarded the page. A human click-through of
  auto-save → Restart App and the preset import/export dialogs is still
  pending.

## Boundaries

- **Always:** validate before auto-save or apply; validate-then-apply + rollback
  on Restart App; last-good copy retained; parity with CLI/deep links; keep the
  config engine GTK-free.
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
- [x] Daemon persists the last saved/applied config
      (`StateDirectory=mouseless`) and serves
      `Reload`/`Save`/`Update`/`Status`; CLI `reload`/`status` parity.
- [x] GTK app (Adwaita window/pages) over the engine — `mouseless-settings`,
      programmatic GTK4/Adwaita; `cargo test/clippy -p mouseless-gtk` green.
- [x] Deep links open the right page; parity with `SPEC-cli.md` table
      (`Verb::Settings`, `MOUSELESS_SETTINGS`, single-instance activation).
- [x] Kill-switch chord (`LCtrl+Space+Esc`) documented in the GUI (Status page
      and the keybinding reference).
- [ ] Manual smoke checklist green on Ubuntu 26.04: auto-save → Restart App,
      preset import/export round-trip, deep link, daemon-down fallback.

## Open questions

1. **Build prerequisite — resolved (2026-11-04).** `libgtk-4-dev` /
   `libadwaita-1-dev` were missing on 2026-10-16 but are now present
   (GTK 4.22.4, libadwaita 1.9.1). The GTK dependency is confined to
   `crates/mouseless-gtk`; the config engine, IPC, daemon, and CLI still build
   and test without GTK. A host without the `-dev` packages can skip
   `mouseless-gtk` and keep the daemon/CLI/extension.
2. **Config ownership — resolved (2026-11-04, revised).** The GUI validates the
   user-facing document, then auto-saves it with `Save`; the daemon is the
   **sole persistence writer** and stores the last-saved copy at
   `/var/lib/mouseless/config.yaml` (`StateDirectory=mouseless`, mode `0750`).
   `Save` does not change the running app; `Update` (Restart App) persists and
   applies. The daemon has no session, so it never reads `~/.config`. On start
   it loads `ConfigStore::effective()` (live → last-good → defaults) and logs
   the fallback reason.
3. **Kanata include writes — resolved (2026-11-04).** The config engine renders
   the include and can run `kanata --check` against a staged copy without
   privileges (`kanata::KanataCheck`), but installing `/etc/kanata/mouseless.kbd`
   and reloading kanata stay with the root `platform/install-system.sh` runbook.
   The daemon never writes `/etc`; `Save` only persists the daemon copy and
   `Update` changes runtime behavior as well.
4. **Keybinding conflict hints.** MVP scope question: which conflicts are
   checked (cell keys vs HRM keys vs modifiers)? Proposed: cell keys vs cell
   keys + reserved keys (`esc`, `space`) only; deeper analysis deferred.
