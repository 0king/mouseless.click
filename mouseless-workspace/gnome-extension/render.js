// Draw-only Clutter rendering for the Mouseless overlay (SPEC-overlay).
//
// The daemon computes cell bounds; this module only converts wire geometry
// (physical pixels) to stage units and draws. It never captures input: actors
// are non-reactive and the chrome is tracked with `affectsInputRegion: false`.
//
// Coordinate space (spike §10.4 / SPEC-overlay OQ-1): wire bounds and the
// cursor are physical pixels, `physical = stage * monitor.geometry_scale`.

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

// Every display constant lives here so Phase-3 settings can replace it.
//
// The grid background is #d4de04 at 80% transparency (20% opacity, alpha 51).
// The primary overlay tiles the active monitor completely, so there is no
// separate dim layer; the translucent fill is the overlay's only tint.
// Labels are #ebe9e8 as requested.
const GRID_RGB = [0xd4, 0xde, 0x04];
const GRID_ALPHA = 51; // 80% transparent (20% opacity)
const LABEL_RGB = [0xeb, 0xe9, 0xe8];
// Visible separator between two adjacent cells is 2 × the per-edge inset, so
// 0.5 px per edge yields the minimum 1 px line for both grids.
const SEPARATOR_PX = 1;

const STYLE = {
    dim: [0, 0, 0, 0],
    cell: [...GRID_RGB, GRID_ALPHA],
    // Context behind the open subgrid: still the same hue, much fainter, so
    // the selected cell's sub-cells stand out.
    cellMuted: [...GRID_RGB, 20],
    subCell: [...GRID_RGB, GRID_ALPHA],
    highlight: [255, 178, 44, 235],
    label: [...LABEL_RGB, 255],
    labelMuted: [...LABEL_RGB, 110],
    labelHighlight: [...LABEL_RGB, 255],
    gap: SEPARATOR_PX / 2,
    subgridGap: SEPARATOR_PX / 2,
    minFontPx: 7,
    maxFontPx: 20,
};

// Clutter 18 (GNOME 47+) removed Clutter.Color in favour of Cogl.Color;
// GNOME 45/46 still expose Clutter.Color. Resolve once at module load so the
// same actor code runs across the supported Shell range.
const Color = Clutter.Color ?? Cogl.Color;

function rgba([red, green, blue, alpha]) {
    return new Color({red, green, blue, alpha});
}

function inset(rect, gap) {
    return {
        x: rect.x + gap,
        y: rect.y + gap,
        w: Math.max(1, rect.w - 2 * gap),
        h: Math.max(1, rect.h - 2 * gap),
    };
}

/**
 * Pick a font size that fits `label` inside `rect` (stage units).
 *
 * The primary grid can be 10×30 (≈192×36 px cells at 1080p), while a subgrid
 * cell can be ≈19×12 px; a single fixed font would either overflow the small
 * cells or look lost in the large ones.
 */
function fontNameFor(rect, label) {
    const chars = Math.max(1, label.length);
    const byHeight = rect.h * 0.55;
    const byWidth = rect.w / (chars * 0.9);
    const size = Math.max(
        STYLE.minFontPx,
        Math.min(STYLE.maxFontPx, Math.floor(Math.min(byHeight, byWidth))));
    return `Sans Bold ${size}`;
}

/**
 * Fullscreen overlay actor rendering the current grid or subgrid.
 *
 * `show` replaces the model, `showSubgrid` keeps the level-1 context behind
 * the sub-cells, and `hide` is idempotent. All methods are safe to call with
 * unknown indices or after `destroy`.
 */
export class OverlayActor {
    constructor() {
        this._root = new Clutter.Actor({
            name: 'mouseless-overlay',
            reactive: false,
            visible: false,
            opacity: 255,
        });
        this._root.set_background_color(rgba(STYLE.dim));
        // Fill the stage across monitor/scale changes without polling.
        this._root.add_constraint(new Clutter.BindConstraint({
            source: global.stage,
            coordinate: Clutter.BindCoordinate.ALL,
        }));
        this._model = null;
        this._subgrid = null;
        this._highlight = null;
        this._layer = null;
        this._activeCells = new Map();
        // GNOME binds tracked-chrome visibility to fullscreen state
        // (`trackFullscreen`), so it can re-show this actor after a monitor,
        // scale, overview, or fullscreen change. With no model there is
        // nothing to draw; force it back down instead of leaving an empty
        // dim layer on screen.
        this._visibleId = this._root.connect('notify::visible', () => {
            if (this._root.visible && !this._model && !this._subgrid)
                this._root.hide();
        });
    }

    get actor() {
        return this._root;
    }

    show(model) {
        this._model = model;
        this._subgrid = null;
        this._highlight = Number.isInteger(model.highlight) ? model.highlight : null;
        this._rebuild();
        this._root.show();
    }

