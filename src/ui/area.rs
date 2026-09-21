//! Area editor canvas, drawn like OpenTabletDriver's AreaDisplay: the usable
//! bounds (monitors or the tablet) behind a translucent accent area with its
//! width, height and aspect ratio.
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, HFONT, TA_CENTER, TA_TOP,
};

use super::canvas::{Canvas, Point};
use super::model::{Bounds, format_number};
use super::theme::Palette;
use crate::mapping::OtdArea;

/// Maps area units (pixels or millimetres) to client pixels.
#[derive(Clone, Copy)]
pub struct AreaView {
    pub rect: RECT,
    pub bounds: Bounds,
    scale: f64,
    origin: (f64, f64),
}

impl AreaView {
    pub fn new(rect: RECT, bounds: Bounds) -> Option<Self> {
        let width = f64::from(rect.right - rect.left - 2);
        let height = f64::from(rect.bottom - rect.top - 2);
        if width <= 0.0 || height <= 0.0 || !bounds.valid() {
            return None;
        }
        let scale = (width / bounds.width()).min(height / bounds.height());
        let origin = (
            f64::from(rect.left + rect.right) / 2.0 - bounds.width() * scale / 2.0,
            f64::from(rect.top + rect.bottom) / 2.0 - bounds.height() * scale / 2.0,
        );
        Some(Self {
            rect,
            bounds,
            scale,
            origin,
        })
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn project(&self, x: f64, y: f64) -> Point {
        (
            (self.origin.0 + (x - self.bounds.left) * self.scale) as f32,
            (self.origin.1 + (y - self.bounds.top) * self.scale) as f32,
        )
    }

    fn pixel_rect(&self, bounds: Bounds) -> RECT {
        let (x0, y0) = self.project(bounds.left, bounds.top);
        let (x1, y1) = self.project(bounds.right, bounds.bottom);
        RECT {
            left: x0.round() as i32,
            top: y0.round() as i32,
            right: x1.round() as i32,
            bottom: y1.round() as i32,
        }
    }

    /// Corners clockwise from the top-left, rotated about the center.
    pub fn corners(&self, area: &OtdArea) -> [Point; 4] {
        let center = self.project(area.x, area.y);
        let (sin, cos) = area.rotation.to_radians().sin_cos();
        let (hw, hh) = (
            area.width / 2.0 * self.scale,
            area.height / 2.0 * self.scale,
        );
        [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| {
            (
                center.0 + (x * cos - y * sin) as f32,
                center.1 + (x * sin + y * cos) as f32,
            )
        })
    }

    pub fn hit(&self, area: &OtdArea, point: (i32, i32)) -> bool {
        let center = self.project(area.x, area.y);
        let (dx, dy) = (
            f64::from(point.0) - f64::from(center.0),
            f64::from(point.1) - f64::from(center.1),
        );
        let (sin, cos) = area.rotation.to_radians().sin_cos();
        let (x, y) = (dx * cos + dy * sin, -dx * sin + dy * cos);
        x.abs() <= area.width / 2.0 * self.scale + 2.0
            && y.abs() <= area.height / 2.0 * self.scale + 2.0
    }

    pub fn contains(&self, point: (i32, i32)) -> bool {
        point.0 >= self.rect.left
            && point.0 < self.rect.right
            && point.1 >= self.rect.top
            && point.1 < self.rect.bottom
    }
}

pub struct AreaFonts {
    pub small: HFONT,
    /// Pixel height of `small`, used to create rotated copies.
    pub small_pixels: i32,
}

fn frame(canvas: &mut Canvas, rect: RECT, color: super::theme::Rgb) {
    let RECT {
        left,
        top,
        right,
        bottom,
    } = rect;
    for edge in [
        RECT {
            left,
            top,
            right,
            bottom: top + 1,
        },
        RECT {
            left,
            top: bottom - 1,
            right,
            bottom,
        },
        RECT {
            left,
            top,
            right: left + 1,
            bottom,
        },
        RECT {
            left: right - 1,
            top,
            right,
            bottom,
        },
    ] {
        canvas.fill(edge, color);
    }
}

/// What one area editor shows.
pub struct AreaScene<'a> {
    pub view: Option<&'a AreaView>,
    pub rect: RECT,
    pub backgrounds: &'a [Bounds],
    pub area: OtdArea,
    pub unit: &'a str,
    pub invalid_text: &'a str,
}

