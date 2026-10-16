# unicfg.kbd edits for Mouseless Phase 1.1

Three minimal edits to `/etc/kanata/unicfg.kbd` (back it up first).
`kanata --check --cfg` must pass before restarting the service.

## 1. defcfg: stable output name + mouse exclusion

```lisp
(defcfg
  process-unmapped-keys yes
  linux-device-detect-mode keyboard-only
  concurrent-tap-hold yes
  linux-output-device-name "kanata-mouseless"
  linux-dev-names-exclude ("mouseless-mouse")
)
```

Renames the virtual output from `kanata` to `kanata-mouseless` so
`mouselessd` finds it without `/dev/input/eventN` hardcodes (plan §5),
and keeps kanata from re-reading our uinput mouse (loop prevention).

## 2. lyr-base: ShiftLeft tap trigger (one token)

`defsrc` only accepts physical keys, so the trigger lives in the layer, not
in `defsrc`. In `lyr-base`, row 5, replace the `lsft` slot (`_`) with `@ml-tap`:

```lisp
 (deflayer lyr-base
   ...
-  _  _  _    _    _    _    _    _    _    _    _    _
+  @ml-tap _  _    _    _    _    _    _    _    _    _    _
   ...
 )
```

`@ml-tap` is defined in `mouseless.kbd` (the include below).
`ShiftRight` and every other layer are untouched. In `lyr-mouseless` the
`lsft` slot stays transparent, so holding Shift in the overlay is plain Shift.

## 3. Include our layer (append at end of file)

```lisp
(include "mouseless.kbd")
```

With: `cp platform/kanata/mouseless.kbd /etc/kanata/mouseless.kbd`
(`include` resolves relative to the main cfg directory.)

## Service (TCP + kill switch, audit W2)

Install `platform/systemd/kanata.service.d-override.conf` as
`/etc/systemd/system/kanata.service.d/mouseless.conf`, then:

```sh
sudo cp mouseless.kbd /etc/kanata/mouseless.kbd
sudo cp kanata.service.d-override.conf /etc/systemd/system/kanata.service.d/mouseless.conf
# apply the 3 unicfg.kbd edits above, then:
kanata --check --cfg /etc/kanata/unicfg.kbd   # must say "config file is valid"
sudo systemctl daemon-reload
sudo systemctl restart kanata.service
ss -tln | grep 7070                            # TCP listening on localhost
```

`--emergency-exit-code 1` is kept for parity, but on Linux kanata raises
`SIGTERM` directly — hence `RestartForceExitStatus=SIGTERM` in the drop-in.
Never add `forceful-exit-code` to defcfg (invalid option).
