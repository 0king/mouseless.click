#!/bin/sh
# Mouseless platform checks (plan §8 #1/#5/#6; SPEC-platform).
#
# Run from mouseless-workspace/:
#   ./scripts/check-platform.sh
#
# Asserts the installable files agree with the Rust constants and the
# documented contracts:
# - kanata include: tap-hold timings, overlay/free layers, no cmd path;
# - daemon unit: user/group, input + uinput groups, runtime dir, sandboxing,
#   no Requires=;
# - kanata drop-in: localhost TCP, SIGTERM restart, idempotent
#   SupplementaryGroups=input uinput, never the human socket group;
# - udev: /dev/uinput on the existing uinput group, never the human socket
#   group; no ineffective flat-accel rule;
# - sysusers: mouselessd/mouseless/uinput/input; idempotency proven in a
#   temp root (kanata GIDs preserved, second run is a no-op);
# - installer: never adds a human account to input/uinput;
# - desktop entry: scheme handler, single-URL Exec, settings launcher, no
#   shell wrapper;
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
DESKTOP="$ROOT/platform/desktop/mouseless-url-handler.desktop"
SETTINGS_DESKTOP="$ROOT/platform/desktop/mouseless-settings.desktop"
INSTALL_SYSTEM="$ROOT/platform/install-system.sh"
INSTALL_EXT="$ROOT/platform/install-extension.sh"
EXT_META="$ROOT/gnome-extension/metadata.json"
EXT_CONFIG="$ROOT/gnome-extension/config.js"
GRAB_RS="$ROOT/crates/mouseless-kbd/src/grab.rs"
UINPUT_RS="$ROOT/crates/mouseless-mouse/src/impls/uinput.rs"
TOML="$ROOT/mouseless.toml"

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

# Extract one quoted string from mouseless.toml; the Rust loader is the
# authority on validity, this only keeps platform files in sync.
toml_value() {
    sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" \
        "$TOML" | head -n 1
}

echo "== application config =="
if [ -f "$TOML" ]; then
    ok "mouseless.toml exists"
else
    bad "mouseless.toml exists"
fi
for section in paths kanata daemon devices layers deep_link ui appearance ipc; do
    require_literal "$TOML" "[$section]" "mouseless.toml [$section]"
done
TOML_SCHEME="$(toml_value scheme)"
TOML_SOCKET="$(toml_value socket)"
TOML_KBD_NAME="$(toml_value kanata_output_name)"
TOML_MOUSE_NAME="$(toml_value mouse_name)"
TOML_INCLUDE="$(toml_value include_name)"
TOML_ADDRESS="$(toml_value address)"
TOML_GRID_COLOR="$(toml_value grid_color)"
TOML_LABEL_COLOR="$(toml_value label_color)"
TOML_HIGHLIGHT_COLOR="$(toml_value highlight_color)"
if [ -n "$TOML_SCHEME" ] && [ -n "$TOML_SOCKET" ] && [ -n "$TOML_KBD_NAME" ] &&
    [ -n "$TOML_MOUSE_NAME" ] && [ -n "$TOML_INCLUDE" ] &&
    [ -n "$TOML_GRID_COLOR" ] && [ -n "$TOML_LABEL_COLOR" ] &&
    [ -n "$TOML_HIGHLIGHT_COLOR" ] && [ -n "$TOML_ADDRESS" ]; then
    ok "mouseless.toml has scheme/socket/device/colour values"
else
    bad "mouseless.toml has scheme/socket/device/include values"
