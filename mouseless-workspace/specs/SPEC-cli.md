# SPEC-cli — Command-line frontend (`mouseless`)

> Module: `cli` · Paths: `crates/mouseless-cli` · Depends on: `kbd` (kanata TCP),
> `ipc` (Phase 3 client verbs) · Plan refs: §4 (contracts), SKILL-08 (parity) ·
> Matrix row: #3 (as a ChangeLayer producer, indirectly)
> Status as of 2026-10-18: **Partial** — parsing + parity table + `status` +
> `--list-input-devices` work; state-changing verbs are not wired yet.

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
| `enter-free-mode` | — | MVP: `ChangeLayer lyr-mouse` (kanata engine). Phase 2: daemon command when `free_mode_engine=custom` (OQ-1) | `mouseless://enter-free-mode` |
| `exit-free-mode` | — | MVP: `ChangeLayer lyr-base`; Phase 2: daemon command (OQ-1) | `mouseless://exit-free-mode` |
| `status` | — | Print kanata layer + daemon socket reachability + device summary | `mouseless://status` |
| `--list-input-devices` | — | Parse `/proc/bus/input/devices`, tag kanata/mouseless entries | `mouseless://list-input-devices` |
| `reload` (Phase 3) | — | Send `Reload` client frame to the daemon | — |

**Show/hide go through kanata's layer switch, never a direct daemon socket
command**: the layer is the single source of truth (plan §1.3); the daemon
reacts to `LayerChange`. A direct `Show` frame would desync kanata.

### Exit codes & output

- `0` success; `1` operational failure (kanata TCP unavailable, daemon
  unreachable, bad device list); `2` usage error (unknown verb/args).
- Human-readable single lines on stdout; errors on stderr with a clear message
  (no stack traces). Keep output stable enough for scripts:
  `status` prints `kanata: <layer>`, `daemon: <ok|unreachable>`, `devices: <n>`.
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
- Phase 1.2 completion: verb handlers against a fake kanata TCP server
  (show → observes `ChangeLayer lyr-mouseless`; toggle → queries layer first).
- `status` against fake TCP + missing socket → prints `unreachable`, exit 1.
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
- [ ] `show/hide/toggle` actually switch layers (fake-TCP tests + live smoke).
- [ ] Free-mode verbs wired (MVP: `lyr-mouse`; Phase 2 decision below).
- [ ] Phase 3: `reload` client verb; `.desktop` deep links installed by platform.
- [ ] Output/exit-code contract documented in `--help`.

## Open questions

1. **Free-mode verbs under `free_mode_engine=custom`.** There is no kanata layer
   signal for the custom engine yet (see `SPEC-state.md` OQ-4). Until the
   custom input path is designed, `enter/exit-free-mode` must either (a) stay
   mapped to kanata's `lyr-mouse` (MVP), or (b) gain a daemon client message.
   Proposed: (a) for MVP; revisit with OQ-4.
2. **`toggle` race.** Query-then-switch is two round-trips; kanata has no atomic
   toggle. Accept the race (local, sub-ms) unless misfires appear.
3. **`status` depth.** Should it query the overlay client count from the daemon
   (needs a `Status` client message not yet in `SPEC-ipc.md`)? Proposed: defer
   until Phase 3 settings needs the same data.
