# SPEC-overlay — GNOME Shell overlay (`mouseless-overlay` + `gnome-extension`)

> Module: `overlay` · Paths: `crates/mouseless-overlay`, `gnome-extension` ·
> Depends on: `ipc`, `core` · Plan refs: §1.5, §3, §4, §10.4 · Audit C2 ·
> Matrix row: #8 (extension side of malformed frames)
> Status as of 2026-10-29: **Implemented (Phase 1.3); Phase-1.4 e2e verified**
> — draw-only Shell extension (split into `extension.js`/`ipc.js`/`render.js`)
> and the daemon-side Unix-socket backend are wired and pass unit +
> headless-Shell smoke tests. Host e2e passed at 100% and 125% (grid/click
> alignment, zero leaked keystrokes). Mixed-scale multi-monitor origins remain
> open (OQ-1).

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
    fn show_subgrid(&mut self, parent: u32, model: &GridModel) -> Result<(), Self::Error>;
    fn hide(&mut self) -> Result<(), Self::Error>;                    // idempotent, best-effort
    fn query_cursor(&mut self) -> Result<(), Self::Error>;            // QueryCursor
    fn poll_events(&mut self) -> Vec<OverlayEvent>;                   // non-blocking
    fn last_cursor(&self) -> Option<CursorPos>;                       // latest report
    fn last_layout(&self) -> Option<MonitorLayout>;                   // latest Report
    fn is_connected(&self) -> bool;                                   // helloed client?
}
```

**Ownership (normative):** the daemon owns the Unix-socket **listener/server**;
the extension is the **client** (`SPEC-ipc.md` lifecycle). `impls/gnome.rs`
binds, authenticates peers, parses `Hello`/`Report`/`CursorReply`, broadcasts
frames, and replays state on reconnect. `impls/fake.rs` mirrors visibility,
cursor, and layout for tests. `GNOME_SHELL_VERSIONS`, `SOCKET_PATH`, and
`SOCKET_GROUP` are pinned constants. The daemon must not acquire a cell grab
unless `is_connected()` and `last_layout()` are both available.

### Extension behavior (normative)

1. **Draw-only.** `reactive: false`; no key handlers anywhere; never
   `EVIOCGRAB`, uinput, or physics. The actor must not affect the input region
   (`affectsInputRegion: false`) or appear in screenshots-taken-by-other-apps
   flows unexpectedly (`trackFullscreen: true`).
   - GNOME 45–49: pass `{affectsInputRegion: false, trackFullscreen: true}`.
   - GNOME 50 removed the `affectsInputRegion` parameter; `enable()` catches
     that specific error, removes the chrome, and retries with
     `{trackFullscreen: true}` only. This is safe because the actor is
     `reactive: false` and tracked chrome no longer joins the input region.
2. **Render daemon-provided geometry.** The extension MUST NOT compute cell
   bounds. It converts physical wire bounds to stage units at one point:
   `stage = monitor_stage_origin + (cell_physical - model.monitor_physical_origin) / geometry_scale`
   (see OQ-1 for the mixed-scale limitation).
3. **Idempotent messages.** `Show` while shown replaces the model; `Hide` while
   hidden is a no-op; `HighlightCell` for an unknown index is ignored.
4. **Hello-first.** On connect: send `Hello{extension_version}` immediately,
   then `Report`; the daemon drops a peer that does not send `Hello` within 2 s
   and replies with a state replay (`Show`/`ShowSubgrid` or `Hide`).
5. **Reports.** Send `Report{geometry, cursor}` on connect and on monitor
   layout/scale changes. Reply `CursorReply` to `QueryCursor` within 100 ms.
   Cursor position comes from `global.get_pointer()` and is reported with
   `physical = stage * monitor.geometry_scale` (single conversion point).
6. **EOF-hide.** On socket error/EOF, hide the overlay immediately and retry
   connect with 500 ms → 10 s backoff. A successfully delivered frame resets
   the backoff to 500 ms. (Kill-switch safety: daemon death must never leave a
   stuck overlay.)
7. **Never crash the Shell.** Every socket/parse/render handler wrapped; frame
   and render errors go to `console.warn`/`console.error`, never to an
   uncaught JS throw. Repeated identical warnings are de-duplicated per
   connection.
8. **Shell restart resilience.** `disable()` disconnects monitors, stops the
   socket, and destroys the actor; a fresh `enable()` re-runs Hello-first and
   receives the daemon's state replay. A replayed `ShowSubgrid` may carry no
   level-1 model; the sub-cells are drawn on their own (the selected parent has
   no fill/label by design, so nothing needs reconstructing).
9. **60 FPS.** Clutter renders at compositor cadence; no per-frame IPC. Frames
   are change-driven only.
10. **No stray chrome.** `trackFullscreen: true` lets GNOME set the tracked
    actor visible after monitor/scale, overview, or fullscreen changes. With no
    model the renderer immediately re-hides its root, so an idle overlay can
    never leave an empty dim layer on screen. With a model, `reposition()`
    rebuilds and re-asserts visibility so a mid-session scale change keeps the
    grid up.
11. **Appearance (config-driven, pinned defaults).** Every model carries the
    daemon-resolved `appearance` block (grid/label/highlight RGB plus
    `grid_opacity`); the renderer draws those values and never parses config.
    The pinned defaults are `#d4de04` at 80% transparency (20% opacity,
    alpha 51), `#ebe9e8` labels, and an amber `#ffb22c` highlight. The
    fullscreen root background is transparent — the tiled grid is the only
    tint on the active monitor, and other monitors are untouched. The selected
    parent is not drawn at all when its subgrid opens, so sub-cells are not
    composited to a darker fill and the parent's two-character label cannot
    bleed through them; non-selected level-1 cells behind an open subgrid drop
    to 20/51 of the configured grid alpha and their labels to alpha 110, so the
    subgrid stands out. Highlight text uses the configured label color. Font
    size is computed per cell from its stage-unit height/width and label length
    (clamped 7–20 px), because the default 10×30 primary grid has ~192×36 px
    cells while a 10×3 sub-cell inside one is ~19×12 px at 1080p. The renderer
    also draws both characters of a primary label (`AQ`) and one character of a
    subgrid label (`Q`); it still never derives geometry or labels. Separators
    are the uncovered gaps, not
    drawn lines: `SEPARATOR_PX = 1` stage px is applied as a 0.5 px per-edge
    inset, giving a 1 px visible line between adjacent primary cells and
    between adjacent sub-cells (monitor/subgrid outer edges keep a 0.5 px
    margin). At fractional scales the compositor scales this like every other
    stage unit.

