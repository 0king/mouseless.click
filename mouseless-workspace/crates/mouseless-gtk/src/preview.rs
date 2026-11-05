//! Live subgrid preview drawn with Cairo.
//!
//! The preview mirrors the configured subgrid shape (columns × rows) and uses
//! one-character labels, so it matches what the overlay renders inside a
//! selected primary cell. It is not a screenshot: it reads the same appearance
//! values the daemon will resolve from the candidate config.

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Preview model shared with form change handlers.
#[derive(Debug, Clone)]
struct PreviewModel {
    grid: [f64; 3],
    opacity: f64,
    label: [f64; 3],
    highlight: [f64; 3],
    keys: Vec<char>,
    columns: usize,
    rows: usize,
}

impl Default for PreviewModel {
    fn default() -> Self {
        let appearance = &crate::model::app_config().appearance;
        Self {
            grid: crate::model::rgb_channels(&appearance.grid_color),
            opacity: appearance.grid_opacity.clamp(0.0, 1.0),
            label: crate::model::rgb_channels(&appearance.label_color),
            highlight: crate::model::rgb_channels(&appearance.highlight_color),
            keys: mouseless_config::SUBGRID_KEY_POOL.chars().collect(),
            columns: 10,
            rows: 3,
        }
    }
}

/// Drawing area plus its mutable model.
#[derive(Clone)]
pub struct Preview {
    /// Widget added to the Appearance page.
    pub area: gtk::DrawingArea,
    model: Rc<RefCell<PreviewModel>>,
}

impl Preview {
    /// Build the drawing area and wire its draw function.
    pub fn new() -> Self {
        let model = Rc::new(RefCell::new(PreviewModel::default()));
        let area = gtk::DrawingArea::builder()
            .content_width(480)
            .content_height(150)
            .build();
        area.add_css_class("card");
        area.set_tooltip_text(Some(
            "Subgrid preview: grid fill, opacity, labels, and highlight",
        ));
        let draw_model = model.clone();
        area.set_draw_func(move |_, cr, width, height| {
            let model = draw_model.borrow();
            draw(cr, width, height, &model);
        });
        Self { area, model }
    }

    /// Replace the visible appearance/labels/shape and redraw.
    pub fn update(
        &self,
        grid_hex: &str,
        opacity: f64,
        label_hex: &str,
        highlight_hex: &str,
        keys: &str,
        shape: (usize, usize),
    ) {
        let mut model = self.model.borrow_mut();
        model.grid = crate::model::rgb_channels(grid_hex);
        model.opacity = opacity.clamp(0.0, 1.0);
        model.label = crate::model::rgb_channels(label_hex);
        model.highlight = crate::model::rgb_channels(highlight_hex);
        model.keys = keys.chars().collect();
        model.columns = shape.0.max(1);
        model.rows = shape.1.max(1);
        drop(model);
        self.area.queue_draw();
    }
}

impl Default for Preview {
    fn default() -> Self {
        Self::new()
    }
}

fn draw(cr: &gtk::cairo::Context, width: i32, height: i32, model: &PreviewModel) {
    let width = f64::from(width.max(1));
    let height = f64::from(height.max(1));

    // Neutral backdrop so transparency is visible.
    cr.set_source_rgb(0.12, 0.12, 0.12);
    let _ = cr.paint();

    let columns = model.columns.max(1);
    let rows = model.rows.max(1);
    let pad = 10.0;
    let gap = 3.0;
    let cell_w = ((width - 2.0 * pad - gap * (columns as f64 - 1.0)) / columns as f64).max(1.0);
    let cell_h = ((height - 2.0 * pad - gap * (rows as f64 - 1.0)) / rows as f64).max(1.0);
    let radius = (cell_h * 0.12).clamp(2.0, 8.0);

    for row in 0..rows {
        for column in 0..columns {
            let index = row * columns + column;
            let x = pad + column as f64 * (cell_w + gap);
            let y = pad + row as f64 * (cell_h + gap);
            let (red, green, blue) = if index == 5 {
                let [r, g, b] = model.highlight;
                (r, g, b)
            } else {
                let [r, g, b] = model.grid;
                (r, g, b)
            };
            rounded_rect(cr, x, y, cell_w, cell_h, radius);
            cr.set_source_rgba(red, green, blue, model.opacity);
            let _ = cr.fill();

            // Faint outline keeps cells legible at low opacity.
            rounded_rect(cr, x, y, cell_w, cell_h, radius);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
            cr.set_line_width(1.0);
            let _ = cr.stroke();

            let label = model
                .keys
                .get(index)
                .map(|ch| ch.to_string())
                .unwrap_or_else(|| "?".to_string());
            let [r, g, b] = model.label;
            cr.set_source_rgba(r, g, b, 1.0);
            let font_size = (cell_h * 0.48).clamp(10.0, 20.0);
            cr.select_font_face(
                "monospace",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Bold,
            );
            cr.set_font_size(font_size);
            if let Ok(extents) = cr.text_extents(&label) {
                cr.move_to(
                    x + (cell_w - extents.width()) / 2.0 - extents.x_bearing(),
                    y + (cell_h - extents.height()) / 2.0 - extents.y_bearing(),
                );
                let _ = cr.show_text(&label);
            }
        }
    }
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, radius: f64) {
    let radius = radius.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(
        x + w - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        x + w - radius,
        y + h - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + h - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}
