# SPEC-platform — OS layer: kanata, systemd, udev, desktop (`platform/`)

> Module: `platform` · Paths: `platform/` · Depends on: — ·
> Plan refs: §2.3, §5.6, §7, §8 #5/#6, §10.6 · Audits W2, G1 ·
> Matrix rows: #1 (config assertion), #5 (udev rule), #6 (exclusions), part of #9
> Status as of 2026-10-18: **Partial** — kanata include, service drop-ins, apply
> script, udev rules, daemon unit exist; extension install, `.desktop` links,
> `sysusers.d`, distro matrix pending.

## Objective

All OS/compositor specifics live here, never in crates (plan §3 rule 4): kanata
include + service drop-in, systemd units, udev rules, `.desktop` deep links,
permissions, and the distro test matrix. Every file is installable and
verifiable by a scripted runbook.

## Scope

- In: kanata `/etc/kanata/mouseless.kbd` + `UNICFG-EDITS.md` + `apply-phase11.sh`;
  `kanata.service.d` override; `mouselessd.service`; `99-mouseless.rules`;
  Phase 3 `.desktop` files; Phase 3 `sysusers.d` for the `mouselessd` user;
  distro matrix notes.
- Out: Rust config parsing (`settings`), IPC (`ipc`), kanata semantics (`kbd`).

## Contracts

### File inventory

| File | Install path | Purpose |
|---|---|---|
| `kanata/mouseless.kbd` | `/etc/kanata/mouseless.kbd` | `ml-tap` (`tap-hold-release 200 200` → `lyr-mouseless`) + clean overlay layer (`a s d f j k l ;`, `spc`, `esc`→base). |
| `kanata/UNICFG-EDITS.md` + `apply-phase11.sh` | runbook (script backs up, edits, `kanata --check`, restarts) | The 3 unicfg edits: defcfg output name + mouse exclude, `@ml-tap` in `lyr-base`, `(include "mouseless.kbd")`. |
| `systemd/kanata.service.d-override.conf` | `/etc/systemd/system/kanata.service.d/mouseless.conf` | TCP `-p 127.0.0.1:7070`, `--emergency-exit-code 1`, `Restart=on-failure` + `RestartForceExitStatus=SIGTERM` (audit W2). |
| `systemd/mouselessd.service` | `/etc/systemd/system/mouselessd.service` | `After=kanata.service`, no `Requires=`; `RuntimeDirectory=mouseless` (0770), `UMask=0007`; `Restart=on-failure`; sandboxing. |
| `udev/99-mouseless.rules` | `/etc/udev/rules.d/99-mouseless.rules` | `uinput` group access + flat pointer accel for `mouseless-mouse` (audit G1). |
| `.desktop` deep links (Phase 3) | `/usr/share/applications/` | `x-scheme-handler/mouseless` → settings/CLI. |
| `sysusers.d` (Phase 3, OQ-2) | `/usr/lib/sysusers.d/mouseless.conf` | Creates the `mouselessd` system user deterministically. |

### Kill-switch semantics (audit W2, cross-ref `SPEC-daemon.md`)

On Linux, kanata's emergency chord (`LCtrl+Space+Esc`) raises `SIGTERM`
directly and ignores `--emergency-exit-code`; hence
`RestartForceExitStatus=SIGTERM` is required or the service would stay dead.
`forceful-exit-code` must never appear in `defcfg` (parse error).
`mouselessd` co-exits on TCP EOF so the supervisor restarts the stack in order.

### Permissions model

- Desktop user: member of `input` (grab `/dev/input`) and `mouseless` (socket +
  `/dev/uinput` via the udev rule). Documented in the runbook.
- Daemon runs as system user `mouselessd`, group `mouseless`.
- Socket dir `/run/mouseless` created by `RuntimeDirectory` (0770).
- No permanent grab; permissions scoped to temp-grab + uinput mouse (plan §10.7).

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
# apply kanata config + drop-in (backs up + validates; aborts on failure)
sudo ./platform/kanata/apply-phase11.sh
# verify:
kanata --check --cfg /etc/kanata/unicfg.kbd
systemd-analyze verify platform/systemd/mouselessd.service
ss -tln | grep 7070
grep -A2 'Name="kanata-mouseless"' /proc/bus/input/devices
# udev reload after rule edit:
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input
```

## Project structure

`platform/kanata/` (include + runbook + apply script), `platform/systemd/`
(units + drop-ins), `platform/udev/`, Phase 3 adds `platform/desktop/` and
`platform/sysusers/`.

## Code style

- Every installed file starts with a comment header: purpose, install path,
  related plan § / audit id, and any manual step it cannot automate.
- Scripts: `set -eu`; every mutation preceded by a timestamped backup; every
  validation failure restores the backup and exits non-zero.
- Never hardcode machine-specific paths beyond the documented defaults; note
  overrides inline (e.g. `--cfg` path in the kanata drop-in).

## Testing strategy (scripted assertions; runbook-style)

- #1: a `scripts/check-platform.sh` (to add) greps the include for
  `tap-hold-release 200 200`, plain cell keys in `lyr-mouseless`, and absence of
  `cmd`; greps `unicfg` after apply for `linux-output-device-name`,
  `linux-dev-names-exclude`. Blocking.
- #5: same script asserts the flat-accel line exists for `mouseless-mouse`.
- #6: exclusion assertions: kanata exclude contains `mouseless-mouse`; output
  name equals `mouseless-kbd::KANATA_OUTPUT_NAME` and the uinput name equals
  `UINPUT_DEVICE_NAME` (string cross-check script — single source of truth
  lives in the Rust constants).
- #9 (part): `kanata --check` gate in the apply script + rollback proven by
  feeding a broken candidate in a test run.
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
- [x] Kanata drop-in with TCP + `SIGTERM` restart semantics (audit W2).
- [x] `mouselessd.service` with `After=` (no `Requires=`), `RuntimeDirectory`, sandboxing.
- [x] udev rules: uinput group access + flat accel for `mouseless-mouse`.
- [ ] `scripts/check-platform.sh` asserting #1/#5/#6 strings + cross-file name sync.
- [ ] `systemd-analyze verify` wired into the checks.
- [ ] Phase 1.3 extension install step added to the runbook
      (`gnome-extensions install` per user; Shell reload instructions).
- [ ] Phase 3: `.desktop` deep links (`x-scheme-handler/mouseless`), settings
      entry, `sysusers.d`.
- [ ] Distro matrix section in the runbook with secondary-target status.

## Open questions

1. **Port.** Fixed `7070` is assumed end-to-end (`SPEC-kbd.md` default,
   drop-in, CLI). Confirm no configurable port is needed before release.
2. **User creation.** `mouselessd` as a system user: ship `sysusers.d` (proposed)
   vs a `useradd` step in the runbook. Also cross-check the auth model decision
   in `SPEC-ipc.md` OQ-1 (uid vs group membership) — it changes which groups
   matter.
3. **Packaging.** Raw script install vs `.deb`/PPA for the extension + binaries
   + platform files. Out of MVP scope but affects file layout; decide before
   any external distribution.
