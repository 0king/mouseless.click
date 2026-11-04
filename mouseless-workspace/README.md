# Mouseless Linux workspace

Kanata-fronted Mouseless for Ubuntu GNOME (Wayland). Architecture source of
truth: `../.agents/plan.md`; roles: `../.agents/AGENTS.md`; operating rules:
`../.agents/skills.md`. See `ARCHITECTURE.md` for the crate map.

## Layout

- `crates/` — Rust workspace (traits + thin orchestration; backends in `impls/`)
- `gnome-extension/` — draw-only Shell overlay (JS ESM, Shell 45–50)
- `platform/` — udev, systemd, sysusers, kanata include, installers, runbook
- `scripts/` — platform checks
- `ui/` — GTK4 Blueprint definitions (Phase 3)
- `specs/` — module specs; index: `specs/SPEC-MAP.md` (acceptance criteria + open questions)

## Build

```sh
cargo test --workspace        # fake-backend regression tests (plan §8)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace
./scripts/check-platform.sh   # unit/udev/kanata/name-sync checks
```

## Install

Full runbook: [`platform/README.md`](platform/README.md).

```sh
cargo build --release --workspace
sudo ./platform/install-system.sh   # binaries, unit, sysusers, udev, kanata
./platform/install-extension.sh     # per desktop user
```

The system installer creates the `mouselessd` user, enables
`mouselessd.service`, and backs up replaced files. The extension installer
packs `ipc.js`/`render.js`, installs, and enables the extension. On Wayland,
log out and back in after installing so GNOME Shell loads it. Use
`sudo ./platform/install-system.sh --dry-run` to preview changes.

## GNOME overlay extension

The Shell 45–50 extension is a draw-only client; `mouselessd` owns the socket
server. It is split into `extension.js`, `ipc.js`, and `render.js`, so the pack
command must include the extras:

```sh
gnome-extensions pack gnome-extension --force -o /tmp \
  --extra-source=ipc.js --extra-source=render.js
gnome-extensions install --force /tmp/mouseless@sonuscape.example.shell-extension.zip
```

If `gnome-extensions enable` returns success but `info` still shows
`Inactive`, check `gsettings get org.gnome.shell disabled-extensions`: GNOME
adds the UUID there on `gnome-extensions install --force`, and while it is
listed `enable` is a silent no-op. `platform/install-extension.sh` now removes
it automatically. On Wayland, log out and back in to load changed extension JS
(the running Shell caches extension modules).

Headless protocol/lifecycle smoke test (private D-Bus session, no real desktop):

```sh
gnome-extension/tests/headless-smoke.sh
```

Dev run against a private socket:

```sh
MOUSELESS_SOCKET=/tmp/mouseless.sock cargo run -p mouseless-daemon -- \
  --kanata 127.0.0.1:7070
```

## Overlay grid (Phase 2.3 / pre-Phase 3)

Tap `ShiftLeft` to show the overlay. The primary grid is 10 columns × 30 rows:

- **Level 1 (column):** `asdfghjkl;` (left → right)
- **Level 2 (row):** the QWERTY letter block, top → bottom:

  ```text
  qwertyuiop
  asdfghjkl;
  zxcvbnm,./
  ```

Every cell is labelled with both keys in that order — the top-left cell is
`AQ`, the cell one column right is `SQ`, and the cell below it is `AW`. Type the
two characters to select a cell; its 10×3 subgrid (keyboard-row layout
`qwertyuiop` / `asdfghjkl;` / `zxcvbnm,./`) opens inside the cell. Type one
subgrid character to click its center. `Space` clicks the selected cell center,
and `Esc` exits.

The kanata overlay layer must emit the full 30-key block plainly; re-apply
`platform/kanata/mouseless.kbd` with the system installer (or copy it to
`/etc/kanata/` and restart `kanata.service`) after pulling this change.

## CLI

`mouseless` switches layers through kanata TCP (`--kanata HOST:PORT` or
`MOUSELESS_KANATA`, default `127.0.0.1:7070`):

```sh
cargo run -p mouseless-cli -- show|hide|toggle
cargo run -p mouseless-cli -- enter-free-mode|exit-free-mode
cargo run -p mouseless-cli -- status
cargo run -p mouseless-cli -- reload
cargo run -p mouseless-cli -- --list-input-devices
cargo run -p mouseless-cli -- --version
```

`status` prints the live kanata layer plus daemon/overlay/session/free-mode and
config-path lines when the daemon socket is reachable (`--socket` or
`MOUSELESS_SOCKET` to override). `reload` asks the daemon to re-read its stored
`/var/lib/mouseless/config.yaml`; an invalid file returns an error and leaves the
previous config running.

Usage errors exit `2`; kanata/daemon operational failures exit `1`. Both
installed binaries accept `--version`/`-V` (for example
`/usr/local/bin/mouselessd --version`).

## Phase status

Phases 1.2/1.3/1.4 and Phase 2 (custom free-mode physics, multi-click,
drag/drop, nudges, modifier clicks, monitor switching, cursor hiding) are
implemented and covered by 164 hermetic tests; the Phase-1.4 host e2e and
visual/fractional acceptance passed on 2026-10-29. The Phase-2 kanata include
is applied on the primary host (kanata on `lyr-base`, TCP `127.0.0.1:7070`),
and live free-mode movement/click behavior was confirmed. The Phase-3 grid
layout is implemented: a 10×30 primary grid (columns `asdfghjkl;`, rows
`qwertyuiopasdfghjkl;zxcvbnm,./`) whose cells are labelled with both keys
(e.g. `AQ`), a 10×3 keyboard-shaped subgrid, and a `#d4de04` grid at 80%
transparency (20% opacity) with `#ebe9e8` labels. The Phase-3 config engine,
presets, atomic apply/rollback, kanata include renderer/check gate, daemon
persistence (`/var/lib/mouseless/config.yaml`), and CLI `reload`/`status` are
implemented and tested headless. The remaining Phase-3 slice is the Adwaita
settings app and deep-link `.desktop` files, which need
`apt install libgtk-4-dev libadwaita-1-dev` (plan §10.5). See
`specs/SPEC-MAP.md` for per-module status.
