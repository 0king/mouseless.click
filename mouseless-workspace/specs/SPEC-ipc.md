# SPEC-ipc — Wire protocol (`mouseless-ipc`)

> Module: `ipc` · Paths: `crates/mouseless-ipc` · Depends on: — ·
> Plan refs: §3 rule 3, §4 (contracts), §8 #8, audit G2/G3 · Matrix row: #8
> Status as of 2026-10-18: **Partial** — envelope + core messages implemented;
> `QueryCursor`/`CursorReply`/`Error` frames, `kind` validation, auth, fuzz corpus pending.
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
    {"index": 0, "label": "a", "x": 0,   "y": 0,   "w": 480, "h": 540},
    {"index": 1, "label": "s", "x": 480, "y": 0,   "w": 480, "h": 540}
  ],
  "highlight": null
}
```

- `cells` are row-major; `index` matches the daemon's `CellId` ordering.
- `label` is a display string (single character today).
- For `ShowSubgrid`, `model.cells` are sub-cells of `parent` and MAY carry
  different labels than level 1.

### overlay → daemon (kind `"overlay"`)

| Message | Payload schema |
|---|---|
| `Hello` | `{"type":"Hello","extension_version":"1.0.0"}` — MUST be the first frame after connect |
| `Report` | `{"type":"Report","geometry":{"monitors":[{"x":0,"y":0,"w":1920,"h":1080,"scale":1.0,"primary":true}]},"cursor":{"x":960,"y":540}}` |
| `CursorReply` | `{"type":"CursorReply","x":960,"y":540}` — response to `QueryCursor` |

- `Report` is sent on connect and whenever monitor layout/scale changes.
- `cursor` is the compositor-reported global cursor position; its coordinate
  space is pinned by spike §10.4 (see `SPEC-core.md` OQ-1). Until the spike
  closes, both sides log the value and the daemon validates the closed loop
  (after `move_rel(dx,dy)`, re-query and check error; systematic scale error →
  correction factor, recorded in the daemon spec).

### client → daemon (kind `"client"`, Phase 2/3)

| Message | Payload schema |
|---|---|
| `Reload` | `{"type":"Reload"}` — re-read config, validate-then-apply |
| `Update` | `{"type":"Update","config":ConfigData}` — apply new config (schema owned by `SPEC-settings.md`) |

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

- Reconnect backoff (overlay): 500 ms doubling to 10 s, no jitter required.
- No keepalive frames: local transport; daily traffic is change-driven.
- Multiple overlay clients MAY connect; the daemon broadcasts to all (only one
  is expected in practice). `Hello` replay covers late joiners.

## Version negotiation

- `schema_version == 1` → accept.
- `schema_version < 1` → accept (forward-compatible reads; v1 is the minimum).
- `schema_version > 1` → send one `Error{code:"version_mismatch"}` frame, then close.

This matches the implemented `decode()` policy: refuse newer, accept older.

## Auth & permissions (audit G3)

- Filesystem: `/run/mouseless` (0770, group `mouseless`) — only group members
  can open the socket.
- Daemon MUST verify `SO_PEERCRED` on accept: peer uid allowed if
  `uid == daemon euid` OR the uid is a member of the `mouseless` group
  (resolved via `getgrouplist`). Disallowed → log + close, no frames parsed.
- Implemented as a pure predicate (`peer_allowed(cred, policy) -> bool`) so the
  policy is unit-testable without spawning peers with foreign uids.
- **OQ-1:** plan §4 says "checks that the client is the daemon UID"; that would
  reject the Shell extension running as the desktop user. Proposed reading
  above (uid or group membership). Requires human sign-off because it shapes
  the systemd/user model in `SPEC-platform.md`.

## Robustness rules

- Parser never panics: any malformed line → log + count + skip (connection
  stays); version mismatch → `Error` + close; oversized line → close.
- Senders never block forever: socket writes use a timeout; on timeout/EPIPE,
  drop the client (daemon) or reconnect (overlay).
- Fuzz corpus: malformed set (empty, `{`, `not json`, truncated valid frames)
  ships as test data; a `cargo-fuzz` target for `decode()` is desired but the
  curated corpus is the blocking test (#8).

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
envelope + message enums, `encode`/`decode`. When `client` messages land, add
`client.rs` with `Reload`/`Update` and keep direction enums in `lib.rs`.

## Code style

- Wire types carry rustdoc with the exact JSON shape; every field documents units.
- `decode` is total: returns `Result`, never panics, accepts older versions.
- New message variants require: spec update (this file), round-trip test,
  corpus entry, and a daemon/extension handler or an explicit "ignore" clause.

## Testing strategy (#8, blocking)

- Round-trips for every message type. *(Show/Hide/Report exist; add others)*
- Newer `schema_version` refused; older accepted. *(exists)*
- Malformed corpus + truncation of every valid frame never panics. *(exists, formalize as shared corpus)*
- Phase 1.3: auth predicate unit tests; `Hello` timeout; late-join replay;
  `QueryCursor` timeout falls back to last `Report`, aborts session if none.

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
- [ ] `QueryCursor`/`CursorReply`/`Error` + `client` messages implemented and tested.
- [ ] `kind` direction validation (drop mismatched, never close).
- [ ] `SO_PEERCRED` predicate implemented + unit tests (audit G3).
- [ ] 1 MiB line limit enforced with test.
- [ ] Fuzz corpus file checked in and wired into #8.
- [ ] Extension side implements Hello-first, backoff reconnects, EOF-hide
      (owned jointly with `SPEC-overlay.md`).
