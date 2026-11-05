# Mouseless platform runbook

OS integration for Mouseless Linux: kanata include/drop-in, systemd unit,
udev rule, `sysusers`, installers, and the platform checks. Architecture:
`../.agents/plan.md`; contracts: `../specs/SPEC-platform.md`.

## Files and install paths

| Source | Installed as | Purpose |
|---|---|---|
| `kanata/mouseless.kbd` | `/etc/kanata/mouseless.kbd` | Overlay + custom free-mode layers. Sourced by `unicfg.kbd`. |
| `kanata/apply-phase11.sh` | run in place | Backs up + edits `unicfg.kbd`, validates with `kanata --check`, installs the drop-in, restarts kanata. |
| `systemd/kanata.service.d-override.conf` | `/etc/systemd/system/kanata.service.d/mouseless.conf` | Localhost TCP, SIGTERM restart, `SupplementaryGroups=input uinput` (idempotent; no-op when kanata already has them). |
| `systemd/mouselessd.service` | `/etc/systemd/system/mouselessd.service` | Daemon unit; `mouselessd:mouseless`, `SupplementaryGroups=input uinput`, `RuntimeDirectory=mouseless`, `StateDirectory=mouseless` (0750) for the last-applied config. |
| `desktop/mouseless-url-handler.desktop` | `/usr/share/applications/mouseless-url-handler.desktop` | `mouseless://` scheme handler → `mouseless %u` (overlay, free-mode, status, device list, settings). |
| `desktop/mouseless-settings.desktop` | `/usr/share/applications/mouseless-settings.desktop` | Application-menu entry → `mouseless-settings %u`. |
| `sysusers/mouseless.conf` | `/usr/lib/sysusers.d/mouseless.conf` | Creates `mouselessd`, the `mouseless` socket group, and joins the existing `uinput`/`input` device groups. |
| `udev/99-mouseless.rules` | `/etc/udev/rules.d/99-mouseless.rules` | `/dev/uinput` for the `uinput` device group (kanata + daemon). No libinput acceleration override. |
| `../mouseless.toml` | `/etc/mouseless/mouseless.toml` | Application config: paths, kanata endpoint, timeouts, device names, base layer, deep-link scheme, app id, appearance fallbacks. Read by all binaries. Re-run the installer after changing `[kanata] address` or `[devices]` names: it regenerates the kanata drop-in and defcfg names. |
| `install-system.sh` | run in place | Installs the system files above + binaries + `mouseless.toml`, and regenerates the scheme handler when `[deep_link] scheme` is not `mouseless`. |
| `install-extension.sh` | run in place | Generates `config.js` from `mouseless.toml` (socket + appearance fallback), packs/installs/enables the GNOME extension for the current user. Removes the UUID from `org.gnome.shell disabled-extensions` after `install --force`, because GNOME leaves it there and `gnome-extensions enable` is then a silent no-op. |
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
3. installs `mouselessd`/`mouseless`/`mouseless-settings` to `/usr/local/bin`
   and `mouseless.toml` to `/etc/mouseless/mouseless.toml`;
4. installs the unit, both desktop entries, and runs `systemctl daemon-reload`;
5. applies the kanata include/drop-in via `apply-phase11.sh` (creates any
   missing `input`/`uinput` group, backs up + edits `unicfg.kbd` with the
   configured device names and TCP endpoint, runs `kanata --check`, restarts
   kanata);
6. adds `$SUDO_USER` to `mouseless` when needed (never to `input`/`uinput`;
   see the privilege model below) and registers the scheme handler with
   `xdg-mime` for that user;
7. enables and restarts `mouselessd.service`.

Existing files are backed up as `<path>.bak.YYYYmmdd-HHMMSS.<pid>`.
Options: `--dry-run`, `--skip-kanata` (leave kanata untouched),
`--no-group-add`, `--no-start`.

## Privilege model (least privilege)

Human accounts never get device-level input access. Membership is split:

| Group | Members | Grants |
|---|---|---|
| `mouseless` | desktop user, `mouselessd` (primary) | overlay socket `/run/mouseless` only |
| `uinput` | `mouselessd`, `kanata-svc` | write `/dev/uinput` (0660) |
| `input` | `mouselessd`, `kanata-svc` | evdev read + temporary grab |

