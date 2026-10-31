# SPEC-platform — OS layer: kanata, systemd, udev, desktop (`platform/`)

> Module: `platform` · Paths: `platform/` · Depends on: — ·
> Plan refs: §2.3, §5.6, §7, §8 #5/#6, §10.6 · Audits W2, G1 ·
> Matrix rows: #1 (config assertion), #5 (udev rule), #6 (exclusions), part of #9
> Status as of 2026-10-31: **Packaging complete, host install pending** — kanata
> include (overlay + `lyr-free`), service drop-ins, apply script, udev rules,
> daemon unit, `sysusers.d`, system/extension installers, runbook, and
> `scripts/check-platform.sh` exist and pass. `.desktop` links and the wider
> distro matrix remain Phase 3/4.

## Objective

All OS/compositor specifics live here, never in crates (plan §3 rule 4): kanata
include + service drop-in, systemd units, udev rules, `.desktop` deep links,
permissions, and the distro test matrix. Every file is installable and
verifiable by a scripted runbook.

## Scope

- In: kanata `/etc/kanata/mouseless.kbd` + `UNICFG-EDITS.md` + `apply-phase11.sh`;
  `kanata.service.d` override; `mouselessd.service`; `99-mouseless.rules`;
  `sysusers.d` for `mouselessd`/`mouseless`; system + extension installers;
  runbook (`platform/README.md`); platform check script; distro matrix notes;
  Phase 3 `.desktop` files.
- Out: Rust config parsing (`settings`), IPC (`ipc`), kanata semantics (`kbd`).

## Contracts

### File inventory

| File | Install path | Purpose |
|---|---|---|
| `kanata/mouseless.kbd` | `/etc/kanata/mouseless.kbd` | `ml-tap` / `ml-free-tap` (`tap-hold-release 200 200` → `lyr-mouseless` / `lyr-free`), clean overlay layer (`a s d f j k l ;`, `spc`, `esc`→base), and the custom free-mode layer (`i/k/j/l`, boosts, buttons, wheel). |
| `kanata/UNICFG-EDITS.md` + `apply-phase11.sh` | runbook (script backs up both files, edits, `kanata --check`, restores on failure, restarts) | The 4 unicfg edits: defcfg output name + mouse exclude, `@ml-tap` and `@ml-free-tap` in `lyr-base`, `(include "mouseless.kbd")`. |
| `systemd/kanata.service.d-override.conf` | `/etc/systemd/system/kanata.service.d/mouseless.conf` | TCP `-p 127.0.0.1:7070`, `--emergency-exit-code 1`, `Restart=on-failure` + `RestartForceExitStatus=SIGTERM` (audit W2), `SupplementaryGroups=mouseless` for `/dev/uinput`. |
| `systemd/mouselessd.service` | `/etc/systemd/system/mouselessd.service` | `After=kanata.service`, no `Requires=`; `User=mouselessd`, `Group=mouseless`, `SupplementaryGroups=input`; `RuntimeDirectory=mouseless` (0770), `UMask=0007`; `Restart=on-failure`, `StartLimitIntervalSec=0`; sandboxing (`ProtectSystem=strict`, empty capability set). |
| `udev/99-mouseless.rules` | `/etc/udev/rules.d/99-mouseless.rules` | `uinput` group access for the `mouseless` group. Pointer accuracy is closed-loop in the daemon, so no libinput accel override is shipped. |
| `sysusers/mouseless.conf` | `/usr/lib/sysusers.d/mouseless.conf` | Creates group `mouseless` and system user `mouselessd` (nologin, primary group `mouseless`, supplementary `input`). Idempotent; existing GIDs are preserved. |
| `install-system.sh` | run in place as root | Installs binaries, unit, sysusers, udev, docs, and applies the kanata include/drop-in. Backs up replaced files as `<path>.bak.YYYYmmdd-HHMMSS.<pid>`. Options: `--dry-run`, `--skip-kanata`, `--no-group-add`, `--no-start`. |
| `install-extension.sh` | run in place as the desktop user | Packs `extension.js` + `ipc.js` + `render.js`, installs, and enables `mouseless@sonuscape.example`. `--dry-run` supported. |
| `README.md` (this dir) | `/usr/local/share/doc/mouseless/README.md` | Install/verify/upgrade/uninstall runbook and troubleshooting. |
| `scripts/check-platform.sh` | run in place | Asserts kanata timings/layers, unit/drop-in/udev/sysusers contents, cross-file device names and UUID, installer paths, POSIX shell syntax, and `systemd-analyze verify`. |
| `.desktop` deep links (Phase 3) | `/usr/share/applications/` | `x-scheme-handler/mouseless` → settings/CLI. |

### Kill-switch semantics (audit W2, cross-ref `SPEC-daemon.md`)

On Linux, kanata's emergency chord (`LCtrl+Space+Esc`) raises `SIGTERM`
directly and ignores `--emergency-exit-code`; hence
`RestartForceExitStatus=SIGTERM` is required or the service would stay dead.
`forceful-exit-code` must never appear in `defcfg` (parse error).
`mouselessd` co-exits on TCP EOF so the supervisor restarts the stack in order.

