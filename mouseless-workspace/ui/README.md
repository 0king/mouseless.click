# Settings UI (Phase 3)

The settings app is `crates/mouseless-gtk` (binary `mouseless-settings`). It is
a native GTK4/Adwaita `Adw.PreferencesWindow` with grid/appearance editors,
monitor assignment, a live subgrid preview, behavior/free-mode/keybinding
pages, presets (built-in + user), status, and `mouseless://settings/<page>`
deep links.

## Programmatic UI, not Blueprint

The plan named Blueprint (`.blp`) files for this directory. The implementation
builds the same widget tree in typed Rust under `crates/mouseless-gtk/src/`
instead, because `blueprint-compiler` is not available as a system package on
every host and would add a build-time dependency for anyone compiling the
workspace. Programmatic Rust keeps `cargo build -p mouseless-gtk`
self-contained and lets the UI reuse the toolkit-free `mouseless-config`
types directly. There are no `.blp` files to compile here.

- `src/main.rs` — `AdwApplication` (single instance) and command-line page
  routing.
- `src/form.rs` — editable Adwaita rows and `to_config`/`load`.
- `src/pages.rs` — the seven preferences pages and preset rows.
- `src/preview.rs` — Cairo subgrid preview (grid fill, opacity, labels,
  highlight).
- `src/state.rs` — load → validate → `kanata --check` → daemon `Update`;
  revert, presets, status, Ctrl+Enter apply shortcut.
- `src/model.rs` — toolkit-free helpers with unit tests.

## Runtime contract

The GUI never writes the live config. It reads `GetConfig` from the daemon,
edits the daemon-owned document, and pushes `Update`; the daemon validates,
persists atomically, and applies. When the daemon is down the app falls back
to the last persisted copy (`/var/lib/mouseless/config.yaml`) read-only and
says so on the Status page.

Build prerequisite: `apt install libgtk-4-dev libadwaita-1-dev` (plan §10.5).
The GTK dependency is confined to `crates/mouseless-gtk`; `mouseless-config`
stays GTK-free.
