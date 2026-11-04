//! `mouseless-core`: pure grid math, zero I/O.
//!
//! Cell → pixel geometry, monitor mapping, and scale-aware layout helpers.
//! This crate never touches evdev, TCP, sockets, or the compositor — geometry
//! inputs (cursor pos, monitor layout) arrive as parameters.
//!
//! See plan §1.8 (trait boundaries) and §4 (geometry contracts).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from pure grid-math validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    /// Grid dimensions must be non-zero and key count must cover all cells.
    #[error("invalid grid config: {0}")]
    InvalidConfig(&'static str),
}

/// 2-D point in physical pixels (compositor coordinate space).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// X in physical pixels, relative to the monitor origin.
    pub x: f64,
    /// Y in physical pixels, relative to the monitor origin.
    pub y: f64,
}

/// Axis-aligned rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge in physical pixels.
    pub x: f64,
    /// Top edge in physical pixels.
    pub y: f64,
    /// Width in physical pixels.
    pub w: f64,
    /// Height in physical pixels.
    pub h: f64,
}

impl Rect {
    /// Center of the rectangle.
    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.w / 2.0,
            y: self.y + self.h / 2.0,
        }
    }
}

/// Monitor geometry as reported by the overlay (plan §4).
///
/// `scale` is the fractional scale factor (1.0, 1.25, 1.5, 2.0, …).
/// All pixel math is done in physical pixels: `logical * scale`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    /// X offset of this monitor in the global compositor layout (physical px).
    pub x: i32,
    /// Y offset of this monitor in the global compositor layout (physical px).
    pub y: i32,
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
    /// Fractional scale factor, e.g. 1.0, 1.25, 1.5, 2.0.
    pub scale: f64,
}

/// Screen corner used for cursor hiding after a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
}

/// Direction for multi-monitor switching (monitors ordered by x position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorDirection {
    /// Previous monitor in x order (wraps).
    Left,
    /// Next monitor in x order (wraps).
    Right,
}

/// Target point for cursor hiding: `inset` physical pixels inside `corner`.
///
/// The inset keeps the pointer on-screen and inside the monitor edge, so the
/// next cursor query still reports the hidden corner rather than a clamp.
pub fn cursor_hide_target(monitor: &Monitor, corner: Corner, inset: f64) -> Point {
    let inset = inset.max(0.0);
    let left = monitor.x as f64 + inset;
    let top = monitor.y as f64 + inset;
    let right = monitor.x as f64 + monitor.w as f64 - inset;
    let bottom = monitor.y as f64 + monitor.h as f64 - inset;
    match corner {
        Corner::TopLeft => Point { x: left, y: top },
        Corner::TopRight => Point { x: right, y: top },
        Corner::BottomLeft => Point { x: left, y: bottom },
        Corner::BottomRight => Point {
            x: right,
            y: bottom,
        },
    }
}

/// Select the monitor adjacent to `current` in x order, wrapping at the ends.
///
/// `monitors` is the extension-reported layout order; `current` indexes that
/// slice. Returns the index of the chosen monitor, or `None` for an empty
/// layout. Ties on x are broken by y so the order is deterministic.
pub fn select_monitor(
    monitors: &[Monitor],
    current: usize,
    direction: MonitorDirection,
) -> Option<usize> {
    if monitors.is_empty() {
        return None;
    }
    let mut order: Vec<usize> = (0..monitors.len()).collect();
    order.sort_by(|&a, &b| {
        monitors[a]
            .x
            .cmp(&monitors[b].x)
            .then_with(|| monitors[a].y.cmp(&monitors[b].y))
    });
    let current = current.min(order.len().saturating_sub(1));
    let pos = order.iter().position(|&i| i == current).unwrap_or(0);
    let next = match direction {
        MonitorDirection::Left => (pos + order.len() - 1) % order.len(),
        MonitorDirection::Right => (pos + 1) % order.len(),
    };
    Some(order[next])
}

