# Mouseless platform runbook

OS integration for Mouseless Linux: kanata include/drop-in, systemd unit,
udev rule, `sysusers`, installers, and the platform checks. Architecture:
`../.agents/plan.md`; contracts: `../specs/SPEC-platform.md`.

## Files and install paths

| Source | Installed as | Purpose |
|---|---|---|
| `kanata/mouseless.kbd` | `/etc/kanata/mouseless.kbd` | Overlay + custom free-mode layers. Sourced by `unicfg.kbd`. |
| `kanata/apply-phase11.sh` | run in place | Backs up + edits `unicfg.kbd`, validates with `kanata --check`, installs the drop-in, restarts kanata. |
| `systemd/kanata.service.d-override.conf` | `/etc/systemd/system/kanata.service.d/mouseless.conf` | Localhost TCP, SIGTERM restart, `mouseless` group for uinput. |
| `systemd/mouselessd.service` | `/etc/systemd/system/mouselessd.service` | Daemon unit; `mouselessd:mouseless`, `SupplementaryGroups=input`, `RuntimeDirectory=mouseless`. |
| `sysusers/mouseless.conf` | `/usr/lib/sysusers.d/mouseless.conf` | Creates `mouselessd`, `mouseless`, and the input membership. |
| `udev/99-mouseless.rules` | `/etc/udev/rules.d/99-mouseless.rules` | `/dev/uinput` group access. No libinput acceleration override. |
| `install-system.sh` | run in place | Installs the system files above + binaries. |
| `install-extension.sh` | run in place | Packs/installs/enables the GNOME extension for the current user. |
| `../scripts/check-platform.sh` | run in place | Asserts names, unit, udev, sysusers, scripts, and unit syntax. |

## Install

Prerequisites: kanata at `/usr/local/bin/kanata`, GNOME Shell 45–50,
systemd, `gnome-extensions`, and a Rust toolchain for the build.

```sh
cd mouseless-workspace
cargo build --release --workspace
./scripts/check-platform.sh
sudo ./platform/install-system.sh
./platform/install-extension.sh
```

The system installer:

1. installs `sysusers` and runs `systemd-sysusers` (idempotent);
2. installs the udev rule and triggers `/dev/uinput`;
3. installs `mouselessd`/`mouseless` to `/usr/local/bin`;
4. installs the unit and runs `systemctl daemon-reload`;
5. applies the kanata include/drop-in via `apply-phase11.sh` (backup +
   `kanata --check` + restart);
6. adds `$SUDO_USER` to `mouseless` and `input` when needed;
7. enables and restarts `mouselessd.service`.

Existing files are backed up as `<path>.bak.YYYYmmdd-HHMMSS.<pid>`.
Options: `--dry-run`, `--skip-kanata` (leave kanata untouched),
`--no-group-add`, `--no-start`.

After the extension install, **log out and back in on Wayland** so GNOME
Shell loads it. On X11, `Alt+F2`, `r`, Enter is enough. Group changes also
require a new login.

## Verify

```sh
systemctl status mouselessd.service
journalctl -u mouselessd.service -b
/usr/local/bin/mouseless status
/usr/local/bin/mouseless --list-input-devices
gnome-extensions info mouseless@sonuscape.example
```

Expected:

- `/run/mouseless/mouselessd.sock` exists, owned `mouselessd:mouseless`,
  mode `0770`;
- `mouseless-mouse` appears in `/proc/bus/input/devices` and is
  kanata-excluded;
- `journalctl` shows `overlay hello (extension …)` after login;
- `mouseless status` prints `kanata: lyr-base` (or the current layer).

## Upgrade

Rebuild, then rerun both installers:

```sh
cargo build --release --workspace
sudo ./platform/install-system.sh
./platform/install-extension.sh
```

The system installer restarts `mouselessd`; the extension installer replaces
the installed copy. A Shell restart (or logout/login) is only needed when the
extension code changed.

## Uninstall

The installers never delete user data. To remove the service:

```sh
systemctl disable --now mouselessd.service
sudo rm -f /etc/systemd/system/mouselessd.service
sudo rm -f /usr/lib/sysusers.d/mouseless.conf
sudo rm -f /etc/udev/rules.d/99-mouseless.rules
sudo rm -f /etc/systemd/system/kanata.service.d/mouseless.conf
sudo systemctl daemon-reload
sudo udevadm control --reload-rules
```

Remove `/usr/local/bin/mouselessd` and `/usr/local/bin/mouseless` if no longer
needed. The `mouselessd` user/group can stay (harmless) or be removed with
`userdel mouselessd` / `groupdel mouseless` after checking no other service
uses them. Do **not** delete `/etc/kanata/mouseless.kbd` while `unicfg.kbd`
still contains `(include "mouseless.kbd")`; remove the include first or kanata
will fail validation. The extension is removed per user:

```sh
gnome-extensions disable mouseless@sonuscape.example
gnome-extensions uninstall mouseless@sonuscape.example
```

## Troubleshooting

- **Installer refuses to start: another daemon on the socket** — a dev daemon
  (often a transient user unit such as `mouselessd-dev`) still owns
  `/run/mouseless/mouselessd.sock`. Stop it first:
  `systemctl --user stop mouselessd-dev.service`. Two daemons would fight over
  kanata and the evdev grab.
- **`uinput unavailable`** — check `ls -l /dev/uinput` (root:mouseless 0660),
  `id mouselessd` (must include `mouseless` and `input`), and
  `journalctl -u mouselessd`. Re-run the installer after fixing groups.
- **Overlay socket permission denied** — the desktop user must be in
  `mouseless`; log out/in after `usermod -aG mouseless <user>`.
- **Extension not listed** — `gnome-extensions list`; repack with
  `./platform/install-extension.sh`; on Wayland a logout/login is required for
  a new install.
- **Kanata not listening** — `systemctl status kanata`,
  `ss -tln | grep 7070`; the daemon retries TCP and does not need boot order.
- **Cursor movement** — closed-loop; see `../specs/SPEC-daemon.md`
  §Movement control loop. `MOUSELESS_DEBUG_MOVE=1` logs each step.
