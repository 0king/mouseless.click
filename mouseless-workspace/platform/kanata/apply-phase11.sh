#!/bin/sh
# Mouseless Phase 1.1 + Phase 2: apply kanata config + service drop-in.
# Run from mouseless-workspace/ as: sudo ./platform/kanata/apply-phase11.sh
# Backs up unicfg.kbd and the existing include, applies the 4 UNICFG-EDITS.md
# changes (output name, @ml-tap, @ml-free-tap, include), validates with
# `kanata --check` (restores both files and aborts on failure), then restarts
# kanata + shows TCP state.
set -eu

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$REPO_DIR/platform/kanata/mouseless.kbd"
DROPIN_SRC="$REPO_DIR/platform/systemd/kanata.service.d-override.conf"
CFG=/etc/kanata/unicfg.kbd
INCLUDE=/etc/kanata/mouseless.kbd
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
import sys
p = sys.argv[1]
lines = open(p).read().split('\n')
found_defcfg = False
for i, l in enumerate(lines):
    if l.strip() == 'concurrent-tap-hold yes':
        found_defcfg = True
        if 'linux-output-device-name' not in '\n'.join(lines[i:i+4]):
            lines.insert(i+1, '  linux-output-device-name "kanata-mouseless"')
            lines.insert(i+2, '  linux-dev-names-exclude ("mouseless-mouse")')
        break
if not found_defcfg:
    sys.exit('unicfg.kbd: concurrent-tap-hold yes not found; cannot set output name')
try:
    idx = next(i for i, l in enumerate(lines) if l.startswith('(deflayer lyr-base'))
except StopIteration:
    sys.exit('unicfg.kbd: no (deflayer lyr-base ...) found')
for j in range(idx, min(len(lines), idx + 10)):
    if lines[j].strip() == '_  _  _    _    _    _    _    _    _    _    _    _':
        if '@ml-tap' not in lines[j]:
            lines[j] = lines[j].replace('_', '@ml-tap', 1)
        break
# Free-mode trigger: first token of the row containing @sp (lctl slot).
found_free = False
for j in range(idx, min(len(lines), idx + 12)):
    if '@sp' in lines[j]:
        found_free = True
        if '@ml-free-tap' not in lines[j]:
            lines[j] = lines[j].replace('_', '@ml-free-tap', 1)
        break
if not found_free:
    print('warning: no @sp row found; @ml-free-tap not added', file=sys.stderr)
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

mkdir -p /etc/systemd/system/kanata.service.d
DROPIN_DST=/etc/systemd/system/kanata.service.d/mouseless.conf
if [ -f "$DROPIN_DST" ]; then
  cp -a "$DROPIN_DST" "$DROPIN_DST.bak.$(date +%Y%m%d-%H%M%S)"
fi
install -m 0644 "$DROPIN_SRC" "$DROPIN_DST"
systemctl daemon-reload
systemctl reset-failed kanata.service || true
systemctl restart kanata.service
for _ in 1 2 3 4 5 6 7 8 9 10; do
  systemctl is-active --quiet kanata.service && break
  sleep 1
done
systemctl is-active kanata.service
systemctl show kanata.service -p User -p Group -p SupplementaryGroups
ss -tln | grep 7070 || echo "WARNING: nothing listening on 7070"
grep -i -A1 'Name="kanata' /proc/bus/input/devices | head -4