fi
require_literal "$TOML" 'state_dir = "/var/lib/mouseless"' "state dir default"
require_literal "$TOML" 'config_file = "config.yaml"' "settings document name"

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
require_literal "$UNIT" "SupplementaryGroups=input uinput" "input + uinput groups for daemon"
require_literal "$UNIT" "RuntimeDirectory=mouseless" "runtime directory"
require_literal "$UNIT" "RuntimeDirectoryMode=0770" "runtime directory mode"
require_literal "$UNIT" "After=kanata.service" "ordered after kanata"
forbid_regex "$UNIT" "^Requires=.*kanata" "no Requires= on kanata"
require_literal "$UNIT" "ExecStart=/usr/local/bin/mouselessd" "installed daemon path"
forbid_literal "$UNIT" "--kanata" "unit does not hardcode the kanata endpoint"
require_literal "$UNIT" "ProtectSystem=strict" "read-only system sandbox"
require_literal "$UNIT" "NoNewPrivileges=true" "no new privileges"
require_literal "$UNIT" "WantedBy=multi-user.target" "install target"

echo "== kanata drop-in =="
require_literal "$DROPIN" "-p 127.0.0.1:7070" "localhost TCP only"
require_literal "$DROPIN" "RestartForceExitStatus=SIGTERM" "SIGTERM restart (W2)"
require_literal "$DROPIN" "SupplementaryGroups=input uinput" "kanata gets idempotent input + uinput"
forbid_regex "$DROPIN" "^[[:space:]]*SupplementaryGroups=.*mouseless([^-[:alnum:]_]|$)" "kanata never gets the human socket group"
forbid_regex "$DROPIN" "^[[:space:]]*danger-enable-cmd" "no danger-enable-cmd"
forbid_regex "$DROPIN" "^[[:space:]]*kanata_cmd_allowed" "no kanata_cmd_allowed"
forbid_regex "$DROPIN" "^[[:space:]]*forceful-exit-code" "no forceful-exit-code"

echo "== udev =="
require_literal "$UDEV" 'KERNEL=="uinput", GROUP="uinput", MODE="0660"' "uinput group access"
forbid_regex "$UDEV" '^[^#]*GROUP="mouseless"' "human socket group never owns /dev/uinput"
forbid_regex "$UDEV" "^[^#]*LIBINPUT_ATTR_POINTER_ACCEL" "no private flat-accel rule"

echo "== sysusers =="
require_regex "$SYSUSERS" "^g mouseless -$" "mouseless (socket) group"
require_regex "$SYSUSERS" "^g input -$" "input group (idempotent)"
require_regex "$SYSUSERS" "^g uinput -$" "uinput group (idempotent)"
require_regex "$SYSUSERS" "^u mouselessd -:mouseless " "mouselessd user with primary mouseless"
require_regex "$SYSUSERS" "^m mouselessd input$" "mouselessd in input"
require_regex "$SYSUSERS" "^m mouselessd uinput$" "mouselessd in uinput"

echo "== sysusers idempotency =="
if command -v systemd-sysusers >/dev/null 2>&1; then
    # Temp root seeded like a host where kanata already has input/uinput.
    _sr="$(mktemp -d)"
    mkdir -p "$_sr/etc" "$_sr/usr/lib/sysusers.d"
    cp "$SYSUSERS" "$_sr/usr/lib/sysusers.d/mouseless.conf"
    printf 'root:x:0:\ninput:x:994:kanata-svc\nuinput:x:972:kanata-svc\n' \
        >"$_sr/etc/group"
    printf 'root:*::\ninput:!::kanata-svc\nuinput:!::kanata-svc\n' \
        >"$_sr/etc/gshadow"
    printf 'root:x:0:0:root:/root:/bin/sh\n' >"$_sr/etc/passwd"
    printf 'root:*:19000:0:99999:7:::\n' >"$_sr/etc/shadow"
    chmod 600 "$_sr/etc/shadow" "$_sr/etc/gshadow"
    _rc=0
    systemd-sysusers --root="$_sr" >"$_sr/run1.log" 2>&1 || _rc=$?
    _mgid="$(awk -F: '$1=="mouseless"{print $3}' "$_sr/etc/group")"
    if [ "$_rc" -eq 0 ] && [ -n "$_mgid" ] &&
        grep -qx 'input:x:994:kanata-svc,mouselessd' "$_sr/etc/group" &&
        grep -qx 'uinput:x:972:kanata-svc,mouselessd' "$_sr/etc/group" &&
        grep -qx "mouselessd:x:.*:$_mgid:.*:/usr/sbin/nologin" "$_sr/etc/passwd"; then
        ok "sysusers keeps kanata GIDs and adds only mouselessd"
    else
        bad "sysusers keeps kanata GIDs and adds only mouselessd"
    fi
    _sum1="$(cksum "$_sr/etc/group" "$_sr/etc/passwd" "$_sr/etc/shadow" "$_sr/etc/gshadow")"
    if systemd-sysusers --root="$_sr" >"$_sr/run2.log" 2>&1 &&
        [ "$_sum1" = "$(cksum "$_sr/etc/group" "$_sr/etc/passwd" "$_sr/etc/shadow" "$_sr/etc/gshadow")" ]; then
        ok "sysusers second run is a no-op"
    else
        bad "sysusers second run is a no-op"
    fi
    rm -rf "$_sr"
