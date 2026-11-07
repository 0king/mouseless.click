#!/bin/sh
# Mouseless Phase 1.1 + Phase 2: apply kanata config + service drop-in.
# Run from repo root as: sudo ./platform/kanata/apply-kanata-config.sh
# Backs up unicfg.kbd and the existing include, applies the UNICFG-EDITS.md
# changes (output name, @ml-tap, include), validates with
# `kanata --check` (restores both files and aborts on failure), then restarts
# kanata + shows TCP state.
set -eu

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$REPO_DIR/platform/kanata/mouseless.kbd"
DROPIN_SRC="$REPO_DIR/platform/systemd/kanata.service.d-override.conf"
CFG=/etc/kanata/unicfg.kbd
INCLUDE=/etc/kanata/mouseless.kbd
KANATA_ADDR="${MOUSELESS_KANATA_ADDR:-127.0.0.1:7070}"
KANATA_OUTPUT_NAME="${MOUSELESS_KANATA_OUTPUT_NAME:-kanata-mouseless}"
MOUSE_NAME="${MOUSELESS_MOUSE_NAME:-mouseless-mouse}"
STAMP="$(date +%Y%m%d-%H%M%S)"
BAK="/etc/kanata/unicfg.kbd.pre-mouseless-$STAMP"
INCLUDE_BAK=""
if [ -f "$INCLUDE" ]; then
    INCLUDE_BAK="$INCLUDE.pre-mouseless-$STAMP"
    cp -a "$INCLUDE" "$INCLUDE_BAK"
fi

cp "$CFG" "$BAK"
echo "backup: $BAK"

python3 - "$CFG" <<'EOF'
import os
import sys
p = sys.argv[1]
output_name = os.environ.get("MOUSELESS_KANATA_OUTPUT_NAME", "kanata-mouseless")
mouse_name = os.environ.get("MOUSELESS_MOUSE_NAME", "mouseless-mouse")
lines = open(p).read().split('\n')
found_defcfg = False
output_line = f'  linux-output-device-name "{output_name}"'
exclude_line = f'  linux-dev-names-exclude ("{mouse_name}")'
for i, l in enumerate(lines):
    if l.strip() == 'concurrent-tap-hold yes':
        found_defcfg = True
        output_set = False
        exclude_set = False
        # Replace existing directives so a changed TOML name is applied on a
        # re-run instead of silently keeping the previous value.
        for idx, existing in enumerate(lines):
            if existing.strip().startswith('linux-output-device-name'):
                lines[idx] = output_line
                output_set = True
            elif existing.strip().startswith('linux-dev-names-exclude'):
                lines[idx] = exclude_line
                exclude_set = True
        insert_at = i + 1
        if not output_set:
            lines.insert(insert_at, output_line)
            insert_at += 1
        if not exclude_set:
            lines.insert(insert_at, exclude_line)
        break
if not found_defcfg:
    sys.exit('unicfg.kbd: concurrent-tap-hold yes not found; cannot set output name')
try:
    idx = next(i for i, l in enumerate(lines) if l.startswith('(deflayer lyr-base'))
except StopIteration:
    sys.exit('unicfg.kbd: no (deflayer lyr-base ...) found')
# Overlay trigger: first token of the row containing @sp (lctl slot).
found_overlay = False
for j in range(idx, min(len(lines), idx + 12)):
    if '@sp' in lines[j]:
        found_overlay = True
        if '@ml-tap' not in lines[j]:
            lines[j] = lines[j].replace('_', '@ml-tap', 1)
        break
if not found_overlay:
    print('warning: no @sp row found; @ml-tap not added', file=sys.stderr)
s = '\n'.join(lines)
if '(include "mouseless.kbd")' not in s:
    s = s.rstrip() + '\n\n(include "mouseless.kbd")\n'
open(p, 'w').write(s)
EOF

cp "$SRC" "$INCLUDE"
echo "installed $INCLUDE"

if ! kanata --check --cfg "$CFG"; then
  echo "VALIDATION FAILED -- restoring backups"
  cp "$BAK" "$CFG"
  if [ -n "$INCLUDE_BAK" ]; then
    cp -a "$INCLUDE_BAK" "$INCLUDE"
  else
    rm -f "$INCLUDE"
  fi
  exit 1
fi

# The drop-in names `input` + `uinput` so it also works on hosts where
# kanata-svc has no device permissions yet. systemd refuses to start a unit
# whose SupplementaryGroups= names a missing group, so create them first
# (idempotent: getent succeeds when they already exist).
for _grp in input uinput; do
  if ! getent group "$_grp" >/dev/null 2>&1; then
    groupadd "$_grp"
    echo "created missing group $_grp (needed by SupplementaryGroups=)"
  fi
done

mkdir -p /etc/systemd/system/kanata.service.d
DROPIN_DST=/etc/systemd/system/kanata.service.d/mouseless.conf
DROPIN_TMP="$(mktemp "${TMPDIR:-/tmp}/mouseless-kanata-dropin.XXXXXX")"
trap 'rm -f "$DROPIN_TMP"' EXIT HUP INT TERM
sed "s|127.0.0.1:7070|$KANATA_ADDR|g" "$DROPIN_SRC" >"$DROPIN_TMP"
if [ -f "$DROPIN_DST" ]; then
  cp -a "$DROPIN_DST" "$DROPIN_DST.bak.$(date +%Y%m%d-%H%M%S)"
fi
install -m 0644 "$DROPIN_TMP" "$DROPIN_DST"
systemctl daemon-reload
systemctl reset-failed kanata.service || true
systemctl restart kanata.service
for _ in 1 2 3 4 5 6 7 8 9 10; do
  systemctl is-active --quiet kanata.service && break
  sleep 1
done
systemctl is-active kanata.service
systemctl show kanata.service -p User -p Group -p SupplementaryGroups
ss -tln | grep "${KANATA_ADDR##*:}" ||
  echo "WARNING: nothing listening on $KANATA_ADDR"
grep -i -A1 'Name="kanata' /proc/bus/input/devices | head -4
