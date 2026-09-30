//! Painting of the main window and of the custom-drawn child controls.
use super::*;

impl App {
    pub(super) fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.hwnd, &mut ps) };
        let client = client_rect(self.hwnd);
        if let Some(mut canvas) = canvas::Canvas::new(hdc, client) {
            self.paint_items(&mut canvas, client);
            canvas.present(hdc);
        }
        unsafe { EndPaint(self.hwnd, &ps) };
    }

    pub(super) fn paint_items(&self, canvas: &mut canvas::Canvas, client: RECT) {
        let style = self.style();
        let p = style.palette;
        let fonts = style.fonts;
        let s = |value: i32| scale(value, self.dpi);
        canvas.fill(client, p.window);
        let focus = unsafe { GetFocus() };
        let mapping = self.editor.absolute(&self.displays);
        for item in &self.items {
            match item {
                Item::Page(r) => {
                    canvas.fill(*r, p.page);
                    canvas.round_rect(*r, [0.0; 4], None, Some((p.tab_border, 1.0)));
                }
                Item::Title(r, text) => {
                    canvas.text(*r, text, fonts.bold, p.text, draw::TEXT_LEFT | DT_NOPREFIX)
                }
                Item::Group(r) => draw::group_box(canvas, *r, &style, p.group),
                Item::Unit(r) => {
                    canvas.round_rect(*r, [style.px(4.0); 4], Some(p.group), Some((p.border, 1.0)))
                }
                Item::Label(r, text, tone, format) => {
                    let color = match tone {
                        Tone::Text => p.text,
                        Tone::Muted => p.muted,
                        Tone::Warning => p.warning,
                    };
                    canvas.text(*r, text, fonts.ui, color, *format | DT_NOPREFIX);
                }
                Item::Frame(r, hwnd) => {
                    let enabled = unsafe { IsWindowEnabled(*hwnd) } != 0;
                    draw::field_frame(
                        canvas,
                        *r,
                        &style,
                        *hwnd == focus,
                        self.invalid.contains(&(*hwnd as isize)),
                        !enabled,
                    );
                }
                Item::Area(r, which) => {
                    let (view, backgrounds, unit, error) = match which {
                        AreaKind::Display => (
                            self.display_view.as_ref(),
                            self.displays
                                .monitors
                                .iter()
                                .map(|m| Bounds::from_rect(*m))
                                .collect::<Vec<_>>(),
                            "px",
                            "Invalid display area.",
                        ),
                        AreaKind::Tablet => (
                            self.tablet_view.as_ref(),
                            vec![Bounds::tablet_for(self.editor.profile.tablet)],
                            "mm",
                            "Invalid tablet area.",
                        ),
                    };
                    let fonts = area::AreaFonts {
                        small: fonts.small,
                        small_pixels: self.fonts.small_pixels,
                    };
                    let scene = area::AreaScene {
                        view,
                        rect: *r,
                        backgrounds: &backgrounds,
                        area: which.area(&mapping),
                        unit,
                        invalid_text: error,
                    };
                    area::paint(canvas, &scene, &fonts, &p);
                }
                Item::Status(r) => self.paint_status(canvas, *r, &style),
                Item::Header(r) => {
                    canvas.fill(*r, p.window.mix(p.group, 0.5));
                    canvas.fill(rect(r.left, r.bottom - 1, r.right, r.bottom), p.border);
                    let columns = with_look(|look| look.log_columns).unwrap_or_default();
                    let titles = ["Time", "Level", "Group", "Message"];
                    let starts = [0, columns[0], columns[1], columns[2]];
                    for (index, title) in titles.iter().enumerate() {
                        let left = r.left + s(10) + starts[index];
                        let right = if index < 3 {
                            r.left + s(10) + starts[index + 1] - s(8)
                        } else {
                            r.right
                        };
                        canvas.text(
                            rect(left, r.top, right, r.bottom),
                            title,
                            fonts.ui,
                            p.muted,
                            draw::TEXT_LEFT | DT_NOPREFIX,
                        );
                    }
                }
            }
        }
    }

    pub(super) fn paint_status(&self, canvas: &mut canvas::Canvas, r: RECT, style: &Style) {
        let p = style.palette;
        let s = |value: i32| scale(value, self.dpi);
        let middle = (r.top + r.bottom) as f32 / 2.0;
        let (color, filled) = match self.driver {
            DriverState::Connected => (p.success, true),
            DriverState::Starting
            | DriverState::Waiting
            | DriverState::Connecting
            | DriverState::Stopping => (p.warning, true),
            DriverState::Failed => (p.error, true),
            DriverState::Disconnected => (p.warning, true),
            DriverState::Stopped => (p.muted, self.tablet_present == Some(true)),
        };
        let dot = (r.left as f32 + style.px(6.0), middle);
        if filled {
            canvas.circle(dot, style.px(4.5), Some(color), None);
        } else {
            canvas.circle(dot, style.px(4.5), None, Some((color, 1.0)));
        }
        let left = r.left + s(18);
        let name = self.tablet_label();
        let (name_width, _) = canvas.measure(style.fonts.bold, &name);
        canvas.text(
            rect(left, r.top, left + name_width + 1, r.bottom),
            &name,
            style.fonts.bold,
            p.text,
            draw::TEXT_LEFT | DT_NOPREFIX,
        );
        let state = self.driver.label();
        let left = left + name_width + s(10);
        let (state_width, _) = canvas.measure(style.fonts.ui, state);
        canvas.text(
            rect(left, r.top, left + state_width + 1, r.bottom),
            state,
            style.fonts.ui,
            p.text,
            draw::TEXT_LEFT | DT_NOPREFIX,
        );
        let left = left + state_width + s(12);
        if left < r.right && !self.status.is_empty() {
            canvas.fill(
                rect(left - s(7), r.top + s(16), left - s(6), r.bottom - s(16)),
                p.border,
            );
            let tone = match self.status_level {
                Level::Info => p.muted,
                Level::Warning => p.warning,
                Level::Error => p.error,
            };
            let first_line = self.status.lines().next().unwrap_or_default();
            canvas.text(
                rect(left, r.top, r.right, r.bottom),
                first_line,
                style.fonts.ui,
                tone,
                draw::TEXT_LEFT | DT_NOPREFIX,
            );
        }
    }
}

