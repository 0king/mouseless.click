#!/usr/bin/env bash
# Headless GNOME Shell smoke test for the draw-only overlay (SPEC-overlay).
#
# Runs a private D-Bus session, packs/installs the extension into a throwaway
# XDG_DATA_HOME, drives every v1 frame from a mock daemon, then verifies:
#   - Hello is first and Report follows
#   - Show/HighlightCell/ShowSubgrid/QueryCursor/CursorReply/Hide round-trip
#   - EOF hides and the extension reconnects (client 2 in the mock log)
#   - no uncaught JS error reaches the Shell
#
# Requires: gnome-shell, gnome-extensions, gjs, unzip, dbus-run-session.
# No real session is touched (the whole script re-execs under dbus-run-session).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
EXT_UUID=mouseless@sonuscape.example

if [[ ${MOUSELESS_SMOKE_INNER:-} != 1 ]]; then
    MOUSELESS_SMOKE_INNER=1 exec dbus-run-session -- "$0" "$@"
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/mouseless-smoke.XXXXXX")
SHELL_PID=
MOCK_PID=

cleanup() {
    [[ -n $SHELL_PID ]] && kill "$SHELL_PID" 2>/dev/null || true
    [[ -n $MOCK_PID ]] && kill "$MOCK_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "smoke: FAIL: $*" >&2
    echo "--- mock log ---" >&2
    cat "$WORK/mock.log" >&2 2>/dev/null || true
    echo "--- shell log (tail) ---" >&2
    tail -40 "$WORK/shell.log" >&2 2>/dev/null || true
    exit 1
}

export XDG_DATA_HOME="$WORK/xdg"
export MOUSELESS_SOCKET="$WORK/mouseless.sock"
mkdir -p "$XDG_DATA_HOME/gnome-shell/extensions/$EXT_UUID"

(
    cd "$ROOT/gnome-extension"
    gnome-extensions pack . --force -o "$WORK" \
        --extra-source=ipc.js --extra-source=render.js
)
unzip -q "$WORK/$EXT_UUID.shell-extension.zip" -d \
    "$XDG_DATA_HOME/gnome-shell/extensions/$EXT_UUID"

gjs -m "$ROOT/gnome-extension/tests/mock-daemon.js" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
gnome-shell --headless --no-x11 --virtual-monitor 1920x1080 >"$WORK/shell.log" 2>&1 &
SHELL_PID=$!

for _ in $(seq 1 50); do
    [[ -S $MOUSELESS_SOCKET ]] && break
    sleep 0.1
done
[[ -S $MOUSELESS_SOCKET ]] || fail "mock socket never appeared"

sleep 4
gnome-extensions enable "$EXT_UUID" || fail "could not enable extension"
sleep 8
gnome-extensions disable "$EXT_UUID" >/dev/null 2>&1 || true
sleep 1

for needle in \
    'mock: got Hello' \
    'mock: got Report' \
    'mock: sent Show' \
    'mock: sent HighlightCell' \
    'mock: sent ShowSubgrid' \
    'mock: sent QueryCursor' \
    'mock: got CursorReply' \
    'mock: sent Hide' \
    'mock: client 2 connected'
do
    grep -Fq "$needle" "$WORK/mock.log" || fail "missing: $needle"
done

if grep -Fq 'JS ERROR' "$WORK/shell.log"; then
    fail "uncaught JS error in the Shell log"
fi

echo "smoke: PASS (all frames rendered, EOF-hide + reconnect observed)"
