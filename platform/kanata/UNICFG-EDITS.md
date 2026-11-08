# unicfg.kbd edits for Mouseless

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

## 2. lyr-base: ControlLeft tap trigger for the overlay (one token)

`defsrc` only accepts physical keys, so the trigger lives in the layer, not
in `defsrc`. In `lyr-base`, replace the `lctl` slot (the first `_` on the row
containing `@sp`) with `@ml-tap`:

```lisp
 (deflayer lyr-base
   ...
-  _  _  _            @sp            _ rctl rmet
+  @ml-tap _  _            @sp            _ rctl rmet
   ...
 )
```

`@ml-tap` is defined in `mouseless.kbd` (the include below):
`tap-hold-release 300 300 (layer-switch lyr-mouseless) lctl`.
A tap (<300 ms) switches to `lyr-mouseless`; holding behaves as left Control.
`ShiftLeft`/`ShiftRight` and every other layer are untouched.

## 3. Include our layers (append at end of file)

```lisp
(include "mouseless.kbd")
```

With: `cp platform/kanata/mouseless.kbd /etc/kanata/mouseless.kbd`
(`include` resolves relative to the main cfg directory.)

## Layer contents

- `lyr-mouseless` — clean 10×30 overlay. Level-1 keys select a column
  (`asdfghjkl;`); level-2 keys select the row inside it (the whole QWERTY
  letter block, emitted plainly):

  ```text
  qwertyuiop
  asdfghjkl;
  zxcvbnm,./
  ```

  `spc` executes the action and `esc` → base. Modifiers are mapped explicitly
  so base aliases (`@ml-tap`, `@sp`) cannot fire while the overlay is open;
  the daemon needs real Shift/Alt press/release events for monitor switching
  and drag.
- `lyr-mouse-precision` — kanata-native precision layer for 1 px movement:
  `i` up, `k` down, `j` left, `l` right, `c` fast, `v` slow; `esc`/`capslock`
  exit to base. All other cells are `_` (transparent, they type into the
  focused app since the daemon does not grab in precision mode). Wheel and
  button cells are intentionally unmapped.
  Entered after a `move` when `behavior.free_mode_after_move` is true, or via
  the optional `keybindings.precision_trigger` tap-hold alias
  `ml-precision-tap`. Has no dedicated trigger by default.

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
