# SPEC-platform — OS layer: kanata, systemd, udev, desktop (`platform/`)

> Module: `platform` · Paths: `platform/` · Depends on: — ·
> Plan refs: §2.3, §5.6, §7, §8 #5/#6, §10.6 · Audits W2, G1 ·
> Matrix rows: #1 (config assertion), #5 (udev rule), #6 (exclusions), part of #9
> Status as of 2026-11-04: **Packaging complete, host install pending** — kanata
> include (overlay + `lyr-free`), service drop-ins, apply script, udev rules,
> daemon unit, `sysusers.d`, system/extension installers, runbook,
> `scripts/check-platform.sh`, and the `mouseless://` scheme handler exist and
> pass local checks. The GTK settings entry and the wider distro matrix remain
> Phase 3/4.

## Objective

All OS/compositor specifics live here, never in crates (plan §3 rule 4): kanata
include + service drop-in, systemd units, udev rules, `.desktop` deep links,
permissions, and the distro test matrix. Every file is installable and
verifiable by a scripted runbook.

## Scope

- In: kanata `/etc/kanata/mouseless.kbd` + `UNICFG-EDITS.md` + `apply-kanata-config.sh`;
  `kanata.service.d` override; `mouselessd.service`; `99-mouseless.rules`;
  `sysusers.d` for `mouselessd`/`mouseless`; system + extension installers;
  runbook (`platform/README.md`); platform check script; distro matrix notes;
  Phase 3 `.desktop` files.
- Out: Rust config parsing (`settings`), IPC (`ipc`), kanata semantics (`kbd`).

## Contracts

### File inventory

| File | Install path | Purpose |
|---|---|---|
| `kanata/mouseless.kbd` | `/etc/kanata/mouseless.kbd` | `ml-tap` / `ml-free-tap` (`tap-hold-release 200 200` → `lyr-mouseless` / `lyr-free`), clean overlay layer (level-1 columns `a s d f g h j k l ;` + every level-2 key `qwertyuiop / asdfghjkl; / zxcvbnm,./` emitted plainly, `spc`, `esc`→base), and the custom free-mode layer (`i/k/j/l`, boosts, buttons, wheel). |
| `kanata/UNICFG-EDITS.md` + `apply-kanata-config.sh` | runbook (script backs up both files, edits, `kanata --check`, restores on failure, restarts) | The 4 unicfg edits: defcfg output name + mouse exclude, `@ml-tap` and `@ml-free-tap` in `lyr-base`, `(include "mouseless.kbd")`. |
| `systemd/kanata.service.d-override.conf` | `/etc/systemd/system/kanata.service.d/mouseless.conf` | TCP `-p 127.0.0.1:7070`, `--emergency-exit-code 1`, `Restart=on-failure` + `RestartForceExitStatus=SIGTERM` (audit W2). `SupplementaryGroups=input uinput` — additive/idempotent (a no-op when kanata-svc already has them, a fix when it does not); kanata never joins the human `mouseless` socket group. |
| `../mouseless.toml` | `/etc/mouseless/mouseless.toml` | Application-level constants (paths, kanata endpoint, timeouts, device names, base layer, deep-link scheme, app id, appearance fallbacks). Read by all binaries via `--config`/`MOUSELESS_CONFIG`/default path. The installer writes `[devices]` names and `[kanata] address` into the defcfg/drop-in, then validates with `kanata --check`; re-run the installer after changing them. |
| `systemd/mouselessd.service` | `/etc/systemd/system/mouselessd.service` | `After=kanata.service`, no `Requires=`; `ExecStart=/usr/local/bin/mouselessd` (endpoint comes from `mouseless.toml`, never the unit); `User=mouselessd`, `Group=mouseless`, `SupplementaryGroups=input uinput`; `RuntimeDirectory=mouseless` (0770), `UMask=0007`; `Restart=on-failure`, `StartLimitIntervalSec=0`; sandboxing (`ProtectSystem=strict`, empty capability set). |
| `udev/99-mouseless.rules` | `/etc/udev/rules.d/99-mouseless.rules` | `/dev/uinput` (0660) for the existing `uinput` group (kanata + daemon) — never the human `mouseless` group. Pointer accuracy is closed-loop in the daemon, so no libinput accel override is shipped. |
| `sysusers/mouseless.conf` | `/usr/lib/sysusers.d/mouseless.conf` | Creates the `mouseless` (overlay socket) group and system user `mouselessd` (nologin, primary group `mouseless`, supplementary `input` + `uinput`); `g uinput -` keeps kanata's existing group/GID. Idempotent; existing GIDs are preserved. |
| `install-system.sh` | run in place as root | Installs binaries, `mouseless.toml`, unit, sysusers, udev, the scheme handler, docs, and applies the kanata include/drop-in. Reads `[deep_link] scheme` and `[paths] socket` from the TOML; regenerates the `.desktop` handler when the scheme is not `mouseless`. Registers the scheme for `$SUDO_USER` when possible. Backs up replaced files as `<path>.bak.YYYYmmdd-HHMMSS.<pid>`. Options: `--dry-run`, `--skip-kanata`, `--no-group-add`, `--no-start`. |
| `install-extension.sh` | run in place as the desktop user | Generates `config.js` from `mouseless.toml`, packs `extension.js` + `ipc.js` + `render.js` + `config.js`, installs, removes the UUID from `org.gnome.shell disabled-extensions` (GNOME leaves it there after `install --force`, making `enable` a silent no-op), and enables `mouseless@0king.dev`. `--dry-run` supported. |
| `README.md` (this dir) | `/usr/local/share/doc/mouseless/README.md` | Install/verify/upgrade/uninstall runbook and troubleshooting. |
| `scripts/check-platform.sh` | run in place | Asserts the TOML sections and cross-file sync (device names, socket, extension fallback colors, scheme), kanata timings/layers, unit/drop-in/udev/sysusers contents, desktop entry validity, UUID, installer paths, POSIX shell syntax, and `systemd-analyze verify`. |
| `desktop/mouseless-url-handler.desktop` | `/usr/share/applications/mouseless-url-handler.desktop` | `x-scheme-handler/mouseless` → `/usr/local/bin/mouseless %u`; the installer rewrites the scheme from `mouseless.toml`; the CLI accepts exact deep links, so overlay/free-mode/status links need no GUI. |

