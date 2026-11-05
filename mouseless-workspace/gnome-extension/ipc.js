// NDJSON wire client for mouselessd (SPEC-ipc).
//
// The daemon owns the listener; this module is the extension side of the
// versioned protocol. It never parses payload semantics — callers validate
// models and dispatch messages. Connection failures and EOF call `onLost`
// and retry with 500 ms → 10 s backoff so a daemon restart cannot leave a
// stuck overlay.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import { CONFIGURED_SOCKET_PATH } from './config.js';

export const SCHEMA_VERSION = 1;
export const KIND_DAEMON = 'daemon';
export const KIND_OVERLAY = 'overlay';
// Dev override mirrors `mouselessd --socket` / `MOUSELESS_SOCKET` so the
// nested-shell smoke test can use a private socket (SPEC-overlay commands).
export const SOCKET_PATH = GLib.getenv('MOUSELESS_SOCKET') ??
    CONFIGURED_SOCKET_PATH;

const MAX_LINE_BYTES = 1024 * 1024;
const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10000;

/** Plain-object guard for untrusted JSON payloads. */
export function isObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Validate the wire `GridModel` shape before handing it to the renderer. */
export function isModel(model) {
    return isObject(model) && isMonitor(model.monitor) && Array.isArray(model.cells) &&
        model.cells.every(cell => isObject(cell) &&
            Number.isInteger(cell.index) &&
            typeof cell.label === 'string' &&
            ['x', 'y', 'w', 'h'].every(key => Number.isFinite(cell[key])) &&
            cell.w > 0 && cell.h > 0);
}

function isMonitor(monitor) {
    return isObject(monitor) &&
        ['x', 'y', 'w', 'h', 'scale'].every(key => Number.isFinite(monitor[key])) &&
        monitor.w > 0 && monitor.h > 0 && monitor.scale > 0;
}

/**
 * Reconnecting client for the daemon's Unix socket.
 *
 * `handlers.onConnected()` runs after a socket is established (send Hello and
 * Report there); `handlers.onFrame(line)` receives complete NDJSON lines;
 * `handlers.onLost(error)` fires on connect failure, EOF, or write error.
 */
export class OverlaySocket {
    constructor(path, handlers) {
        this._path = path;
        this._handlers = handlers;
        this._address = Gio.UnixSocketAddress.new(path);
        this._client = new Gio.SocketClient();
        this._encoder = new TextEncoder();
        this._cancellable = null;
        this._connection = null;
        this._input = null;
        this._output = null;
        this._retrySource = 0;
        this._backoff = RECONNECT_MIN_MS;
        this._closed = false;
    }

    start() {
        this._closed = false;
        this._connect();
    }

    stop() {
        this._closed = true;
        if (this._retrySource) {
            GLib.source_remove(this._retrySource);
            this._retrySource = 0;
        }
        this._teardown();
    }

    /** Send one envelope; returns false when the link is down. */
    send(kind, payload) {
        if (!this._output)
            return false;
        const frame = JSON.stringify({
            schema_version: SCHEMA_VERSION,
            kind,
            payload,
        });
        try {
            // Local socket, small change-driven frames: a blocking write is
            // bounded in practice and must keep frame order.
            const [ok] = this._output.write_all(
                this._encoder.encode(`${frame}\n`), null);
            return ok;
        } catch (error) {
            this._lost(error);
            return false;
        }
    }

    _connect() {
        if (this._closed || this._connection)
            return;
        this._cancellable = new Gio.Cancellable();
        this._client.connect_async(this._address, this._cancellable,
            (client, result) => this._onConnected(client, result));
    }

    _onConnected(client, result) {
        let connection;
        try {
            connection = client.connect_finish(result);
        } catch (error) {
            if (!this._isCancelled(error)) {
                this._handlers.onLost(error);
                this._scheduleRetry();
            }
            return;
        }
        if (this._closed) {
            connection.close(null);
            return;
        }
        this._connection = connection;
        this._input = new Gio.DataInputStream({
            base_stream: connection.get_input_stream(),
        });
        this._output = connection.get_output_stream();
        this._handlers.onConnected();
        this._readNext();
    }

    _readNext() {
        if (!this._input || this._closed)
            return;
        this._input.read_line_async(GLib.PRIORITY_DEFAULT, this._cancellable,
            (stream, result) => this._onLine(stream, result));
    }

    _onLine(stream, result) {
        let line;
        let length;
        try {
            [line, length] = stream.read_line_finish_utf8(result);
        } catch (error) {
            if (!this._isCancelled(error))
                this._lost(error);
            return;
        }
        if (line === null) {
            this._lost(new Error('EOF'));
            return;
        }
        if (length > MAX_LINE_BYTES) {
            this._lost(new Error(`frame larger than ${MAX_LINE_BYTES} bytes`));
            return;
        }
        // A delivered frame proves a healthy session; reset the backoff.
        this._backoff = RECONNECT_MIN_MS;
        try {
            this._handlers.onFrame(line);
        } catch (error) {
            // Render/parse bugs are logged, never thrown into the Shell.
            console.error(`mouseless: frame handler failed: ${error.message}`);
        }
        this._readNext();
    }

    _lost(error) {
        if (this._closed)
            return;
        this._teardown();
        this._handlers.onLost(error);
        this._scheduleRetry();
    }

    _scheduleRetry() {
        if (this._closed || this._retrySource)
            return;
        const delay = this._backoff;
        this._backoff = Math.min(delay * 2, RECONNECT_MAX_MS);
        this._retrySource = GLib.timeout_add(GLib.PRIORITY_DEFAULT, delay, () => {
            this._retrySource = 0;
            this._connect();
            return GLib.SOURCE_REMOVE;
        });
    }

    _teardown() {
        this._cancellable?.cancel();
        this._cancellable = null;
        this._connection?.close(null);
        this._connection = null;
        this._input = null;
        this._output = null;
    }

    _isCancelled(error) {
        return error instanceof GLib.Error &&
            error.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED);
    }
}
