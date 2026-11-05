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

import { CONFIGURED_APPEARANCE } from './config.js';

// Display defaults and layout constants. Colors and opacity come from the
// model; these defaults cover older/malformed payloads and are generated from
// mouseless.toml by the extension installer.
// The primary overlay tiles the active monitor completely, so there is no
// separate dim layer; the translucent fill is the overlay's only tint.
const DEFAULT_APPEARANCE = CONFIGURED_APPEARANCE;
// Muted context behind an open subgrid keeps the same hue at 20/51 of the
// configured grid alpha; labels drop to alpha 110 (pinned contrast).
const MUTED_GRID_FACTOR = 20 / 51;
const MUTED_LABEL_ALPHA = 110;
const HIGHLIGHT_ALPHA = 235;
// Visible separator between two adjacent cells is 2 × the per-edge inset, so
// 0.5 px per edge yields the minimum 1 px line for both grids.
const SEPARATOR_PX = 1;

const LAYOUT = {
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

function isRgb(value) {
    return Array.isArray(value) && value.length === 3 &&
        value.every(channel => Number.isInteger(channel) &&
            channel >= 0 && channel <= 255);
}

/** Resolve one model's appearance, falling back per field. */
function appearanceFor(model) {
    const raw = model?.appearance && typeof model.appearance === 'object'
        ? model.appearance
        : {};
    const opacity = Number.isFinite(raw.grid_opacity)
        ? Math.min(1, Math.max(0, raw.grid_opacity))
        : DEFAULT_APPEARANCE.grid_opacity;
    return {
        grid_color: isRgb(raw.grid_color)
            ? raw.grid_color
            : DEFAULT_APPEARANCE.grid_color,
        grid_opacity: opacity,
        label_color: isRgb(raw.label_color)
            ? raw.label_color
            : DEFAULT_APPEARANCE.label_color,
        highlight_color: isRgb(raw.highlight_color)
            ? raw.highlight_color
            : DEFAULT_APPEARANCE.highlight_color,
    };
}

function styleFor(model) {
    const appearance = appearanceFor(model);
    const alpha = Math.round(appearance.grid_opacity * 255);
    return {
        cell: [...appearance.grid_color, alpha],
        // Context behind the open subgrid: still the same hue, much fainter,
        // so the selected cell's sub-cells stand out.
        cellMuted: [...appearance.grid_color, Math.round(alpha * MUTED_GRID_FACTOR)],
        subCell: [...appearance.grid_color, alpha],
        highlight: [...appearance.highlight_color, HIGHLIGHT_ALPHA],
        label: [...appearance.label_color, 255],
        labelMuted: [...appearance.label_color, MUTED_LABEL_ALPHA],
        labelHighlight: [...appearance.label_color, 255],
        ...LAYOUT,
    };
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
        LAYOUT.minFontPx,
        Math.min(LAYOUT.maxFontPx, Math.floor(Math.min(byHeight, byWidth))));
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
        this._root.set_background_color(rgba([0, 0, 0, 0]));
        // Fill the stage across monitor/scale changes without polling.
        this._root.add_constraint(new Clutter.BindConstraint({
            source: global.stage,
            coordinate: Clutter.BindCoordinate.ALL,
        }));
        this._model = null;
        this._subgrid = null;
        this._highlight = null;
        // Pending column emphasis from `HighlightColumn`: `{column, cols}`.
        this._highlightColumn = null;
        this._style = styleFor(null);
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
        // A fresh model starts a fresh selection; the daemon re-sends any
        // pending column emphasis after Show.
        this._highlightColumn = null;
        this._style = styleFor(model);
        this._rebuild();
        this._root.show();
    }

    showSubgrid(parent, model) {
        this._subgrid = {parent, model};
        this._highlight = Number.isInteger(model.highlight) ? model.highlight : null;
        // The open subgrid replaces the level-1 column emphasis.
        this._highlightColumn = null;
        this._style = styleFor(model);
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

    /**
     * Emphasise one primary-grid column.
     *
     * Labels in every other column drop to the muted alpha; the selected
     * column keeps the configured label colour at full opacity. `column` is
     * `null` to clear. `cols` maps a row-major cell `index` back to its
     * column (`index % cols`), so the renderer never re-derives geometry.
     */
    highlightColumn(column, cols) {
        if (column === null) {
            this._highlightColumn = null;
        } else if (Number.isInteger(column) && column >= 0 &&
                   Number.isInteger(cols) && cols > 0) {
            this._highlightColumn = {column, cols};
        } else {
            return; // Malformed emphasis: keep the last valid state.
        }
        this._applyHighlight();
    }

    hide() {
        this._model = null;
        this._subgrid = null;
        this._highlight = null;
        this._highlightColumn = null;
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

        const style = this._style;
        const subgrid = this._subgrid;
        if (this._model) {
            for (const cell of this._model.cells) {
                const isParent = subgrid !== null && subgrid.parent === cell.index;
                // The sub-cells replace the selected parent. Drawing the
                // parent underneath would composite to 75% in the sub-cells
                // and leak its two-character label through them.
                if (isParent)
                    continue;
                const background = subgrid ? style.cellMuted : style.cell;
                const labelColor = subgrid ? style.labelMuted : style.label;
                this._addCell(this._model, cell, background, labelColor,
                    style.gap, subgrid === null);
            }
        }

        if (subgrid) {
            for (const cell of subgrid.model.cells) {
                this._addCell(subgrid.model, cell, style.subCell, style.label,
                    style.subgridGap, true);
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
        const column = this._highlightColumn;
        for (const [index, entry] of this._activeCells) {
            const highlighted = index === this._highlight;
            // A pending column key dims every label outside that column; the
            // backgrounds stay untouched so the grid does not flicker.
            const dimmed = column !== null && index % column.cols !== column.column;
            entry.actor.set_background_color(
                rgba(highlighted ? this._style.highlight : entry.background));
            entry.label?.set_color(rgba(
                highlighted ? this._style.labelHighlight
                    : dimmed ? this._style.labelMuted
                        : entry.labelColor));
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
