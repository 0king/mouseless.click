#!/bin/sh
# Mouseless system installer: binaries, systemd unit, sysusers, udev, kanata.
#
# Run from mouseless-workspace/ after building:
#   cargo build --release --workspace
#   sudo ./platform/install-system.sh
#
# Installs:
#   /usr/local/bin/mouselessd, /usr/local/bin/mouseless,
#   /usr/local/bin/mouseless-settings
#   /etc/systemd/system/mouselessd.service
#   /usr/lib/sysusers.d/mouseless.conf  (mouselessd user, mouseless group;
#                                        daemon joins input + uinput)
#   /etc/udev/rules.d/99-mouseless.rules
#   /usr/share/applications/mouseless-url-handler.desktop
#   /usr/share/applications/mouseless-settings.desktop
#   /usr/local/share/doc/mouseless/README.md
#   /etc/kanata/mouseless.kbd + kanata drop-in (via apply-phase11.sh)
#
# Existing files are backed up as <path>.bak.YYYYmmdd-HHMMSS.<pid>.
# The kanata step edits /etc/kanata/unicfg.kbd, validates it with
# `kanata --check`, and restarts kanata. Use --skip-kanata to leave kanata
# entirely untouched (no include, no drop-in, no unicfg edit).
#
# Options:
#   --dry-run        print actions without changing the system
#   --skip-kanata    do not install/apply the kanata include + drop-in
#   --no-group-add   do not add $SUDO_USER to the mouseless socket group
#                    (the installer never adds a human account to input
#                    or uinput)
#   --no-start       install files but do not enable/start mouselessd
#   --help
#
# Uninstall steps are documented in platform/README.md.
set -eu

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DRY_RUN=0
SKIP_KANATA=0
NO_GROUP_ADD=0
NO_START=0

usage() {
    sed -n '2,/^set -eu$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --help|-h)
            usage
            exit 0
            ;;
        --dry-run) DRY_RUN=1 ;;
        --skip-kanata) SKIP_KANATA=1 ;;
        --no-group-add) NO_GROUP_ADD=1 ;;
        --no-start) NO_START=1 ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

