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
    let w = monitor.w as f64 / config.cols as f64;
    let h = monitor.h as f64 / config.rows as f64;
    let x = monitor.x as f64 + cell.col as f64 * w;
    let y = monitor.y as f64 + cell.row as f64 * h;
    // Absorb float remainder in the last column/row so tiles sum exactly.
    let (w, h) = (
        if cell.col + 1 == config.cols {
            monitor.x as f64 + monitor.w as f64 - x
        } else {
            w
        },
        if cell.row + 1 == config.rows {
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