/// Grid configuration for one overlay level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridConfig {
    /// Number of cells across (level columns).
    pub cols: u32,
    /// Number of cells down (level rows).
    pub rows: u32,
    /// Key labels, one per cell, row-major. Length must equal `cols * rows`.
    pub keys: Vec<char>,
}

impl GridConfig {
    /// Validate dimensions and key coverage.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.cols == 0 || self.rows == 0 {
            return Err(CoreError::InvalidConfig("cols and rows must be non-zero"));
        }
        if self.keys.len() as u64 != self.cols as u64 * self.rows as u64 {
            return Err(CoreError::InvalidConfig(
                "keys.len() must equal cols * rows",
            ));
        }
        Ok(())
    }

    /// Number of cells at this level.
    pub fn cell_count(&self) -> u32 {
        self.cols * self.rows
    }
}

/// Two-level primary grid: a level-1 key selects a column, a level-2 key
/// selects a row, and each displayed cell carries both characters as its
/// label (e.g. `AQ`, `SQ`, `AW`).
///
/// This mirrors the mouseless.click "position based columns" preset: ten
/// `asdfghjkl;` columns over thirty `qwerty…/` rows. Selecting a cell takes
/// two key presses (first column, then row); the subgrid is then shown inside
/// that cell. The composite is `cols * rows` cells even though only
/// `cols + rows` keys are needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimaryGrid {
    /// Number of displayed columns; equals `column_keys.len()`.
    pub cols: u32,
    /// Number of displayed rows; equals `row_keys.len()`.
    pub rows: u32,
    /// Level-1 (column) keys, one per displayed column, left to right.
    pub column_keys: Vec<char>,
    /// Level-2 (row) keys, one per displayed row, top to bottom.
    pub row_keys: Vec<char>,
}

impl PrimaryGrid {
    /// Validate dimensions, key coverage, and key uniqueness.
    ///
    /// Duplicate column or row keys are rejected because selection by key
    /// would be ambiguous (the first occurrence would always win).
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.cols == 0 || self.rows == 0 {
            return Err(CoreError::InvalidConfig("cols and rows must be non-zero"));
        }
        if self.column_keys.len() as u64 != self.cols as u64 {
            return Err(CoreError::InvalidConfig(
                "column_keys.len() must equal cols",
            ));
        }
        if self.row_keys.len() as u64 != self.rows as u64 {
            return Err(CoreError::InvalidConfig("row_keys.len() must equal rows"));
        }
        if has_duplicates(&self.column_keys) {
            return Err(CoreError::InvalidConfig("column keys must be unique"));
        }
        if has_duplicates(&self.row_keys) {
            return Err(CoreError::InvalidConfig("row keys must be unique"));
        }
        Ok(())
    }

    /// Number of displayed cells (`cols * rows`).
    pub fn cell_count(&self) -> u32 {
        self.cols * self.rows
    }

    /// Index of `cell` in row-major order.
    pub fn index(&self, col: u32, row: u32) -> u32 {
        row * self.cols + col
    }

    /// Row-major index → cell id (clamped to the grid).
    pub fn cell_id(&self, index: u32) -> CellId {
        CellId {
            col: index % self.cols,
            row: (index / self.cols).min(self.rows.saturating_sub(1)),
        }
    }

    /// Two-character display label for `cell` (uppercase).
    ///
    /// Uppercasing is display-only: the daemon still matches the lowercase
    /// characters emitted by the grabbed keyboard device.
    pub fn label(&self, cell: CellId) -> String {
        let column = self.column_keys[cell.col as usize].to_ascii_uppercase();
        let row = self.row_keys[cell.row as usize].to_ascii_uppercase();
        format!("{column}{row}")
    }

    /// Column index for a level-1 key (case-insensitive).
    pub fn column_of(&self, key: char) -> Option<u32> {
        let key = key.to_ascii_lowercase();
        self.column_keys
            .iter()
            .position(|candidate| *candidate == key)
            .map(|i| i as u32)
    }

    /// Row index for a level-2 key (case-insensitive).
    pub fn row_of(&self, key: char) -> Option<u32> {
        let key = key.to_ascii_lowercase();
        self.row_keys
            .iter()
            .position(|candidate| *candidate == key)
            .map(|i| i as u32)
    }

    /// Bounds of `cell` on `monitor` in physical pixels.
    pub fn bounds(&self, monitor: &Monitor, cell: CellId) -> Rect {
        debug_assert!(cell.col < self.cols && cell.row < self.rows);
        tile_bounds(self.cols, self.rows, monitor, cell)
    }

    /// Center of `cell` on `monitor` in physical pixels.
    pub fn center(&self, monitor: &Monitor, cell: CellId) -> Point {
        self.bounds(monitor, cell).center()
    }
}