### GNOME compatibility

`metadata.json`: `"shell-version": ["45","46","47","48","49","50"]`, ESM module
syntax (`export default class … { enable() … disable() … }`). Primary test
target: GNOME 50+ / Ubuntu 26.04.

Compatibility shims (keep both while 45–50 are supported):

- **Colors:** Clutter 18 (GNOME 47+) removed `Clutter.Color`; GNOME 45/46 still
  expose it. Resolve once at module load: `const Color = Clutter.Color ?? Cogl.Color`.
  This is the only extra GI import approved beyond `Clutter/GLib/Gio/Main`.
- **Chrome options:** the GNOME-50 `affectsInputRegion` removal fallback above.
- **Labels:** `Clutter.BinLayout` centers both axes on 45–50; the explicit
  `BinAlignment` enum was removed in Clutter 18, so no alignment flags are set.

## Tech stack

Rust **server** (`UnixListener`, `serde_json`) in `impls/gnome.rs`; JS ESM
client with `gi://` imports (`Clutter`, `Cogl`, `GLib`, `Gio`, `Main`), no npm
dependencies, no bundler.

## Commands

```sh
cargo test -p mouseless-overlay
# JS syntax gate (GI imports are syntactically valid ESM):
node --check gnome-extension/extension.js
node --check gnome-extension/ipc.js
node --check gnome-extension/render.js
# dev pack must include the split modules:
gnome-extensions pack gnome-extension --force -o /tmp \
  --extra-source=ipc.js --extra-source=render.js
gnome-extensions install --force /tmp/mouseless@sonuscape.example.shell-extension.zip
# reload after code change (Wayland): disable/enable, or log out/in
gnome-extensions disable mouseless@sonuscape.example
gnome-extensions enable  mouseless@sonuscape.example
# dev socket override (both sides; no system paths touched):
MOUSELESS_SOCKET=/tmp/mouseless.sock cargo run -p mouseless-daemon
# checked-in headless smoke (private D-Bus; no real session touched):
gnome-extension/tests/headless-smoke.sh
```

The headless harness uses `XDG_DATA_HOME` plus `MOUSELESS_SOCKET`, unpacks the
zip into `$XDG_DATA_HOME/gnome-shell/extensions/mouseless@sonuscape.example/`,
and enables it against `gnome-shell --headless --no-x11 --virtual-monitor 1920x1080`.
`gnome-extensions install` may refuse to copy a directory on some distros; the
unzip fallback is equivalent for testing. See
`gnome-extension/tests/README.md` for the asserted frame sequence.

## Project structure

- `crates/mouseless-overlay/src/lib.rs` — trait, events, re-exported wire types.
- `crates/mouseless-overlay/src/impls/gnome.rs` — Unix-socket **server**,
  auth, replay, reconnecting-client bookkeeping + constants.
- `crates/mouseless-overlay/src/impls/fake.rs` — recorder (incl. fault injection).
- `gnome-extension/extension.js` — entry point, chrome lifecycle, frame
  dispatch, `Hello`/`Report`/`CursorReply`.
- `gnome-extension/ipc.js` — NDJSON client, reconnect/backoff, model validation.
- `gnome-extension/render.js` — Clutter actor + physical→stage conversion.
- `gnome-extension/metadata.json` — uuid + shell-version.

The extension was split once `extension.js` approached the ~400-line threshold;
the pack command must list `--extra-source=ipc.js --extra-source=render.js`.

## Code style