if [ "$DRY_RUN" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
    echo "run as root: sudo $0" >&2
    exit 1
fi

need_file() {
    if [ ! -f "$1" ]; then
        echo "missing $1" >&2
        case "$1" in
            */target/release/*) echo "build first: cargo build --release --workspace" >&2 ;;
        esac
        exit 1
    fi
}

need_file "$REPO_DIR/target/release/mouselessd"
need_file "$REPO_DIR/target/release/mouseless"
need_file "$REPO_DIR/target/release/mouseless-settings"
need_file "$REPO_DIR/platform/systemd/mouselessd.service"
need_file "$REPO_DIR/platform/udev/99-mouseless.rules"
need_file "$REPO_DIR/platform/sysusers/mouseless.conf"
need_file "$REPO_DIR/platform/desktop/mouseless-url-handler.desktop"
need_file "$REPO_DIR/platform/desktop/mouseless-settings.desktop"
need_file "$REPO_DIR/platform/README.md"

for tool in systemctl systemd-sysusers udevadm; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "required tool not found: $tool" >&2
        exit 1
    fi
done

# Refuse to start a second daemon on the same socket. A dev daemon often runs
# as a transient *user* unit (e.g. `systemd-run --user --unit=mouselessd-dev`),
# which the system manager cannot see; binding would unlink its socket and
# leave two processes competing for kanata and the evdev grab.
if [ "$DRY_RUN" -eq 0 ] && [ "$NO_START" -eq 0 ] &&
    command -v ss >/dev/null 2>&1 &&
    ss -xl 2>/dev/null | grep -Fq " /run/mouseless/mouselessd.sock " &&
    ! systemctl is-active --quiet mouselessd.service; then
    echo "another daemon is already listening on /run/mouseless/mouselessd.sock" >&2
    echo "stop it first, for example: systemctl --user stop mouselessd-dev.service" >&2
    exit 1
fi

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  + %s\n' "$*"
    else
        "$@"
    fi
}

backup() {
    _path=$1
    [ -e "$_path" ] || return 0
    _bak="$_path.bak.$(date +%Y%m%d-%H%M%S).$$"
    run cp -a "$_path" "$_bak"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'would back up %s\n' "$_bak"
    else
        printf 'backup: %s\n' "$_bak"
    fi
}

install_file() {
    _src=$1
    _dst=$2
    _mode=$3
    backup "$_dst"
    run install -Dm"$_mode" "$_src" "$_dst"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'would install %s\n' "$_dst"
    else
        printf 'installed %s\n' "$_dst"
    fi
}

if [ "$DRY_RUN" -eq 1 ]; then
    echo "mouseless: system install (dry run) — no changes will be made"
else
    echo "mouseless: system install"
fi

# 1. System user/groups (idempotent). The unit's User=/Group= and the
#    SupplementaryGroups=input uinput line depend on this.
install_file "$REPO_DIR/platform/sysusers/mouseless.conf" \
    /usr/lib/sysusers.d/mouseless.conf 0644
run systemd-sysusers

# 2. /dev/uinput access through the existing `uinput` group (kanata already
#    has it; the daemon joins it). Trigger the node so the rule applies
#    without a reboot.
install_file "$REPO_DIR/platform/udev/99-mouseless.rules" \
    /etc/udev/rules.d/99-mouseless.rules 0644
run udevadm control --reload-rules
run udevadm trigger --sysname-match=uinput

# 3. Binaries, unit, and the mouseless:// scheme handler.
install_file "$REPO_DIR/target/release/mouselessd" /usr/local/bin/mouselessd 0755
install_file "$REPO_DIR/target/release/mouseless" /usr/local/bin/mouseless 0755
install_file "$REPO_DIR/target/release/mouseless-settings" \
    /usr/local/bin/mouseless-settings 0755
install_file "$REPO_DIR/platform/systemd/mouselessd.service" \
    /etc/systemd/system/mouselessd.service 0644
install_file "$REPO_DIR/platform/desktop/mouseless-url-handler.desktop" \
    /usr/share/applications/mouseless-url-handler.desktop 0644
install_file "$REPO_DIR/platform/desktop/mouseless-settings.desktop" \
    /usr/share/applications/mouseless-settings.desktop 0644
if command -v update-desktop-database >/dev/null 2>&1; then
    run update-desktop-database -q /usr/share/applications
fi
install_file "$REPO_DIR/platform/README.md" \
    /usr/local/share/doc/mouseless/README.md 0644
run systemctl daemon-reload

# Register the handler for the invoking desktop user. The .desktop file routes
# every mouseless:// verb to the CLI, so overlay verbs keep working without the
# GTK app; `mouseless://settings/...` is forwarded to `mouseless-settings`.
_scheme_user="${SUDO_USER:-}"
if [ -n "$_scheme_user" ] && [ "$_scheme_user" != root ] &&
    command -v xdg-mime >/dev/null 2>&1; then
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  + sudo -u %s xdg-mime default mouseless-url-handler.desktop x-scheme-handler/mouseless\n' "$_scheme_user"
    elif command -v sudo >/dev/null 2>&1; then
        sudo -u "$_scheme_user" xdg-mime default \
            mouseless-url-handler.desktop x-scheme-handler/mouseless ||
            echo "warning: could not register mouseless:// for $_scheme_user; run: xdg-mime default mouseless-url-handler.desktop x-scheme-handler/mouseless" >&2
    elif command -v runuser >/dev/null 2>&1; then
        runuser -u "$_scheme_user" -- xdg-mime default \
            mouseless-url-handler.desktop x-scheme-handler/mouseless ||
            echo "warning: could not register mouseless:// for $_scheme_user" >&2
    fi
else
    echo "mouseless:// handler installed; register it per user with:" >&2
    echo "  xdg-mime default mouseless-url-handler.desktop x-scheme-handler/mouseless" >&2
fi

# 4. Kanata include + drop-in. apply-phase11.sh also edits unicfg.kbd
#    (backup + `kanata --check` + restart); --skip-kanata leaves that alone.
if [ "$SKIP_KANATA" -eq 0 ]; then
    if [ -f /etc/kanata/unicfg.kbd ] && command -v kanata >/dev/null 2>&1; then
        run sh "$REPO_DIR/platform/kanata/apply-phase11.sh"
    else
        echo "kanata config not found or kanata not in PATH; installing include/drop-in only" >&2
        install_file "$REPO_DIR/platform/kanata/mouseless.kbd" \
            /etc/kanata/mouseless.kbd 0644
        install_file "$REPO_DIR/platform/systemd/kanata.service.d-override.conf" \
            /etc/systemd/system/kanata.service.d/mouseless.conf 0644
        run systemctl daemon-reload
        echo "apply platform/kanata/UNICFG-EDITS.md manually, then restart kanata" >&2
    fi
else
    echo "skipping kanata include/drop-in (--skip-kanata)"
fi

# 5. Desktop user group membership. The 0770:mouseless overlay socket is the
#    ONLY thing a human account needs. Never add a human to `input` or
#    `uinput`: those allow keylogging and synthetic input injection for the
#    whole session. The daemon already holds them.
#    Existing elevated memberships are reported, never silently removed.
if [ "$NO_GROUP_ADD" -eq 0 ]; then
    _target="${SUDO_USER:-}"
    if [ -n "$_target" ] && [ "$_target" != root ]; then
        if getent group mouseless >/dev/null 2>&1; then
            if ! id -nG "$_target" | tr ' ' '\n' | grep -qx mouseless; then
                run usermod -aG mouseless "$_target"
                echo "added $_target to group mouseless (log out/in required)"
            fi
        else
            echo "warning: group mouseless does not exist; add $_target manually" >&2
        fi

        _elevated=
        for _grp in input uinput; do
            if id -nG "$_target" | tr ' ' '\n' | grep -qx "$_grp"; then
                _elevated="$_elevated $_grp"
            fi
        done
        if [ -n "$_elevated" ]; then
            echo "warning: $_target is in privileged input group(s):$_elevated" >&2
            echo "         these allow keylogging/input injection; remove with:" >&2
            for _grp in $_elevated; do
                echo "           sudo gpasswd -d $_target $_grp" >&2
            done
            echo "         then log out and back in." >&2
        fi
    else
        echo "no SUDO_USER; add your desktop user to group mouseless manually" >&2
    fi
else
    echo "skipping desktop user group changes (--no-group-add)"
fi

# 6. Enable + restart so a reinstall picks up new binaries.
if [ "$NO_START" -eq 0 ]; then
    run systemctl enable mouselessd.service
    if ! run systemctl restart mouselessd.service; then
        echo "mouselessd failed to start; recent logs:" >&2
        systemctl status --no-pager mouselessd.service || true
        journalctl -u mouselessd.service -n 30 --no-pager || true
        exit 1
    fi
else
    echo "not starting mouselessd (--no-start)"
fi

cat <<'EOF'

System install complete. Verify:
  systemctl status mouselessd.service
  journalctl -u mouselessd.service -b
  /usr/local/bin/mouseless status
  /usr/local/bin/mouseless --list-input-devices
  xdg-mime query default x-scheme-handler/mouseless
  /usr/local/bin/mouseless settings    # opens the GTK settings app

Install the GNOME extension as your desktop user:
  ./platform/install-extension.sh

If the installer added you to the mouseless socket group, log out and back in
before using the overlay; the running session keeps its old group membership.
Your account must NOT be in input or uinput (the daemon and kanata hold
those). Check with: id
EOF