`uinput` and `input` are the device groups kanata normally uses; the daemon
joins them rather than creating a parallel group. The kanata drop-in lists both
in `SupplementaryGroups=`, which systemd treats as additive — a no-op when
kanata-svc already has them, and a fix when it does not. Because systemd refuses
to start a unit whose `SupplementaryGroups=` names a missing group, both
`install-system.sh` (sysusers) and `apply-phase11.sh` create them first if the
host lacks them. The daemon and kanata
are the only processes that can observe or inject input. The desktop user needs
`mouseless` solely to talk to the overlay socket. Do not add a human account to
`input` or `uinput`:
those allow keylogging and synthetic input injection for the whole session. If
the installer finds an elevated membership it prints the `gpasswd -d` command
and continues; it never removes memberships silently.

After the extension install, **log out and back in on Wayland** so GNOME
Shell loads it. On X11, `Alt+F2`, `r`, Enter is enough. Group changes also
require a new login.

GNOME pitfall: `gnome-extensions install --force` can leave the UUID in
`org.gnome.shell disabled-extensions`. While it is there, `gnome-extensions
enable` exits 0 but does nothing (state stays INACTIVE) and the shell log shows
no error. The installer now removes the UUID from that list automatically. To
check/fix it manually:

```sh
# remove just the mouseless entry, then enable it:
gsettings set org.gnome.shell disabled-extensions \
  "$(gsettings get org.gnome.shell disabled-extensions \
     | sed "s/'mouseless@0king.dev', //; s/, 'mouseless@0king.dev'//")"
gnome-extensions enable mouseless@0king.dev
gnome-extensions info mouseless@0king.dev   # Enabled: Yes / ACTIVE
```

If `info` still says INACTIVE, log out and back in; the running Shell caches
extension ES modules, so changed `render.js`/`extension.js` code is only picked
up on a fresh login on Wayland.

## Verify

```sh
systemctl status mouselessd.service
journalctl -u mouselessd.service -b
/usr/local/bin/mouseless --version
/usr/local/bin/mouselessd --version
/usr/local/bin/mouseless-settings --version
/usr/local/bin/mouseless status
/usr/local/bin/mouseless reload
/usr/local/bin/mouseless --list-input-devices
/usr/local/bin/mouseless --config /etc/mouseless/mouseless.toml status
xdg-mime query default x-scheme-handler/mouseless
gnome-extensions info mouseless@0king.dev
```

Expected:

- `/etc/mouseless/mouseless.toml` exists (0644) and parses; restart
  `mouselessd` after editing it;
- `/run/mouseless/mouselessd.sock` exists, owned `mouselessd:mouseless`,
  mode `0770`;
- `id mouselessd` includes `mouseless`, `uinput`, and `input`, while
  `id "$USER"` includes `mouseless` and **not** `input`/`uinput`;
- `mouseless-mouse` appears in `/proc/bus/input/devices` and is
  kanata-excluded;
- `journalctl` shows `overlay hello (extension …)` after login;
- `mouseless status` prints `kanata: lyr-base` (or the current layer) plus
  `daemon:`, `overlay:`, `session:`, `free-mode:`, and `config:` lines;
- `mouseless reload` prints the daemon ack (`configuration reloaded`), or a
  clear `Error` when the stored config is invalid — the previous config stays
  live;
- `xdg-mime query default x-scheme-handler/mouseless` prints
  `mouseless-url-handler.desktop`; `xdg-open mouseless://status` runs the CLI
  verb (its stdout goes to the journal in a desktop session);
- `xdg-open mouseless://settings/grid` activates `mouseless-settings` on the
  Grid page (single instance; a second link switches pages).

## Deep links

`mouseless://show-overlay`, `hide-overlay`, `toggle-overlay`,
`enter-free-mode`, `exit-free-mode`, `status`, and `list-input-devices` are
handled by the installed `.desktop` entry, which calls `mouseless %u`. The CLI
accepts the exact URL as its verb, so no extra router is needed. `reload` is
socket-only and has no link. Unknown links exit `2` without side effects.

`mouseless://settings` and `mouseless://settings/<page>` are the exception: the
CLI forwards them to `mouseless-settings`, which activates (or starts) the
single-instance window on that page. Pages: `grid` (default), `appearance`,
`behavior`, `free-mode`, `keybindings`, `presets`, `status`. The same binary is
also in the application menu via `mouseless-settings.desktop`.

To register the handler manually (for a user the installer could not reach):

```sh
xdg-mime default mouseless-url-handler.desktop x-scheme-handler/mouseless
```

## Config lifecycle

`mouselessd` is the only writer of the applied config:

