//! Page layout. Positions the native controls and builds the display list
//! that `paint` draws.
use super::*;

/// Pairs field controls with their label and unit text.
fn with_units(
    fields: &[HWND],
    specs: &[(&'static str, &'static str)],
) -> Vec<(HWND, &'static str, &'static str)> {
    fields
        .iter()
        .zip(specs)
        .map(|(hwnd, (label, unit))| (*hwnd, *label, *unit))
        .collect()
}

impl App {
    pub(super) fn layout(&mut self) {
        let client = client_rect(self.hwnd);
        if client.right <= 0 || client.bottom <= 0 {
            return;
        }
        let style = self.style();
        let fonts = style.fonts;
        let dc = unsafe { GetDC(self.hwnd) };
        let measure = |font: HFONT, text: &str| canvas::measure(dc, font, text);
        let s = |value: i32| scale(value, self.dpi);
        let mut items = Vec::new();
        let mut shown: Vec<(HWND, RECT)> = Vec::new();
        self.display_view = None;
        self.tablet_view = None;

        // Menu bar.
        let mut x = s(6);
        for (index, hwnd) in self.c.menus.iter().enumerate() {
            let width = measure(fonts.ui, &MENUS[index].replace('&', "")).0 + s(20);
            shown.push((*hwnd, rect(x, s(3), x + width, s(27))));
            x += width + s(2);
        }

        // Tabs open into the page, whose top border they overlap.
        let tabs_top = s(32);
        let page_top = tabs_top + s(28) - 1;
        let mut x = s(10);
        for (index, hwnd) in self.c.tabs.iter().enumerate() {
            let width = measure(fonts.ui, TABS[index].1).0 + s(28);
            shown.push((*hwnd, rect(x, tabs_top, x + width, page_top + 1)));
            x += width + s(2);
        }

        // Command bar, as in upstream's TabletSwitcherPanel.
        let bar_top = client.bottom - s(48);
        let button_top = bar_top + s(9);
        let mut right = client.right - s(10);
        for (hwnd, width) in [(self.c.apply, 92), (self.c.save, 92), (self.c.start, 124)] {
            shown.push((
                hwnd,
                rect(right - s(width), button_top, right, button_top + s(30)),
            ));
            right -= s(width) + s(8);
        }
        let status = rect(s(10), bar_top, right - s(4), client.bottom);
        items.push(Item::Status(status));
        self.set_tool_rect(3, status);

        let page = rect(s(8), page_top, client.right - s(8), bar_top);
        items.push(Item::Page(page));
        let content = draw::inset(page, s(12), s(12));
        match self.tab {
            Tab::Output => self.layout_output(content, &mut items, &mut shown, &measure, dc),
            Tab::Filters => self.layout_filters(content, &mut items, &mut shown, &measure, dc),
            Tab::Pen => self.layout_pen(content, &mut items, &mut shown, &measure, dc),
            Tab::Console => self.layout_console(content, &mut items, &mut shown),
        }
        unsafe { ReleaseDC(self.hwnd, dc) };
        self.set_tool_rect(1, self.display_view.map_or(RECT::default(), |v| v.rect));
        self.set_tool_rect(2, self.tablet_view.map_or(RECT::default(), |v| v.rect));

        let mut all = self.static_controls();
        all.extend(
            self.properties
                .iter()
                .flat_map(|row| [Some(row.hwnd), row.label_control, row.default_control])
                .flatten(),
        );
        unsafe {
            let mut defer = BeginDeferWindowPos(all.len() as i32);
            for hwnd in all {
                let placement = shown.iter().find(|(h, _)| *h == hwnd).map(|(_, r)| *r);
                let (r, flags) = match placement {
                    Some(r) => (r, SWP_SHOWWINDOW),
                    None => (RECT::default(), SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE),
                };
                let flags = flags | SWP_NOZORDER | SWP_NOACTIVATE;
                if defer.is_null() {
                    SetWindowPos(
                        hwnd,
                        ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        flags,
                    );
                } else {
                    defer = DeferWindowPos(
                        defer,
                        hwnd,
                        ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        flags,
                    );
                }
            }
            if !defer.is_null() {
                EndDeferWindowPos(defer);
            }
            // Keep keyboard focus on a visible control after a page change.
            let focus = GetFocus();
            if !focus.is_null() && focus != self.hwnd && IsWindowVisible(focus) == 0 {
                SetFocus(self.c.tabs[TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0)]);
            }
            InvalidateRect(self.hwnd, ptr::null(), 0);
        }
        self.items = items;
    }

    /// A horizontal group like upstream's UnitGroup: label, field and unit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn unit_row(
        &self,
        fields: &[(HWND, &str, &str)],
        row: RECT,
        field_width: i32,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let fonts = self.style().fonts;
        let widths: Vec<(i32, i32)> = fields
            .iter()
            .map(|(_, label, unit)| (measure(fonts.ui, label).0, measure(fonts.ui, unit).0))
            .collect();
        let gap = s(8);
        let natural = |field: i32| -> i32 {
            widths
                .iter()
                .map(|(label, unit)| {
                    s(10) + label + s(8) + field + if *unit > 0 { s(6) + unit } else { 0 } + s(10)
                })
                .sum::<i32>()
                + gap * (fields.len() as i32 - 1)
        };
        let available = row.right - row.left;
        let mut field = field_width;
        while field > s(44) && natural(field) > available {
            field -= s(4);
        }
        let total = natural(field);
        let mut x = row.left + ((available - total) / 2).max(0);
        for ((hwnd, label, unit), (label_width, unit_width)) in fields.iter().zip(widths) {
            let width = s(10)
                + label_width
                + s(8)
                + field
                + if unit_width > 0 { s(6) + unit_width } else { 0 }
                + s(10);
            let group = rect(x, row.top, x + width, row.bottom);
            items.push(Item::Unit(group));
            let mut cursor = x + s(10);
            self.place_label(
                *hwnd,
                rect(cursor, row.top, cursor + label_width, row.bottom),
                label,
                items,
                shown,
            );
            cursor += label_width + s(8);
            let frame = self.frame_rect(cursor, (row.top + row.bottom) / 2, field);
            self.place_field(*hwnd, frame, items, shown);
            cursor += field + s(6);
            if unit_width > 0 {
                items.push(Item::Label(
                    rect(cursor, row.top, cursor + unit_width, row.bottom),
                    (*unit).into(),
                    Tone::Text,
                    draw::TEXT_LEFT,
                ));
            }
            x += width + gap;
        }
    }

    /// Shows the label control that names `control`, inset so it does not
    /// cover the border of its group. Controls without one get painted text.
    pub(super) fn place_label(
        &self,
        control: HWND,
        area: RECT,
        text: &str,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
    ) {
        match self.labels.get(&(control as isize)) {
            Some(label) => shown.push((*label, draw::inset(area, 0, self.s(4)))),
            None => items.push(Item::Label(area, text.into(), Tone::Text, draw::TEXT_LEFT)),
        }
    }

    pub(super) fn frame_rect(&self, left: i32, middle: i32, width: i32) -> RECT {
        let height = self.s(26);
        rect(
            left,
            middle - height / 2,
            left + width,
            middle - height / 2 + height,
        )
    }

    pub(super) fn place_field(
        &self,
        hwnd: HWND,
        frame: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
    ) {
        items.push(Item::Frame(frame, hwnd));
        let text_height = canvas::measure_font_height(self.style().fonts.ui) + 2;
        let top = (frame.top + frame.bottom - text_height) / 2;
        shown.push((
            hwnd,
            rect(
                frame.left + self.s(7),
                top,
                frame.right - self.s(7),
                top + text_height,
            ),
        ));
    }

    pub(super) fn group(&self, title: &str, rect_: RECT, items: &mut Vec<Item>) -> RECT {
        let s = |value: i32| scale(value, self.dpi);
        items.push(Item::Title(
            rect(rect_.left + s(2), rect_.top, rect_.right, rect_.top + s(20)),
            title.into(),
        ));
        let body = rect(rect_.left, rect_.top + s(24), rect_.right, rect_.bottom);
        items.push(Item::Group(body));
        body
    }

    pub(super) fn layout_output(
        &mut self,
        content: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
        dc: HDC,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let mode_top = content.bottom - s(30);
        shown.push((
            self.c.mode,
            rect(
                content.left,
                mode_top,
                content.left + s(300),
                content.bottom,
            ),
        ));
        let region = rect(content.left, content.top, content.right, mode_top - s(12));
        match self.editor.mode() {
            OutputMode::Absolute => {
                let gap = s(12);
                let height = (region.bottom - region.top - gap) / 2;
                for (index, which) in [AreaKind::Display, AreaKind::Tablet]
                    .into_iter()
                    .enumerate()
                {
                    let top = region.top + index as i32 * (height + gap);
                    let title = if which == AreaKind::Display {
                        "Display"
                    } else {
                        "Tablet"
                    };
                    let body = self.group(
                        title,
                        rect(region.left, top, region.right, top + height),
                        items,
                    );
                    let inner = draw::inset(body, s(10), s(10));
                    let row = rect(inner.left, inner.bottom - s(36), inner.right, inner.bottom);
                    let canvas_rect = rect(inner.left, inner.top, inner.right, row.top - s(8));
                    items.push(Item::Area(canvas_rect, which));
                    match which {
                        AreaKind::Display => {
                            self.display_view = AreaView::new(
                                canvas_rect,
                                Bounds::from_rect(self.displays.virtual_screen),
                            );
                            let fields = with_units(&self.c.display, &DISPLAY_FIELDS);
                            self.unit_row(&fields, row, s(92), items, shown, measure);
                        }
                        AreaKind::Tablet => {
                            self.tablet_view = AreaView::new(
                                canvas_rect,
                                Bounds::tablet_for(self.editor.profile.tablet),
                            );
                            let fields = with_units(&self.c.tablet, &TABLET_FIELDS);
                            self.unit_row(&fields, row, s(92), items, shown, measure);
                        }
                    }
                }
            }
            OutputMode::Relative => {
                let note = "Relative mode moves the cursor like a mouse. Sensitivity is in pointer counts per millimetre of pen movement; Windows pointer speed and acceleration still apply. After the pen is idle for longer than the reset time, the next report starts a new movement instead of jumping.";
                let note_height = canvas::wrapped_height(
                    dc,
                    self.style().fonts.ui,
                    note,
                    region.right - region.left - s(32),
                );
                let height = s(24) + s(12) + s(36) + s(12) + note_height + s(14);
                let body = self.group(
                    "Relative",
                    rect(region.left, region.top, region.right, region.top + height),
                    items,
                );
                let inner = draw::inset(body, s(12), s(12));
                let fields = with_units(&self.c.relative, &RELATIVE_FIELDS);
                let row = rect(inner.left, inner.top, inner.right, inner.top + s(36));
                self.unit_row(&fields, row, s(92), items, shown, measure);
                items.push(Item::Label(
                    rect(
                        inner.left + s(4),
                        row.bottom + s(12),
                        inner.right - s(4),
                        row.bottom + s(12) + note_height,
                    ),
                    note.into(),
                    Tone::Muted,
                    DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                ));
            }
        }
    }

    pub(super) fn layout_filters(
        &mut self,
        content: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
        dc: HDC,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let style = self.style();
        let list_width = s(290).min((content.right - content.left) / 2);
        let buttons_top = content.bottom - s(30);
        let list_box = rect(
            content.left,
            content.top,
            content.left + list_width,
            buttons_top - s(46),
        );
        items.push(Item::Group(list_box));
        shown.push((self.c.filter_list, draw::inset(list_box, 1, s(4))));
        let width = (list_width - s(12)) / 3;
        for (index, hwnd) in [self.c.filter_up, self.c.filter_down, self.c.filter_defaults]
            .into_iter()
            .enumerate()
        {
            let left = content.left + index as i32 * (width + s(6));
            shown.push((
                hwnd,
                rect(left, buttons_top - s(36), left + width, buttons_top - s(6)),
            ));
        }
        for (index, hwnd) in [self.c.add_dotnet, self.c.add_native, self.c.remove_filter]
            .into_iter()
            .enumerate()
        {
            let left = content.left + index as i32 * (width + s(6));
            shown.push((hwnd, rect(left, buttons_top, left + width, content.bottom)));
        }

        let panel = rect(
            list_box.right + s(12),
            content.top,
            content.right,
            content.bottom,
        );
        items.push(Item::Group(panel));
        let inner = draw::inset(panel, s(16), s(14));
        let Some(target) = self.selected_target() else {
            return;
        };
        let mut y = inner.top;
        shown.push((
            self.c.filter_enable,
            rect(inner.left, y, inner.right, y + s(26)),
        ));
        y += s(30);
        let detail = match target {
            FilterRef::Radial(_) => {
                let mut text = "Built-in Rust port of AbstractQbit's RadialFollow 0.3.0. It runs in tablet coordinates before any DLL filters and loads no .NET code.".to_owned();
                if self.editor.profile.auto_enabled_radial_follow > 0 {
                    text.push_str(" OpenTabletDriver had this filter disabled; this driver enables imported Radial Follow settings automatically.");
                }
                text
            }
            FilterRef::Plugin(index) => {
                let plugin = &self.editor.profile.plugins[index];
                match plugin.kind {
                    PluginKind::Dotnet => format!(
                        ".NET filter {} from {}. Saved order applies within each pipeline stage; tablet filters run before mapping and pixel filters after it.",
                        plugin.type_name,
                        plugin.path.display()
                    ),
                    PluginKind::DotnetTool => format!(
                        ".NET tool {} from {}. Tools run beside the pen pipeline: the driver starts them with the tablet and stops them with it.",
                        plugin.type_name,
                        plugin.path.display()
                    ),
                    PluginKind::Native => format!("Native filter DLL {}", plugin.path.display()),
                }
            }
        };
        let detail_height =
            canvas::wrapped_height(dc, style.fonts.ui, &detail, inner.right - inner.left);
        items.push(Item::Label(
            rect(inner.left, y, inner.right, y + detail_height),
            detail,
            Tone::Muted,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        ));
        y += detail_height + s(14);

        // This toolbar stays outside the property rows, so switching pages never
        // destroys edits or takes space from a row's Use default button.
        let toolbar_y = y;
        if let FilterRef::Plugin(index) = target {
            let plugin = &self.editor.profile.plugins[index];
            let editable = plugin.kind.is_managed() && self.metadata_for(plugin).is_some();
            set_text(
                self.c.filter_json_toggle,
                if self.json_visible {
                    "Properties"
                } else {
                    "Edit JSON"
                },
            );
            unsafe {
                EnableWindow(self.c.filter_json_toggle, editable.into());
            }
            shown.push((
                self.c.filter_json_toggle,
                rect(inner.right - s(104), y, inner.right, y + s(28)),
            ));
        }
        y += s(36);

        let mut notes: Vec<(String, Tone)> = Vec::new();
        if let FilterRef::Plugin(_) = target {
            notes.push((
                "Plugins run with the driver's permissions. Only enable DLLs you trust.".into(),
                Tone::Muted,
            ));
        }
        if self.editor.duplicate_radial_follow() {
            notes.push(("The built-in Radial Follow and the Radial Follow DLL are both enabled, so smoothing is applied twice. Disable one of them.".into(), Tone::Warning));
        }
        notes.push((
            "Changes take effect when the driver starts or when you click Apply.".into(),
            Tone::Muted,
        ));
        let note_height: Vec<i32> = notes
            .iter()
            .map(|(text, _)| {
                canvas::wrapped_height(dc, style.fonts.ui, text, inner.right - inner.left)
            })
            .collect();
        let notes_total: i32 = note_height.iter().sum::<i32>() + s(6) * notes.len() as i32;

        let label_width = self
            .properties
            .iter()
            .map(|row| measure(style.fonts.ui, &row.label).0)
            .max()
            .unwrap_or(0)
            .min(s(180))
            .min((inner.right - inner.left - s(230)).max(s(60)));
        // Rows tighten when many properties would run into the notes.
        let room = inner.bottom - notes_total - s(12) - y;
        let page_size = (room / s(32)).max(1) as usize;
        let pages = self.properties.len().div_ceil(page_size).max(1);
        self.property_page = self.property_page.min(pages - 1);
        if pages > 1 {
            for (hwnd, left, enabled) in [
                (self.c.property_prev, inner.left, self.property_page > 0),
                (
                    self.c.property_next,
                    inner.left + s(84),
                    self.property_page + 1 < pages,
                ),
            ] {
                unsafe {
                    EnableWindow(hwnd, enabled.into());
                }
                shown.push((hwnd, rect(left, toolbar_y, left + s(78), toolbar_y + s(28))));
            }
            items.push(Item::Label(
                rect(
                    inner.left + s(168),
                    toolbar_y,
                    inner.right - s(110),
                    toolbar_y + s(28),
                ),
                format!("{} / {pages}", self.property_page + 1),
                Tone::Muted,
                draw::TEXT_LEFT,
            ));
        }
        let visible_count = self
            .properties
            .len()
            .saturating_sub(self.property_page * page_size)
            .min(page_size);
        let pitch = if visible_count == 0 {
            s(44)
        } else {
            (room / visible_count as i32).clamp(s(32), s(44))
        };
        let row_height = pitch - s(6);
        for row in self
            .properties
            .iter()
            .skip(self.property_page * page_size)
            .take(page_size)
        {
            let mut line = rect(inner.left, y, inner.right, y + row_height);
            if let Some(reset) = row.default_control {
                let reset_width = s(94);
                shown.push((
                    reset,
                    rect(line.right - reset_width, line.top, line.right, line.bottom),
                ));
                line.right -= reset_width + s(6);
            }
            items.push(Item::Unit(line));
            let is_check = with_look(|look| look.info(row.hwnd).map(|i| i.kind)).flatten()
                == Some(Kind::Check);
            if is_check {
                shown.push((
                    row.hwnd,
                    rect(
                        line.left + s(10),
                        line.top + s(3),
                        line.right - s(10),
                        line.bottom - s(3),
                    ),
                ));
            } else {
                let label = rect(
                    line.left + s(10),
                    line.top,
                    line.left + s(10) + label_width,
                    line.bottom,
                );
                match row.label_control {
                    Some(control) => shown.push((control, draw::inset(label, 0, s(4)))),
                    None => items.push(Item::Label(
                        label,
                        row.label.clone(),
                        Tone::Text,
                        draw::TEXT_LEFT,
                    )),
                }
                let unit_width = if row.unit.is_empty() {
                    0
                } else {
                    (measure(style.fonts.ui, &row.unit).0 + s(8)).min(s(72))
                };
                let field_left = label.right + s(12);
                let field_width = (line.right - s(10) - unit_width - field_left)
                    .min(s(400))
                    .max(s(60));
                let frame = self.frame_rect(field_left, (line.top + line.bottom) / 2, field_width);
                if with_look(|look| look.info(row.hwnd).map(|info| info.kind)).flatten()
                    == Some(Kind::Dropdown)
                {
                    shown.push((row.hwnd, frame));
                } else {
                    self.place_field(row.hwnd, frame, items, shown);
                }
                if !row.unit.is_empty() {
                    items.push(Item::Label(
                        rect(frame.right + s(8), line.top, line.right, line.bottom),
                        row.unit.clone(),
                        Tone::Text,
                        draw::TEXT_LEFT,
                    ));
                }
            }
            y += pitch;
        }

        if self.json_visible {
            items.push(Item::Label(
                rect(inner.left, y, inner.right, y + s(20)),
                "Settings (JSON)".into(),
                Tone::Text,
                draw::TEXT_LEFT,
            ));
            y += s(24);
            let error_height = if self.json_error.is_some() { s(22) } else { 0 };
            let bottom = (inner.bottom - notes_total - s(12) - error_height).max(y + s(80));
            let frame = rect(inner.left, y, inner.right, bottom);
            items.push(Item::Frame(frame, self.c.filter_json));
            shown.push((self.c.filter_json, draw::inset(frame, s(6), s(5))));
            y = bottom + s(4);
            if let Some(error) = &self.json_error {
                items.push(Item::Label(
                    rect(inner.left, y, inner.right, y + s(18)),
                    error.clone(),
                    Tone::Error,
                    draw::TEXT_LEFT | DT_NOPREFIX,
                ));
            }
        }

        let mut y = inner.bottom - notes_total;
        for ((text, tone), height) in notes.into_iter().zip(note_height) {
            items.push(Item::Label(
                rect(inner.left, y, inner.right, y + height),
                text,
                tone,
                DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
            ));
            y += height + s(6);
        }
    }

    pub(super) fn layout_pen(
        &mut self,
        content: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
        dc: HDC,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let style = self.style();
        let gap = s(12);
        let column = (content.right - content.left - gap) / 2;
        let height = s(24) + s(12) + s(38) * 2 + s(8) + s(12);
        for (index, eraser) in [false, true].into_iter().enumerate() {
            let left = content.left + index as i32 * (column + gap);
            let title = if eraser {
                "Eraser Settings"
            } else {
                "Tip Settings"
            };
            let body = self.group(
                title,
                rect(left, content.top, left + column, content.top + height),
                items,
            );
            let inner = draw::inset(body, s(12), s(12));
            let (binding, slider, field) = if eraser {
                (
                    self.c.eraser_binding,
                    self.c.eraser_slider,
                    self.c.eraser_field,
                )
            } else {
                (self.c.tip_binding, self.c.tip_slider, self.c.tip_field)
            };
            let labels = if eraser {
                ["Eraser Binding", "Eraser Threshold"]
            } else {
                ["Tip Binding", "Tip Threshold"]
            };
            let label_width = labels
                .iter()
                .map(|l| measure(style.fonts.ui, l).0)
                .max()
                .unwrap_or(0);
            let first = rect(inner.left, inner.top, inner.right, inner.top + s(38));
            items.push(Item::Unit(first));
            self.place_label(
                binding,
                rect(
                    first.left + s(10),
                    first.top,
                    first.left + s(10) + label_width,
                    first.bottom,
                ),
                labels[0],
                items,
                shown,
            );
            let control_left = first.left + s(10) + label_width + s(12);
            let middle = (first.top + first.bottom) / 2;
            shown.push((
                binding,
                rect(
                    control_left,
                    middle - s(14),
                    (control_left + s(180)).min(first.right - s(10)),
                    middle + s(14),
                ),
            ));

            let second = rect(
                inner.left,
                first.bottom + s(8),
                inner.right,
                first.bottom + s(8) + s(38),
            );
            items.push(Item::Unit(second));
            self.place_label(
                field,
                rect(
                    second.left + s(10),
                    second.top,
                    second.left + s(10) + label_width,
                    second.bottom,
                ),
                labels[1],
                items,
                shown,
            );
            let percent_width = measure(style.fonts.ui, "%").0;
            let field_right = second.right - s(10) - percent_width - s(6);
            let field_left = field_right - s(84);
            let middle = (second.top + second.bottom) / 2;
            shown.push((
                slider,
                rect(
                    control_left - s(4),
                    middle - s(13),
                    field_left - s(8),
                    middle + s(13),
                ),
            ));
            let frame = self.frame_rect(field_left, middle, s(84));
            self.place_field(field, frame, items, shown);
            items.push(Item::Label(
                rect(frame.right + s(6), second.top, second.right, second.bottom),
                "%".into(),
                Tone::Text,
                draw::TEXT_LEFT,
            ));
        }

        let top = content.top + height + gap;
        let text = match self.editor.profile.output {
            crate::config::OutputKind::Pen => "Pen output carries pressure, tilt and eraser state through Windows Ink. Pen button bindings are not implemented; the side buttons have no assigned actions.",
            crate::config::OutputKind::Mouse => "Mouse output moves the cursor and maps tip or eraser contact to the left mouse button. Pen button bindings are not implemented; the side buttons have no assigned actions.",
        };
        let text_height = canvas::wrapped_height(
            dc,
            style.fonts.ui,
            text,
            content.right - content.left - s(24),
        );
        let body = self.group(
            "Pen Buttons",
            rect(
                content.left,
                top,
                content.right,
                top + s(24) + text_height + s(26),
            ),
            items,
        );
        items.push(Item::Label(
            draw::inset(body, s(12), s(12)),
            text.into(),
            Tone::Muted,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        ));
    }

    pub(super) fn layout_console(
        &mut self,
        content: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let buttons_top = content.bottom - s(30);
        let body = rect(
            content.left,
            content.top,
            content.right,
            buttons_top - s(10),
        );
        items.push(Item::Group(body));
        let header = rect(
            body.left + 1,
            body.top + 1,
            body.right - 1,
            body.top + s(28),
        );
        items.push(Item::Header(header));
        shown.push((
            self.c.log,
            rect(
                body.left + 1,
                header.bottom,
                body.right - 1,
                body.bottom - 1,
            ),
        ));
        let columns = [s(84), s(84) + s(76), s(84) + s(76) + s(86)];
        update_look(|look| look.log_columns = columns);
        shown.push((
            self.c.copy_log,
            rect(
                content.left,
                buttons_top,
                content.left + s(100),
                content.bottom,
            ),
        ));
        shown.push((
            self.c.clear_log,
            rect(
                content.left + s(108),
                buttons_top,
                content.left + s(208),
                content.bottom,
            ),
        ));
    }
}
