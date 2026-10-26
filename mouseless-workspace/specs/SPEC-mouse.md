# SPEC-mouse — Mouse backend (`mouseless-mouse`)

> Module: `mouse` · Paths: `crates/mouseless-mouse` · Depends on: — ·
> Plan refs: §2.1, §2.4, §4, §8 #5 · Matrix rows: #5 (with `platform`), #6 (with `platform`)
> Status as of 2026-10-18: **Implemented** for Phase-1 scope; accuracy verified
> only in e2e (Phase 1.4); hi-res wheel undecided.

## Objective

Give the daemon a sanctioned way to move and click the system cursor: our own
uinput mouse device (`mouseless-mouse`), because kanata cannot absolute-position
on Linux (`setmouse`/`SetMouse` are no-ops — plan §2.1). Absolute clicks are
achieved by computing a REL delta from the extension-reported cursor position.

## Scope

- In: `MouseBackend` trait, uinput device creation/lifecycle, `compute_delta`,
  button/scroll emission.
- Out (Phase 4): `libei`/portal backend. Out (state-owned): multi-click
  cadence, drag state, modifier semantics — the daemon executes those as
  sequences of `click`/`move_rel` calls.

## Contracts

### `MouseBackend` (trait — pin)

```rust
fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error>;
fn click(&mut self, button: MouseButton) -> Result<(), Self::Error>;   // down+up
fn scroll(&mut self, dx_lines: i32, dy_lines: i32) -> Result<(), Self::Error>;
```

`MouseButton`: `Left, Right, Middle, Back, Forward` (serde-stable names).

### uinput device (pinned attributes)

| Attribute | Value | Why |
|---|---|---|
| Name | `mouseless-mouse` | Must match kanata `linux-dev-names-exclude` and the udev flat-accel rule (loop prevention + G1). |
| Vendor | `0x736e` (`VENDOR_ID_SONUSCAPE`) | Matches upstream Mouseless. |
| Capabilities | `REL_X`, `REL_Y`, `REL_WHEEL` (v+h), 5 buttons | Everything the daemon emits. |
| Sync | `synchronize()` after each logical action | Batching is allowed if it keeps one SYN per action. |
| Lifetime | Device destroyed on `Drop` | No zombie devices across daemon restarts. |

### Delta contract (matrix #5)

- `compute_delta(target, current) = (target.0 − current.0, target.1 − current.1)`
  with `saturating_sub` — never panics, saturates at `i32` edges.
- Both inputs are **global physical pixels** in the `core::Monitor` space
  (compositor layout, origin top-left, offsets included).
- Accuracy depends on the flat pointer-acceleration udev rule
  (`platform/udev/99-mouseless.rules`); without it libinput distorts large
  deltas (audit G1). The rule's presence is asserted by `platform` tests.

### Degraded mode

If `/dev/uinput` is unavailable/denied, `UinputMouse::create` fails; the daemon
MUST continue running ("overlay unavailable, typing unaffected" — plan §6),
log loudly, and expose the state in `status`. Never panic, never retry-storm.

## Tech stack

Rust 2021; `uinput` 0.1, `thiserror`, `serde` (button names).

## Commands

```sh
cargo test -p mouseless-mouse
# live check (needs mouseless group + udev rule):
ls -l /dev/input/by-id/ | grep mouseless   # device name check on a live system
```

## Project structure

`src/lib.rs` — trait, `MouseButton`, `compute_delta`, tests.
`src/impls/uinput.rs` — real device. `src/impls/fake.rs` — `FakeMouse` recorder.

## Code style

- Every fallible syscall maps to `MouseError::Io` with context, no `unwrap` on
  the emit path.
- Comments on the device builder note the two cross-file couplings (kanata
  exclude, udev rule) so nobody "cleans up" the name.

## Testing strategy

- #5: basic/negative/zero deltas. *(exist)*
- #5: saturation at `i32::MIN/MAX`. *(exists)*
- #5: multi-monitor offsets — target/current from different monitors, negative
  origins, e.g. `compute_delta((-1200, 0), (1920, 540))`. *(add)*
- `FakeMouse` records move/click/scroll order. *(exists)*
- Phase 1.4 e2e (with `core`/`daemon`/`overlay`): click lands within tolerance
  of cell center at 100/125/150/200% scale, 1080p/4K/vertical.
- Degraded mode: daemon test with a failing mouse backend keeps the session
  alive and typing unaffected. *(with `daemon`)*

## Boundaries

- **Always:** keep device name/vendor stable; saturating math on deltas;
  synchronize per action; document the flat-accel dependency wherever accuracy
  is discussed.
- **Ask first:** changing device attributes (name/vendor/capabilities) —
  three other files depend on them; switching to hi-res wheel events;
  adding a second device.
- **Never:** create the mouse without the kanata exclusion (loop risk);
  emit absolute events/depend on `SetMouse`; block indefinitely on device I/O;
  panic when uinput is missing.

## Success criteria

- [x] Trait + fake + uinput device with pinned name/vendor/capabilities.
- [x] `compute_delta` saturating, unit-tested incl. edges.
- [x] `click` emits down+up with SYN; `scroll` emits both axes.
- [ ] Multi-monitor offset delta tests (negative origins).
- [ ] udev flat-accel rule asserted by a blocking platform test (matrix #5).
- [ ] Click accuracy verified in Phase-1.4 e2e within tolerance (proposed:
      ≤ 2 px at 100%, ≤ 4 px at fractional scales).
- [ ] Degraded-mode test: uinput denied → daemon alive, status reports it.

## Open questions

1. **Wheel units.** MVP: `REL_WHEEL` detents (`lines`). Hi-res
   (`REL_WHEEL_HI_RES`) would need per-tick pixel accumulation in `state`.
   Confirm detents are acceptable for Phase 2 wheel physics.
2. **Click gap.** `click()` sends down+up back-to-back. Some apps/toolkits
   occasionally need a small inter-event delay; decide only if e2e shows
   missed clicks (do not add sleeps preemptively).
3. **Multi-click ownership.** Cadence is `state`-owned (counts) but emitted
   here as N click pairs. Confirm the daemon batches counts within one SYN
   window or sends discrete pairs — spec default: discrete pairs, cadence from
   the state machine.
