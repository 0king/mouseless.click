# unicfg.kbd edits for Mouseless (Phase 1.1 + Phase 2)

Four minimal edits to `/etc/kanata/unicfg.kbd` (back it up first).
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

`@ml-tap` is defined in `mouseless.kbd` (the include below):
`tap-hold-release 200 200 (layer-switch lyr-mouseless) lsft`.
`ShiftRight` and every other layer are untouched.

## 3. lyr-base: ControlLeft tap trigger for custom free mode (one token)

In `lyr-base`, the last row (`lctl lmet lalt spc ralt rmet rctl`), replace the
leading `_` (`lctl` slot) with `@ml-free-tap`:

```lisp
 (deflayer lyr-base
   ...
-  _  _  _            @sp            _ rctl rmet
+  @ml-free-tap  _  _            @sp            _ rctl rmet
 )
```

`@ml-free-tap` is defined in `mouseless.kbd`:
`tap-hold-release 200 200 (layer-switch lyr-free) lctl`.
A tap (<200 ms) switches to `lyr-free`; holding behaves as left Control.
This is the default trigger and is configurable in `mouseless.kbd`.

> **Known tuning point:** a plain `tap-hold-release` has no key list, so a
> very fast Ctrl+key chord can be interpreted as a tap. The live
> `concurrent-tap-hold yes` mitigates fast typing, but if misfires appear,
> change `ml-free-tap` to `tap-hold-press` (hold resolves on the next key) or
> `tap-hold-release-keys` with the chord keys. Do not add a second tap-hold
> engine in the daemon (plan SKILL-01).

## 4. Include our layers (append at end of file)

```lisp
(include "mouseless.kbd")
```

With: `cp platform/kanata/mouseless.kbd /etc/kanata/mouseless.kbd`
(`include` resolves relative to the main cfg directory.)

## Layer contents

- `lyr-mouseless` — clean 4×2 overlay: `a s d f j k l ;` cells, `spc` action,
  `esc` → base. Modifiers are mapped explicitly so base aliases (`@ml-tap`,
  `@ml-free-tap`, `@sp`) cannot fire while the overlay is open; the daemon
  needs real Shift/Alt press/release events for monitor switching and drag.
- `lyr-free` — custom free mode for the daemon's physics loop:
  `i/k/j/l` move, `s/d/f` boost, `a` decel, `space` left button,
  `r/e/q/w` right/middle/back/forward, `m/,/.//` wheel, `esc` → base.
  All other keys are `XX` (blocked) so base HRM/layer aliases cannot fire.

## Service (TCP + kill switch, audit W2)

Install `platform/systemd/kanata.service.d-override.conf` as
`/etc/systemd/system/kanata.service.d/mouseless.conf`, then:

```sh
sudo cp mouseless.kbd /etc/kanata/mouseless.kbd
sudo cp kanata.service.d-override.conf /etc/systemd/system/kanata.service.d/mouseless.conf
# apply the 4 unicfg.kbd edits above, then:
kanata --check --cfg /etc/kanata/unicfg.kbd   # must say "config file is valid"
sudo systemctl daemon-reload
sudo systemctl restart kanata.service
ss -tln | grep 7070                            # TCP listening on localhost
```

`--emergency-exit-code 1` is kept for parity, but on Linux kanata raises
`SIGTERM` directly — hence `RestartForceExitStatus=SIGTERM` in the drop-in.
Never add `forceful-exit-code` to defcfg (invalid option).
