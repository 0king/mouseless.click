# SPEC-cli — Command-line frontend (`mouseless`)

> Module: `cli` · Paths: `crates/mouseless-cli` · Depends on: `kbd` (kanata TCP),
> `ipc` (Phase 3 client verbs) · Plan refs: §4 (contracts), SKILL-08 (parity) ·
> Matrix row: #3 (as a ChangeLayer producer, indirectly)
> Status as of 2026-10-29: **Implemented (Phase 1.2 + Phase 2 verbs)** — parsing,
> parity table, `status`, `--list-input-devices`, and all layer-changing verbs
> speak kanata TCP. `toggle` queries the current layer first. Free-mode verbs
> target `lyr-free` (custom engine); `lyr-mouse` remains the kanata fallback.

## Objective

One scriptable, documented frontend whose verbs map 1:1 to `mouseless://` deep
links (parity requirement) and which can be used for status/diagnostics even
when the GUI is not wanted.

## Scope

- In: `show|hide|toggle`, `enter-free-mode|exit-free-mode`, `status`,
  `--list-input-devices`; exits codes; `.desktop` deep-link parity (file owned
  by `platform`).
- In (Phase 3): `reload` (config re-apply), mapping to the daemon `Reload`
  client message.
- Out: anything the daemon can do only while the overlay is visible (no GUI
  automation); label mode and other Phase-4 features.

## Contracts

### Verbs

| Verb | Aliases | Effect | Deep link |
|---|---|---|---|
| `show` | `show-overlay` | `ChangeLayer lyr-mouseless` (let kanata + daemon drive the rest) | `mouseless://show-overlay` |
| `hide` | `hide-overlay` | `ChangeLayer lyr-base` | `mouseless://hide-overlay` |
| `toggle` | `toggle-overlay` | Query current layer, then show-or-hide | `mouseless://toggle-overlay` |
| `enter-free-mode` | — | `ChangeLayer lyr-free` (custom engine; the daemon starts physics on the layer event). `lyr-mouse` remains the kanata fallback | `mouseless://enter-free-mode` |
| `exit-free-mode` | — | `ChangeLayer lyr-base` | `mouseless://exit-free-mode` |
| `status` | — | Print `kanata: <layer>` (daemon/device summary deferred to Phase 3) | `mouseless://status` |
| `--list-input-devices` | — | Parse `/proc/bus/input/devices`, tag kanata/mouseless entries | `mouseless://list-input-devices` |
| `reload` (Phase 3) | — | Send `Reload` client frame to the daemon | — |

**Show/hide go through kanata's layer switch, never a direct daemon socket
command**: the layer is the single source of truth (plan §1.3); the daemon
reacts to `LayerChange`. A direct `Show` frame would desync kanata.

### Exit codes & output

- `0` success; `1` operational failure (kanata TCP unavailable, daemon
  unreachable, bad device list); `2` usage error (unknown verb/args).
- Human-readable single lines on stdout; errors on stderr with a clear message
  (no stack traces). Keep output stable enough for scripts: layer changes print
  `mouseless: layer <name>`; `status` prints `kanata: <layer>`.
- Daemon reachability and the device count in `status` need a Phase-3 `Status`
  client message; until then `--list-input-devices` covers diagnostics.
- Timeouts: kanata queries ≤ 3 s; daemon socket ops ≤ 1 s.

### Input device listing

Source: `/proc/bus/input/devices` (no special permissions needed vs the
`EVIOCGNAME` ioctl path). Classification tags: `[kanata output]` for
`kanata-mouseless`, `[ours: kanata-excluded]` for `mouseless-mouse`.
Exit `0` even if zero devices found (diagnostic command).

## Tech stack

Rust 2021; `thiserror`; lib part (`Verb::parse`, `deep_link`) is toolkit-free and
unit-tested; binary part does I/O.

## Commands

```sh
cargo test -p mouseless-cli
cargo run -p mouseless-cli -- status
cargo run -p mouseless-cli -- --list-input-devices
```

## Project structure

- `src/lib.rs` — `Verb`, `CliError`, deep-link table, parity tests.
- `src/main.rs` — verb dispatch, kanata TCP calls, device listing.

## Code style

- Every verb handler returns `ExitCode`; no panics; errors printed by one helper.
- New verb = same commit adds: `Verb` variant, deep-link string, spec table row,
  parity test entry, and (Phase 3) `.desktop` scheme registration if needed.

## Testing strategy

- Parity test: every verb ↔ deep link round-trip. *(exists)*
- Unknown verb → usage, exit 2. *(exists as parse test)*
- Fake-kanata TCP tests (`tests/kanata_verbs.rs`): show/enter-free-mode →
  observed `ChangeLayer`; toggle → queries layer first; status → prints the
  reported layer; unreachable kanata → exit 1. *(exists)*
- `--help` documents the exit-code contract. *(exists)*
- `--list-input-devices` parsing against a fixture file.

## Boundaries

- **Always:** keep deep-link parity 1:1; go through kanata layers for
  show/hide/toggle; keep verbs idempotent.
- **Ask first:** adding/renaming verbs (breaks deep links + `platform` .desktop
  entries); changing exit-code semantics.
- **Never:** send daemon frames that bypass the kanata layer for show/hide;
  require root; block longer than the pinned timeouts.

## Success criteria

- [x] `Verb::parse` + `deep_link` parity table with tests.
- [x] `status` queries live kanata TCP; failure exits 1 with message.
- [x] `--list-input-devices` works permission-free with tags.
- [x] `show/hide/toggle/enter-free-mode/exit-free-mode` switch layers
      (fake-TCP tests; live smoke after the Phase-2 kanata apply).
- [x] Free-mode verbs wired (`lyr-free` for the custom engine; `lyr-mouse`
      remains available as the kanata fallback).
- [x] Output/exit-code contract documented in `--help` and README.
- [ ] Phase 3: `reload` client verb; `.desktop` deep links installed by platform.

## Open questions

1. **Free-mode verbs under `free_mode_engine=custom` — resolved (2026-10-29).**
   The CLI switches to `lyr-free`; kanata remains the single source of truth and
   the daemon starts/stops physics on the resulting `LayerChange`. No direct
   daemon client message is needed. `free_mode_engine=kanata` keeps
   `lyr-mouse` as the manual/fallback layer.
2. **`toggle` race.** Query-then-switch is two round-trips; kanata has no atomic
   toggle. Accept the race (local, sub-ms) unless misfires appear.
3. **`status` depth.** Should it query the overlay client count from the daemon
   (needs a `Status` client message not yet in `SPEC-ipc.md`)? Proposed: defer
   until Phase 3 settings needs the same data.
