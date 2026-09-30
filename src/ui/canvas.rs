//! Double-buffered drawing. Shapes are antialiased from signed distances in a
//! 32-bit DIB; text uses GDI so it keeps ClearType and the system font.
use std::ptr;

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

use super::theme::Rgb;
use super::{wide, wide_text};

pub type Point = (f32, f32);

pub struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
    left: i32,
    top: i32,
    gdi_pending: bool,
}

#[inline]
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

#[inline]
fn pack(color: Rgb) -> u32 {
    (u32::from(color.0) << 16) | (u32::from(color.1) << 8) | u32::from(color.2)
}

#[inline]
fn blend(destination: u32, color: Rgb, alpha: f32) -> u32 {
    if alpha >= 0.999 {
        return pack(color);
    }
    let mix = |d: u32, s: u8| (d as f32 + (f32::from(s) - d as f32) * alpha + 0.5) as u32;
    (mix((destination >> 16) & 255, color.0) << 16)
        | (mix((destination >> 8) & 255, color.1) << 8)
        | mix(destination & 255, color.2)
}

impl Canvas {
    /// A buffer covering `area`, addressed in the target's coordinates.
    pub fn new(target: HDC, area: RECT) -> Option<Self> {
        let width = (area.right - area.left).max(1);
        let height = (area.bottom - area.top).max(1);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let dc = CreateCompatibleDC(target);
            if dc.is_null() {
                return None;
            }
            let mut bits = ptr::null_mut();
            let bitmap =
                CreateDIBSection(target, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, bitmap);
            SetViewportOrgEx(dc, -area.left, -area.top, ptr::null_mut());
            SetBkMode(dc, TRANSPARENT as i32);
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                width,
                height,
                left: area.left,
                top: area.top,
                gdi_pending: false,
            })
        }
    }

    pub fn present(&mut self, target: HDC) {
        unsafe {
            GdiFlush();
            BitBlt(
                target,
                self.left,
                self.top,
                self.width,
                self.height,
                self.dc,
                self.left,
                self.top,
                SRCCOPY,
            );
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        if self.gdi_pending {
            unsafe { GdiFlush() };
            self.gdi_pending = false;
        }
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }

    /// Pixel range covering `[x0, x1) x [y0, y1)` in target coordinates.
    fn span(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<(i32, i32, i32, i32)> {
        let left = ((x0.floor() as i32) - self.left).max(0);
        let top = ((y0.floor() as i32) - self.top).max(0);
        let right = ((x1.ceil() as i32) - self.left).min(self.width);
        let bottom = ((y1.ceil() as i32) - self.top).min(self.height);
        (left < right && top < bottom).then_some((left, top, right, bottom))
    }

    fn shade(
        &mut self,
        bounds: (f32, f32, f32, f32),
        color: Rgb,
        alpha: f32,
        mut cover: impl FnMut(f32, f32) -> f32,
    ) {
        let Some((left, top, right, bottom)) = self.span(bounds.0, bounds.1, bounds.2, bounds.3)
        else {
            return;
        };
        let (origin_x, origin_y, width) = (self.left as f32, self.top as f32, self.width);
        let pixels = self.pixels();
        for y in top..bottom {
            let py = y as f32 + origin_y + 0.5;
            let row = (y * width) as usize;
            for x in left..right {
                let amount = cover(x as f32 + origin_x + 0.5, py) * alpha;
                if amount > 0.002 {
                    let pixel = &mut pixels[row + x as usize];
                    *pixel = blend(*pixel, color, amount);
                }
            }
        }
    }

    pub fn fill(&mut self, rect: RECT, color: Rgb) {
        self.fill_alpha(rect, color, 1.0);
    }

    pub fn fill_alpha(&mut self, rect: RECT, color: Rgb, alpha: f32) {
        let Some((left, top, right, bottom)) = self.span(
            rect.left as f32,
            rect.top as f32,
            rect.right as f32,
            rect.bottom as f32,
        ) else {
            return;
        };
        let width = self.width;
        let value = pack(color);
        let pixels = self.pixels();
        for y in top..bottom {
            let row = &mut pixels[(y * width + left) as usize..(y * width + right) as usize];
            if alpha >= 0.999 {
                row.fill(value);
            } else {
                for pixel in row {
                    *pixel = blend(*pixel, color, alpha);
                }
            }
        }
    }

    /// Rounded rectangle whose outer edge lies on `rect`; the border is drawn
    /// inside it. Radii are top-left, top-right, bottom-right, bottom-left.
    pub fn round_rect(
        &mut self,
        rect: RECT,
        radii: [f32; 4],
        fill: Option<Rgb>,
        border: Option<(Rgb, f32)>,
    ) {
        let (x0, y0, x1, y1) = (
            rect.left as f32,
            rect.top as f32,
            rect.right as f32,
            rect.bottom as f32,
        );
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
        let limit = hx.min(hy);
        let radii = radii.map(|r| r.clamp(0.0, limit));
        let distance = move |px: f32, py: f32| {
            let r = match (px > cx, py > cy) {
                (false, false) => radii[0],
                (true, false) => radii[1],
                (true, true) => radii[2],
                (false, true) => radii[3],
            };
            let qx = (px - cx).abs() - hx + r;
            let qy = (py - cy).abs() - hy + r;
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            outside + qx.max(qy).min(0.0) - r
        };
        // Only the edge bands need distances; the interior is a plain fill.
        let edge = (radii.iter().copied().fold(0.0, f32::max)
            + border.map_or(0.0, |(_, width)| width)
            + 1.0)
            .ceil();
        let bands = if x1 - x0 > 2.0 * edge && y1 - y0 > 2.0 * edge {
            if let Some(color) = fill {
                self.fill(
                    RECT {
                        left: (x0 + edge) as i32,
                        top: (y0 + edge) as i32,
                        right: (x1 - edge) as i32,
                        bottom: (y1 - edge) as i32,
                    },
                    color,
                );
            }
            vec![
                (x0, y0, x1, y0 + edge),
                (x0, y1 - edge, x1, y1),
                (x0, y0 + edge, x0 + edge, y1 - edge),
                (x1 - edge, y0 + edge, x1, y1 - edge),
            ]
        } else {
            vec![(x0, y0, x1, y1)]
        };
        for band in bands {
            if let Some(color) = fill {
                self.shade(band, color, 1.0, |px, py| coverage(distance(px, py)));
            }
            if let Some((color, width)) = border {
                self.shade(band, color, 1.0, |px, py| {
                    let d = distance(px, py);
                    coverage(d) - coverage(d + width)
                });
            }
        }
    }

    /// Antialiased convex polygon.
    pub fn polygon(&mut self, points: &[Point], color: Rgb, alpha: f32) {
        if points.len() < 3 {
            return;
        }
        let count = points.len() as f32;
        let center = points
            .iter()
            .fold((0.0, 0.0), |a, p| (a.0 + p.0 / count, a.1 + p.1 / count));
        let mut edges = Vec::with_capacity(points.len());
        for (index, a) in points.iter().enumerate() {
            let b = points[(index + 1) % points.len()];
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let length = ex.hypot(ey);
            if length < 1e-4 {
                continue;
            }
            let (mut nx, mut ny) = (ey / length, -ex / length);
            if (center.0 - a.0) * nx + (center.1 - a.1) * ny > 0.0 {
                (nx, ny) = (-nx, -ny);
            }
            edges.push((a.0, a.1, nx, ny));
        }
        let bounds = bounds_of(points, 1.0);
        self.shade(bounds, color, alpha, |px, py| {
            let d = edges
                .iter()
                .map(|(ax, ay, nx, ny)| (px - ax) * nx + (py - ay) * ny)
                .fold(f32::MIN, f32::max);
            coverage(d)
        });
    }

    /// Antialiased stroke with round joins, drawn centered on the path.
    pub fn polyline(&mut self, points: &[Point], width: f32, color: Rgb, closed: bool) {
        if points.len() < 2 {
            return;
        }
        let mut segments: Vec<(Point, Point)> = points.windows(2).map(|w| (w[0], w[1])).collect();
        if closed {
            segments.push((points[points.len() - 1], points[0]));
        }
        let half = width / 2.0;
        let bounds = bounds_of(points, half + 1.0);
        self.shade(bounds, color, 1.0, |px, py| {
            let d = segments
                .iter()
                .map(|(a, b)| segment_distance((px, py), *a, *b))
                .fold(f32::MAX, f32::min);
            coverage(d - half)
        });
    }

    pub fn circle(
        &mut self,
        center: Point,
        radius: f32,
        fill: Option<Rgb>,
        border: Option<(Rgb, f32)>,
    ) {
        let bounds = (
            center.0 - radius - 1.0,
            center.1 - radius - 1.0,
            center.0 + radius + 1.0,
            center.1 + radius + 1.0,
        );
        let distance = move |px: f32, py: f32| (px - center.0).hypot(py - center.1) - radius;
        if let Some(color) = fill {
            self.shade(bounds, color, 1.0, |px, py| coverage(distance(px, py)));
        }
        if let Some((color, width)) = border {
            self.shade(bounds, color, 1.0, |px, py| {
                let d = distance(px, py);
                coverage(d) - coverage(d + width)
            });
        }
    }

    pub fn text(&mut self, rect: RECT, text: &str, font: HFONT, color: Rgb, format: u32) {
        if text.is_empty() {
            return;
        }
        let mut rect = rect;
        // DrawText's wrapping code may inspect the terminator even with an
        // explicit length. Keep it in allocated storage at the FFI boundary.
        let mut wide = wide(text);
        unsafe {
            let previous = SelectObject(self.dc, font);
            SetTextColor(self.dc, color.colorref());
            DrawTextW(
                self.dc,
                wide.as_mut_ptr(),
                (wide.len() - 1) as i32,
                &mut rect,
                format,
            );
            SelectObject(self.dc, previous);
        }
        self.gdi_pending = true;
    }

    /// Text drawn with a font's own escapement, anchored with `SetTextAlign`.
    pub fn text_at(&mut self, point: (i32, i32), text: &str, font: HFONT, color: Rgb, align: u32) {
        let wide = wide_text(text);
        unsafe {
            let previous_font = SelectObject(self.dc, font);
            SetTextColor(self.dc, color.colorref());
            let previous = SetTextAlign(self.dc, align);
            TextOutW(self.dc, point.0, point.1, wide.as_ptr(), wide.len() as i32);
            SetTextAlign(self.dc, previous);
            SelectObject(self.dc, previous_font);
        }
        self.gdi_pending = true;
    }

    /// The buffer as a 32-bit BMP file, for rendering previews in tests.
    #[cfg(test)]
    pub fn to_bmp(&self) -> Vec<u8> {
        unsafe { GdiFlush() };
        let pixels = (self.width * self.height) as usize;
        let data = unsafe { std::slice::from_raw_parts(self.bits.cast::<u8>(), pixels * 4) };
        let size = 54 + data.len() as u32;
        let mut file = Vec::with_capacity(size as usize);
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&size.to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&self.width.to_le_bytes());
        file.extend_from_slice(&(-self.height).to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&[0; 24]);
        file.extend_from_slice(data);
        file
    }

    /// Height of `text` word-wrapped in `width` pixels.
    pub fn wrapped_height(&self, font: HFONT, text: &str, width: i32) -> i32 {
        wrapped_height(self.dc, font, text, width)
    }

    pub fn measure(&self, font: HFONT, text: &str) -> (i32, i32) {
        measure(self.dc, font, text)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// Signed distance from a point to a rounded box centered at `c`.
fn rounded_box(p: Point, c: Point, half: Point, radius: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - half.0 + radius;
    let qy = (p.1 - c.1).abs() - half.1 + radius;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// The window icon: a tablet with its active area, drawn at `size` pixels
/// so it stays sharp at any DPI.
pub fn app_icon(size: i32, accent: Rgb) -> HICON {
    let s = size as f32;
    let center = (s / 2.0, s / 2.0);
    let stroke = (s / 16.0).max(1.0);
    let pixels: Vec<u32> = (0..size * size)
        .map(|i| {
            let p = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
            let body = coverage(rounded_box(
                p,
                center,
                (s / 2.0 - 0.5, s / 2.0 - 0.5),
                s * 0.22,
            ));
            let area = rounded_box(p, center, (s * 0.3, s * 0.2), s * 0.05);
            let ring = coverage(area) - coverage(area + stroke);
            let dot = coverage((p.0 - center.0).hypot(p.1 - center.1) - s * 0.075);
            let white = ring.max(dot);
            let mix = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * white).round() as u32;
            let alpha = (body * 255.0).round() as u32;
            (alpha << 24) | (mix(accent.0) << 16) | (mix(accent.1) << 8) | mix(accent.2)
        })
        .collect();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let screen = GetDC(ptr::null_mut());
        let mut bits = ptr::null_mut();
        let color = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
        ReleaseDC(ptr::null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
        let icon = windows_sys::Win32::UI::WindowsAndMessaging::CreateIconIndirect(
            &windows_sys::Win32::UI::WindowsAndMessaging::ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            },
        );
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

pub fn measure(dc: HDC, font: HFONT, text: &str) -> (i32, i32) {
    let wide = wide_text(text);
    let mut size = SIZE::default();
    unsafe {
        let previous = SelectObject(dc, font);
        GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
        SelectObject(dc, previous);
    }
    (size.cx, size.cy)
}

pub fn measure_font_height(font: HFONT) -> i32 {
    let mut metrics = TEXTMETRICW::default();
    unsafe {
        let dc = GetDC(ptr::null_mut());
        let previous = SelectObject(dc, font);
        GetTextMetricsW(dc, &mut metrics);
        SelectObject(dc, previous);
        ReleaseDC(ptr::null_mut(), dc);
    }
    metrics.tmHeight
}

/// Height of word-wrapped text in `width` pixels.
pub fn wrapped_height(dc: HDC, font: HFONT, text: &str, width: i32) -> i32 {
    // DrawTextW can read the input pointer for an empty, length-counted
    // string. An empty Vec<u16> has address 0x2, not readable storage.
    if text.is_empty() {
        return 0;
    }
    let mut wide = wide(text);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width.max(1),
        bottom: 0,
    };
    unsafe {
        let previous = SelectObject(dc, font);
        DrawTextW(
            dc,
            wide.as_mut_ptr(),
            (wide.len() - 1) as i32,
            &mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(dc, previous);
    }
    rect.bottom
}

fn bounds_of(points: &[Point], margin: f32) -> (f32, f32, f32, f32) {
    points.iter().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(x0, y0, x1, y1), p| {
            (
                x0.min(p.0 - margin),
                y0.min(p.1 - margin),
                x1.max(p.0 + margin),
                y1.max(p.1 + margin),
            )
        },
    )
}

fn segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (p.0 - a.0, p.1 - a.1);
    let length = abx * abx + aby * aby;
    let t = if length > 0.0 {
        ((apx * abx + apy * aby) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (apx - abx * t).hypot(apy - aby * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_one_inside_and_zero_outside() {
        assert_eq!(coverage(-2.0), 1.0);
        assert_eq!(coverage(2.0), 0.0);
        assert_eq!(coverage(0.0), 0.5);
        assert_eq!(blend(0, Rgb(255, 255, 255), 1.0), 0x00FF_FFFF);
        assert_eq!(blend(0x00FF_FFFF, Rgb(0, 0, 0), 0.5), 0x0080_8080);
        assert!((segment_distance((0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)) - 1.0).abs() < 1e-6);
    }
}