    showSubgrid(parent, model) {
        this._subgrid = {parent, model};
        this._highlight = Number.isInteger(model.highlight) ? model.highlight : null;
        this._rebuild();
        this._root.show();
    }

    highlight(index) {
        // Unknown indices are ignored (SPEC-overlay: idempotent handler).
        if (!this._activeCells.has(index))
            return;
        this._highlight = index;
        this._applyHighlight();
    }

    hide() {
        this._model = null;
        this._subgrid = null;
        this._highlight = null;
        this._clear();
        this._root.hide();
    }

    /** Re-map the current model after a monitor/scale change. */
    reposition() {
        if (this._model || this._subgrid) {
            this._rebuild();
            // GNOME re-adds tracked chrome on `monitors-changed`; re-assert
            // visibility so a mid-session scale change keeps the overlay up.
            this._root.show();
        } else {
            // The same re-add can make a hidden actor visible again. With no
            // model there is nothing to draw, so keep the chrome hidden.
            this._root.hide();
        }
    }

    destroy() {
        if (this._visibleId) {
            this._root.disconnect(this._visibleId);
            this._visibleId = 0;
        }
        this._clear();
        this._root.destroy();
    }

    _rebuild() {
        this._clear();
        this._layer = new Clutter.Actor({reactive: false});
        this._root.add_child(this._layer);

        const subgrid = this._subgrid;
        if (this._model) {
            for (const cell of this._model.cells) {
                const isParent = subgrid !== null && subgrid.parent === cell.index;
                // The sub-cells replace the selected parent. Drawing the
                // parent underneath would composite to 75% in the sub-cells
                // and leak its two-character label through them.
                if (isParent)
                    continue;
                const background = subgrid ? STYLE.cellMuted : STYLE.cell;
                const labelColor = subgrid ? STYLE.labelMuted : STYLE.label;
                this._addCell(this._model, cell, background, labelColor,
                    STYLE.gap, subgrid === null);
            }
        }

        if (subgrid) {
            for (const cell of subgrid.model.cells) {
                this._addCell(subgrid.model, cell, STYLE.subCell, STYLE.label,
                    STYLE.subgridGap, true);
            }
        }

        this._applyHighlight();
    }

    _addCell(model, cell, background, labelColor, gap, active) {
        const rect = inset(this._layoutRect(model, cell), gap);
        const actor = new Clutter.Actor({reactive: false});
        actor.set_position(rect.x, rect.y);
        actor.set_size(rect.w, rect.h);
        actor.set_background_color(rgba(background));

        let label = null;
        if (cell.label.length > 0) {
            label = new Clutter.Text({
                text: cell.label,
                font_name: fontNameFor(rect, cell.label),
                color: rgba(labelColor),
            });
            actor.add_child(label);
            // BinLayout defaults to centering both axes on 45–50; the
            // explicit BinAlignment enum was removed in Clutter 18.
            actor.set_layout_manager(new Clutter.BinLayout());
        }
        this._layer.add_child(actor);
        if (active)
            this._activeCells.set(cell.index, {actor, label, background, labelColor});
    }

    _applyHighlight() {
        for (const [index, entry] of this._activeCells) {
            const highlighted = index === this._highlight;
            entry.actor.set_background_color(
                rgba(highlighted ? STYLE.highlight : entry.background));
            entry.label?.set_color(
                rgba(highlighted ? STYLE.labelHighlight : entry.labelColor));
        }
    }

    _clear() {
        this._activeCells.clear();
        if (this._layer) {
            this._layer.destroy();
            this._layer = null;
        }
    }

    /** Wire (physical) cell rect → stage units, anchored on its monitor. */
    _layoutRect(model, cell) {
        const monitor = this._stageMonitor(model.monitor);
        const scale = monitor.geometry_scale > 0
            ? monitor.geometry_scale
            : model.monitor.scale;
        return {
            x: monitor.x + (cell.x - model.monitor.x) / scale,
            y: monitor.y + (cell.y - model.monitor.y) / scale,
            w: cell.w / scale,
            h: cell.h / scale,
        };
    }

    /** Match the wire monitor (physical) to a compositor monitor (stage). */
    _stageMonitor(wire) {
        let best = null;
        let bestScore = Infinity;
        for (const monitor of Main.layoutManager.monitors) {
            const scale = monitor.geometry_scale || 1;
            const score =
                Math.abs(monitor.width * scale - wire.w) +
                Math.abs(monitor.height * scale - wire.h) +
                Math.abs(monitor.x * scale - wire.x) +
                Math.abs(monitor.y * scale - wire.y);
            if (score < bestScore) {
                bestScore = score;
                best = monitor;
            }
        }
        return best ?? Main.layoutManager.primaryMonitor ??
            {x: 0, y: 0, geometry_scale: wire.scale};
    }

}
