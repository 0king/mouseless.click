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
const STYLE = {
    dim: [0, 0, 0, 150],
    cell: [16, 20, 28, 180],
    cellMuted: [16, 20, 28, 70],
    parent: [36, 62, 96, 220],
    subCell: [26, 32, 44, 230],
    highlight: [255, 178, 44, 235],
    label: [255, 255, 255, 255],
    labelMuted: [255, 255, 255, 110],
    labelHighlight: [20, 20, 20, 255],
    gap: 1.5,
    fontName: 'Sans Bold 22',
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
        if (this._model || this._subgrid)
            this._rebuild();
    }

    destroy() {
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
                const background = subgrid
                    ? (isParent ? STYLE.parent : STYLE.cellMuted)
                    : STYLE.cell;
                const labelColor = subgrid && !isParent ? STYLE.labelMuted : STYLE.label;
                this._addCell(this._model, cell, background, labelColor,
                    isParent ? 0 : STYLE.gap, subgrid === null);
            }
        }

        if (subgrid) {
            // A replayed ShowSubgrid after a Shell restart carries no level-1
            // model; reconstruct the parent footprint from the subcell union.
            if (!this._model) {
                const footprint = this._unionRect(subgrid.model);
                if (footprint)
                    this._addBlank(footprint, STYLE.parent);
            }
            for (const cell of subgrid.model.cells) {
                this._addCell(subgrid.model, cell, STYLE.subCell, STYLE.label,
                    STYLE.gap, true);
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
                font_name: STYLE.fontName,
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

    _addBlank(rect, background) {
        const actor = new Clutter.Actor({reactive: false});
        actor.set_position(rect.x, rect.y);
        actor.set_size(rect.w, rect.h);
        actor.set_background_color(rgba(background));
        this._layer.add_child(actor);
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

    _unionRect(model) {
        let left = Infinity;
        let top = Infinity;
        let right = -Infinity;
        let bottom = -Infinity;
        for (const cell of model.cells) {
            const rect = this._layoutRect(model, cell);
            left = Math.min(left, rect.x);
            top = Math.min(top, rect.y);
            right = Math.max(right, rect.x + rect.w);
            bottom = Math.max(bottom, rect.y + rect.h);
        }
        if (!Number.isFinite(left))
            return null;
        return {x: left, y: top, w: right - left, h: bottom - top};
    }
}