- JS: defensive everything at the socket boundary (`try/catch`, type checks);
  no `log()`-spam — one log line per state change or error, prefixed `mouseless:`.
- Rust: `OverlayError::{Connection, Schema}` with context; backoff constants named.
- Display defaults (colors, opacity) and layout constants are centralized;
  colors/opacity come from the model's `appearance`, and every field falls back
  per-field so a malformed payload cannot blank the overlay.

## Testing strategy

- Rust: idempotent show/hide with `FakeOverlay`; daemon-side `GnomeOverlay`
  tests cover Hello replay, `Report`/`HighlightCell`/`Hide`, malformed frames,
  and the 2 s Hello timeout. *(exists)*
- #8 (extension side): `node --check` syntax gate for the three JS files plus
  the checked-in `gnome-extension/tests/headless-smoke.sh`: a GJS mock daemon
  sends `Show`, `HighlightCell`, `ShowSubgrid`, `QueryCursor`, `Hide`, then
  closes the socket; the Shell stays up, hides on EOF, and reconnects (the mock
  observes client 2). No uncaught `JS ERROR` is allowed.
- Manual visual smoke (passed 2026-10-29 on GNOME Shell 50.1): tap Shift →
  grid on the monitor under the cursor; grid matches click targets; subgrid
  opens; Space clicks cell center; Esc hides; disable/enable mid-session →
  replay restores state; kill daemon while visible → overlay hides. The
  100%/125% runs used a two-monitor host (1920×1080 + 1920×1080) and pressed
  `k`/`k`: the daemon logged the expected cell centers and the grab leaked no
  keystrokes (`leak.txt` 0 bytes). A 125% → 100% → 125% → 100% scale sequence
  with no model left the actor hidden each time (regression guard).
- Spike §10.4 (blocking for multi-scale): cursor + geometry accuracy at
  100/125/150/200%. 100% and 125% verified on the host; 150/200% and
  mixed-scale origins remain open. The headless shell confirms
  protocol/lifecycle only; screenshots are denied there, so visual/fractional
  acceptance is manual.
- Nested-shell smoke command documented above.

## Boundaries

- **Always:** idempotent handlers; EOF → hide; try/catch at every IPC boundary;
  keep `metadata.json` shell versions in sync with `GNOME_SHELL_VERSIONS`.
- **Ask first:** changing the coordinate conversion (spike §10.4 outcome);
  adding GNOME imports beyond `Clutter/Cogl/GLib/Gio/Main`; multi-actor
  rendering strategy; animations that affect responsiveness.
- **Never:** capture input, grab devices, run physics, send frames the spec
  does not define, block the Shell thread with synchronous work, throw uncaught.

## Success criteria

- [x] Fullscreen draw-only actor via `addTopChrome({affectsInputRegion:false, trackFullscreen:true})`,
      with the GNOME-50 parameter fallback.
- [x] `Show`/`HighlightCell`/`ShowSubgrid`/`Hide` rendered; idempotent replays harmless.
- [x] `Hello` on connect; `Report` on connect/geometry change; `CursorReply` < 100 ms.
- [x] EOF → hide + reconnect backoff (kill-switch safety).
- [x] Shell restart (disable/enable) → replay restores correct state.
- [x] Malformed frame → log, no Shell crash.
- [x] Phase-1.4 e2e: grid matches click targets at 100% and one fractional
      scale. Verified 2026-10-29 on GNOME 50.1 at 100% (`k`/`k` target
      `(2580,945)`) and 125% (`(3060,945)`), zero leaked keystrokes.
- [x] Visual: labels legible, highlight distinct, no input-region interference
      (typing unaffected while overlay visible). Verified on the host
      2026-10-29 at 100% and 125%; headless screenshots are denied.
- [ ] Phase-3 appearance: config-driven `appearance` (defaults `#d4de04` at 80%
      transparency, `#ebe9e8` labels), two-char primary labels, and per-cell
      font scaling legible on the host (pending a 10×30 host smoke; the
      headless Shell cannot capture screenshots).

## Open questions

1. **Coordinate space (spike §10.4) — pinned for single-monitor.** Wire bounds
   and cursor are physical px; the extension converts at one point for drawing
   and reports. **Still open:** mixed-scale multi-monitor origins. Physical
   origins computed as `stage_origin * own_scale` are wrong when neighboring
   monitors use different scales (OQ-3/Phase 2.2). Until closed, acceptance uses
   the closed-loop check (daemon re-queries cursor after a move; systematic
   error → correction factor).
2. **Label content for level 2 — resolved (2026-11-03).** The daemon sends
   explicit labels: two characters for every primary cell (column key + row
   key) and one character for every subgrid cell. The extension never composes,
   resolves, or re-derives a label, so upstream's blank-level-2 inheritance
   will be a daemon/settings concern, not a rendering one.
3. **Multi-monitor overlay.** MVP: one actor, reposition/resize on monitor
   switch. Confirm versus per-monitor actors (Phase 2.2 decision).
4. **Initial overlay monitor — implemented.** Default: monitor under the last
   reported cursor, falling back to the primary monitor. A per-monitor setting
   remains Phase 3.