| Path | Owner | Purpose |
|---|---|---|
| `/var/lib/mouseless/config.yaml` | `mouselessd` (0750 state dir) | last-applied schema-v1 document, loaded on start and returned by `GetConfig` |
| `/var/lib/mouseless/config.last-good.yaml` | `mouselessd` | previous validated document; used when the live file is corrupt |
| `~/.config/mouseless/presets/*.yaml` | desktop user | user presets read/written by the settings app |

On start the daemon calls `ConfigStore::effective()` (live → last-good →
defaults) and logs which fallback it used. A client `Update` is validated,
persisted atomically, then swapped into the live runtime; `Reload` re-reads the
stored file. Installing `/etc/kanata/mouseless.kbd` remains a root action of
`install-system.sh` — the daemon never writes outside its state directory.


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
sudo rm -f /usr/share/applications/mouseless-url-handler.desktop
sudo rm -f /usr/share/applications/mouseless-settings.desktop
sudo rm -f /usr/lib/sysusers.d/mouseless.conf
sudo rm -f /etc/udev/rules.d/99-mouseless.rules
sudo rm -f /etc/systemd/system/kanata.service.d/mouseless.conf
sudo systemctl daemon-reload
sudo udevadm control --reload-rules
```

Remove `/usr/local/bin/mouselessd`, `/usr/local/bin/mouseless`, and
`/usr/local/bin/mouseless-settings` if no longer needed. The applied config
state (`/var/lib/mouseless/`) is data, not an install artifact: `sudo rm -rf /var/lib/mouseless` only if you deliberately want
to discard the saved config and its last-good copy. The `mouselessd` user and
`mouseless` group can stay (harmless) or be
removed with `userdel mouselessd` / `groupdel mouseless` after checking no
other service uses them, and after removing the desktop user from `mouseless`
(`sudo gpasswd -d <user> mouseless`). Leave the shared `uinput` and `input`
groups alone; kanata owns them. Do **not** delete `/etc/kanata/mouseless.kbd` while `unicfg.kbd`
still contains `(include "mouseless.kbd")`; remove the include first or kanata
will fail validation. The extension is removed per user:

```sh
gnome-extensions disable mouseless@0king.dev
gnome-extensions uninstall mouseless@0king.dev
```

## Troubleshooting

- **Installer refuses to start: another daemon on the socket** — a dev daemon
  (often a transient user unit such as `mouselessd-dev`) still owns
  `/run/mouseless/mouselessd.sock`. Stop it first:
  `systemctl --user stop mouselessd-dev.service`. Two daemons would fight over
  kanata and the evdev grab.
- **`uinput unavailable`** — check `ls -l /dev/uinput`
  (root:uinput 0660), `id mouselessd` (must include
  `uinput` and `input`), and `journalctl -u mouselessd`. Re-run the
  installer after fixing groups.
- **Overlay socket permission denied** — the desktop user must be in
  `mouseless`; log out/in after `usermod -aG mouseless <user>`.
- **Installer warns the user is in an input group** — remove it with
  `sudo gpasswd -d <user> input` (same for `uinput`) and log out/in. The daemon
  and kanata do not need the human account to hold these.
- **`/dev/uinput` group is not `uinput`** — another udev rule that sorts after
  `99-mouseless.rules` (for example a launcher's `99-vicinae-uinput.rules`,
  required by its setgid `vicinae-input-server`) re-assigned the node. The
  daemon still works because it is in both `input` and `uinput`; the
  least-privilege invariant is that no human account is in `input`/`uinput`.
  Fix: rename the competing rule to sort **before** ours (e.g.
  `sudo mv /etc/udev/rules.d/99-vicinae-uinput.rules
  /etc/udev/rules.d/90-vicinae-uinput.rules && sudo udevadm control
  --reload-rules && sudo udevadm trigger --subsystem-match=misc
  --sysname-match=uinput`). This makes `uinput` own the node while the
  launcher's `setfacl` still grants its `input`-group process access. Do **not**
  delete the rule if that tool depends on it.
- **Extension not listed** — `gnome-extensions list`; repack with
  `./platform/install-extension.sh`; on Wayland a logout/login is required for
  a new install.
- **Kanata not listening** — `systemctl status kanata`,
  `ss -tln | grep 7070`; the daemon retries TCP and does not need boot order.
- **Cursor movement** — closed-loop; see `../specs/SPEC-daemon.md`
  §Movement control loop. `MOUSELESS_DEBUG_MOVE=1` logs each step.