else
    echo "skip sysusers idempotency (systemd-sysusers not installed)"
fi

echo "== desktop deep links =="
require_literal "$DESKTOP" "Exec=/usr/local/bin/mouseless %u" "scheme handler passes one URL to the CLI"
require_literal "$DESKTOP" "MimeType=x-scheme-handler/$TOML_SCHEME;" "registers the configured scheme"
require_literal "$DESKTOP" "TryExec=/usr/local/bin/mouseless" "handler hidden when the CLI is absent"
forbid_regex "$DESKTOP" "^Exec=.*(sh -c|bash -c)" "no shell wrapper in Exec"
require_literal "$SETTINGS_DESKTOP" "Exec=/usr/local/bin/mouseless-settings %u" "settings launcher opens the app"
require_literal "$SETTINGS_DESKTOP" "TryExec=/usr/local/bin/mouseless-settings" "settings launcher hidden when the app is absent"
forbid_regex "$SETTINGS_DESKTOP" "^Exec=.*(sh -c|bash -c)" "no shell wrapper in the settings launcher"
if command -v desktop-file-validate >/dev/null 2>&1; then
    if desktop-file-validate "$DESKTOP" "$SETTINGS_DESKTOP" >/dev/null 2>&1; then
        ok "desktop-file-validate"
    else
        bad "desktop-file-validate"
    fi
else
    echo "skip desktop-file-validate (not installed)"
fi

echo "== installer privilege model =="
require_literal "$INSTALL_SYSTEM" 'usermod -aG mouseless "$_target"' "only mouseless is added automatically"
forbid_regex "$INSTALL_SYSTEM" "usermod -aG (input|uinput)" "never add a human to input/uinput"
forbid_regex "$INSTALL_SYSTEM" "for _grp in mouseless input" "no combined mouseless+input group add"