### Kill-switch semantics (audit W2, cross-ref `SPEC-daemon.md`)

On Linux, kanata's emergency chord (`LCtrl+Space+Esc`) raises `SIGTERM`
directly and ignores `--emergency-exit-code`; hence
`RestartForceExitStatus=SIGTERM` is required or the service would stay dead.
`forceful-exit-code` must never appear in `defcfg` (parse error).
`mouselessd` co-exits on TCP EOF so the supervisor restarts the stack in order.

### Permissions model (least privilege)

- **Desktop user: member of `mouseless` only.** That group grants the overlay
  socket. A human account must never be in `input` or `uinput`: those allow
  keylogging and synthetic input injection for the whole session.
  `install-system.sh` never adds them; it warns if an existing membership is
  found and prints the `gpasswd -d` command.
- **`uinput`** is the existing `/dev/uinput` group (0660) that kanata already
  uses. Members: `mouselessd` and `kanata-svc` only.
- **`input`** grants evdev read + temporary grab. Members: `mouselessd` and
  `kanata-svc` only.
- `kanata.service` runs as `kanata-svc` and lists `SupplementaryGroups=input
  uinput` in the drop-in. systemd extends (never replaces) the account's
  database groups, so this is a no-op where kanata already has both and fixes
  hosts where it does not. Reusing the human `mouseless` group for
  `/dev/uinput` is forbidden: it would let the desktop user inject input.
- Daemon runs as system user `mouselessd`, primary group `mouseless`
  (socket-dir ownership), supplementary `input uinput`.
- Socket dir `/run/mouseless` created by `RuntimeDirectory` (0770).
- No permanent grab; permissions scoped to temp-grab + uinput mouse (plan §10.7).

### Deep links (`mouseless://`)

`desktop/mouseless-url-handler.desktop` installs to
`/usr/share/applications/mouseless-url-handler.desktop` and declares
`MimeType=x-scheme-handler/mouseless;` with
`Exec=/usr/local/bin/mouseless %u`. The CLI accepts both a plain verb and its
exact `mouseless://` link (`Verb::from_deep_link`), so overlay, free-mode,
status, and device-list links work without a GUI. Unknown links exit `2` as
usage errors before touching kanata or the daemon.

`install-system.sh` installs the entry, refreshes the desktop database when
`update-desktop-database` is available, and registers the handler for
`$SUDO_USER` with `xdg-mime default`. Without `SUDO_USER` it prints the
per-user command. Settings-only links (for example `mouseless://settings`) will
be added with the GTK app; until then they are rejected.

### Installer contract

- Both installers are POSIX `sh`, `set -eu`, idempotent, and safe to rerun for
  upgrades. `--dry-run` prints the exact commands and changes nothing.
- The system installer requires root (`sudo`) and fails before mutating anything
  if the release binaries or platform sources are missing. It also refuses to
  start when another process (for example a transient `mouselessd-dev` user
  unit) is already listening on `/run/mouseless/mouselessd.sock`.