### Permissions model

- Desktop user: member of `input` (grab `/dev/input`) and `mouseless` (socket +
  `/dev/uinput` via the udev rule). Documented in the runbook.
- `kanata.service` runs as `kanata-svc` and gains `mouseless` through
  `SupplementaryGroups=mouseless` in the drop-in. This extends, rather than
  replaces, the account's database groups, so `input` + `uinput` membership is
  preserved. Do not solve `/dev/uinput` access by widening the udev rule to a
  shared system group.
- Daemon runs as system user `mouselessd`, group `mouseless`.
- Socket dir `/run/mouseless` created by `RuntimeDirectory` (0770).
- No permanent grab; permissions scoped to temp-grab + uinput mouse (plan §10.7).

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
  `daemon-reload` → kanata include/drop-in → desktop-user groups → enable and
  restart `mouselessd`. A failed restart prints `systemctl status` +
  `journalctl` and exits non-zero.
- `--skip-kanata` leaves kanata entirely untouched: no include, no drop-in,
  no `unicfg.kbd` edit, no kanata restart.
- The extension installer refuses root, verifies the UUID, packs `ipc.js` and
  `render.js`, and enables the extension. It does not restart GNOME Shell;
  Wayland requires logout/login.
- Uninstall is documented, not scripted: stop/disable the unit, remove the
  installed files, `daemon-reload`, reload udev, then remove the extension.
  The `mouselessd` user/group may remain harmlessly.

### Distro matrix

| Target | Status |
|---|---|
| Ubuntu 26.04, GNOME 50 Wayland | Primary. All acceptance runs here. |
| Ubuntu 24.04, GNOME 46 Wayland | Secondary (extension shell-version range 45–50). |
| Other distros / X11 / wlroots | Phase 4 (new `impls/` + platform entries). |

## Tech stack

POSIX shell for scripts (`set -eu`, backup-before-edit, validate-before-restart);
systemd unit/drop-in syntax; udev rules; desktop entry spec. No Python beyond
the embedded edit script in `apply-phase11.sh` (already present, keep stdlib-only).

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
sudo ./platform/kanata/apply-phase11.sh

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
  `cmd`; it greps `apply-phase11.sh` for `linux-output-device-name` and
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
- [ ] Live `/etc/kanata` apply + kanata restart (sudo) and the Phase-2 smoke
      matrix (free mode, drag/drop, nudge, monitor switch, cursor hide).
      _Pending interactive sudo; the dev `mouselessd-dev` unit must be stopped
      first so `/run/mouseless/mouselessd.sock` is free._
- [x] Kanata drop-in with TCP + `SIGTERM` restart semantics (audit W2).
- [x] `mouselessd.service` with `After=` (no `Requires=`), `RuntimeDirectory`, sandboxing.
- [x] udev rules: uinput group access for `mouseless-mouse` (no accel override;
      accuracy is closed-loop in the daemon).
- [x] Kanata drop-in grants `/dev/uinput` via `SupplementaryGroups=mouseless`.
- [x] `scripts/check-platform.sh` asserting #1/#6 strings + cross-file name sync.
- [x] `systemd-analyze verify` wired into the checks (temp copy with `/bin/true`
      so it runs before the binary is installed).
- [x] `sysusers.d` creates `mouselessd`, `mouseless`, and input membership.
- [x] System + extension installers with `--dry-run`; system installer backs up
      replaced files and supports `--skip-kanata`/`--no-group-add`/`--no-start`.
- [x] Phase 1.3 extension install step added to the runbook
      (`gnome-extensions install` per user; Shell reload instructions).
- [x] `--version`/`-V` on both binaries; usage errors exit `2`, operational
      failures `1`.
- [ ] Host install: run both installers, verify the service, socket, udev node,
      and extension on the primary target.
- [ ] Phase 3: `.desktop` deep links (`x-scheme-handler/mouseless`), settings
      entry.
- [ ] Distro matrix section in the runbook with secondary-target status.

## Open questions

1. **Port.** Fixed `7070` is assumed end-to-end (`SPEC-kbd.md` default,
   drop-in, CLI). Confirm no configurable port is needed before release.
2. **User creation — resolved 2026-10-31.** Ship `sysusers.d`; the installer
   runs `systemd-sysusers` (idempotent, preserves existing GIDs). The desktop
   user is added to `mouseless` and `input` by the installer unless
   `--no-group-add`. Auth remains group-based per `SPEC-ipc.md` OQ-1
   (`0770` socket dir, `SO_PEERCRED`).
3. **Packaging — MVP decision 2026-10-31.** Raw, idempotent POSIX installers
   (`install-system.sh`, `install-extension.sh`) plus a runbook; no `.deb`/PPA
   yet. This keeps the closed-loop fast path and the kanata edit auditable
   without introducing a packaging toolchain. Revisit a `.deb` (and a proper
   `debian/` tree) only before external distribution; the current install paths
   are already FHS-compatible.