pub(super) fn custom_draw(draw: &mut NMCUSTOMDRAW) -> LRESULT {
    let hwnd = draw.hdr.hwndFrom;
    with_look(|look| {
        let info = look.info(hwnd)?;
        if draw.dwDrawStage != CDDS_PREPAINT {
            return Some(CDRF_DODEFAULT as LRESULT);
        }
        let style = look.style;
        let surface = look.surface(info.surface);
        let flags = draw.uItemState;
        let mut state = State {
            hot: flags & CDIS_HOT != 0,
            pressed: flags & CDIS_SELECTED != 0,
            focus: flags & CDIS_FOCUS != 0,
            disabled: flags & CDIS_DISABLED != 0,
            cues: flags & CDIS_SHOWKEYBOARDCUES != 0,
            ..State::default()
        };
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client) };
        let mut canvas = canvas::Canvas::new(draw.hdc, client)?;
        match info.kind {
            Kind::Slider => {
                let mut channel = RECT::default();
                let mut thumb = RECT::default();
                unsafe {
                    SendMessageW(
                        hwnd,
                        TBM_GETCHANNELRECT,
                        0,
                        &mut channel as *mut RECT as isize,
                    );
                    SendMessageW(hwnd, TBM_GETTHUMBRECT, 0, &mut thumb as *mut RECT as isize);
                }
                state.disabled = unsafe { IsWindowEnabled(hwnd) } == 0;
                state.focus = unsafe { GetFocus() } == hwnd;
                canvas.fill(client, surface);
                let thumb_x = (thumb.left + thumb.right) / 2;
                let channel = RECT {
                    left: channel.left + style.ipx(2.0),
                    right: channel.right - style.ipx(2.0),
                    ..channel
                };
                draw::slider(&mut canvas, channel, thumb_x, &style, state.disabled);
                let size = style.ipx(20.0);
                let middle = (channel.top + channel.bottom) / 2;
                let knob = rect(
                    thumb_x - size / 2,
                    middle - size / 2,
                    thumb_x - size / 2 + size,
                    middle - size / 2 + size,
                );
                draw::slider_thumb(&mut canvas, knob, &style, surface, state);
            }
            kind => {
                let label = text(hwnd);
                match kind {
                    Kind::Menu => {
                        state.selected = look.menu_open == hwnd as isize;
                        draw::flat_button(&mut canvas, client, &label, &style, surface, state);
                    }
                    Kind::Tab(tab) => {
                        state.selected = look.tab == tab;
                        draw::tab(&mut canvas, client, &label, &style, state);
                    }
                    Kind::Dropdown => {
                        state.selected = look.menu_open == hwnd as isize;
                        draw::dropdown(&mut canvas, client, &label, &style, surface, state);
                    }
                    Kind::Check => {
                        state.checked = unsafe { SendMessageW(hwnd, BM_GETCHECK, 0, 0) }
                            == BST_CHECKED as isize;
                        draw::checkbox(&mut canvas, client, &label, &style, surface, state);
                    }
                    Kind::Button => draw::button(
                        &mut canvas,
                        client,
                        &label,
                        &style,
                        surface,
                        state,
                    ),
                    _ => return Some(CDRF_DODEFAULT as LRESULT),
                }
            }
        }
        canvas.present(draw.hdc);
        Some(CDRF_SKIPDEFAULT as LRESULT)
    })
    .flatten()
    .unwrap_or(CDRF_DODEFAULT as LRESULT)
}