- Every replaced file is copied to `<path>.bak.YYYYmmdd-HHMMSS.<pid>` before
  the new copy is installed; backups are never deleted automatically.
- Install order is: sysusers → udev reload/trigger → binaries → unit →
  desktop handler → `daemon-reload` → kanata include/drop-in → desktop-user
  groups → enable and restart `mouselessd`. A failed restart prints
  `systemctl status` + `journalctl` and exits non-zero.
- `--skip-kanata` leaves kanata entirely untouched: no include, no drop-in,
  no `unicfg.kbd` edit, no kanata restart.
- The extension installer refuses root, verifies the UUID, packs `ipc.js` and
  `render.js`, and enables the extension. It does not restart GNOME Shell;
  Wayland requires logout/login.
- Uninstall is documented, not scripted: stop/disable the unit, remove the
  installed files, `daemon-reload`, reload udev, then remove the extension.
  The `mouselessd` user and `mouseless` group may remain harmlessly; leave the
  shared `uinput`/`input` groups to kanata.

### Distro matrix

| Target | Status |
|---|---|
| Ubuntu 26.04, GNOME 50 Wayland | Primary. All acceptance runs here. |
| Ubuntu 24.04, GNOME 46 Wayland | Secondary (extension shell-version range 45–50). |
| Other distros / X11 / wlroots | Phase 4 (new `impls/` + platform entries). |

## Tech stack

POSIX shell for scripts (`set -eu`, backup-before-edit, validate-before-restart);
systemd unit/drop-in syntax; udev rules; desktop entry spec. No Python beyond
the embedded edit script in `apply-kanata-config.sh` (already present, keep stdlib-only).

## Commands

```sh
# build + verify before installing
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace
./scripts/check-platform.sh

# install (system files, then the per-user extension)
sudo ./platform/install-system.sh
./platform/install-extension.sh

# apply kanata config + drop-in alone (backs up + validates; aborts on failure)
sudo ./platform/kanata/apply-kanata-config.sh

# verify:
kanata --check --cfg /etc/kanata/unicfg.kbd
systemd-analyze verify platform/systemd/mouselessd.service
systemctl show kanata -p User -p Group -p SupplementaryGroups
systemctl status mouselessd.service
ss -tln | grep 7070
grep -A2 'Name="kanata-mouseless"' /proc/bus/input/devices
# udev reload after rule edit:
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input
```

## Project structure

`platform/kanata/` (include + runbook + apply script), `platform/systemd/`
(units + drop-ins), `platform/udev/`, `platform/sysusers/`, the two installers
and `platform/README.md` runbook; `scripts/check-platform.sh` asserts the
installed-file contracts. Phase 3 adds `platform/desktop/`.

## Code style

- Every installed file starts with a comment header: purpose, install path,
  related plan § / audit id, and any manual step it cannot automate.
- Scripts: `set -eu`; every mutation preceded by a timestamped backup; every
  validation failure restores the backup and exits non-zero.
- Never hardcode machine-specific paths beyond the documented defaults; note
  overrides inline (e.g. `--cfg` path in the kanata drop-in).

## Testing strategy (scripted assertions; runbook-style)

- #1: `scripts/check-platform.sh` greps the include for
  `tap-hold-release 200 200` on both `ml-tap` and `ml-free-tap`, plain cell keys
  in `lyr-mouseless`, the `lyr-free` movement/button/wheel keys, and absence of
  `cmd`; it greps `apply-kanata-config.sh` for `linux-output-device-name` and
  `linux-dev-names-exclude`. Blocking; runs without root.
- #5: pointer accuracy is covered by the daemon's closed-loop movement
  regressions; the platform script only asserts the `uinput` group rule.
- #6: exclusion assertions: kanata exclude contains `mouseless-mouse`; output
  name equals `mouseless-kbd::KANATA_OUTPUT_NAME` and the uinput name equals
  `UINPUT_DEVICE_NAME` (string cross-check script — single source of truth
  lives in the Rust constants).
- #9 (part): `kanata --check` gate in the apply script + rollback of both
  `unicfg.kbd` and `mouseless.kbd`, proven by feeding a broken candidate in a
  test run.
- `systemd-analyze verify` for both units (where available).
- Manual e2e runbook on the primary target (Phase 1.4).

## Boundaries

- **Always:** backup + validate before any system mutation; keep names synced
  with Rust constants; document manual steps in file headers; keep TCP localhost-only.
- **Ask first:** changing install paths; adding a second systemd unit or service;
  changing the permission model (groups/users); packaging (deb/PPA).
- **Never:** `cmd` / `danger-enable-cmd` / `kanata_cmd_allowed` (audit W1);
  `forceful-exit-code` in `defcfg`; world-readable socket dirs; `Requires=` on
  kanata for the daemon unit (boot-order independence, plan §5 rule 6);
  permanent grabs.

