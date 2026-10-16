#!/bin/sh
# Mouseless Phase 1.1: apply kanata config + service drop-in.
# Run from mouseless-workspace/ as: sudo ./platform/kanata/apply-phase11.sh
# Backs up unicfg.kbd, applies the 3 UNICFG-EDITS.md changes, validates with
# `kanata --check` (aborts on failure), then restarts kanata + shows TCP state.
set -eu

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$REPO_DIR/platform/kanata/mouseless.kbd"
DROPIN_SRC="$REPO_DIR/platform/systemd/kanata.service.d-override.conf"
CFG=/etc/kanata/unicfg.kbd
BAK="/etc/kanata/unicfg.kbd.pre-mouseless-$(date +%Y%m%d-%H%M%S)"

cp "$CFG" "$BAK"
echo "backup: $BAK"

python3 - "$CFG" <<'EOF'
import sys
p = sys.argv[1]
lines = open(p).read().split('\n')
for i, l in enumerate(lines):
    if l.strip() == 'concurrent-tap-hold yes':
        if 'linux-output-device-name' not in '\n'.join(lines[i:i+4]):
            lines.insert(i+1, '  linux-output-device-name "kanata-mouseless"')
            lines.insert(i+2, '  linux-dev-names-exclude ("mouseless-mouse")')
        break
idx = next(i for i, l in enumerate(lines) if l.startswith('(deflayer lyr-base'))
for j in range(idx, idx+10):
    if lines[j].strip() == '_  _  _    _    _    _    _    _    _    _    _    _':
        if '@ml-tap' not in lines[j]:
            lines[j] = lines[j].replace('_', '@ml-tap', 1)
        break
s = '\n'.join(lines)
if '(include "mouseless.kbd")' not in s:
    s = s.rstrip() + '\n\n(include "mouseless.kbd")\n'
open(p, 'w').write(s)
EOF

cp "$SRC" /etc/kanata/mouseless.kbd
echo "installed /etc/kanata/mouseless.kbd"

kanata --check --cfg "$CFG" || {
  echo "VALIDATION FAILED -- restoring backup"
  cp "$BAK" "$CFG"
  exit 1
}

mkdir -p /etc/systemd/system/kanata.service.d
cp "$DROPIN_SRC" /etc/systemd/system/kanata.service.d/mouseless.conf
systemctl daemon-reload
systemctl restart kanata.service
sleep 1
systemctl is-active kanata.service
ss -tln | grep 7070 || echo "WARNING: nothing listening on 7070"
grep -i -A1 'Name="kanata' /proc/bus/input/devices | head -4