pub(super) fn draw_item(item: &DRAWITEMSTRUCT) {
    if item.CtlType != ODT_LISTBOX {
        return;
    }
    with_look(|look| {
        let Some(info) = look.info(item.hwndItem) else {
            return;
        };
        let style = look.style;
        let p = style.palette;
        let Some(mut canvas) = canvas::Canvas::new(item.hDC, item.rcItem) else {
            return;
        };
        let r = item.rcItem;
        let selected = item.itemState & ODS_SELECTED != 0;
        let focused = item.itemState & ODS_FOCUS != 0;
        let index = item.itemID as usize;
        match info.kind {
            Kind::List => match look.filters.get(index) {
                Some(filter) => {
                    let state = State {
                        selected,
                        focus: focused,
                        ..State::default()
                    };
                    draw::filter_row(&mut canvas, r, filter, &style, state);
                }
                None => canvas.fill(r, p.field),
            },
            Kind::Log => {
                canvas.fill(r, if selected { p.selection } else { p.field });
                if let Some(entry) = look.log.get(index) {
                    let base = r.left + style.ipx(10.0);
                    let columns = look.log_columns;
                    let text_color = if selected { p.selection_text() } else { p.text };
                    let level_color = match entry.level {
                        Level::Info => text_color,
                        Level::Warning => p.warning,
                        Level::Error => p.error,
                    };
                    let cells = [
                        (0, columns[0], entry.time.as_str(), p.muted),
                        (columns[0], columns[1], entry.level.label(), level_color),
                        (columns[1], columns[2], entry.group, text_color),
                    ];
                    for (start, end, value, color) in cells {
                        canvas.text(
                            rect(base + start, r.top, base + end - style.ipx(8.0), r.bottom),
                            value,
                            style.fonts.ui,
                            color,
                            draw::TEXT_LEFT | DT_NOPREFIX,
                        );
                    }
                    let message = entry.message.replace(['\r', '\n'], " ");
                    canvas.text(
                        rect(base + columns[2], r.top, r.right - style.ipx(6.0), r.bottom),
                        &message,
                        style.fonts.ui,
                        text_color,
                        draw::TEXT_LEFT | DT_NOPREFIX,
                    );
                }
                if focused {
                    canvas.round_rect(r, [0.0; 4], None, Some((p.accent.mix(p.field, 0.4), 1.0)));
                }
            }
            _ => {}
        }
        canvas.present(item.hDC);
    });
}

pub(super) fn ctl_color(dc: HDC, control: HWND) -> Option<LRESULT> {
    with_look(|look| {
        let p = look.style.palette;
        let enabled = unsafe { IsWindowEnabled(control) } != 0;
        let (background, foreground) = match look.info(control) {
            Some(ControlInfo {
                kind: Kind::Slider | Kind::Label,
                surface,
            }) => (look.surface(surface), p.text),
            Some(ControlInfo {
                kind: Kind::Field | Kind::Memo | Kind::List | Kind::Log,
                ..
            }) => (p.field, if enabled { p.text } else { p.disabled }),
            _ => (p.page, p.text),
        };
        unsafe {
            SetTextColor(dc, foreground.colorref());
            SetBkColor(dc, background.colorref());
        }
        look.brush(background) as LRESULT
    })
}
