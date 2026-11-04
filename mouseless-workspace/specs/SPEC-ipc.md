# SPEC-ipc — Wire protocol (`mouseless-ipc`)

> Module: `ipc` · Paths: `crates/mouseless-ipc` · Depends on: — ·
> Plan refs: §3 rule 3, §4 (contracts), §8 #8, audit G2/G3 · Matrix row: #8
> Status as of 2026-11-04: **Implemented (v1)** — typed grid/layout payloads,
> `QueryCursor`/`CursorReply`/`Error`, `kind` validation, 1 MiB limit,
> `SO_PEERCRED` auth, Hello timeout, a checked-in malformed corpus, and the
> one-shot client messages `Reload`/`Save`/`Update`/`Status` with their replies.
>
> This document IS the 1-page wire spec deliverable required by audit G2.

## Objective

One versioned protocol so daemon, extension, CLI, and GUI can never silently
misinterpret each other. Mismatched peers negotiate or refuse loudly; malformed
frames never crash either side; the socket is only usable by authorized peers.

## Transport

- `AF_UNIX` stream socket at `/run/mouseless/mouselessd.sock`
  (`RuntimeDirectory=mouseless`, dir + socket mode `0770`, group `mouseless`).
- Newline-delimited JSON (NDJSON), UTF-8, one envelope per line.
- Max line length 1 MiB. Longer → log, drop connection. Partial lines are a
  protocol violation (each frame ends with `\n`).
- Plain `'\n'`, no CR. Receivers must accept trailing whitespace but not require it.

## Envelope

```json
{"schema_version": 1, "kind": "daemon", "payload": {"type": "Hide"}}
```

| Field | Rule |
|---|---|
| `schema_version` | Integer. Current: `1`. See negotiation below. |
| `kind` | Sender role: `"daemon"`, `"overlay"`, or `"client"`. Receiver MUST drop a frame whose `kind` does not match its peer's expected role (log + ignore, do not close). |
| `payload` | Message object, discriminated by `"type"`. Unknown `type`: log + ignore (forward-compatible). Unknown fields inside known types: ignored. |

## Messages

### daemon → overlay (kind `"daemon"`)

| Message | Payload schema |
|---|---|
| `Show` | `{"type":"Show","model":GridModel}` |
| `HighlightCell` | `{"type":"HighlightCell","index":0}` |
| `ShowSubgrid` | `{"type":"ShowSubgrid","parent":3,"model":GridModel}` |
| `Hide` | `{"type":"Hide"}` |
| `QueryCursor` | `{"type":"QueryCursor"}` — request a fresh cursor position before a click |
| `Error` | `{"type":"Error","code":"version_mismatch","message":"…","got":2,"expected":1}` |

`GridModel` (pinned; bounds in **physical px**, computed by `core`):

```json
{
  "monitor": {"x": 0, "y": 0, "w": 1920, "h": 1080, "scale": 1.0},
  "cells": [
    {"index": 0, "label": "AQ", "x": 0,   "y": 0,   "w": 192, "h": 36},
    {"index": 1, "label": "SQ", "x": 192, "y": 0,   "w": 192, "h": 36}
  ],
  "highlight": null,
  "appearance": {
    "grid_color": [212, 222, 4],
    "grid_opacity": 0.2,
    "label_color": [235, 233, 232],
    "highlight_color": [255, 178, 44]
  }
}
```

- `cells` are row-major; `index` matches the daemon's `CellId` ordering.
- `label` is a display string: two characters for a primary cell (column key +
  row key, e.g. `AQ`), one for a subgrid cell.
- For `ShowSubgrid`, `model.cells` are sub-cells of `parent` and MAY carry
  different labels than level 1.
- `appearance` carries the daemon-resolved `appearance:` values as RGB triples
  plus `grid_opacity` (`0.0..=1.0`). The extension only draws these values; it
  never reads config or invents colors. The field is additive: older payloads
  that omit it decode to the pinned defaults (`#d4de04` at 0.2, `#ebe9e8`
  labels, `#ffb22c` highlight), so `schema_version` stays at 1.

### overlay → daemon (kind `"overlay"`)

| Message | Payload schema |
|---|---|
| `Hello` | `{"type":"Hello","extension_version":"1.0.0"}` — MUST be the first frame after connect |
| `Report` | `{"type":"Report","geometry":{"monitors":[{"x":0,"y":0,"w":1920,"h":1080,"scale":1.0,"primary":true,"connector":"eDP-1"}]},"cursor":{"x":960,"y":540}}` |
| `CursorReply` | `{"type":"CursorReply","x":960,"y":540}` — response to `QueryCursor` |

- `Report` is sent on connect and whenever monitor layout/scale changes.
- `connector` is the compositor output name (`eDP-1`, `DP-1`, …). It is
  optional for backwards compatibility; `monitor_assignment: custom` falls back
  to the primary monitor when it is absent.
