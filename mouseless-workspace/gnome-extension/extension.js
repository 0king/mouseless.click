// Mouseless overlay extension stub (Phase 1.3 implements rendering + IPC).
//
// GNOME 45+ ESM (audit C2). Draw-only: fullscreen transparent Clutter actor
// via Main.layoutManager.addTopChrome(actor, { affectsInputRegion: false,
// trackFullscreen: true }). Never captures keys; reports monitor geometry +
// cursor position to mouselessd over /run/mouseless/mouselessd.sock; all
// IPC/socket handlers wrapped so a malformed frame can never crash the Shell.

import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

let _actor = null;

export default class MouselessOverlayExtension {
    enable() {
        _actor = new Clutter.Actor({
            visible: false,
            reactive: false,
            opacity: 0,
        });
        // Real placement (Phase 1.3):
        // Main.layoutManager.addTopChrome(_actor, {
        //     affectsInputRegion: false,
        //     trackFullscreen: true,
        // });
    }

    disable() {
        _actor?.destroy();
        _actor = null;
    }
}