echo "== cross-file names =="
require_literal "$GRAB_RS" "pub const KANATA_OUTPUT_NAME: &str = \"$TOML_KBD_NAME\";" "KANATA_OUTPUT_NAME matches mouseless.toml"
require_literal "$UINPUT_RS" "pub const UINPUT_DEVICE_NAME: &str = \"$TOML_MOUSE_NAME\";" "UINPUT_DEVICE_NAME matches mouseless.toml"
require_literal "$APPLY" 'MOUSELESS_KANATA_OUTPUT_NAME' "apply reads the configured kanata output name"
require_literal "$APPLY" 'MOUSELESS_MOUSE_NAME' "apply reads the configured mouse name"
require_literal "$APPLY" 'MOUSELESS_KANATA_ADDR' "apply reads the configured endpoint"
require_literal "$APPLY" 'linux-output-device-name' "apply writes linux-output-device-name"
require_literal "$APPLY" 'linux-dev-names-exclude' "apply writes linux-dev-names-exclude"
require_literal "$INSTALL_SYSTEM" 'MOUSELESS_KANATA_OUTPUT_NAME' "installer passes the kanata output name"
require_literal "$INSTALL_SYSTEM" 'MOUSELESS_MOUSE_NAME' "installer passes the mouse name"
require_literal "$INSTALL_SYSTEM" 'MOUSELESS_KANATA_ADDR' "installer passes the endpoint"
require_literal "$INSTALL_SYSTEM" 'install_kanata_dropin' "installer regenerates the kanata drop-in"
require_literal "$APPLY" 'groupadd "$_grp"' "apply creates missing device groups"
require_literal "$EXT_META" '"uuid": "mouseless@0king.dev"' "extension UUID"
require_literal "$EXT_CONFIG" "export const CONFIGURED_SOCKET_PATH = '$TOML_SOCKET';" "extension socket matches mouseless.toml"
require_literal "$EXT_CONFIG" "grid_color: hex('$TOML_GRID_COLOR')" "extension grid colour matches mouseless.toml"
require_literal "$EXT_CONFIG" "label_color: hex('$TOML_LABEL_COLOR')" "extension label colour matches mouseless.toml"
require_literal "$EXT_CONFIG" "highlight_color: hex('$TOML_HIGHLIGHT_COLOR')" "extension highlight colour matches mouseless.toml"
require_literal "$INSTALL_EXT" 'UUID="mouseless@0king.dev"' "installer UUID matches"
require_literal "$INSTALL_EXT" '--extra-source=config.js' "installer packs generated config.js"
require_literal "$INSTALL_EXT" 'CONFIGURED_SOCKET_PATH' "installer generates extension socket config"
require_literal "$INSTALL_EXT" "--extra-source=ipc.js" "extension packs ipc.js"
require_literal "$INSTALL_EXT" "--extra-source=render.js" "extension packs render.js"
require_literal "$INSTALL_SYSTEM" "/usr/lib/sysusers.d/mouseless.conf" "installer sysusers path"
require_literal "$INSTALL_SYSTEM" "/etc/systemd/system/mouselessd.service" "installer unit path"
require_literal "$INSTALL_SYSTEM" "/usr/local/bin/mouselessd" "installer daemon path"
require_literal "$INSTALL_SYSTEM" "/usr/local/bin/mouseless-settings" "installer settings path"
require_literal "$INSTALL_SYSTEM" "/usr/share/applications/mouseless-url-handler.desktop" "installer desktop path"
require_literal "$INSTALL_SYSTEM" "/usr/share/applications/mouseless-settings.desktop" "installer settings desktop path"
require_literal "$INSTALL_SYSTEM" "/etc/mouseless/mouseless.toml" "installer app config path"
require_literal "$INSTALL_SYSTEM" 'x-scheme-handler/$SCHEME' "installer registers the configured scheme"

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

    # The kanata drop-in must merge with a unit whose service account has no
    # supplementary groups yet ("kanata has no permissions" host). The
    # ExecStart reset pair is verified together with the real file.
    _tmp2="$(mktemp -d)"
    mkdir -p "$_tmp2/kanata.service.d"
    printf '[Unit]\nDescription=fake kanata\n[Service]\nExecStart=/bin/true\n' \
        >"$_tmp2/kanata.service"
    sed 's#^ExecStart=/usr/local/bin/kanata .*#ExecStart=/bin/true#' "$DROPIN" \
        >"$_tmp2/kanata.service.d/mouseless.conf"
    if SYSTEMD_UNIT_PATH="$_tmp2:" systemd-analyze verify kanata.service >/dev/null 2>&1; then
        ok "systemd-analyze verify (kanata drop-in)"
    else
        bad "systemd-analyze verify (kanata drop-in)"
    fi
    rm -rf "$_tmp2"
else
    echo "skip systemd-analyze verify (not installed)"
fi

echo "== version =="
if ! "$ROOT/scripts/check-version.sh"; then
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "platform checks failed" >&2
    exit 1
fi
echo "platform checks passed"
