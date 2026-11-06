#!/bin/sh
# Verify the single project version is consistent across the tree.
#
# The source of truth is VERSION. Cargo crates inherit it through
# [workspace.package], and the GNOME extension carries it in metadata.json
# (version-name) and extension.js (extension_version in the Hello frame).
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION_FILE="$ROOT/VERSION"
CARGO="$ROOT/Cargo.toml"
EXT_META="$ROOT/gnome-extension/metadata.json"
EXT_JS="$ROOT/gnome-extension/extension.js"
CHANGELOG="$ROOT/CHANGELOG.md"

fail=0
ok() { printf 'ok   %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1" >&2; fail=1; }

if [ ! -f "$VERSION_FILE" ]; then
    bad "VERSION exists"
    exit 1
fi
VERSION="$(tr -d '[:space:]' <"$VERSION_FILE")"
if [ -n "$VERSION" ]; then
    ok "VERSION is $VERSION"
else
    bad "VERSION is not empty"
    exit 1
fi

# `version = "x"` inside the [workspace.package] table.
CARGO_VERSION="$(
    awk '
        /^\[workspace\.package\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section && $1 == "version" {
            gsub(/["[:space:]]/, "", $3)
            print $3
            exit
        }
    ' "$CARGO"
)"
if [ "$CARGO_VERSION" = "$VERSION" ]; then
    ok "workspace version matches VERSION"
else
    bad "workspace version '$CARGO_VERSION' != VERSION '$VERSION'"
fi

for manifest in "$ROOT"/crates/*/Cargo.toml; do
    [ -f "$manifest" ] || continue
    if grep -Fq 'version.workspace = true' "$manifest"; then
        ok "$(basename "$(dirname "$manifest")") inherits the workspace version"
    else
        bad "$(basename "$(dirname "$manifest")") does not inherit the workspace version"
    fi
done

if grep -Fq "\"version-name\": \"$VERSION\"" "$EXT_META"; then
    ok "extension metadata version-name matches VERSION"
else
    bad "extension metadata version-name does not match VERSION"
fi

if grep -Fq "const EXTENSION_VERSION = '$VERSION';" "$EXT_JS"; then
    ok "extension Hello version matches VERSION"
else
    bad "extension Hello version does not match VERSION"
fi

if grep -Fq "## [$VERSION]" "$CHANGELOG"; then
    ok "CHANGELOG has a $VERSION entry"
else
    bad "CHANGELOG has no $VERSION entry"
fi

if [ "$fail" -ne 0 ]; then
    echo "version checks failed" >&2
    exit 1
fi
echo "version checks passed"
