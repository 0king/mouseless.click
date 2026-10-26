# SPEC-overlay — GNOME Shell overlay (`mouseless-overlay` + `gnome-extension`)

> Module: `overlay` · Paths: `crates/mouseless-overlay`, `gnome-extension` ·
> Depends on: `ipc`, `core` · Plan refs: §1.5, §3, §4, §10.4 · Audit C2 ·
> Matrix row: #8 (extension side of malformed frames)
> Status as of 2026-10-18: **Stub** — Rust trait exists; the extension only
> creates a hidden actor. Phase 1.3 implements everything below.

## Objective

A draw-only overlay on Mutter: a fullscreen transparent Clutter actor that
renders the grid, labels, and highlight, and reports monitor geometry + cursor
position back to the daemon. It never captures input — keys stay at
evdev/kanata (plan §1.5). The daemon computes; the extension only draws.

## Scope

- In (Phase 1.3): actor lifecycle via
  `Main.layoutManager.addTopChrome(actor, { affectsInputRegion: false, trackFullscreen: true })`;
  GNOME 45+ ESM; grid/label/highlight rendering; `Show`/`HighlightCell`/`ShowSubgrid`/`Hide`
  handling; `Hello`/`Report`/`CursorReply`; reconnect + EOF-hide; Shell-restart
  resilience; malformed-frame safety.
- In (Phase 2.2): multi-monitor switching (move the active overlay), continuous
  mode rendering, drag/drop visuals.
- Out (Phase 4): AT-SPI label mode; overlay lock/pass-through; wlroots/X11
  backends (`impls/wlr.rs`, `impls/x11.rs`).

## Contracts

### Rust trait (pin)

```rust
trait OverlayBackend {
    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error>; // idempotent
    fn highlight(&mut self, index: u32) -> Result<(), Self::Error>;   // idempotent
    fn hide(&mut self) -> Result<(), Self::Error>;                    // idempotent, best-effort
    fn last_cursor(&self) -> Option<CursorPos>;                       // latest report
}
```

`impls/gnome.rs` is the socket client (sends frames, tracks last `Report`).
`impls/fake.rs` mirrors visibility/cursor for tests. `GNOME_SHELL_VERSIONS`
and `SOCKET_PATH` are pinned constants.

### Extension behavior (normative)

1. **Draw-only.** `reactive: false`; no key handlers anywhere; never
   `EVIOCGRAB`, uinput, or physics. The actor must not affect the input region
   (`affectsInputRegion: false`) or appear in screenshots-taken-by-other-apps
   flows unexpectedly (`trackFullscreen: true`).
2. **Render daemon-provided geometry.** The extension MUST NOT compute cell
   bounds. It converts physical bounds to stage units with `bounds / monitor.scale`
   (or `logical × scale` — exact direction confirmed by spike §10.4; see OQ-1)
   and draws.
3. **Idempotent messages.** `Show` while shown replaces the model; `Hide` while
   hidden is a no-op; `HighlightCell` for an unknown index is ignored.
4. **Hello-first.** On connect: send `Hello{extension_version}`; expect a
   state replay (`Show` or `Hide`) within 2 s.
5. **Reports.** Send `Report{geometry, cursor}` on connect and on monitor
   layout/scale changes. Reply `CursorReply` to `QueryCursor` within 100 ms.
   Cursor position via `global.get_pointer()`; reported in the pinned
   coordinate space (spike §10.4).
6. **EOF-hide.** On socket error/EOF, hide the overlay immediately and retry
   connect with 500 ms → 10 s backoff. (Kill-switch safety: daemon death must
   never leave a stuck overlay.)
7. **Never crash the Shell.** Every socket/parse/render handler wrapped; errors
   go to logs (extension logger), never to an uncaught JS throw.
8. **Shell restart resilience.** `disable()` destroys the actor and closes the
   socket; a fresh `enable()` re-runs Hello-first and receives a state replay.
9. **60 FPS.** Clutter renders at compositor cadence; no per-frame IPC. Frames
   are change-driven only.

### GNOME compatibility

`metadata.json`: `"shell-version": ["45","46","47","48","49","50"]`, ESM module
syntax (`export default class … { enable() … disable() … }`). Primary test
target: GNOME 50+ / Ubuntu 26.04.

## Tech stack

Rust client (`unix socket`, `serde_json`) in `impls/gnome.rs`; JS ESM with
`gi://` imports (`Clutter`, `GLib`, `Gio`), no npm dependencies, no bundler.

