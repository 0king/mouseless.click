#!/bin/sh
# Mouseless platform checks (plan §8 #1/#5/#6; SPEC-platform).
#
# Run from mouseless-workspace/:
#   ./scripts/check-platform.sh
#
# Asserts the installable files agree with the Rust constants and the
# documented contracts:
# - kanata include: tap-hold timings, overlay/free layers, no cmd path;
# - daemon unit: user/group, input group, runtime dir, sandboxing, no Requires=;
# - kanata drop-in: localhost TCP, SIGTERM restart, mouseless group;
# - udev: uinput group access, no ineffective flat-accel rule;
# - sysusers: mouselessd/mouseless/input;
# - cross-file names (kanata-mouseless, mouseless-mouse, extension UUID);
# - installer paths and extension pack sources;
# - POSIX shell syntax and systemd unit syntax.
#
# Exit 0 when all checks pass, 1 otherwise. No root required.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KBD="$ROOT/platform/kanata/mouseless.kbd"
APPLY="$ROOT/platform/kanata/apply-phase11.sh"
UNIT="$ROOT/platform/systemd/mouselessd.service"
DROPIN="$ROOT/platform/systemd/kanata.service.d-override.conf"
UDEV="$ROOT/platform/udev/99-mouseless.rules"
SYSUSERS="$ROOT/platform/sysusers/mouseless.conf"
INSTALL_SYSTEM="$ROOT/platform/install-system.sh"
INSTALL_EXT="$ROOT/platform/install-extension.sh"
EXT_META="$ROOT/gnome-extension/metadata.json"
GRAB_RS="$ROOT/crates/mouseless-kbd/src/grab.rs"
UINPUT_RS="$ROOT/crates/mouseless-mouse/src/impls/uinput.rs"

