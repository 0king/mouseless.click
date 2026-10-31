#!/bin/sh
# Mouseless GNOME extension installer (per desktop user).
#
# Run from mouseless-workspace/ as your desktop user:
#   ./platform/install-extension.sh
#
# Packs the draw-only extension (extension.js + ipc.js + render.js), installs
# it for the current user, and enables it. On Wayland, log out and back in so
# GNOME Shell loads the new extension; on X11, Alt+F2 → r → Enter also works.
#
# Options:
#   --dry-run   print actions without installing
#   --help
set -eu

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
EXT_DIR="$REPO_DIR/gnome-extension"
UUID="mouseless@sonuscape.example"
DRY_RUN=0

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
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

if [ "$(id -u)" -eq 0 ]; then
    echo "run as your desktop user, not root: ./platform/install-extension.sh" >&2
    exit 1
fi

if ! command -v gnome-extensions >/dev/null 2>&1; then
    echo "gnome-extensions not found; install the GNOME Shell tools" >&2
    exit 1
fi

if [ ! -f "$EXT_DIR/metadata.json" ]; then
    echo "missing $EXT_DIR/metadata.json" >&2
    exit 1
fi

if ! grep -Fq "\"uuid\": \"$UUID\"" "$EXT_DIR/metadata.json"; then
    echo "metadata.json does not declare uuid $UUID" >&2
    exit 1
fi

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  + %s\n' "$*"
    else
        "$@"
    fi
}

PACK_DIR="$(mktemp -d)"
trap 'rm -rf "$PACK_DIR"' EXIT

if [ "$DRY_RUN" -eq 1 ]; then
    echo "mouseless: extension install (dry run) — no changes will be made"
else
    echo "mouseless: extension install"
fi

run gnome-extensions pack "$EXT_DIR" --force -o "$PACK_DIR" \
    --extra-source=ipc.js --extra-source=render.js
run gnome-extensions install --force "$PACK_DIR/$UUID.shell-extension.zip"

if ! run gnome-extensions enable "$UUID"; then
    echo "extension installed but not enabled yet; after logging in run:" >&2
    echo "  gnome-extensions enable $UUID" >&2
fi

if [ "$DRY_RUN" -eq 1 ]; then
    echo "Dry run: extension $UUID would be installed."
else
    echo "Extension $UUID installed."
fi

cat <<EOF
On Wayland, log out and back in so GNOME Shell loads it.
Verify:
  gnome-extensions info $UUID
  gnome-extensions list --enabled
  systemctl status mouselessd.service
EOF