- `cursor` is the compositor-reported global cursor position; its coordinate
  space is pinned by spike §10.4 (see `SPEC-core.md` OQ-1). The daemon closes
  the loop on every move: it emits bounded REL steps carrying half of the
  remaining error (the compositor's acceleration gain is up to ~2×), then
  re-queries and continues until the target is within tolerance. If the first
  reply still equals the pre-move position it may predate the motion, so the
  daemon waits for a change before computing the next delta. This corrects
  acceleration or scale distortion per move instead of with a fixed factor
  (see `SPEC-daemon.md` movement semantics).

### client → daemon (kind `"client"`, one-shot)

| Message | Payload schema |
|---|---|
| `Reload` | `{"type":"Reload"}` — re-read the daemon-owned config, validate-then-apply |
| `Update` | `{"type":"Update","config":ConfigData}` — validate, persist, then apply (schema owned by `SPEC-settings.md`); used by Restart App |
| `Save` | `{"type":"Save","config":ConfigData}` — validate and persist **without** changing the running config; used by the settings auto-save |
| `Status` | `{"type":"Status"}` — read-only daemon/overlay/session state |
| `GetConfig` | `{"type":"GetConfig"}` — read-only effective config document (store: saved file → last-good → defaults; used by the settings GUI) |

Clients connect, send exactly **one** frame, read exactly **one** reply, and
close. They do not send `Hello`; the daemon classifies a peer as a client from
its first frame, replies to that frame, and never broadcasts overlay messages to
it. A client that stays silent past the 2 s handshake budget is closed.

### daemon → client (kind `"daemon"`)

| Message | Payload schema |
|---|---|
| `Ack` | `{"type":"Ack","message":"configuration saved and applied"}` |
| `Error` | `{"type":"Error","code":"invalid_config","message":"…"}` |
| `Status` | `{"type":"Status","daemon_version":"0.1.0","overlay_connected":true,"session_active":false,"free_mode":false,"config_path":"/var/lib/mouseless/config.yaml"}` |
| `Config` | `{"type":"Config","config":ConfigData}` — effective document; identical schema to `Update.config` |

- `Error.code` is stable for scripts (`invalid_config`, `io`, …); `message` is
  for humans and may change.
- A rejected `Reload`/`Update` leaves the previously applied config live.
- `Save` returns an `Ack` merely because the document was persisted; the
  running config is unchanged until a later `Reload`/`Update`.
- The client helper uses a 1 s budget (`CLIENT_TIMEOUT`). On timeout the outcome
  is **unknown** (the daemon may have committed the write); callers re-read
  `Status` instead of retrying blindly.

## Lifecycle

```text
overlay connects
  → sends Hello (within 2 s of connect, else daemon closes)
  → daemon replies with current-state replay (Show or Hide) — idempotent
  → steady state: daemon sends Show/HighlightCell/ShowSubgrid/Hide on change;
    overlay sends Report on geometry change; daemon may QueryCursor anytime
overlay disconnects
  → daemon prunes client; state unchanged; next Hello replays state
daemon dies
  → overlay sees EOF, MUST hide the overlay and retry with backoff
```

- Reconnect backoff (overlay): 500 ms doubling to 10 s, no jitter required; a
  successfully delivered frame resets the delay to 500 ms.
- No keepalive frames: local transport; daily traffic is change-driven.
- The daemon broadcasts only to clients that completed `Hello`; a client that
  stays silent past the 2 s handshake budget is closed without being counted
  as connected. `is_connected()` requires at least one helloed client.
- Multiple overlay clients MAY connect; the daemon broadcasts to all (only one
  is expected in practice). `Hello` replay covers late joiners.
- The replay frame is the last `Show`/`ShowSubgrid` while visible (so a Shell
  restart restores the subgrid), otherwise `Hide`.
- Client peers are not part of the overlay lifecycle: they never receive the
  replay, and `Disconnected` is only reported for helloed overlays.

```text
client connects
  → sends exactly one Reload/Save/Update/Status frame (no Hello)
  → daemon validates/persists/applies, then replies Ack/Error/Status
  → client closes; a timeout leaves the outcome unknown
```


## Version negotiation

- `schema_version == 1` → accept.
- `schema_version < 1` → accept (forward-compatible reads; v1 is the minimum).
- `schema_version > 1` → send one `Error{code:"version_mismatch"}` frame, then close.

This matches the implemented `decode()` policy: refuse newer, accept older.

## Auth & permissions (audit G3)

- Filesystem: `/run/mouseless` (0770, group `mouseless`) — only group members
  can open the socket.
- Daemon MUST verify `SO_PEERCRED` on accept: peer uid allowed if
  `uid == daemon euid` OR the peer's primary gid or any supplementary gid
  (read from `/proc/<pid>/status`) equals the `mouseless` gid. Disallowed →
  log + close, no frames parsed.
- Implemented as a pure predicate (`peer_allowed(cred, groups, policy) -> bool`)
  so the policy is unit-testable without spawning peers with foreign uids.
  `group_gid("mouseless")` resolves the gid via `getgrnam`; when the group does
  not exist the daemon falls back to uid-only auth.
- **OQ-1 resolved (2026-10-29):** plan §4's "daemon UID" wording alone would
  reject the Shell extension running as the desktop user. The accepted reading
  is uid **or** `mouseless` group membership; the GNOME extension runs as the
  session user, who is a member of `mouseless` on supported installs
  (`SPEC-platform.md`).

## Robustness rules

- Parser never panics: any malformed line → log + count + skip (connection
  stays); version mismatch → `Error` + close; oversized line → close.
- Senders never block forever: socket writes use a timeout; on timeout/EPIPE,
  drop the client (daemon) or reconnect (overlay).
- Fuzz corpus: `crates/mouseless-ipc/tests/corpus/malformed.ndjson` is checked
  in and exercised by `checked_in_corpus_never_panics`; the inline test also
  truncates every byte prefix of a valid frame. A `cargo-fuzz` target for
  `decode()` remains desirable but the curated corpus is the blocking test (#8).

## Tech stack

Rust 2021; `serde`/`serde_json`, `thiserror`. JS side: plain ESM in
`gnome-extension/` — no bundler, no npm deps.

## Commands

```sh
cargo test -p mouseless-ipc
# manual: one-shot decoder against a frame
printf '%s\n' '{"schema_version":1,"kind":"daemon","payload":{"type":"Hide"}}' | nc -U /run/mouseless/mouselessd.sock
```

## Project structure

`crates/mouseless-ipc/src/lib.rs` — constants (`SCHEMA_VERSION`, `SOCKET_PATH`),
envelope + overlay message enums, `encode`/`decode`.
`crates/mouseless-ipc/src/client.rs` — one-shot `ClientToDaemon` /
`DaemonToClient` frames and the blocking `request()` helper (1 s default).

## Code style

- Wire types carry rustdoc with the exact JSON shape; every field documents units.
- `decode` is total: returns `Result`, never panics, accepts older versions.
- New message variants require: spec update (this file), round-trip test,
  corpus entry, and a daemon/extension handler or an explicit "ignore" clause.

## Testing strategy (#8, blocking)

- Round-trips for every v1 message type. *(exists)*
- Newer `schema_version` refused; older accepted. *(exists)*
- Malformed corpus + truncation of every valid frame never panics. *(exists:
  checked-in corpus plus inline truncation)*
- Auth predicate unit tests, including `SO_PEERCRED` and `/proc` group reads.
  *(exists)*
- `Hello` timeout (2 s) and late-join replay. *(exists: daemon unit tests)*
- `QueryCursor` timeout falls back to the last `Report`; aborts the session when
  no cursor has ever been reported (`DaemonError::NoCursor`). *(exists in the
  daemon click path)*
- Client `Reload`/`Save`/`Update`/`Status` round-trips and a real Unix-socket
  request/reply (ack, error, timeout, missing socket). *(exists in `client.rs`;
  CLI parity covered by `crates/mouseless-cli/tests/daemon_client.rs`)*

## Boundaries

- **Always:** bump `SCHEMA_VERSION` on any breaking shape change; keep
  `SOCKET_PATH` in sync with `platform/systemd/mouselessd.service`.
- **Ask first:** changing the auth model; adding a new `kind`; changing the
  max line length; pinning the cursor coordinate space before spike §10.4.
- **Never:** send frames without `schema_version`; parse a frame from a
  disallowed peer; let a malformed frame or disconnect throw into the Shell.

## Success criteria

- [x] Envelope round-trips with `schema_version` enforced.
- [x] Newer schema refused; malformed input never panics.
- [x] `Envelope` used by the daemon broadcast server (Phase 1.2).
- [x] `QueryCursor`/`CursorReply`/`Error` implemented and tested; one-shot
      `client` messages (`Reload`/`Save`/`Update`/`Status`) implemented with replies.
- [x] `kind` direction validation (drop mismatched, never close).
- [x] `SO_PEERCRED` predicate implemented + unit tests (audit G3).
- [x] 1 MiB line limit enforced on decode and on the accepted socket.
- [x] Fuzz corpus file checked in and wired into #8.
- [x] Extension side implements Hello-first, backoff reconnects, EOF-hide
      (owned jointly with `SPEC-overlay.md`).
- [x] Daemon closes peers that do not send `Hello` within 2 s.
- [x] Late-join replay restores `Show`/`ShowSubgrid` state, or `Hide`.