fn has_duplicates(keys: &[char]) -> bool {
    keys.iter()
        .enumerate()
        .any(|(i, key)| keys[..i].contains(key))
}

/// Index of a cell within a grid level (row-major).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellId {
    /// Column, 0-based.
    pub col: u32,
    /// Row, 0-based.
    pub row: u32,
}

/// Bounds of cell `(col, row)` on `monitor` in physical pixels.
///
/// Cells tile the monitor exactly; the last column/row absorbs rounding.
pub fn cell_bounds(config: &GridConfig, monitor: &Monitor, cell: CellId) -> Rect {
    debug_assert!(cell.col < config.cols && cell.row < config.rows);
    tile_bounds(config.cols, config.rows, monitor, cell)
}

/// Shared tiling math for any `cols × rows` grid.
///
/// The last column/row absorbs the float remainder so tiles sum exactly to
/// the monitor bounds.
fn tile_bounds(cols: u32, rows: u32, monitor: &Monitor, cell: CellId) -> Rect {
    let w = monitor.w as f64 / cols as f64;
    let h = monitor.h as f64 / rows as f64;
    let x = monitor.x as f64 + cell.col as f64 * w;
    let y = monitor.y as f64 + cell.row as f64 * h;
    let (w, h) = (
        if cell.col + 1 == cols {
            monitor.x as f64 + monitor.w as f64 - x
        } else {
            w
        },
        if cell.row + 1 == rows {
            monitor.y as f64 + monitor.h as f64 - y
        } else {
            h
        },
    );
    Rect { x, y, w, h }
}

/// Center of cell `(col, row)` on `monitor` in physical pixels.
pub fn cell_center(config: &GridConfig, monitor: &Monitor, cell: CellId) -> Point {
    cell_bounds(config, monitor, cell).center()
}

/// Sub-cell bounds: divide `parent` into a `sub`-configured grid.
pub fn subcell_bounds(parent: &Rect, sub: &GridConfig, cell: CellId) -> Rect {
    debug_assert!(cell.col < sub.cols && cell.row < sub.rows);
    let w = parent.w / sub.cols as f64;
    let h = parent.h / sub.rows as f64;
    Rect {
        x: parent.x + cell.col as f64 * w,
        y: parent.y + cell.row as f64 * h,
        w,
        h,
    }
}