fail=0
ok() { printf 'ok   %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1" >&2; fail=1; }

require_literal() {
    _file=$1
    _text=$2
    _desc=$3
    if [ ! -f "$_file" ]; then
        bad "$_desc (missing $_file)"
    elif grep -Fq -- "$_text" "$_file"; then
        ok "$_desc"
    else
        bad "$_desc (missing '$_text' in $_file)"
    fi
}

forbid_literal() {
    _file=$1
    _text=$2
    _desc=$3
    if [ ! -f "$_file" ]; then
        bad "$_desc (missing $_file)"
    elif grep -Fq -- "$_text" "$_file"; then
        bad "$_desc (found '$_text' in $_file)"
    else
        ok "$_desc"
    fi
}

require_regex() {
    _file=$1
    _re=$2
    _desc=$3
    if [ ! -f "$_file" ]; then
        bad "$_desc (missing $_file)"
    elif grep -Eq -- "$_re" "$_file"; then
        ok "$_desc"
    else
        bad "$_desc (missing /$_re/ in $_file)"
    fi
}

forbid_regex() {
    _file=$1
    _re=$2
    _desc=$3
    if [ ! -f "$_file" ]; then
        bad "$_desc (missing $_file)"
    elif grep -Eq -- "$_re" "$_file"; then
        bad "$_desc (found /$_re/ in $_file)"
    else
        ok "$_desc"
    fi
}

echo "== kanata include =="
require_literal "$KBD" "ml-tap (tap-hold-release 200 200" "ml-tap 200/200"
require_literal "$KBD" "ml-free-tap (tap-hold-release 200 200" "ml-free-tap 200/200"
require_literal "$KBD" "(deflayer lyr-mouseless" "overlay layer present"
require_literal "$KBD" "(deflayer lyr-free" "free-mode layer present"
require_literal "$KBD" "j k l" "free-mode movement keys"
require_literal "$KBD" "q w e r" "free-mode button keys"
require_literal "$KBD" "m , . /" "free-mode wheel keys"
forbid_literal "$KBD" "danger-enable-cmd" "no danger-enable-cmd"
forbid_literal "$KBD" "kanata_cmd_allowed" "no kanata_cmd_allowed"
forbid_literal "$KBD" "(cmd " "no cmd action"

echo "== daemon unit =="
require_literal "$UNIT" "User=mouselessd" "runs as mouselessd"
require_literal "$UNIT" "Group=mouseless" "primary group mouseless"
require_literal "$UNIT" "SupplementaryGroups=input" "input group for evdev grab"
require_literal "$UNIT" "RuntimeDirectory=mouseless" "runtime directory"
require_literal "$UNIT" "RuntimeDirectoryMode=0770" "runtime directory mode"
require_literal "$UNIT" "After=kanata.service" "ordered after kanata"
forbid_regex "$UNIT" "^Requires=.*kanata" "no Requires= on kanata"
require_literal "$UNIT" "ExecStart=/usr/local/bin/mouselessd" "installed daemon path"
require_literal "$UNIT" "ProtectSystem=strict" "read-only system sandbox"
require_literal "$UNIT" "NoNewPrivileges=true" "no new privileges"
require_literal "$UNIT" "WantedBy=multi-user.target" "install target"

echo "== kanata drop-in =="
require_literal "$DROPIN" "-p 127.0.0.1:7070" "localhost TCP only"
require_literal "$DROPIN" "RestartForceExitStatus=SIGTERM" "SIGTERM restart (W2)"
require_literal "$DROPIN" "SupplementaryGroups=mouseless" "uinput group"
forbid_regex "$DROPIN" "^[[:space:]]*danger-enable-cmd" "no danger-enable-cmd"
forbid_regex "$DROPIN" "^[[:space:]]*kanata_cmd_allowed" "no kanata_cmd_allowed"
forbid_regex "$DROPIN" "^[[:space:]]*forceful-exit-code" "no forceful-exit-code"

echo "== udev =="
require_literal "$UDEV" 'KERNEL=="uinput", GROUP="mouseless", MODE="0660"' "uinput group access"
forbid_regex "$UDEV" "^[^#]*LIBINPUT_ATTR_POINTER_ACCEL" "no private flat-accel rule"

echo "== sysusers =="
require_regex "$SYSUSERS" "^g mouseless -$" "mouseless group"
require_regex "$SYSUSERS" "^u mouselessd -:mouseless " "mouselessd user with primary mouseless"
require_regex "$SYSUSERS" "^m mouselessd input$" "mouselessd in input"

echo "== cross-file names =="
require_literal "$GRAB_RS" 'pub const KANATA_OUTPUT_NAME: &str = "kanata-mouseless";' "KANATA_OUTPUT_NAME"
require_literal "$UINPUT_RS" 'pub const UINPUT_DEVICE_NAME: &str = "mouseless-mouse";' "UINPUT_DEVICE_NAME"
require_literal "$APPLY" 'linux-output-device-name "kanata-mouseless"' "apply sets kanata output"
require_literal "$APPLY" 'linux-dev-names-exclude ("mouseless-mouse")' "apply excludes mouse device"
require_literal "$EXT_META" '"uuid": "mouseless@sonuscape.example"' "extension UUID"
require_literal "$INSTALL_EXT" 'UUID="mouseless@sonuscape.example"' "installer UUID matches"
require_literal "$INSTALL_EXT" "--extra-source=ipc.js" "extension packs ipc.js"
require_literal "$INSTALL_EXT" "--extra-source=render.js" "extension packs render.js"
require_literal "$INSTALL_SYSTEM" "/usr/lib/sysusers.d/mouseless.conf" "installer sysusers path"
require_literal "$INSTALL_SYSTEM" "/etc/systemd/system/mouselessd.service" "installer unit path"
require_literal "$INSTALL_SYSTEM" "/usr/local/bin/mouselessd" "installer daemon path"

echo "== scripts =="
for _script in "$ROOT"/platform/*.sh "$ROOT"/platform/kanata/*.sh "$ROOT"/scripts/*.sh; do
    [ -f "$_script" ] || continue
    if sh -n "$_script" 2>/dev/null; then
        ok "sh -n $(basename "$_script")"
    else
        bad "sh -n $(basename "$_script")"
    fi
done

echo "== systemd unit syntax =="
if command -v systemd-analyze >/dev/null 2>&1; then
    _tmp="$(mktemp -d)"
    sed 's#^ExecStart=.*#ExecStart=/bin/true#' "$UNIT" >"$_tmp/mouselessd.service"
    chmod 0644 "$_tmp/mouselessd.service"
    if systemd-analyze verify "$_tmp/mouselessd.service" >/dev/null 2>&1; then
        ok "systemd-analyze verify"
    else
        bad "systemd-analyze verify"
    fi
    rm -rf "$_tmp"
else
    echo "skip systemd-analyze verify (not installed)"
fi

if [ "$fail" -ne 0 ]; then
    echo "platform checks failed" >&2
    exit 1
fi
echo "platform checks passed"
