// Mouseless overlay extension entry point.
//
// Architecture (SPEC-overlay, SPEC-ipc):
// - The daemon owns the Unix socket listener; this extension is the client.
// - The extension never captures input: the actor is non-reactive and tracked
//   with `affectsInputRegion: false`, so keys and pointer pass through.
// - Every socket/parse/render path is guarded: a malformed frame must never
//   take down the Shell, and EOF must hide the overlay immediately.
// - The daemon computes cell bounds; the extension only draws and reports
//   monitor geometry + cursor position.

import GLib from 'gi://GLib';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {
    KIND_DAEMON,
    KIND_OVERLAY,
    OverlaySocket,
    SCHEMA_VERSION,
    SOCKET_PATH,
    isModel,
    isObject,
} from './ipc.js';
import {OverlayActor} from './render.js';

const EXTENSION_VERSION = '1.0.0';

export default class MouselessOverlayExtension {
    constructor() {
        this._overlay = null;
        this._socket = null;
        this._monitorsChangedId = 0;
        this._warned = new Set();
    }

    enable() {
        this._overlay = new OverlayActor();
        this._addChrome(this._overlay.actor);
        this._monitorsChangedId = Main.layoutManager.connect('monitors-changed', () => {
            this._overlay?.reposition();
            this._sendReport();
        });

        this._socket = new OverlaySocket(SOCKET_PATH, {
            onConnected: () => this._onConnected(),
            onFrame: line => this._onFrame(line),
            onLost: error => this._onLost(error),
        });
        this._socket.start();
    }

    /**
     * GNOME 50 removed the `affectsInputRegion` parameter: tracked chrome is
     * no longer part of the input region, and `reactive: false` keeps this
     * actor click-through. GNOME 45–49 still need the explicit opt-out.
     */
    _addChrome(actor) {
        try {
            Main.layoutManager.addTopChrome(actor, {
                affectsInputRegion: false,
                trackFullscreen: true,
            });
        } catch (error) {
            if (!String(error.message).includes('affectsInputRegion'))
                throw error;
            Main.layoutManager.removeChrome(actor);
            Main.layoutManager.addTopChrome(actor, {trackFullscreen: true});
        }
    }

    disable() {
        if (this._monitorsChangedId) {
            Main.layoutManager.disconnect(this._monitorsChangedId);
            this._monitorsChangedId = 0;
        }
        this._socket?.stop();
        this._socket = null;
        if (this._overlay) {
            Main.layoutManager.removeChrome(this._overlay.actor);
            this._overlay.destroy();
            this._overlay = null;
        }
        this._warned.clear();
    }

    _onConnected() {
        this._warned.clear();
        // Hello MUST be the first frame; Report follows immediately so the
        // daemon can compute a grid before the first layer event.
        this._socket?.send(KIND_OVERLAY, {
            type: 'Hello',
            extension_version: EXTENSION_VERSION,
        });
        this._sendReport();
    }

    _onFrame(line) {
        if (!this._overlay)
            return;
        let envelope;
        try {
            envelope = JSON.parse(line);
        } catch (error) {
            this._warnOnce('parse', `dropping malformed frame: ${error.message}`);
            return;
        }
        if (!isObject(envelope) || !Number.isInteger(envelope.schema_version))
            return;
        if (envelope.schema_version > SCHEMA_VERSION) {
            this._warnOnce('schema',
                `daemon schema ${envelope.schema_version} is newer than ${SCHEMA_VERSION}`);
            return;
        }
        // Wrong sender role: drop the frame, keep the connection (SPEC-ipc).
        if (envelope.kind !== KIND_DAEMON)
            return;

        const message = envelope.payload;
        if (!isObject(message) || typeof message.type !== 'string')
            return;
        switch (message.type) {
        case 'Show':
            if (!isModel(message.model)) {
                this._warnOnce('model', 'ignoring Show with an invalid model');
                return;
            }
            this._overlay.show(message.model);
            break;
        case 'ShowSubgrid':
            if (!Number.isInteger(message.parent) || !isModel(message.model)) {
                this._warnOnce('model', 'ignoring ShowSubgrid with an invalid model');
                return;
            }
            this._overlay.showSubgrid(message.parent, message.model);
            break;
        case 'HighlightCell':
            if (Number.isInteger(message.index))
                this._overlay.highlight(message.index);
            break;
        case 'Hide':
            this._overlay.hide();
            break;
        case 'QueryCursor':
            this._sendCursorReply();
            break;
        case 'Error':
            console.warn(`mouseless: daemon error ${message.code}: ${message.message}`);
            break;
        default:
            // Unknown message types are forward-compatible: log-free ignore.
            break;
        }
    }

    _onLost(error) {
        // Kill-switch safety: a dead daemon must never leave a stuck overlay.
        this._overlay?.hide();
        const message = error instanceof GLib.Error ? error.message : String(error);
        this._warnOnce(`lost:${message}`, `overlay link lost (${message}); retrying`);
    }

    _sendReport() {
        if (!this._overlay || !this._socket)
            return;
        const primaryIndex = Main.layoutManager.primaryIndex;
        const monitors = Main.layoutManager.monitors.map(monitor => {
            const scale = monitor.geometry_scale || 1;
            return {
                // Wire geometry is physical px; stage units are physical/scale.
                x: Math.round(monitor.x * scale),
                y: Math.round(monitor.y * scale),
                w: Math.round(monitor.width * scale),
                h: Math.round(monitor.height * scale),
                scale,
                primary: monitor.index === primaryIndex,
            };
        });
        this._socket.send(KIND_OVERLAY, {
            type: 'Report',
            geometry: {monitors},
            cursor: this._physicalPointer(),
        });
    }

    _sendCursorReply() {
        const cursor = this._physicalPointer();
        this._socket?.send(KIND_OVERLAY, {
            type: 'CursorReply',
            x: cursor.x,
            y: cursor.y,
        });
    }

    _physicalPointer() {
        const [x, y] = global.get_pointer();
        const monitor = Main.layoutManager.findMonitorForPoint(x, y) ??
            Main.layoutManager.primaryMonitor;
        const scale = monitor?.geometry_scale || 1;
        return {
            x: Math.round(x * scale),
            y: Math.round(y * scale),
        };
    }

    _warnOnce(key, message) {
        if (this._warned.has(key))
            return;
        this._warned.add(key);
        console.warn(`mouseless: ${message}`);
    }
}