/// Convert a logical (scale-independent) size to physical pixels.
pub fn logical_to_physical(logical: f64, scale: f64) -> f64 {
    logical * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_3x2() -> GridConfig {
        GridConfig {
            cols: 3,
            rows: 2,
            keys: vec!['a', 's', 'd', 'f', 'j', 'k'],
        }
    }

    fn monitor_1080p() -> Monitor {
        Monitor {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            scale: 1.0,
        }
    }

    #[test]
    fn validate_ok() {
        assert!(grid_3x2().validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_and_mismatched_keys() {
        assert!(GridConfig {
            cols: 0,
            rows: 2,
            keys: vec![],
        }
        .validate()
        .is_err());
        assert!(GridConfig {
            cols: 2,
            rows: 2,
            keys: vec!['a'],
        }
        .validate()
        .is_err());
    }

    #[test]
    fn golden_1080p_cell_centers() {
        // 1920/3 = 640 wide, 1080/2 = 540 tall.
        let g = grid_3x2();
        let m = monitor_1080p();
        assert_eq!(
            cell_center(&g, &m, CellId { col: 0, row: 0 }),
            Point { x: 320.0, y: 270.0 }
        );
        assert_eq!(
            cell_center(&g, &m, CellId { col: 2, row: 1 }),
            Point {
                x: 1600.0,
                y: 810.0
            }
        );
    }

    #[test]
    fn tiles_cover_monitor_exactly() {
        // Last column/row absorbs rounding so tiles sum to the full size.
        let g = GridConfig {
            cols: 7,
            rows: 5,
            keys: vec!['x'; 35],
        };
        let m = Monitor {
            x: 1920,
            y: 0,
            w: 1080,
            h: 1920,
            scale: 1.0,
        };
        let mut max_x = 0.0f64;
        let mut max_y = 0.0f64;
        for col in 0..7 {
            for row in 0..5 {
                let r = cell_bounds(&g, &m, CellId { col, row });
                max_x = max_x.max(r.x + r.w);
                max_y = max_y.max(r.y + r.h);
            }
        }
        assert!((max_x - 3000.0).abs() < 1e-6);
        assert!((max_y - 1920.0).abs() < 1e-6);
    }

    #[test]
    fn scale_150_percent_centers() {
        // 4K at 200%: geometry is already physical; scale only affects
        // logical→physical conversion of UI sizes.
        assert_eq!(logical_to_physical(10.0, 1.5), 15.0);
        let g = grid_3x2();
        let m = Monitor {
            x: 0,
            y: 0,
            w: 3840,
            h: 2160,
            scale: 2.0,
        };
        assert_eq!(
            cell_center(&g, &m, CellId { col: 1, row: 0 }),
            Point {
                x: 1920.0,
                y: 540.0
            }
        );
    }

    #[test]
    fn subgrid_center_nests() {
        let g = grid_3x2();
        let m = monitor_1080p();
        let parent = cell_bounds(&g, &m, CellId { col: 0, row: 0 });
        let sub = GridConfig {
            cols: 2,
            rows: 2,
            keys: vec!['a', 's', 'd', 'f'],
        };
        let c = subcell_bounds(&parent, &sub, CellId { col: 1, row: 1 }).center();
        assert_eq!(c, Point { x: 480.0, y: 405.0 });
    }

    #[test]
    fn cursor_hide_target_insets_each_corner() {
        let m = monitor_1080p();
        assert_eq!(
            cursor_hide_target(&m, Corner::TopLeft, 1.0),
            Point { x: 1.0, y: 1.0 }
        );
        assert_eq!(
            cursor_hide_target(&m, Corner::TopRight, 1.0),
            Point { x: 1919.0, y: 1.0 }
        );
        assert_eq!(
            cursor_hide_target(&m, Corner::BottomLeft, 1.0),
            Point { x: 1.0, y: 1079.0 }
        );
        assert_eq!(
            cursor_hide_target(&m, Corner::BottomRight, 1.0),
            Point {
                x: 1919.0,
                y: 1079.0
            }
        );
        // Negative inset is clamped to the monitor edge.
        assert_eq!(
            cursor_hide_target(&m, Corner::TopLeft, -5.0),
            Point { x: 0.0, y: 0.0 }
        );
    }

    #[test]
    fn monitor_switch_orders_by_x_and_wraps() {
        let monitors = [
            Monitor {
                x: 1920,
                y: 0,
                w: 1080,
                h: 1920,
                scale: 1.0,
            },
            Monitor {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
            Monitor {
                x: 3000,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
        ];
        // x order is index 1, 0, 2.
        assert_eq!(
            select_monitor(&monitors, 1, MonitorDirection::Right),
            Some(0)
        );
        assert_eq!(
            select_monitor(&monitors, 0, MonitorDirection::Right),
            Some(2)
        );
        assert_eq!(
            select_monitor(&monitors, 2, MonitorDirection::Right),
            Some(1)
        );
        assert_eq!(
            select_monitor(&monitors, 1, MonitorDirection::Left),
            Some(2)
        );
        assert_eq!(select_monitor(&[], 0, MonitorDirection::Right), None);
    }

    fn primary_10x30() -> PrimaryGrid {
        PrimaryGrid {
            cols: 10,
            rows: 30,
            column_keys: "asdfghjkl;".chars().collect(),
            row_keys: "qwertyuiopasdfghjkl;zxcvbnm,./".chars().collect(),
        }
    }

    #[test]
    fn primary_grid_validates_keys_and_dimensions() {
        assert!(primary_10x30().validate().is_ok());
        let mut grid = primary_10x30();
        grid.cols = 9;
        assert!(grid.validate().is_err());
        let mut grid = primary_10x30();
        grid.row_keys.truncate(29);
        assert!(grid.validate().is_err());
        let mut grid = primary_10x30();
        grid.column_keys[1] = 'a';
        assert!(grid.validate().is_err());
        let mut grid = primary_10x30();
        grid.rows = 0;
        assert!(grid.validate().is_err());
    }

    #[test]
    fn primary_grid_labels_combine_column_then_row() {
        let grid = primary_10x30();
        assert_eq!(grid.label(CellId { col: 0, row: 0 }), "AQ");
        assert_eq!(grid.label(CellId { col: 1, row: 0 }), "SQ");
        assert_eq!(grid.label(CellId { col: 0, row: 1 }), "AW");
        assert_eq!(grid.label(CellId { col: 9, row: 29 }), ";/");
        // Every pair is unique (contains each column × row exactly once).
        let mut labels: Vec<String> = (0..grid.cell_count())
            .map(|i| grid.label(grid.cell_id(i)))
            .collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), grid.cell_count() as usize);
    }

    #[test]
    fn primary_grid_looks_up_keys_case_insensitively() {
        let grid = primary_10x30();
        assert_eq!(grid.column_of('a'), Some(0));
        assert_eq!(grid.column_of('A'), Some(0));
        assert_eq!(grid.column_of(';'), Some(9));
        assert_eq!(grid.column_of('q'), None);
        assert_eq!(grid.row_of('q'), Some(0));
        assert_eq!(grid.row_of('/'), Some(29));
        assert_eq!(grid.row_of('o'), Some(8));
        assert_eq!(grid.row_of('o').map(|row| grid.index(0, row)), Some(80));
    }

    #[test]
    fn primary_grid_tiles_monitor_exactly() {
        let grid = primary_10x30();
        let monitor = monitor_1080p();
        let first = grid.bounds(&monitor, CellId { col: 0, row: 0 });
        assert_eq!((first.w, first.h), (192.0, 36.0));
        let last = grid.bounds(&monitor, CellId { col: 9, row: 29 });
        assert!((last.x + last.w - 1920.0).abs() < 1e-6);
        assert!((last.y + last.h - 1080.0).abs() < 1e-6);
    }

    #[test]
    fn vertical_monitor_with_offset() {
        // Second monitor to the right: global coords include the x offset.
        let g = grid_3x2();
        let m = Monitor {
            x: 1920,
            y: 0,
            w: 1080,
            h: 1920,
            scale: 1.0,
        };
        let c = cell_center(&g, &m, CellId { col: 0, row: 0 });
        assert_eq!(
            c,
            Point {
                x: 1920.0 + 180.0,
                y: 480.0
            }
        );
    }
}
