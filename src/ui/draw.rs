//! Painting for the custom-drawn controls. Shapes follow the Windows WPF
//! controls OpenTabletDriver uses, with the active palette's colors.
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;

use super::canvas::Canvas;
use super::model::FilterItem;
use super::theme::{Palette, Rgb};

#[derive(Clone, Copy, Debug)]
pub struct Fonts {
    pub ui: HFONT,
    pub bold: HFONT,
    pub small: HFONT,
    pub mono: HFONT,
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub palette: Palette,
    pub fonts: Fonts,
    pub scale: f32,
}

impl Style {
    pub fn px(&self, value: f32) -> f32 {
        (value * self.scale).round()
    }

    pub fn ipx(&self, value: f32) -> i32 {
        self.px(value) as i32
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub hot: bool,
    pub pressed: bool,
    pub focus: bool,
    pub disabled: bool,
    pub checked: bool,
    pub selected: bool,
    pub cues: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    None,
    Play,
    Stop,
}

pub const TEXT_CENTER: u32 = DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;
pub const TEXT_LEFT: u32 = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;

fn prefix(state: &State) -> u32 {
    if state.cues { 0 } else { DT_HIDEPREFIX }
}

pub fn inset(rect: RECT, x: i32, y: i32) -> RECT {
    RECT {
        left: rect.left + x,
        top: rect.top + y,
        right: rect.right - x,
        bottom: rect.bottom - y,
    }
}

fn radius(style: &Style, value: f32) -> [f32; 4] {
    [style.px(value); 4]
}

pub fn button(
    canvas: &mut Canvas,
    rect: RECT,
    text: &str,
    style: &Style,
    surface: Rgb,
    state: State,
    glyph: Glyph,
) {
    let p = &style.palette;
    canvas.fill(rect, surface);
    let (fill, border, color) = if state.disabled {
        (
            p.button.mix(surface, 0.5),
            p.button_border.mix(surface, 0.5),
            p.disabled,
        )
    } else if state.pressed {
        (p.button_pressed, p.button_pressed_border, p.text)
    } else if state.hot {
        (p.button_hot, p.button_hot_border, p.text)
    } else if state.focus {
        (p.button, p.button_hot_border, p.text)
    } else {
        (p.button, p.button_border, p.text)
    };
    canvas.round_rect(rect, radius(style, 3.0), Some(fill), Some((border, 1.0)));
    if state.focus && state.cues && !state.disabled {
        let ring = inset(rect, 1, 1);
        canvas.round_rect(
            ring,
            radius(style, 2.0),
            None,
            Some((p.button_hot_border, 1.0)),
        );
    }
    let (text_width, _) = canvas.measure(style.fonts.ui, text);
    let glyph_size = style.px(8.0);
    let gap = if glyph == Glyph::None {
        0.0
    } else {
        glyph_size + style.px(7.0)
    };
    let start = ((rect.left + rect.right) as f32 - (text_width as f32 + gap)) / 2.0;
    let middle = (rect.top + rect.bottom) as f32 / 2.0;
    let accent = if state.disabled { p.disabled } else { p.accent };
    match glyph {
        Glyph::None => {}
        Glyph::Play => {
            let x = start.round();
            canvas.polygon(
                &[
                    (x, middle - glyph_size / 2.0),
                    (x + glyph_size * 0.85, middle),
                    (x, middle + glyph_size / 2.0),
                ],
                accent,
                1.0,
            );
        }
        Glyph::Stop => {
            let x = start.round() as i32;
            let size = glyph_size as i32;
            let top = (middle - glyph_size / 2.0).round() as i32;
            canvas.round_rect(
                RECT {
                    left: x,
                    top,
                    right: x + size,
                    bottom: top + size,
                },
                radius(style, 1.5),
                Some(p.error),
                None,
            );
        }
    }
    let (text_rect, format) = if glyph == Glyph::None {
        (rect, TEXT_CENTER)
    } else {
        let left = (start + gap).round() as i32;
        (RECT { left, ..rect }, TEXT_LEFT)
    };
    canvas.text(
        text_rect,
        text,
        style.fonts.ui,
        color,
        format | prefix(&state),
    );
}

/// Menu bar entry: text only until hovered or open.
pub fn flat_button(
    canvas: &mut Canvas,
    rect: RECT,
    text: &str,
    style: &Style,
    surface: Rgb,
    state: State,
) {
    let p = &style.palette;
    canvas.fill(rect, surface);
    let fill = if state.pressed || state.selected {
        Some(p.selection)
    } else if state.hot || (state.focus && state.cues) {
        Some(p.hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        canvas.round_rect(
            inset(rect, 0, style.ipx(1.0)),
            radius(style, 3.0),
            Some(fill),
            None,
        );
    }
    let color = if state.disabled {
        p.disabled
    } else if state.pressed || state.selected {
        p.selection_text()
    } else {
        p.text
    };
    canvas.text(
        rect,
        text,
        style.fonts.ui,
        color,
        TEXT_CENTER | prefix(&state),
    );
}

/// Classic tab header. The selected tab is taller and opens into the page;
/// others sit on the page border line at the bottom of `rect`.
pub fn tab(canvas: &mut Canvas, rect: RECT, text: &str, style: &Style, state: State) {
    let p = &style.palette;
    canvas.fill(rect, p.window);
    let r = style.px(3.0);
    if state.selected {
        let shape = RECT {
            bottom: rect.bottom + r as i32 + 2,
            ..rect
        };
        canvas.round_rect(
            shape,
            [r, r, 0.0, 0.0],
            Some(p.page),
            Some((p.tab_border, 1.0)),
        );
        canvas.fill(
            RECT {
                left: rect.left + 1,
                top: rect.bottom - 1,
                right: rect.right - 1,
                bottom: rect.bottom,
            },
            p.page,
        );
    } else {
        let (fill, border) = if state.hot {
            (p.button_hot, p.button_hot_border.mix(p.tab_border, 0.5))
        } else {
            (
                p.window.mix(p.page, if p.dark { 0.35 } else { 0.0 }),
                p.tab_border,
            )
        };
        let shape = RECT {
            top: rect.top + style.ipx(2.0),
            bottom: rect.bottom + r as i32 + 1,
            ..rect
        };
        canvas.round_rect(
            shape,
            [r, r, 0.0, 0.0],
            Some(fill),
            Some((border.mix(p.window, 0.25), 1.0)),
        );
        canvas.fill(
            RECT {
                top: rect.bottom - 1,
                ..rect
            },
            p.tab_border,
        );
    }
    let text_rect = RECT {
        top: rect.top + if state.selected { 0 } else { style.ipx(2.0) },
        ..rect
    };
    let color = if state.selected {
        p.text
    } else {
        p.text.mix(p.window, 0.15)
    };
    canvas.text(
        text_rect,
        text,
        style.fonts.ui,
        color,
        TEXT_CENTER | prefix(&state),
    );
    if state.focus && state.cues {
        canvas.round_rect(
            inset(text_rect, 3, 4),
            radius(style, 2.0),
            None,
            Some((p.accent, 1.0)),
        );
    }
}

pub fn dropdown(
    canvas: &mut Canvas,
    rect: RECT,
    text: &str,
    style: &Style,
    surface: Rgb,
    state: State,
) {
    let p = &style.palette;
    canvas.fill(rect, surface);
    let (fill, border) = if state.disabled {
        (
            p.button.mix(surface, 0.5),
            p.button_border.mix(surface, 0.5),
        )
    } else if state.pressed || state.selected {
        (p.button_pressed, p.button_pressed_border)
    } else if state.hot {
        (p.button_hot, p.button_hot_border)
    } else if state.focus {
        (p.field, p.button_hot_border)
    } else {
        (p.field, p.button_border)
    };
    canvas.round_rect(rect, radius(style, 3.0), Some(fill), Some((border, 1.0)));
    let padding = style.ipx(8.0);
    let arrow = style.px(4.0);
    let cx = rect.right as f32 - padding as f32 - arrow;
    let cy = (rect.top + rect.bottom) as f32 / 2.0;
    let color = if state.disabled { p.disabled } else { p.text };
    canvas.polyline(
        &[
            (cx - arrow, cy - arrow / 2.0),
            (cx, cy + arrow / 2.0),
            (cx + arrow, cy - arrow / 2.0),
        ],
        style.px(1.3).max(1.0),
        color,
        false,
    );
    let text_rect = RECT {
        left: rect.left + padding,
        right: rect.right - padding * 2 - arrow as i32 * 2,
        ..rect
    };
    canvas.text(
        text_rect,
        text,
        style.fonts.ui,
        color,
        TEXT_LEFT | prefix(&state),
    );
}

pub fn checkbox(
    canvas: &mut Canvas,
    rect: RECT,
    text: &str,
    style: &Style,
    surface: Rgb,
    state: State,
) {
    let p = &style.palette;
    canvas.fill(rect, surface);
    let size = style.ipx(16.0);
    let top = (rect.top + rect.bottom - size) / 2;
    let boxed = RECT {
        left: rect.left + 1,
        top,
        right: rect.left + 1 + size,
        bottom: top + size,
    };
    let unchecked_border = if p.dark {
        p.muted
    } else {
        p.text.mix(p.page, 0.35)
    };
    if state.checked {
        let fill = if state.disabled {
            p.disabled
        } else if state.pressed {
            p.accent.mix(surface, 0.25)
        } else if state.hot {
            p.accent.mix(p.text, 0.12)
        } else {
            p.accent
        };
        canvas.round_rect(boxed, radius(style, 3.0), Some(fill), None);
        let (x, y, s) = (boxed.left as f32, boxed.top as f32, size as f32);
        canvas.polyline(
            &[
                (x + s * 0.24, y + s * 0.52),
                (x + s * 0.42, y + s * 0.70),
                (x + s * 0.77, y + s * 0.32),
            ],
            style.px(1.6).max(1.2),
            p.accent_text,
            false,
        );
    } else {
        let (fill, border) = if state.disabled {
            (surface, p.disabled)
        } else if state.pressed {
            (p.button_pressed, p.accent)
        } else if state.hot {
            (p.hover, p.accent)
        } else {
            (p.field, unchecked_border)
        };
        canvas.round_rect(boxed, radius(style, 3.0), Some(fill), Some((border, 1.0)));
    }
    let text_rect = RECT {
        left: boxed.right + style.ipx(8.0),
        ..rect
    };
    let color = if state.disabled { p.disabled } else { p.text };
    canvas.text(
        text_rect,
        text,
        style.fonts.ui,
        color,
        TEXT_LEFT | prefix(&state),
    );
    if state.focus && state.cues {
        let (width, height) = canvas.measure(style.fonts.ui, text);
        let middle = (rect.top + rect.bottom) / 2;
        let ring = RECT {
            left: text_rect.left - 3,
            top: middle - height / 2 - 1,
            right: (text_rect.left + width + 3).min(rect.right),
            bottom: middle + height / 2 + 1,
        };
        canvas.round_rect(ring, radius(style, 2.0), None, Some((p.accent, 1.0)));
    }
}

/// Trackbar channel with the part left of the thumb filled.
pub fn slider(canvas: &mut Canvas, channel: RECT, thumb_x: i32, style: &Style, disabled: bool) {
    let p = &style.palette;
    let height = style.ipx(4.0);
    let middle = (channel.top + channel.bottom) / 2;
    let track = RECT {
        left: channel.left,
        top: middle - height / 2,
        right: channel.right,
        bottom: middle - height / 2 + height,
    };
    let rest = if p.dark {
        Rgb::hex(0x9A9A9A)
    } else {
        Rgb::hex(0x8A8A8A)
    };
    let r = [height as f32 / 2.0; 4];
    canvas.round_rect(
        track,
        r,
        Some(if disabled { p.disabled } else { rest }),
        None,
    );
    let filled = RECT {
        right: thumb_x.clamp(track.left, track.right),
        ..track
    };
    if filled.right > filled.left + height {
        canvas.round_rect(
            filled,
            r,
            Some(if disabled { p.disabled } else { p.accent }),
            None,
        );
    }
}

pub fn slider_thumb(canvas: &mut Canvas, thumb: RECT, style: &Style, surface: Rgb, state: State) {
    let p = &style.palette;
    canvas.fill(thumb, surface);
    let center = (
        (thumb.left + thumb.right) as f32 / 2.0,
        (thumb.top + thumb.bottom) as f32 / 2.0,
    );
    let outer = style.px(9.0);
    let ring = if p.dark {
        Rgb::hex(0x454545)
    } else {
        Rgb::hex(0xCCCCCC)
    };
    canvas.circle(
        center,
        outer,
        Some(if p.dark {
            Rgb::hex(0x454545)
        } else {
            Rgb::hex(0xFFFFFF)
        }),
        Some((ring, 1.0)),
    );
    let inner = if state.pressed {
        style.px(4.0)
    } else if state.hot || state.focus {
        style.px(6.0)
    } else {
        style.px(5.0)
    };
    canvas.circle(
        center,
        inner,
        Some(if state.disabled { p.disabled } else { p.accent }),
        None,
    );
}

/// Frame drawn by the parent around a borderless edit control.
pub fn field_frame(
    canvas: &mut Canvas,
    rect: RECT,
    style: &Style,
    focused: bool,
    invalid: bool,
    disabled: bool,
) {
    let p = &style.palette;
    let border = if invalid {
        p.error
    } else if focused {
        p.accent
    } else if disabled {
        p.disabled
    } else {
        p.field_border
    };
    canvas.round_rect(rect, radius(style, 3.0), Some(p.field), Some((border, 1.0)));
    if (focused || invalid) && !p.high_contrast {
        let underline = style.ipx(2.0);
        canvas.round_rect(
            RECT {
                left: rect.left + 1,
                top: rect.bottom - underline,
                right: rect.right - 1,
                bottom: rect.bottom,
            },
            [0.0, 0.0, style.px(3.0), style.px(3.0)],
            Some(border),
            None,
        );
    }
}

pub fn group_box(canvas: &mut Canvas, rect: RECT, style: &Style, fill: Rgb) {
    canvas.round_rect(
        rect,
        radius(style, 4.0),
        Some(fill),
        Some((style.palette.border, 1.0)),
    );
}

/// Row in the filter list: enabled dot, name and a detail line.
pub fn filter_row(canvas: &mut Canvas, rect: RECT, item: &FilterItem, style: &Style, state: State) {
    let (name, detail, enabled) = (item.name.as_str(), item.detail.as_str(), item.enabled);
    let (selected, focused) = (state.selected, state.focus);
    let p = &style.palette;
    canvas.fill(rect, p.field);
    let row = inset(rect, style.ipx(4.0), style.ipx(2.0));
    if selected {
        canvas.round_rect(row, radius(style, 3.0), Some(p.selection), None);
        let bar = style.ipx(3.0);
        let height = (row.bottom - row.top) / 2;
        let top = (row.top + row.bottom - height) / 2;
        canvas.round_rect(
            RECT {
                left: row.left,
                top,
                right: row.left + bar,
                bottom: top + height,
            },
            [bar as f32 / 2.0; 4],
            Some(p.accent),
            None,
        );
    }
    if focused && !selected {
        canvas.round_rect(row, radius(style, 3.0), None, Some((p.accent, 1.0)));
    }
    let dot = (
        row.left as f32 + style.px(16.0),
        (row.top + row.bottom) as f32 / 2.0,
    );
    if enabled {
        canvas.circle(dot, style.px(4.0), Some(p.success), None);
    } else {
        canvas.circle(dot, style.px(4.0), None, Some((p.muted, 1.0)));
    }
    let text_left = row.left + style.ipx(30.0);
    let middle = (row.top + row.bottom) / 2;
    let color = if selected { p.selection_text() } else { p.text };
    canvas.text(
        RECT {
            left: text_left,
            top: row.top,
            right: row.right - style.ipx(6.0),
            bottom: middle + 1,
        },
        name,
        style.fonts.ui,
        color,
        DT_LEFT | DT_BOTTOM | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    canvas.text(
        RECT {
            left: text_left,
            top: middle + 1,
            right: row.right - style.ipx(6.0),
            bottom: row.bottom,
        },
        detail,
        style.fonts.small,
        if selected {
            color.mix(p.selection, 0.25)
        } else {
            p.muted
        },
        DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
}