## Success criteria

- [x] Kanata include + runbook + apply script with backup/validate/restart.
- [x] `lyr-free` + `@ml-free-tap` include for custom free mode; validated with
      `kanata --check` on 2026-10-29.
- [x] Apply script replayed against a temp unicfg copy: all four edits land,
      a second run is idempotent, and `kanata --check` passes.
- [x] Apply script creates missing `input`/`uinput` groups first, so systemd
      accepts `SupplementaryGroups=` on a host where kanata has no device
      permissions yet (getent-guarded, idempotent).
- [x] Live `/etc/kanata` apply + kanata restart on 2026-11-01: `kanata.service`
      active with `SupplementaryGroups=input uinput`, TCP listening on
      `127.0.0.1:7070`, `mouselessd.service` active (`uinput mouseless-mouse
      ready`, layer `lyr-base`, overlay hello from extension 0.1.0).
- [ ] Phase-2 smoke matrix (free mode, drag/drop, nudge, monitor switch,
      cursor hide). _Needs hands-on hotkey testing by the desktop user._
- [x] Kanata drop-in with TCP + `SIGTERM` restart semantics (audit W2).
- [x] `mouselessd.service` with `After=` (no `Requires=`), `RuntimeDirectory`, sandboxing.
- [x] udev rules: `/dev/uinput` for the existing `uinput` group (kanata +
      daemon; no accel override; accuracy is closed-loop in the daemon).
- [x] Kanata drop-in re-asserts `SupplementaryGroups=input uinput`
      idempotently, so it works whether or not kanata-svc already has them.
- [x] `scripts/check-platform.sh` asserting #1/#6 strings + cross-file name sync.
- [x] `systemd-analyze verify` wired into the checks (temp copy with `/bin/true`
      so it runs before the binary is installed), including a synthetic
      `kanata.service` without supplementary groups to prove the drop-in merges.
- [x] `sysusers.d` creates `mouselessd` + `mouseless`, plus `input`/`uinput`
      only if absent, and the daemon's memberships; existing GIDs are kept
      idempotently. Human accounts never join `input`/uinput. `check-platform.sh`
      proves this in a temp root: kanata's GIDs survive and a second run is a
      byte-identical no-op.
- [x] System + extension installers with `--dry-run`; system installer backs up
      replaced files and supports `--skip-kanata`/`--no-group-add`/`--no-start`.
- [x] Phase 1.3 extension install step added to the runbook
      (`gnome-extensions install` per user; Shell reload instructions).
- [x] `--version`/`-V` on all three binaries; usage errors exit `2`,
      operational failures `1`. `VERSION` is the single source of truth, the
      Cargo workspace inherits it, and `scripts/check-version.sh` (wired into
      `check-platform.sh`) keeps the extension metadata/Hello version in sync.
      Releases are tagged `vX.Y.Z` and recorded in `CHANGELOG.md`.
- [ ] Host install: run both installers, verify the service, socket, udev node,
      and extension on the primary target.
- [x] `mouseless://` scheme handler installed by `install-system.sh`
      (`MimeType=x-scheme-handler/mouseless;`, `Exec=... %u`), registered for
      `$SUDO_USER` via `xdg-mime`; desktop entry validated in
      `check-platform.sh`. CLI accepts exact deep links (unit + fake-kanata
      tests). Settings-only links wait for the GTK app.
- [ ] Distro matrix section in the runbook with secondary-target status.

## Open questions

1. **Port.** Fixed `7070` is assumed end-to-end (`SPEC-kbd.md` default,
   drop-in, CLI). Confirm no configurable port is needed before release.
2. **User creation — resolved 2026-10-31; privilege split 2026-11-01.** Ship
   `sysusers.d`; the installer runs `systemd-sysusers` (idempotent, preserves
   existing GIDs). The desktop user is added to `mouseless` only (overlay
   socket) unless `--no-group-add`; it is never added to `input` or `uinput`.
   Only `mouselessd` and `kanata-svc` hold device-level groups. Auth remains
   group-based per `SPEC-ipc.md` OQ-1 (`0770` socket dir,
   `SO_PEERCRED`).
3. **Packaging — MVP decision 2026-10-31.** Raw, idempotent POSIX installers
   (`install-system.sh`, `install-extension.sh`) plus a runbook; no `.deb`/PPA
   yet. This keeps the closed-loop fast path and the kanata edit auditable
   without introducing a packaging toolchain. Revisit a `.deb` (and a proper
   `debian/` tree) only before external distribution; the current install paths
   are already FHS-compatible.
