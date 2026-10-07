//! Pen button, express key and wheel binding editors, like upstream's
//! `PenBindingEditor`, `AuxiliaryBindingEditor` and `WheelBindingEditor`.
//! Each binding is a dropdown with None, the pen's barrel buttons (pen
//! buttons only), mouse buttons and a key or shortcut captured in a dialog.
//! The rows follow the detected tablet's declared buttons and wheels; with
//! no tablet detected they show the profile's saved bindings.
use super::*;
use otd_core::actions::MouseButton;
use otd_core::output::buttons::{ButtonAction, ScrollAxis, WheelBinding};

pub(super) const ID_BINDING: u16 = 7000;
pub(super) const MAX_BINDING_ROWS: u16 = 256;
pub(super) const ID_WHEEL_THRESHOLD: u16 = 7400;
const MAX_WHEEL_FIELDS: u16 = 2 * otd_core::reports::MAX_WHEELS as u16;

const CHOICE_NONE: u16 = 1;
const CHOICE_BARREL: u16 = 10;
const CHOICE_MOUSE: u16 = 20;
const CHOICE_KEYS: u16 = 30;
const CHOICE_SCROLL: u16 = 40;
const SCROLL_CHOICES: [(&str, &str); 4] = [
    ("scroll:up", "Scroll Up"), ("scroll:down", "Scroll Down"),
    ("scroll:left", "Scroll Left"), ("scroll:right", "Scroll Right"),
];
const MOUSE_CHOICES: [(MouseButton, &str); 5] = [
    (MouseButton::Left, "Left Click"),
    (MouseButton::Right, "Right Click"),
    (MouseButton::Middle, "Middle Click"),
    (MouseButton::Backward, "Back"),
    (MouseButton::Forward, "Forward"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BindingTarget {
    Pen(usize),
    Aux(usize),
    Mouse(usize),
    MouseScrollUp,
    MouseScrollDown,
    Clockwise(usize),
    CounterClockwise(usize),
    WheelButton(usize, usize),
}

impl BindingTarget {
    /// The row label, as upstream's binding lists name them.
    fn label(self) -> String {
        match self {
            Self::Pen(index) => format!("Pen Binding {}", index + 1),
            Self::Aux(index) => format!("Express Key {}", index + 1),
            Self::Mouse(0) => "Primary Binding".into(),
            Self::Mouse(1) => "Alternate Binding".into(),
            Self::Mouse(2) => "Middle Binding".into(),
            Self::Mouse(index) => format!("Mouse Binding {}", index + 1),
            Self::MouseScrollUp => "Scroll Up".into(),
            Self::MouseScrollDown => "Scroll Down".into(),
            Self::Clockwise(_) => "Clockwise".into(),
            Self::CounterClockwise(_) => "Counter-Clockwise".into(),
            Self::WheelButton(_, index) => format!("Wheel Button {}", index + 1),
        }
    }

    /// What the shortcut dialog says it binds.
    fn describe(self) -> String {
        match self {
            Self::Pen(index) => format!("pen button {}", index + 1),
            Self::Aux(index) => format!("express key {}", index + 1),
            Self::Mouse(index) => format!("mouse button {}", index + 1),
            Self::MouseScrollUp => "mouse scroll up".into(),
            Self::MouseScrollDown => "mouse scroll down".into(),
            Self::Clockwise(wheel) => format!("wheel {} clockwise rotation", wheel + 1),
            Self::CounterClockwise(wheel) => {
                format!("wheel {} counter-clockwise rotation", wheel + 1)
            }
            Self::WheelButton(wheel, index) => format!("wheel {} button {}", wheel + 1, index + 1),
        }
    }
}

pub(super) struct BindingRow {
    pub(super) target: BindingTarget,
    pub(super) hwnd: HWND,
    pub(super) label: HWND,
}

pub(super) struct WheelField {
    pub(super) wheel: usize,
    pub(super) counter_clockwise: bool,
    pub(super) hwnd: HWND,
}

/// The dropdown text of an action.
pub(super) fn action_text(action: &ButtonAction) -> String {
    match action {
        ButtonAction::None => "None".into(),
        ButtonAction::Barrel(number) => format!("Pen Button {number}"),
        ButtonAction::Mouse(button) => MOUSE_CHOICES
            .iter()
            .find(|(candidate, _)| candidate == button)
            .map_or("Mouse", |(_, label)| label)
            .into(),
        ButtonAction::Keys(keys) => otd_core::keys::chord_text(keys),
        ButtonAction::Scroll(scroll) => {
            SCROLL_CHOICES.iter().find(|(value, _)| value.parse::<ButtonAction>().ok().as_ref() == Some(action))
                .map_or_else(|| format!("{} scroll: {} units, {} ms",
                    match scroll.axis { ScrollAxis::Vertical => "Vertical", ScrollAxis::Horizontal => "Horizontal" },
                    scroll.amount, scroll.interval_ms), |(_, label)| (*label).into())
        },
    }
}

/// The rows and wheel fields to show, in Tab order.
struct Plan {
    rows: Vec<BindingTarget>,
    fields: Vec<(usize, bool)>,
    detected: bool,
}

impl App {
    fn binding_plan(&self) -> Plan {
        let profile = &self.editor.profile;
        let detected = self.editor.detected_controls(&self.connected_tablets);
        let pen = detected.map_or(profile.pen_buttons.len(), |c| usize::from(c.pen_buttons));
        let aux = detected.map_or_else(
            || {
                profile
                    .aux_buttons
                    .iter()
                    .rposition(|action| *action != ButtonAction::None)
                    .map_or(0, |last| last + 1)
            },
            |c| usize::from(c.aux_buttons),
        );
        let wheels: Vec<usize> = match &detected {
            Some(controls) => controls
                .wheels()
                .iter()
                .map(|wheel| usize::from(wheel.buttons))
                .collect(),
            None => profile.wheels.iter().map(|wheel| wheel.buttons.len()).collect(),
        };
        let mut rows: Vec<BindingTarget> = (0..pen).map(BindingTarget::Pen).collect();
        rows.extend((0..aux).map(BindingTarget::Aux));
        let mouse = detected.map_or(profile.mouse_buttons.len(), |c| usize::from(c.mouse_buttons));
        rows.extend((0..mouse).map(BindingTarget::Mouse));
        rows.extend([BindingTarget::MouseScrollUp, BindingTarget::MouseScrollDown]);
        let mut fields = Vec::new();
        for (wheel, buttons) in wheels.iter().enumerate() {
            rows.push(BindingTarget::Clockwise(wheel));
            rows.push(BindingTarget::CounterClockwise(wheel));
            rows.extend((0..*buttons).map(|button| BindingTarget::WheelButton(wheel, button)));
            fields.push((wheel, false));
            fields.push((wheel, true));
        }
        rows.truncate(MAX_BINDING_ROWS as usize);
        fields.truncate(MAX_WHEEL_FIELDS as usize);
        Plan {
            rows,
            fields,
            detected: detected.is_some(),
        }
    }

    pub(super) fn binding_action(&self, target: BindingTarget) -> ButtonAction {
        let profile = &self.editor.profile;
        let wheel = |index: usize| profile.wheels.get(index);
        match target {
            BindingTarget::Pen(index) => profile.pen_buttons.get(index).cloned(),
            BindingTarget::Aux(index) => profile.aux_buttons.get(index).cloned(),
            BindingTarget::Mouse(index) => profile.mouse_buttons.get(index).cloned(),
            BindingTarget::MouseScrollUp => Some(profile.mouse_scroll_up.clone()),
            BindingTarget::MouseScrollDown => Some(profile.mouse_scroll_down.clone()),
            BindingTarget::Clockwise(index) => wheel(index).map(|wheel| wheel.clockwise.clone()),
            BindingTarget::CounterClockwise(index) => {
                wheel(index).map(|wheel| wheel.counter_clockwise.clone())
            }
            BindingTarget::WheelButton(index, button) => {
                wheel(index).and_then(|wheel| wheel.buttons.get(button).cloned())
            }
        }
        .unwrap_or(ButtonAction::None)
    }

    fn wheel_mut(&mut self, index: usize) -> &mut WheelBinding {
        let wheels = &mut self.editor.profile.wheels;
        if wheels.len() <= index {
            wheels.resize(index + 1, WheelBinding::default());
        }
        &mut wheels[index]
    }

    pub(super) fn set_binding_action(&mut self, target: BindingTarget, action: ButtonAction) {
        if self.binding_action(target) == action {
            return;
        }
        fn put(list: &mut Vec<ButtonAction>, index: usize, action: ButtonAction) {
            if list.len() <= index {
                list.resize(index + 1, ButtonAction::None);
            }
            list[index] = action;
        }
        match target {
            BindingTarget::Pen(index) => put(&mut self.editor.profile.pen_buttons, index, action),
            BindingTarget::Aux(index) => put(&mut self.editor.profile.aux_buttons, index, action),
            BindingTarget::Mouse(index) => put(&mut self.editor.profile.mouse_buttons, index, action),
            BindingTarget::MouseScrollUp => self.editor.profile.mouse_scroll_up = action,
            BindingTarget::MouseScrollDown => self.editor.profile.mouse_scroll_down = action,
            BindingTarget::Clockwise(index) => self.wheel_mut(index).clockwise = action,
            BindingTarget::CounterClockwise(index) => {
                self.wheel_mut(index).counter_clockwise = action;
            }
            BindingTarget::WheelButton(index, button) => {
                put(&mut self.wheel_mut(index).buttons, button, action);
            }
        }
        self.mark_dirty();
        self.sync_bindings();
    }

    /// The target of a binding dropdown and whether it offers barrel buttons.
    pub(super) fn binding_row(&self, hwnd: HWND) -> Option<(BindingTarget, ButtonAction)> {
        let row = self.binding_rows.iter().find(|row| row.hwnd == hwnd)?;
        Some((row.target, self.binding_action(row.target)))
    }

    fn destroy_binding_controls(&mut self) {
        let rows = std::mem::take(&mut self.binding_rows);
        let fields = std::mem::take(&mut self.wheel_fields);
        let controls = rows
            .iter()
            .flat_map(|row| [row.hwnd, row.label])
            .chain(fields.iter().flat_map(|field| {
                [Some(field.hwnd), self.labels.get(&(field.hwnd as isize)).copied()]
                    .into_iter()
                    .flatten()
            }))
            .collect::<Vec<_>>();
        for hwnd in controls {
            self.remove_tool(hwnd);
            self.invalid.remove(&(hwnd as isize));
            self.labels.remove(&(hwnd as isize));
            update_look(|look| {
                look.controls.remove(&(hwnd as isize));
            });
            unsafe { DestroyWindow(hwnd) };
        }
    }

    /// Creates the rows the tablet needs and shows the profile's bindings.
    /// Existing controls are kept while the rows stay the same, so editing a
    /// field does not lose focus.
    pub(super) fn sync_bindings(&mut self) {
        let plan = self.binding_plan();
        let same = plan.rows.len() == self.binding_rows.len()
            && plan.rows.iter().zip(&self.binding_rows).all(|(a, b)| *a == b.target)
            && plan.fields.len() == self.wheel_fields.len()
            && plan
                .fields
                .iter()
                .zip(&self.wheel_fields)
                .all(|((wheel, ccw), field)| field.wheel == *wheel && field.counter_clockwise == *ccw);
        self.bindings_detected = plan.detected;
        if !same {
            self.destroy_binding_controls();
            // Pen rows follow the eraser threshold in the Tab order; the
            // auxiliary page's controls follow them.
            let mut after = self.c.eraser_field;
            let place = |hwnd: HWND, after: &mut HWND| {
                unsafe {
                    SetWindowPos(hwnd, *after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
                *after = hwnd;
            };
            let mut fields = plan.fields.iter().enumerate().peekable();
            for (index, target) in plan.rows.iter().enumerate() {
                let Ok(label) = self.label(&target.label()) else {
                    continue;
                };
                let Ok(hwnd) = self.button("None", ID_BINDING + index as u16, Kind::Dropdown, Surface::Group)
                else {
                    unsafe { DestroyWindow(label) };
                    continue;
                };
                self.labels.insert(hwnd as isize, label);
                place(label, &mut after);
                place(hwnd, &mut after);
                self.binding_rows.push(BindingRow {
                    target: *target,
                    hwnd,
                    label,
                });
                // A wheel's threshold fields follow its rotation rows.
                if let BindingTarget::CounterClockwise(wheel) = target {
                    while let Some((field_index, (field_wheel, ccw))) =
                        fields.next_if(|(_, (field_wheel, _))| field_wheel == wheel)
                    {
                        let text = if *ccw {
                            "Counter-Clockwise Threshold"
                        } else {
                            "Clockwise Threshold"
                        };
                        let Ok(field) = self.labeled_field(ID_WHEEL_THRESHOLD + field_index as u16, text)
                        else {
                            continue;
                        };
                        if let Some(label) = self.labels.get(&(field as isize)).copied() {
                            place(label, &mut after);
                        }
                        place(field, &mut after);
                        self.add_tool(field, 0, "The rotation in degrees that activates the binding once. Empty means one step of the wheel.");
                        self.wheel_fields.push(WheelField {
                            wheel: *field_wheel,
                            counter_clockwise: *ccw,
                            hwnd: field,
                        });
                    }
                }
            }
            if matches!(self.tab, Tab::Pen | Tab::Aux | Tab::Mouse) {
                self.layout();
            }
        }
        for row in &self.binding_rows {
            set_text(row.hwnd, &action_text(&self.binding_action(row.target)));
        }
        self.sync_wheel_fields(None);
    }

    /// One wheel step in degrees, when the tablet declares it.
    fn wheel_step(&self, wheel: usize) -> Option<f64> {
        self.editor
            .detected_controls(&self.connected_tablets)
            .or_else(|| Some(self.editor.profile.tablet.controls))
            .and_then(|controls| controls.wheels().get(wheel).and_then(|w| w.degrees_per_step()))
    }

    pub(super) fn sync_wheel_fields(&mut self, skip: Option<HWND>) {
        for index in 0..self.wheel_fields.len() {
            let (wheel, ccw, hwnd) = {
                let field = &self.wheel_fields[index];
                (field.wheel, field.counter_clockwise, field.hwnd)
            };
            let cue = match self.wheel_step(wheel) {
                Some(step) => format!("{} (one step)", model::format_number(step, 2)),
                None => "One step".into(),
            };
            unsafe {
                SendMessageW(hwnd, EM_SETCUEBANNER, 1, wide(&cue).as_ptr() as isize);
            }
            if Some(hwnd) == skip {
                continue;
            }
            let value = self.editor.profile.wheels.get(wheel).and_then(|binding| {
                if ccw {
                    binding.counter_clockwise_threshold
                } else {
                    binding.clockwise_threshold
                }
            });
            set_text(
                hwnd,
                &value.map(|value| model::format_number(f64::from(value), 2)).unwrap_or_default(),
            );
            self.set_invalid(hwnd, false, None);
        }
    }

    /// Returns whether `hwnd` is a wheel threshold field and handled it.
    pub(super) fn edit_wheel_threshold(&mut self, hwnd: HWND, value: &str) -> bool {
        let Some(field) = self.wheel_fields.iter().find(|field| field.hwnd == hwnd) else {
            return false;
        };
        let (wheel, ccw) = (field.wheel, field.counter_clockwise);
        let degrees = if value.trim().is_empty() {
            None
        } else {
            match model::parse_number(value).filter(|degrees| *degrees > 0.0 && *degrees <= 3600.0) {
                Some(degrees) => Some(degrees as f32),
                None => {
                    self.set_invalid(hwnd, true, Some("Enter a positive number of degrees, or leave it empty for one wheel step."));
                    return true;
                }
            }
        };
        let binding = self.wheel_mut(wheel);
        let slot = if ccw {
            &mut binding.counter_clockwise_threshold
        } else {
            &mut binding.clockwise_threshold
        };
        if *slot != degrees {
            *slot = degrees;
            self.mark_dirty();
        }
        self.set_invalid(hwnd, false, None);
        true
    }

    pub(super) fn is_wheel_field(&self, hwnd: HWND) -> bool {
        self.wheel_fields.iter().any(|field| field.hwnd == hwnd)
    }

    pub(super) fn binding_controls(&self) -> Vec<HWND> {
        self.binding_rows
            .iter()
            .flat_map(|row| [row.hwnd, row.label])
            .chain(self.wheel_fields.iter().map(|field| field.hwnd))
            .collect()
    }

    /// Lays out binding rows in as many columns as fit, from `top`. Returns
    /// the bottom of the last row.
    #[allow(clippy::too_many_arguments)]
    fn layout_binding_grid(
        &self,
        targets: &[BindingTarget],
        area: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
    ) -> i32 {
        let s = |value: i32| scale(value, self.dpi);
        let font = self.style().fonts.ui;
        let rows: Vec<&BindingRow> = targets
            .iter()
            .filter_map(|target| self.binding_rows.iter().find(|row| row.target == *target))
            .collect();
        if rows.is_empty() {
            return area.top;
        }
        let label_width = rows
            .iter()
            .map(|row| measure(font, &row.target.label()).0)
            .max()
            .unwrap_or(0);
        let gap = s(8);
        let width = area.right - area.left;
        // The same columns for every group, so rows line up between groups.
        let columns = self.binding_grid_columns(targets, area, measure) as i32;
        let cell = (width - gap * (columns - 1)) / columns;
        let height = s(38);
        let mut bottom = area.top;
        for (index, row) in rows.iter().enumerate() {
            let column = index as i32 % columns;
            let line = index as i32 / columns;
            let left = area.left + column * (cell + gap);
            let top = area.top + line * (height + gap);
            let unit = rect(left, top, left + cell, top + height);
            bottom = unit.bottom;
            if unit.bottom > area.bottom {
                continue;
            }
            items.push(Item::Unit(unit));
            self.place_label(
                row.hwnd,
                rect(unit.left + s(10), unit.top, unit.left + s(10) + label_width, unit.bottom),
                &row.target.label(),
                items,
                shown,
            );
            let control_left = unit.left + s(10) + label_width + s(12);
            let middle = (unit.top + unit.bottom) / 2;
            shown.push((
                row.hwnd,
                rect(control_left, middle - s(14), unit.right - s(10), middle + s(14)),
            ));
        }
        bottom
    }

    fn no_rows_text(&self, what: &str) -> String {
        if self.bindings_detected {
            format!("The detected tablet declares no {what}.")
        } else {
            format!("No tablet detected. Connect the tablet to list its {what}; saved bindings are kept.")
        }
    }

    /// The Pen Buttons group under the tip and eraser settings.
    pub(super) fn layout_pen_buttons(
        &self,
        area: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
        dc: HDC,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let targets: Vec<BindingTarget> = self
            .binding_rows
            .iter()
            .map(|row| row.target)
            .filter(|target| matches!(target, BindingTarget::Pen(_)))
            .collect();
        let note = match self.editor.profile.output {
            crate::config::OutputKind::Pen => "With Windows Ink output, Pen Button bindings press the pen's barrel button.",
            crate::config::OutputKind::Mouse => "With mouse output, Pen Button 1 clicks right and Pen Button 2 clicks middle.",
        };
        let empty = targets.is_empty().then(|| self.no_rows_text("pen buttons"));
        let text = empty.as_deref().unwrap_or(note);
        let inner_width = area.right - area.left - s(24);
        let text_height = canvas::wrapped_height(dc, self.style().fonts.ui, text, inner_width);
        let gap = s(8);
        let label_width = targets
            .iter()
            .map(|target| measure(self.style().fonts.ui, &target.label()).0)
            .max()
            .unwrap_or(0);
        let columns = ((inner_width + gap) / (label_width + s(200) + gap)).max(1) as usize;
        let lines = targets.len().div_ceil(columns) as i32;
        let grid = if lines > 0 { lines * (s(38) + gap) } else { 0 };
        let bottom = (area.top + s(24) + s(12) + grid + text_height + s(14)).min(area.bottom);
        let body = self.group("Pen Buttons", rect(area.left, area.top, area.right, bottom), items);
        let inner = draw::inset(body, s(12), s(12));
        let grid_bottom = self.layout_binding_grid(&targets, inner, items, shown, measure);
        let text_top = if targets.is_empty() { inner.top } else { grid_bottom + gap };
        items.push(Item::Label(
            rect(inner.left, text_top, inner.right, inner.bottom),
            text.into(),
            Tone::Muted,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        ));
    }

    /// Mouse tools have buttons and a separate scroll direction binding pair.
    pub(super) fn layout_mouse(&mut self, content: RECT, items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>, measure: &dyn Fn(HFONT, &str) -> (i32, i32), dc: HDC) {
        let s = |value: i32| scale(value, self.dpi);
        let targets: Vec<_> = self.binding_rows.iter().map(|row| row.target)
            .filter(|target| matches!(target, BindingTarget::Mouse(_))).collect();
        let note = "Use these bindings for a tablet mouse tool. Configure tablet rings and dials in Auxiliary Settings.";
        let note_height = canvas::wrapped_height(dc, self.style().fonts.ui, note, content.right - content.left - s(24));
        let columns = self.binding_grid_columns(&[BindingTarget::MouseScrollUp, BindingTarget::MouseScrollDown], draw::inset(content, s(12), 0), measure);
        let scroll_height = s(48) + (2usize.div_ceil(columns) as i32) * s(46) + note_height;
        let scroll_top = content.bottom - scroll_height;
        let body = self.group("Mouse Buttons", rect(content.left, content.top, content.right, scroll_top - s(12)), items);
        let mut inner = draw::inset(body, s(12), s(12));
        if targets.is_empty() {
            let text = self.no_rows_text("mouse buttons");
            let height = canvas::wrapped_height(dc, self.style().fonts.ui, &text, inner.right - inner.left);
            items.push(Item::Label(rect(inner.left, inner.top, inner.right, inner.top + height),
                text, Tone::Muted, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX));
        } else {
            let columns = self.binding_grid_columns(&targets, inner, measure);
            // Reserve the navigation row even when one page currently fits,
            // keeping the page size stable when the number of buttons changes.
            let rows = ((inner.bottom - inner.top - s(36) + s(8)) / s(46)).max(1) as usize;
            let page_size = columns * rows;
            let pages = targets.len().div_ceil(page_size).max(1);
            self.property_page = self.property_page.min(pages - 1);
            if pages > 1 {
                for (hwnd, left, enabled) in [
                    (self.c.property_prev, inner.left, self.property_page > 0),
                    (self.c.property_next, inner.left + s(84), self.property_page + 1 < pages),
                ] {
                    unsafe { EnableWindow(hwnd, enabled.into()); }
                    shown.push((hwnd, rect(left, inner.top, left + s(78), inner.top + s(28))));
                }
                items.push(Item::Label(rect(inner.left + s(170), inner.top, inner.right, inner.top + s(28)),
                    format!("{} / {pages}", self.property_page + 1), Tone::Muted, draw::TEXT_LEFT));
                inner.top += s(36);
            }
            let start = self.property_page * page_size;
            self.layout_binding_grid(&targets[start..(start + page_size).min(targets.len())], inner, items, shown, measure);
        }
        let body = self.group("Mouse Scrollwheel", rect(content.left, scroll_top, content.right, content.bottom), items);
        let inner = draw::inset(body, s(12), s(12));
        let bottom = self.layout_binding_grid(&[BindingTarget::MouseScrollUp, BindingTarget::MouseScrollDown], inner, items, shown, measure);
        items.push(Item::Label(rect(inner.left, bottom + s(8), inner.right, inner.bottom),
            note.into(), Tone::Muted, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX));
    }

    fn binding_grid_columns(&self, targets: &[BindingTarget], area: RECT,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32)) -> usize {
        let s = |value: i32| scale(value, self.dpi);
        let label = targets.iter().map(|target| measure(self.style().fonts.ui, &target.label()).0).max().unwrap_or(0);
        ((area.right - area.left + s(8)) / (label.max(s(130)) + s(200) + s(8))).max(1) as usize
    }

    /// The Auxiliary Settings page: express keys, then one group per wheel.
    pub(super) fn layout_aux(
        &mut self,
        content: RECT,
        items: &mut Vec<Item>,
        shown: &mut Vec<(HWND, RECT)>,
        measure: &dyn Fn(HFONT, &str) -> (i32, i32),
        dc: HDC,
    ) {
        let s = |value: i32| scale(value, self.dpi);
        let gap = s(12);
        let keys: Vec<BindingTarget> = self
            .binding_rows
            .iter()
            .map(|row| row.target)
            .filter(|target| matches!(target, BindingTarget::Aux(_)))
            .collect();
        let font = self.style().fonts.ui;
        let mut top = content.top;
        // Express keys.
        {
            let inner_width = content.right - content.left - s(24);
            let label_width = keys.iter().map(|t| measure(font, &t.label()).0).max().unwrap_or(0);
            let columns = ((inner_width + s(8)) / (label_width + s(200) + s(8))).max(1) as usize;
            let lines = keys.len().div_ceil(columns) as i32;
            let text = keys.is_empty().then(|| self.no_rows_text("express keys"));
            let body_height = match &text {
                Some(text) => canvas::wrapped_height(dc, font, text, inner_width),
                None => lines * (s(38) + s(8)) - s(8),
            };
            let bottom = (top + s(24) + s(24) + body_height).min(content.bottom);
            let body = self.group("Express Keys", rect(content.left, top, content.right, bottom), items);
            let inner = draw::inset(body, s(12), s(12));
            match text {
                Some(text) => items.push(Item::Label(inner, text, Tone::Muted, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX)),
                None => {
                    self.layout_binding_grid(&keys, inner, items, shown, measure);
                }
            }
            top = bottom + gap;
        }
        let wheels: Vec<usize> = self
            .binding_rows
            .iter()
            .filter_map(|row| match row.target {
                BindingTarget::Clockwise(wheel) => Some(wheel),
                _ => None,
            })
            .collect();
        for wheel in wheels {
            if top >= content.bottom {
                break;
            }
            let buttons: Vec<BindingTarget> = self
                .binding_rows
                .iter()
                .map(|row| row.target)
                .filter(|target| matches!(target, BindingTarget::WheelButton(w, _) if *w == wheel))
                .collect();
            let rotation = [BindingTarget::Clockwise(wheel), BindingTarget::CounterClockwise(wheel)];
            let fields: Vec<(HWND, &str, &str)> = self
                .wheel_fields
                .iter()
                .filter(|field| field.wheel == wheel)
                .map(|field| {
                    (
                        field.hwnd,
                        if field.counter_clockwise {
                            "Counter-Clockwise Threshold"
                        } else {
                            "Clockwise Threshold"
                        },
                        "°",
                    )
                })
                .collect();
            let button_lines = buttons.len().div_ceil(2) as i32;
            let height = s(24)
                + s(24)
                + s(38)
                + s(8)
                + if fields.is_empty() { 0 } else { s(38) + s(8) }
                + button_lines * (s(38) + s(8))
                - s(8);
            let bottom = (top + height).min(content.bottom);
            let title = if wheels_total(self) > 1 {
                format!("Wheel {}", wheel + 1)
            } else {
                "Wheel".to_owned()
            };
            let body = self.group(&title, rect(content.left, top, content.right, bottom), items);
            let inner = draw::inset(body, s(12), s(12));
            let mut row_top = self.layout_binding_grid(&rotation, inner, items, shown, measure) + s(8);
            if !fields.is_empty() && row_top + s(38) <= inner.bottom {
                self.unit_row(&fields, rect(inner.left, row_top, inner.right, row_top + s(38)), s(84), items, shown, measure);
                row_top += s(38) + s(8);
            }
            if !buttons.is_empty() {
                self.layout_binding_grid(&buttons, rect(inner.left, row_top, inner.right, inner.bottom), items, shown, measure);
            }
            top = bottom + gap;
        }
    }
}

fn wheels_total(app: &App) -> usize {
    app.binding_rows
        .iter()
        .filter(|row| matches!(row.target, BindingTarget::Clockwise(_)))
        .count()
}

/// The dropdown menu of a binding row. Runs the shortcut dialog for keys.
pub(super) fn choose(window: HWND, control: HWND) {
    let Some((target, current)) = with_app(|app| app.binding_row(control)).flatten() else {
        return;
    };
    let menu = unsafe { CreatePopupMenu() };
    commands::append(menu, MFT_RADIOCHECK | commands::checked(current == ButtonAction::None), CHOICE_NONE, "None");
    if matches!(target, BindingTarget::Pen(_)) {
        for number in 1..=otd_core::output::buttons::MAX_BARREL {
            commands::append(
                menu,
                MFT_RADIOCHECK | commands::checked(current == ButtonAction::Barrel(number)),
                CHOICE_BARREL + u16::from(number),
                &format!("Pen Button {number}"),
            );
        }
    }
    for (index, (button, label)) in MOUSE_CHOICES.iter().enumerate() {
        commands::append(
            menu,
            MFT_RADIOCHECK | commands::checked(current == ButtonAction::Mouse(*button)),
            CHOICE_MOUSE + index as u16,
            label,
        );
    }
    for (index, (value, label)) in SCROLL_CHOICES.iter().enumerate() {
        let action = value.parse::<ButtonAction>().expect("valid built-in scroll action");
        commands::append(menu, MFT_RADIOCHECK | commands::checked(current == action),
            CHOICE_SCROLL + index as u16, label);
    }
    let keys = matches!(current, ButtonAction::Keys(_));
    commands::append(
        menu,
        MFT_RADIOCHECK | commands::checked(keys),
        CHOICE_KEYS,
        &if keys {
            format!("{}…", action_text(&current))
        } else {
            "Key or Shortcut…".to_owned()
        },
    );
    let action = match commands::popup(window, menu, control) {
        CHOICE_NONE => Some(ButtonAction::None),
        choice if (CHOICE_BARREL + 1..=CHOICE_BARREL + 3).contains(&choice) => {
            Some(ButtonAction::Barrel((choice - CHOICE_BARREL) as u8))
        }
        choice if (CHOICE_MOUSE..CHOICE_MOUSE + MOUSE_CHOICES.len() as u16).contains(&choice) => {
            Some(ButtonAction::Mouse(MOUSE_CHOICES[(choice - CHOICE_MOUSE) as usize].0))
        }
        choice if (CHOICE_SCROLL..CHOICE_SCROLL + SCROLL_CHOICES.len() as u16).contains(&choice) => {
            SCROLL_CHOICES[(choice - CHOICE_SCROLL) as usize].0.parse::<ButtonAction>().ok()
        }
        CHOICE_KEYS => match shortcut::show(window, &target.describe()) {
            Ok(keys) => keys.map(ButtonAction::Keys),
            Err(error) => {
                with_app(|app| app.log(Level::Error, "Bindings", format!("The shortcut dialog failed: {error}")));
                None
            }
        },
        _ => None,
    };
    if let Some(action) = action {
        with_app(|app| app.set_binding_action(target, action));
    }
}