pub fn paint(canvas: &mut Canvas, scene: &AreaScene, fonts: &AreaFonts, palette: &Palette) {
    let AreaScene {
        view,
        rect,
        backgrounds,
        area,
        unit,
        invalid_text,
    } = *scene;
    let area = &area;
    let valid = area.width > 0.0 && area.height > 0.0 && area.x.is_finite() && area.y.is_finite();
    let Some(view) = view.filter(|_| valid) else {
        canvas.text(
            rect,
            invalid_text,
            fonts.small,
            palette.text,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        return;
    };
    for bounds in backgrounds {
        let pixels = view.pixel_rect(*bounds);
        canvas.fill(pixels, palette.bounds_fill);
        frame(canvas, pixels, palette.bounds_border);
    }

    let corners = view.corners(area);
    if area.rotation.rem_euclid(90.0) == 0.0 {
        // Keep axis-aligned areas on whole pixels so the border stays crisp.
        let rect = RECT {
            left: corners.iter().map(|p| p.0).fold(f32::MAX, f32::min).round() as i32,
            top: corners.iter().map(|p| p.1).fold(f32::MAX, f32::min).round() as i32,
            right: corners.iter().map(|p| p.0).fold(f32::MIN, f32::max).round() as i32,
            bottom: corners.iter().map(|p| p.1).fold(f32::MIN, f32::max).round() as i32,
        };
        canvas.fill_alpha(rect, palette.area_fill, palette.area_alpha);
        frame(canvas, rect, palette.area_border);
    } else {
        canvas.polygon(&corners, palette.area_fill, palette.area_alpha);
        canvas.polyline(&corners, 1.0, palette.area_border, true);
    }
    let center = view.project(area.x, area.y);
    canvas.circle(center, 1.5, Some(palette.area_border), None);

    // Labels sit where upstream draws them: width inside the top edge, height
    // along the left edge, ratio just below the center. Areas too small for a
    // label get it outside the edge instead of on top of the border.
    let (width_px, height_px) = (area.width * view.scale, area.height * view.scale);
    let (sin, cos) = area.rotation.to_radians().sin_cos();
    let place = |x: f64, y: f64| {
        (
            (f64::from(center.0) + x * cos - y * sin).round() as i32,
            (f64::from(center.1) + x * sin + y * cos).round() as i32,
        )
    };
    let text_height = f64::from(canvas.measure(fonts.small, "0").1);
    let angle = -(area.rotation * 10.0).round() as i32;
    let ratio = format_number(area.width / area.height, 4);
    let width_text = format!("{}{unit}", format_number(area.width, 3));
    let height_text = format!("{}{unit}", format_number(area.height, 3));
    let tall = height_px >= text_height * 3.5;
    let wide = width_px >= text_height * 3.5;
    let ratio_y = if tall {
        text_height / 2.0
    } else {
        height_px / 2.0 + 2.0
    };
    let width_y = if tall {
        -height_px / 2.0 + 3.0
    } else {
        -height_px / 2.0 - text_height - 2.0
    };
    let height_x = if wide {
        -width_px / 2.0 + 3.0
    } else {
        -width_px / 2.0 - text_height - 2.0
    };
    let labels = [
        (place(0.0, ratio_y), ratio, angle),
        (place(0.0, width_y), width_text, angle),
        (place(height_x, 0.0), height_text, angle + 900),
    ];
    for (point, text, angle) in labels {
        let rotated = angle.rem_euclid(3600) != 0;
        let font = if rotated {
            super::create_font(fonts.small_pixels, 400, "Segoe UI", angle.rem_euclid(3600))
        } else {
            fonts.small
        };
        canvas.text_at(point, &text, font, palette.text, TA_CENTER | TA_TOP);
        if rotated {
            unsafe { DeleteObject(font) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_fits_bounds_and_hit_tests_rotated_areas() {
        let rect = RECT {
            left: 10,
            top: 10,
            right: 236,
            bottom: 160,
        };
        let view = AreaView::new(rect, Bounds::tablet()).unwrap();
        assert!((view.scale() - 1.0).abs() < 1e-9);
        let area = OtdArea {
            width: 40.0,
            height: 10.0,
            x: 112.0,
            y: 74.0,
            rotation: 90.0,
        };
        let center = view.project(112.0, 74.0);
        let center = (center.0 as i32, center.1 as i32);
        assert!(view.hit(&area, (center.0, center.1 + 15)));
        assert!(!view.hit(&area, (center.0 + 15, center.1)));
        assert!(AreaView::new(RECT::default(), Bounds::tablet()).is_none());
    }
}
