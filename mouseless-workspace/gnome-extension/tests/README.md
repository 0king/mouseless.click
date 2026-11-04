# Headless GNOME Shell smoke test

`headless-smoke.sh` runs the real extension against `mock-daemon.js` in a
private D-Bus session with a throwaway `XDG_DATA_HOME`. It never touches the
logged-in desktop.

```sh
gnome-extension/tests/headless-smoke.sh
```

Prerequisites: `gnome-shell`, `gnome-extensions`, `gjs`, `unzip`, and
`dbus-run-session` (all standard on Ubuntu GNOME). The script packs the
extension with `--extra-source=ipc.js --extra-source=render.js`, installs it by
unzipping (some distros' `gnome-extensions install` refuses directories), and
asserts the mock saw:

- `Hello` first, then `Report`
- `Show`, `HighlightCell`, `ShowSubgrid`, `QueryCursor` → `CursorReply`, `Hide`
- a second client connection after the mock closes the first socket (EOF-hide +
  500 ms reconnect)
- no `JS ERROR` in the Shell log

The mock sends the daemon's real Phase-3 grid sizes: a 10×30 primary model
with two-character labels (`AQ`, `SQ`, …) and a 10×3 subgrid, so the renderer's
font scaling and 300-cell actor build are exercised in the Shell. It also sends
the resolved `appearance` block, so the config-driven color/opacity path is
packed and parsed rather than only the legacy defaults. The mock uses
GJS (`gjs -m`), not Node, because the extension imports `gi://` modules. The
Shell output is intentionally not asserted beyond uncaught JS errors: headless
Mutter allows protocol/lifecycle checks but denies screenshots, so visual and
fractional-scale acceptance remain manual (`SPEC-overlay.md`).