## Commands

```sh
cargo test -p mouseless-overlay
# dev install (session user):
gnome-extensions pack gnome-extension --force -o /tmp
gnome-extensions install --force /tmp/mouseless@sonuscape.example.shell-extension.zip
# reload after code change (Wayland): disable/enable, or log out/in
gnome-extensions disable mouseless@sonuscape.example
gnome-extensions enable  mouseless@sonuscape.example
# nested-shell smoke (no real session touched):
dbus-run-session -- gnome-shell --nested --wayland
```

## Project structure

- `crates/mouseless-overlay/src/lib.rs` — trait, `GridModel`, `OverlayError`.
- `crates/mouseless-overlay/src/impls/gnome.rs` — socket client + constants.
- `crates/mouseless-overlay/src/impls/fake.rs` — recorder.
- `gnome-extension/extension.js` — the whole extension (keep single-file until
  it exceeds ~400 lines, then split `render.js`, `ipc.js`).
- `gnome-extension/metadata.json` — uuid + shell-version.

## Code style

- JS: defensive everything at the socket boundary (`try/catch`, type checks);
  no `log()`-spam — one log line per state change or error, prefixed `mouseless:`.
- Rust: `OverlayError::{Connection, Schema}` with context; backoff constants named.
- Every display constant (colors, opacity, label font) clustered in one object
  so Phase-3 settings can replace it with config.

## Testing strategy

- Rust: idempotent show/hide with `FakeOverlay`. *(exists)*
- #8 (extension side): malformed frame harness — a JS unit test is not
  scaffolded; pragmatic gate = `node --input-type=module --check`-style syntax
  validation in CI plus a scripted manual checklist:
  send garbage to the socket → Shell keeps running, error logged.
- Manual smoke (Phase 1.3 acceptance): tap Shift → grid on the target monitor;
  arrows of the grid match click targets; type keys → highlight follows; subgrid
  opens; Space clicks cell center; Esc hides; disable/enable extension mid-session
  → replay works; kill daemon while visible → overlay hides.
- Spike §10.4 (blocking): cursor + geometry accuracy at 100/125/150/200%.
- Nested-shell smoke: optional but desired; document exact command in CI notes.

## Boundaries

- **Always:** idempotent handlers; EOF → hide; try/catch at every IPC boundary;
  keep `metadata.json` shell versions in sync with `GNOME_SHELL_VERSIONS`.
- **Ask first:** changing the coordinate conversion (spike §10.4 outcome);
  adding GNOME imports beyond `Clutter/GLib/Gio/Main`; multi-actor rendering
  strategy; animations that affect responsiveness.
- **Never:** capture input, grab devices, run physics, send frames the spec
  does not define, block the Shell thread with synchronous work, throw uncaught.

## Success criteria

- [ ] Fullscreen draw-only actor via `addTopChrome({affectsInputRegion:false, trackFullscreen:true})`.
- [ ] `Show`/`HighlightCell`/`ShowSubgrid`/`Hide` rendered; idempotent replays harmless.
- [ ] `Hello` on connect; `Report` on connect/geometry change; `CursorReply` < 100 ms.
- [ ] EOF → hide + reconnect backoff (kill-switch safety).
- [ ] Shell restart (disable/enable) → replay restores correct state.
- [ ] Malformed frame → log, no Shell crash.
- [ ] Phase-1.4 e2e: grid matches click targets at 100% and one fractional scale.
- [ ] Visual: labels legible, highlight distinct, no input-region interference
      (typing unaffected while overlay visible).

## Open questions

1. **Coordinate space (spike §10.4).** GNOME stage units vs physical pixels under
   fractional scaling — pin the single conversion point (proposed: extension
   converts for drawing; reports physical). Until closed, acceptance uses the
   closed-loop check (daemon re-queries cursor after a move; systematic error →
   correction factor).
2. **Label content for level 2.** Upstream: if level-2 keys are blank, level-1
   keys are reused. Who resolves that (settings at config time vs daemon at run
   time)? Proposed: resolved at config time; wire always carries explicit labels.
3. **Multi-monitor overlay.** MVP: one actor, reposition/resize on monitor
   switch. Confirm versus per-monitor actors (Phase 2.2 decision).
4. **Initial overlay monitor.** Proposed default: monitor under cursor; upstream
   has a configurable "initial overlay monitor" (Phase 3 setting).
