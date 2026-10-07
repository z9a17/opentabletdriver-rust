//! Tablet debugger, laid out like OpenTabletDriver's: the pen on an outline
//! of the tablet, the device and its report rate, the decoded report in
//! upstream's text format and the raw packet, all in the panel's palette.
//! A background thread polls the daemon and decodes the packet in this
//! process; the daemon only copies packets while this window is open and
//! never decodes them.
//!
//! Upstream: OpenTabletDriver.UX/Windows/Tablet/TabletDebugger.cs and
//! OpenTabletDriver.Plugin/ReportFormatter.cs at 736003e.
use super::*;
use crate::control::{self, Command, DebugReport, Reply, Request};
use otd_core::spec::TabletSpec;
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
use std::time::Instant;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_F10,
};

const CLASS: &str = "OpenTabletDriverRustDebugger";
const TITLE: &str = "Tablet Debugger";
const WM_DEBUG_REPORT: u32 = WM_APP + 20;
const POLL: Duration = Duration::from_millis(33);
const CMD_VISUALIZER: u16 = 1;
const CMD_HEX: u16 = 2;
const CMD_BINARY: u16 = 3;
const CMD_CLOSE: u16 = 4;
const CMD_RECORD: u16 = 5;
const CMD_STATS: u16 = 6;
const CMD_RESET_STATS: u16 = 7;
const CMD_COPY_STATS: u16 = 8;

/// Upstream's debugger text sizes, in the panel's monospace face.
struct MonoFonts {
    text: HFONT,
    large: HFONT,
}

impl MonoFonts {
    fn new(dpi: u32) -> Self {
        let points = |pt: i32| (pt * dpi as i32 + 36) / 72;
        Self {
            text: create_font(points(10), 400, "Consolas", 0),
            large: create_font(points(14), 400, "Consolas", 0),
        }
    }
}

impl Drop for MonoFonts {
    fn drop(&mut self) {
        for font in [self.text, self.large] {
            unsafe { DeleteObject(font) };
        }
    }
}

struct Debugger {
    window: HWND,
    dpi: u32,
    fonts: FontSet,
    mono: MonoFonts,
    palette: Palette,
    updates: Receiver<Result<DebugReport, String>>,
    cancelled: Arc<AtomicBool>,
    latest: Option<DebugReport>,
    /// The latest packet's bytes.
    raw: Vec<u8>,
    error: Option<String>,
    /// (time, packet counter) samples for the report rate.
    history: VecDeque<(Instant, u64)>,
    /// The last tablet looked up, and its specification when it has one.
    spec: Option<(String, Option<TabletSpec>)>,
    title: String,
    visualizer: bool,
    binary: bool,
    statistics: debugger_data::Statistics,
    show_statistics: bool,
    recorder: Option<debugger_data::Recorder>,
    recording_started: Instant,
    recording_status: String,
    /// The File menu entry, where the last paint put it.
    menu: Cell<RECT>,
    menu_hot: bool,
    menu_open: bool,
    tracking_mouse: bool,
}

thread_local! {
    static DEBUGGER: RefCell<Option<Debugger>> = const { RefCell::new(None) };
}

/// Runs `f` on the debugger that owns `window`. The borrow ends before the
/// caller makes any Win32 call that can dispatch messages.
fn with_debugger<R>(window: HWND, f: impl FnOnce(&mut Debugger) -> R) -> Option<R> {
    DEBUGGER
        .try_with(|slot| {
            let mut slot = slot.try_borrow_mut().ok()?;
            let debugger = slot.as_mut().filter(|debugger| debugger.window == window)?;
            Some(f(debugger))
        })
        .ok()
        .flatten()
}

/// Opens the debugger, or brings it forward if it is open.
pub(super) fn open() -> Result<(), String> {
    if let Some(window) = DEBUGGER.with(|slot| slot.borrow().as_ref().map(|d| d.window)) {
        unsafe {
            ShowWindow(window, SW_RESTORE);
            SetForegroundWindow(window);
        }
        return Ok(());
    }
    // Like the upstream DesktopForm, this window belongs to the panel.
    // Finish borrowing App before creation dispatches any window messages.
    let owner = with_app(|app| app.hwnd).unwrap_or(ptr::null_mut());
    let window = create_window(owner)?;
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let palette = with_look(|look| look.style.palette).unwrap_or_else(Palette::light);
    let (sender, updates) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&cancelled);
    let target = window as isize;
    if let Err(error) = std::thread::Builder::new()
        .name("tablet-debugger".into())
        .spawn(move || poll(sender, stop, target))
    {
        unsafe { DestroyWindow(window) };
        return Err(error.to_string());
    }
    DEBUGGER.with(|slot| {
        *slot.borrow_mut() = Some(Debugger::new(window, dpi, palette, updates, cancelled))
    });
    refresh_theme();
    unsafe {
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
    }
    Ok(())
}

/// Creates the hidden window at upstream's size for its DPI.
fn create_window(owner: HWND) -> Result<HWND, String> {
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class = wide(CLASS);
    let registration = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        ..Default::default()
    };
    // A second registration fails harmlessly with ERROR_CLASS_ALREADY_EXISTS.
    unsafe { RegisterClassW(&registration) };
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide(TITLE).as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            owner,
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    // Upstream's window opens at 940 x 560 plus its menu bar.
    let mut frame = rect(0, 0, scale(940, dpi), scale(590, dpi));
    unsafe {
        AdjustWindowRectExForDpi(&mut frame, WS_OVERLAPPEDWINDOW, 0, 0, dpi);
        SetWindowPos(
            window,
            ptr::null_mut(),
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    Ok(window)
}

/// Keep the debugger's client area and title bar on the panel's palette.
/// No state borrow may survive calls that can dispatch Win32 callbacks.
pub(super) fn refresh_theme() {
    let Some(palette) = with_look(|look| look.style.palette) else {
        return;
    };
    let window = DEBUGGER.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        let debugger = slot.as_mut()?;
        debugger.palette = palette;
        Some(debugger.window)
    });
    if let Some(window) = window {
        theme::DarkMode::load().apply_title_bar(window, palette.dark);
        unsafe { InvalidateRect(window, ptr::null(), 0) };
    }
}

/// Called before panel resources are released or shutdown waits for the daemon.
pub(super) fn close() {
    let window = DEBUGGER.with(|slot| slot.try_borrow().ok()?.as_ref().map(|debugger| debugger.window));
    if let Some(window) = window { unsafe { SendMessageW(window, WM_CLOSE, 0, 0); } }
}

/// The panel also waits for buffered recordings before exiting its process.
pub(super) fn is_open() -> bool {
    DEBUGGER.with(|slot| slot.borrow().is_some())
}

impl Drop for Debugger {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn poll(
    sender: SyncSender<Result<DebugReport, String>>,
    cancelled: Arc<AtomicBool>,
    window: isize,
) {
    while !cancelled.load(Ordering::Acquire) {
        let update =
            match control::request(&Request::new(1, Command::Debug), Duration::from_secs(1)) {
                Ok(response) => match response.reply {
                    Reply::Debug { mut report } => {
                        // Decoded here, never in the daemon.
                        crate::decode_cli::decode_debug_report(&mut report);
                        Ok(report)
                    }
                    Reply::Error { error } => Err(error.message),
                    _ => Err("unexpected daemon reply".into()),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err("The driver is not running. Start it to see reports.".into())
                }
                Err(error) => Err(format!("Cannot reach the driver: {error}")),
            };
        // The window takes the newest update; a full channel means it has
        // not drawn the previous one yet.
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        match sender.try_send(update) {
            Ok(()) => {
                unsafe { PostMessageW(window as HWND, WM_DEBUG_REPORT, 0, 0) };
            }
            Err(mpsc::TrySendError::Disconnected(_)) => break,
            Err(mpsc::TrySendError::Full(_)) => {}
        }
        std::thread::sleep(POLL);
    }
}

impl Debugger {
    fn new(
        window: HWND,
        dpi: u32,
        palette: Palette,
        updates: Receiver<Result<DebugReport, String>>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            window,
            dpi,
            fonts: FontSet::new(dpi),
            mono: MonoFonts::new(dpi),
            palette,
            updates,
            cancelled,
            latest: None,
            raw: Vec::new(),
            error: None,
            history: VecDeque::new(),
            spec: None,
            title: TITLE.into(),
            visualizer: true,
            binary: false,
            statistics: debugger_data::Statistics::default(),
            show_statistics: false,
            recorder: None,
            recording_started: Instant::now(),
            recording_status: String::new(),
            menu: Cell::new(RECT::default()),
            menu_hot: false,
            menu_open: false,
            tracking_mouse: false,
        }
    }

    /// Takes the newest reports; returns a new window title if the tablet
    /// changed, for the caller to set once this borrow has ended.
    fn take_updates(&mut self) -> Option<String> {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Ok(report) => {
                    let now = Instant::now();
                    // Another tablet or a restarted session counts from zero.
                    if self
                        .latest
                        .as_ref()
                        .is_some_and(|latest| latest.tablet != report.tablet)
                        || self
                            .history
                            .back()
                            .is_some_and(|(_, sequence)| *sequence > report.sequence)
                    {
                        self.history.clear();
                        self.spec = None;
                    }
                    self.history.push_back((now, report.sequence));
                    while self
                        .history
                        .front()
                        .is_some_and(|(time, _)| now - *time > Duration::from_secs(1))
                    {
                        self.history.pop_front();
                    }
                    // Look each tablet up once, including one without a
                    // usable specification.
                    if let Some(name) = &report.tablet
                        && self.spec.as_ref().is_none_or(|(known, _)| known != name)
                    {
                        self.spec =
                            Some((name.clone(), otd_core::config::runtime_tablet(name).ok()));
                    }
                    if self.statistics.observe(&report) {
                        if let Some(recorder) = &self.recorder {
                            recorder.push(self.recording_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64, report.clone());
                        }
                    }
                    self.raw = parse_hex(&report.raw_hex).unwrap_or_default();
                    self.error = None;
                    self.latest = Some(report);
                }
                Err(error) => {
                    self.error = Some(error);
                    self.history.clear();
                    self.latest = None;
                    self.raw.clear();
                    self.spec = None;
                }
            }
        }
        // Like upstream, the title names the tablet being debugged.
        let title = match self
            .latest
            .as_ref()
            .and_then(|report| report.tablet.as_deref())
        {
            Some(name) => format!("{TITLE} - {name}"),
            None => TITLE.into(),
        };
        (title != self.title).then(|| {
            self.title.clone_from(&title);
            title
        })
    }

    /// Every message that can observe completion uses this same terminal path.
    /// Callers log a returned error after releasing the debugger state borrow.
    fn finish_recording(&mut self) -> Option<Result<(), String>> {
        let result = self.recorder.as_ref()?.result()?;
        self.recording_status = match &result { Ok(()) => "Recording saved.".into(), Err(error) => error.clone() };
        self.recorder = None;
        Some(result)
    }

    fn rate(&self) -> f64 {
        match (self.history.front(), self.history.back()) {
            (Some((start, first)), Some((end, last))) if end > start => {
                last.saturating_sub(*first) as f64 / (*end - *start).as_secs_f64()
            }
            _ => 0.0,
        }
    }

    /// The Tablet Report text: upstream's lines, or why there are none.
    fn report_lines(&self) -> (Vec<String>, bool) {
        let status = |text: &str| (vec![text.to_owned()], true);
        if let Some(error) = &self.error {
            return status(error);
        }
        let Some(report) = &self.latest else {
            return status("Waiting for the driver...");
        };
        if report.raw_hex.is_empty() {
            return status("Waiting for a tablet report...");
        }
        if report.values.is_null() {
            return status("This report could not be decoded.");
        }
        (format_report(&report.values), false)
    }

    fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.window, &mut ps) };
        let client = client_rect(self.window);
        let style = draw::Style {
            palette: self.palette,
            fonts: self.fonts.fonts,
            scale: self.dpi as f32 / 96.0,
        };
        if !hdc.is_null()
            && let Some(mut canvas) = canvas::Canvas::new(hdc, client)
        {
            self.draw(&mut canvas, client, &style);
            canvas.present(hdc);
        }
        unsafe { EndPaint(self.window, &ps) };
    }

    /// Upstream's layout: the visualizer above the device and report rate,
    /// then the decoded report and the raw packet in fixed-width columns.
    fn draw(&self, canvas: &mut canvas::Canvas, client: RECT, style: &draw::Style) {
        let p = style.palette;
        let s = |value: f32| style.ipx(value);
        canvas.fill(client, p.window);

        // The menu bar entry, drawn like the panel's.
        let (label_width, _) = canvas.measure(style.fonts.ui, "File");
        let menu = rect(
            client.left + s(4.0),
            client.top + s(2.0),
            client.left + s(20.0) + label_width,
            client.top + s(24.0),
        );
        self.menu.set(menu);
        draw::flat_button(
            canvas,
            menu,
            "&File",
            style,
            p.window,
            State {
                hot: self.menu_hot,
                selected: self.menu_open,
                ..State::default()
            },
        );

        let (pad, gap, header) = (s(10.0), s(10.0), s(22.0));
        let (top, bottom) = (menu.bottom + s(6.0), client.bottom - pad);
        let (text, large) = (self.mono.text, self.mono.large);
        let line = canvas.measure(text, "F").1.max(1);
        let sample = if self.binary {
            "10101010 10101010 10101010 10101010"
        } else {
            "FF FF FF FF FF FF FF FF"
        };
        let raw_width = canvas.measure(text, sample).0 + s(24.0);
        let raw_column = rect(
            client.right - pad - raw_width,
            top,
            client.right - pad,
            bottom,
        );
        let report_column = rect(
            raw_column.left - gap - s(220.0),
            top,
            raw_column.left - gap,
            bottom,
        );
        let left = rect(client.left + pad, top, report_column.left - gap, bottom);

        let report_box = section(canvas, report_column, "Tablet Report", style, header);
        let (lines, status) = self.report_lines();
        let inner = draw::inset(report_box, s(10.0), s(8.0));
        let mut y = inner.top;
        for line_text in lines {
            let height = canvas
                .wrapped_height(text, &line_text, inner.right - inner.left)
                .max(line);
            if y + height > inner.bottom {
                break;
            }
            canvas.text(
                rect(inner.left, y, inner.right, y + height),
                &line_text,
                text,
                if status { p.muted } else { p.text },
                DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
            );
            y += height;
        }

        let raw_box = section(canvas, raw_column, "Raw Tablet Data", style, header);
        let inner = draw::inset(raw_box, s(10.0), s(8.0));
        let visible = usize::try_from((inner.bottom - inner.top) / line).unwrap_or(0);
        let per_line = if self.binary { 4 } else { 8 };
        for (index, bytes) in self.raw.chunks(per_line).take(visible).enumerate() {
            let y = inner.top + index as i32 * line;
            canvas.text(
                rect(inner.left, y, inner.right, y + line),
                &raw_line(bytes, self.binary),
                text,
                p.text,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
            );
        }

        // Device and report rate sit below the visualizer, or at the top
        // when it is hidden.
        let box_height = canvas.measure(large, "W").1 + s(18.0);
        let name = self
            .latest
            .as_ref()
            .and_then(|report| report.tablet.as_deref())
            .unwrap_or_default();
        let rate_width = canvas.measure(large, "1234.56Hz").0 + s(24.0);
        let device_width = (canvas.measure(large, name).0 + s(24.0)).max(s(140.0));
        let row = header + box_height;
        // Side by side as upstream lays them out, or stacked when the
        // binary raw view leaves too little width.
        let stacked = device_width + gap + rate_width > left.right - left.left;
        let rows_height = if stacked { row * 2 + gap } else { row };
        let stats_height = if self.show_statistics { s(155.0) } else { 0 };
        let rows_top = if self.visualizer {
            bottom - rows_height - stats_height
        } else {
            top
        };
        let device = section(
            canvas,
            rect(
                left.left,
                rows_top,
                (left.left + device_width).min(left.right),
                rows_top + row,
            ),
            "Device",
            style,
            header,
        );
        canvas.text(
            draw::inset(device, s(10.0), 0),
            name,
            large,
            p.text,
            draw::TEXT_LEFT | DT_NOPREFIX,
        );
        let (rate_left, rate_top) = if stacked {
            (left.left, rows_top + row + gap)
        } else {
            (device.right + gap, rows_top)
        };
        let rate = section(
            canvas,
            rect(
                rate_left,
                rate_top,
                (rate_left + rate_width).min(left.right),
                rate_top + row,
            ),
            "Report Rate",
            style,
            header,
        );
        if self.latest.is_some() {
            canvas.text(
                draw::inset(rate, s(10.0), 0),
                &format!("{:>7.2}Hz", self.rate()),
                large,
                p.text,
                draw::TEXT_LEFT | DT_NOPREFIX,
            );
        }
        if self.show_statistics {
            let stats_top = rows_top + rows_height + gap;
            let stats = section(canvas, rect(left.left, stats_top, left.right, bottom), "Additional Statistics", style, header);
            let inner = draw::inset(stats, s(8.0), s(6.0));
            let status = self.recorder.as_ref().map_or_else(|| self.recording_status.clone(), debugger_data::Recorder::status);
            let mut lines = self.statistics.lines();
            if !status.is_empty() { lines.insert(0, status); }
            let room = usize::try_from((inner.bottom - inner.top) / line).unwrap_or(0);
            for (index, value) in lines.iter().take(room).enumerate() {
                let y = inner.top + index as i32 * line;
                canvas.text(rect(inner.left, y, inner.right, y + line), value, text, p.text,
                    DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
            }
        }
        if self.visualizer {
            let area = section(
                canvas,
                rect(left.left, top, left.right, rows_top - gap),
                "Visualizer",
                style,
                header,
            );
            self.draw_tablet(canvas, area, style);
        }
    }

    /// The tablet's active area, centered and outlined in the accent color,
    /// with the pen position as a dot, as upstream's visualizer draws them.
    fn draw_tablet(&self, canvas: &mut canvas::Canvas, area: RECT, style: &draw::Style) {
        let p = style.palette;
        let Some((_, Some(spec))) = &self.spec else {
            return;
        };
        let margin = style.ipx(6.0);
        let width = f64::from(area.right - area.left - margin * 2);
        let height = f64::from(area.bottom - area.top - margin * 2);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let fit = (width / spec.width_mm).min(height / spec.height_mm);
        let (w, h) = (spec.width_mm * fit, spec.height_mm * fit);
        let left = f64::from(area.left + area.right) / 2.0 - w / 2.0;
        let top = f64::from(area.top + area.bottom) / 2.0 - h / 2.0;
        let outline = rect(
            left.round() as i32,
            top.round() as i32,
            (left + w).round() as i32,
            (top + h).round() as i32,
        );
        canvas.round_rect(
            outline,
            [0.0; 4],
            Some(p.bounds_fill),
            Some((p.accent, 1.0)),
        );
        let Some(report) = &self.latest else {
            return;
        };
        if report.values.get("kind").and_then(Value::as_str) != Some("data") {
            return;
        }
        let position = report.values.pointer("/values/position");
        let coordinate = |index: usize| position.and_then(|p| p.get(index)).and_then(Value::as_f64);
        if let (Some(x), Some(y)) = (coordinate(0), coordinate(1)) {
            let x = left + x / f64::from(spec.max_x.max(1)) * w;
            let y = top + y / f64::from(spec.max_y.max(1)) * h;
            // Upstream does not clamp the dot; keep it inside the box.
            if (f64::from(area.left)..f64::from(area.right)).contains(&x)
                && (f64::from(area.top)..f64::from(area.bottom)).contains(&y)
            {
                canvas.circle(
                    (x as f32, y as f32),
                    2.5 * style.scale,
                    Some(p.accent),
                    None,
                );
            }
        }
    }
}

/// A titled box, like upstream's debugger groups. Returns the box.
fn section(
    canvas: &mut canvas::Canvas,
    area: RECT,
    title: &str,
    style: &draw::Style,
    header: i32,
) -> RECT {
    canvas.text(
        rect(area.left, area.top, area.right, area.top + header),
        title,
        style.fonts.bold,
        style.palette.text,
        draw::TEXT_LEFT | DT_NOPREFIX,
    );
    let content = rect(area.left, area.top + header, area.right, area.bottom);
    draw::group_box(canvas, content, style, style.palette.group);
    content
}

fn parse_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect()
}

/// One line of the packet, as upstream's raw view shows it: uppercase hex
/// bytes, or eight-digit binary bytes, separated by spaces.
fn raw_line(bytes: &[u8], binary: bool) -> String {
    bytes
        .iter()
        .map(|byte| {
            if binary {
                format!("{byte:08b}")
            } else {
                format!("{byte:02X}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A decoded number as .NET prints it: whole values without a fraction.
fn number(value: &Value) -> String {
    match value.as_f64() {
        Some(float) if float.fract() == 0.0 && float.abs() < 1e15 => format!("{float:.0}"),
        Some(_) => value.to_string(),
        None => "null".into(),
    }
}

fn boolean(value: &Value) -> &'static str {
    if value.as_bool() == Some(true) {
        "True"
    } else {
        "False"
    }
}

fn buttons(value: &Value) -> String {
    let states = value.as_array().map(Vec::as_slice).unwrap_or_default();
    states.iter().map(boolean).collect::<Vec<_>>().join(" ")
}

fn pair(value: &Value) -> String {
    format!("[{},{}]", number(&value[0]), number(&value[1]))
}

/// The decoded report as upstream's ReportFormatter.GetStringFormat writes
/// it, in its order. Values upstream's report types do not carry are left
/// out.
fn format_report(report: &Value) -> Vec<String> {
    if let Some(error) = report.get("error") {
        return vec![format!(
            "Decode error: {}",
            error
                .as_str()
                .map_or_else(|| error.to_string(), str::to_owned)
        )];
    }
    if report.get("kind").and_then(Value::as_str) == Some("out_of_range") {
        return vec!["Pen is out of Range".into()];
    }
    let values = &report["values"];
    let field = |key: &str| values.get(key).filter(|value| !value.is_null());
    let mut lines = Vec::new();
    if let Some(position) = field("position") {
        lines.push(format!("Position:{}", pair(position)));
    }
    if let Some(pressure) = field("pressure") {
        lines.push(format!("Pressure:{}", number(pressure)));
    }
    if let Some(pen) = field("pen_buttons") {
        lines.push(format!("PenButtons:[{}]", buttons(pen)));
    }
    if let Some(aux) = field("aux_buttons") {
        lines.push(format!("AuxButtons:[{}]", buttons(aux)));
    }
    if let Some(eraser) = field("eraser") {
        lines.push(format!("Eraser:{}", boolean(eraser)));
    }
    if let Some(near) = field("near_proximity") {
        lines.push(format!("NearProximity:{}", boolean(near)));
    }
    if let Some(distance) = field("hover_distance") {
        lines.push(format!("HoverDistance:{}", number(distance)));
    }
    if let Some(tilt) = field("tilt") {
        lines.push(format!("Tilt:{}", pair(tilt)));
    }
    if let Some(touches) = field("touches").and_then(Value::as_array) {
        lines.push("Touch data:".into());
        for touch in touches.iter().filter(|touch| !touch.is_null()) {
            let position = &touch["position"];
            lines.push(format!(
                "Point #{}: <{}, {}>;",
                number(&touch["id"]),
                number(&position[0]),
                number(&position[1])
            ));
        }
    }
    if let Some(positions) = field("absolute_analog")
        .and_then(|analog| analog.get("positions"))
        .and_then(Value::as_array)
    {
        for (index, position) in positions.iter().enumerate() {
            let position = if position.is_null() {
                "Idle".into()
            } else {
                number(position)
            };
            lines.push(format!("Wheel {}:{position}", index + 1));
        }
    }
    if let Some(deltas) = field("relative_analog")
        .and_then(|analog| analog.get("deltas"))
        .and_then(Value::as_array)
    {
        for (index, delta) in deltas.iter().enumerate() {
            lines.push(format!("Wheel {} Delta:{}", index + 1, number(delta)));
        }
    }
    if let Some(wheels) = field("wheel_buttons").and_then(Value::as_array) {
        for (index, wheel) in wheels.iter().enumerate() {
            lines.push(format!("Wheel {} Buttons:[{}]", index + 1, buttons(wheel)));
        }
    }
    if let Some(mouse) = field("mouse_buttons") {
        lines.push(format!("MouseButtons:[{}]", buttons(mouse)));
    }
    if let Some(scroll) = field("mouse_scroll") {
        lines.push(format!("Scroll:{}", pair(scroll)));
    }
    if let Some(tool) = field("tool") {
        let kind = match tool["kind"].as_str() {
            Some("eraser") => "Eraser",
            _ => "Pen",
        };
        lines.push(format!("Tool:{kind}"));
        lines.push(format!("RawToolID:{}", number(&tool["raw_tool_id"])));
        lines.push(format!("Serial:{}", number(&tool["serial"])));
    }
    lines
}

/// Opens the File menu below its entry. No debugger borrow survives the
/// menu's message loop.
fn open_menu(window: HWND) {
    let Some((entry, visualizer, binary, stats, recording, finishing)) = with_debugger(window, |debugger| {
        debugger.menu_open = true;
        (debugger.menu.get(), debugger.visualizer, debugger.binary, debugger.show_statistics,
            debugger.recorder.as_ref().is_some_and(debugger_data::Recorder::active),
            debugger.recorder.as_ref().is_some_and(|recorder| !recorder.active()))
    }) else {
        return;
    };
    unsafe { InvalidateRect(window, &entry, 0) };
    let command = unsafe {
        let menu = CreatePopupMenu();
        append(menu, checked(visualizer), CMD_VISUALIZER, "Visualizer");
        append(menu, checked(stats), CMD_STATS, "Additional Statistics");
        append(menu, MF_STRING, CMD_RESET_STATS, "Reset Statistics");
        append(menu, MF_STRING, CMD_COPY_STATS, "Copy All Statistics");
        append(menu, if finishing { MF_GRAYED } else { MF_STRING }, CMD_RECORD,
            if recording { "Stop Recording" } else if finishing { "Finishing Recording..." } else { "Record Sampled Reports..." });
        let modes = CreatePopupMenu();
        append(modes, MF_STRING, CMD_HEX, "Hex");
        append(modes, MF_STRING, CMD_BINARY, "Binary");
        let mode = if binary { CMD_BINARY } else { CMD_HEX };
        CheckMenuRadioItem(
            modes,
            u32::from(CMD_HEX),
            u32::from(CMD_BINARY),
            u32::from(mode),
            MF_BYCOMMAND,
        );
        AppendMenuW(
            menu,
            MF_POPUP,
            modes as usize,
            wide("Raw Data Mode").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        append(menu, MF_STRING, CMD_CLOSE, "Close Window\tEsc");
        let mut corners = [
            POINT {
                x: entry.left,
                y: entry.top,
            },
            POINT {
                x: entry.right,
                y: entry.bottom,
            },
        ];
        for corner in &mut corners {
            ClientToScreen(window, corner);
        }
        let params = TPMPARAMS {
            cbSize: size_of::<TPMPARAMS>() as u32,
            rcExclude: rect(corners[0].x, corners[0].y, corners[1].x, corners[1].y),
        };
        let command = TrackPopupMenuEx(
            menu,
            TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD | TPM_VERTICAL,
            corners[0].x,
            corners[1].y,
            window,
            &params,
        );
        DestroyMenu(menu);
        // A click on File that closed the menu must not open it again.
        let mut message = MSG::default();
        if PeekMessageW(
            &mut message,
            window,
            WM_LBUTTONDOWN,
            WM_LBUTTONDOWN,
            PM_NOREMOVE,
        ) != 0
            && inside(entry, message.lParam)
        {
            PeekMessageW(
                &mut message,
                window,
                WM_LBUTTONDOWN,
                WM_LBUTTONDOWN,
                PM_REMOVE,
            );
        }
        command as u16
    };
    with_debugger(window, |debugger| {
        debugger.menu_open = false;
        match command {
            CMD_VISUALIZER => debugger.visualizer = !debugger.visualizer,
            CMD_HEX => debugger.binary = false,
            CMD_BINARY => debugger.binary = true,
            CMD_STATS => debugger.show_statistics = !debugger.show_statistics,
            CMD_RESET_STATS => debugger.statistics = debugger_data::Statistics::default(),
            _ => {}
        }
    });
    if command == CMD_COPY_STATS {
        if let Some(lines) = with_debugger(window, |debugger| debugger.statistics.lines().join("\r\n")) {
            copy_to_clipboard(window, &lines);
        }
    }
    if command == CMD_RECORD {
        if recording {
            with_debugger(window, |debugger| { if let Some(recorder) = &mut debugger.recorder { recorder.stop(); } });
        } else if !finishing {
            match commands::file_dialog(window, true, commands::FileKind::Recording, "Record sampled reports") {
                Ok(Some(path)) => {
                    let result = debugger_data::Recorder::start(&path);
                    with_debugger(window, |debugger| {
                        debugger.show_statistics = true;
                        match result {
                            Ok(recorder) => {
                                debugger.recorder = Some(recorder);
                                debugger.recording_started = Instant::now();
                                debugger.statistics = debugger_data::Statistics::default();
                                debugger.recording_status = String::new();
                            }
                            Err(error) => debugger.recording_status = error,
                        }
                    });
                }
                Err(error) => { with_debugger(window, |debugger| debugger.recording_status = error); }
                Ok(None) => {},
            }
        }
    }
    if command == CMD_CLOSE {
        unsafe { SendMessageW(window, WM_CLOSE, 0, 0); };
    } else {
        unsafe { InvalidateRect(window, ptr::null(), 0) };
    }
}

fn inside(area: RECT, lparam: LPARAM) -> bool {
    let (x, y) = (
        i32::from((lparam & 0xFFFF) as u16 as i16),
        i32::from(((lparam >> 16) & 0xFFFF) as u16 as i16),
    );
    x >= area.left && x < area.right && y >= area.top && y < area.bottom
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DEBUG_REPORT => {
            if let Some((title, result)) = with_debugger(window, |debugger| {
                (debugger.take_updates(), debugger.finish_recording())
            }) {
                if let Some(title) = title { unsafe { SetWindowTextW(window, wide(&title).as_ptr()); } }
                if let Some(Err(error)) = result { with_app(|app| app.log(Level::Error, "Debugger", error)); }
            }
            unsafe { InvalidateRect(window, ptr::null(), 0) };
            0
        }
        WM_PAINT => {
            let painted = DEBUGGER
                .try_with(|slot| {
                    if let Ok(slot) = slot.try_borrow()
                        && let Some(debugger) = slot.as_ref()
                        && debugger.window == window
                    {
                        debugger.paint();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            // An unpainted window must still be validated, or Windows keeps
            // sending WM_PAINT.
            if painted {
                0
            } else {
                unsafe { DefWindowProcW(window, message, wparam, lparam) }
            }
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            if with_debugger(window, |debugger| inside(debugger.menu.get(), lparam)) == Some(true) {
                open_menu(window);
            }
            0
        }
        WM_MOUSEMOVE => {
            let changed = with_debugger(window, |debugger| {
                let track = !debugger.tracking_mouse;
                debugger.tracking_mouse = true;
                let hot = inside(debugger.menu.get(), lparam);
                let entry = (hot != debugger.menu_hot).then(|| debugger.menu.get());
                debugger.menu_hot = hot;
                (track, entry)
            });
            if let Some((track, entry)) = changed {
                if track {
                    let mut event = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut event) };
                }
                if let Some(entry) = entry {
                    unsafe { InvalidateRect(window, &entry, 0) };
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            if let Some(entry) = with_debugger(window, |debugger| {
                debugger.tracking_mouse = false;
                std::mem::take(&mut debugger.menu_hot).then(|| debugger.menu.get())
            })
            .flatten()
            {
                unsafe { InvalidateRect(window, &entry, 0) };
            }
            0
        }
        // Escape closes the window, as upstream's does; Alt+F and F10 open
        // the File menu.
        WM_KEYDOWN if wparam == usize::from(VK_ESCAPE) => {
            unsafe { SendMessageW(window, WM_CLOSE, 0, 0); };
            0
        }
        WM_SYSCHAR if matches!(wparam, 0x46 | 0x66) => {
            open_menu(window);
            0
        }
        WM_SYSKEYDOWN if wparam == usize::from(VK_F10) => {
            open_menu(window);
            0
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(window) }.max(96);
            let mut frame = rect(0, 0, scale(760, dpi), scale(480, dpi));
            unsafe {
                AdjustWindowRectExForDpi(&mut frame, WS_OVERLAPPEDWINDOW, 0, 0, dpi);
                if let Some(info) = (lparam as *mut MINMAXINFO).as_mut() {
                    info.ptMinTrackSize = POINT {
                        x: frame.right - frame.left,
                        y: frame.bottom - frame.top,
                    };
                }
            }
            0
        }
        WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            with_debugger(window, |debugger| {
                debugger.dpi = dpi.max(96);
                debugger.fonts = FontSet::new(debugger.dpi);
                debugger.mono = MonoFonts::new(debugger.dpi);
            });
            unsafe {
                if let Some(area) = (lparam as *const RECT).as_ref() {
                    SetWindowPos(
                        window,
                        ptr::null_mut(),
                        area.left,
                        area.top,
                        area.right - area.left,
                        area.bottom - area.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                InvalidateRect(window, ptr::null(), 0);
            }
            0
        }
        WM_CLOSE => {
            let finishing = with_debugger(window, |debugger| {
                debugger.cancelled.store(true, Ordering::Release);
                if let Some(recorder) = &mut debugger.recorder { recorder.stop(); true } else { false }
            }).unwrap_or(false);
            if finishing { unsafe { SetTimer(window, 1, 33, None); } }
            else { unsafe { DestroyWindow(window); } }
            0
        }
        WM_TIMER if wparam == 1 => {
            let result = with_debugger(window, Debugger::finish_recording).flatten();
            if let Some(result) = result {
                if let Err(error) = result { with_app(|app| app.log(Level::Error, "Debugger", error)); }
                unsafe { KillTimer(window, 1); DestroyWindow(window); }
            } else if with_debugger(window, |debugger| debugger.recorder.is_none()) == Some(true) {
                unsafe { KillTimer(window, 1); DestroyWindow(window); }
            }
            0
        }
        WM_DESTROY => {
            // Never panic in a window procedure: the slot may be gone when
            // Windows destroys the window at thread exit.
            let _ = DEBUGGER.try_with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut()
                    && slot
                        .as_ref()
                        .is_some_and(|debugger| debugger.window == window)
                {
                    slot.take();
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_read_like_upstream() {
        let report = serde_json::json!({"kind": "data", "values": {
            "position": [20914.0, 4845.0], "pressure": 0, "pen_buttons": [false, false],
            "eraser": false, "near_proximity": false, "hover_distance": 63,
            "tilt": [-6.0, 9.0], "tip_switch": false, "rotation": null}});
        assert_eq!(
            format_report(&report),
            [
                "Position:[20914,4845]",
                "Pressure:0",
                "PenButtons:[False False]",
                "Eraser:False",
                "NearProximity:False",
                "HoverDistance:63",
                "Tilt:[-6,9]",
            ]
        );
        let other = serde_json::json!({"kind": "data", "values": {
            "position": [1.5, 2.0], "aux_buttons": [true, false],
            "absolute_analog": {"kind": "wheel", "positions": [null, 12]},
            "touches": [{"id": 3, "position": [10.0, 20.0]}, null],
            "tool": {"kind": "eraser", "raw_tool_id": 2210, "serial": 77}}});
        assert_eq!(
            format_report(&other),
            [
                "Position:[1.5,2]",
                "AuxButtons:[True False]",
                "Touch data:",
                "Point #3: <10, 20>;",
                "Wheel 1:Idle",
                "Wheel 2:12",
                "Tool:Eraser",
                "RawToolID:2210",
                "Serial:77",
            ]
        );
        assert_eq!(
            format_report(&serde_json::json!({"kind": "out_of_range", "values": {}})),
            ["Pen is out of Range"]
        );
        assert_eq!(
            format_report(&serde_json::json!({"error": "short packet"})),
            ["Decode error: short packet"]
        );
    }

    fn title(window: HWND) -> String {
        let mut buffer = [0u16; 128];
        let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    fn point(x: i32, y: i32) -> LPARAM {
        ((y as u16 as isize) << 16) | x as u16 as isize
    }

    /// A hidden window, never shown or activated, with no driver.
    #[test]
    fn a_hidden_debugger_follows_reports_hover_and_escape() {
        let window = create_window(ptr::null_mut()).unwrap();
        let (sender, updates) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        DEBUGGER.with(|slot| {
            *slot.borrow_mut() = Some(Debugger::new(
                window,
                96,
                Palette::light(),
                updates,
                Arc::clone(&cancelled),
            ))
        });
        let report = DebugReport {
            tablet: Some("Wacom PTH-660".into()),
            parser: None,
            sequence: 1,
            raw_hex: "10ff".into(),
            values: Value::Null,
        };
        sender.send(Ok(report)).unwrap();
        unsafe { SendMessageW(window, WM_DEBUG_REPORT, 0, 0) };
        assert_eq!(title(window), "Tablet Debugger - Wacom PTH-660");
        assert_eq!(
            with_debugger(window, |debugger| debugger.raw.clone()),
            Some(vec![0x10, 0xFF])
        );
        with_debugger(window, |debugger| debugger.menu.set(rect(4, 2, 40, 24)));
        unsafe { SendMessageW(window, WM_MOUSEMOVE, 0, point(10, 10)) };
        assert_eq!(
            with_debugger(window, |debugger| debugger.menu_hot),
            Some(true)
        );
        unsafe { SendMessageW(window, WM_MOUSEMOVE, 0, point(200, 200)) };
        assert_eq!(
            with_debugger(window, |debugger| debugger.menu_hot),
            Some(false)
        );
        sender
            .send(Err("The driver is not running.".into()))
            .unwrap();
        unsafe { SendMessageW(window, WM_DEBUG_REPORT, 0, 0) };
        assert_eq!(title(window), "Tablet Debugger");
        assert_eq!(
            with_debugger(window, |debugger| debugger.report_lines()),
            Some((vec!["The driver is not running.".to_owned()], true))
        );
        unsafe { SendMessageW(window, WM_KEYDOWN, usize::from(VK_ESCAPE), 0) };
        assert_eq!(unsafe { IsWindow(window) }, 0);
        assert!(DEBUGGER.with(|slot| slot.borrow().is_none()));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn raw_data_is_uppercase_hex_or_binary() {
        let bytes = parse_hex("1000b251ff").unwrap();
        assert_eq!(raw_line(&bytes, false), "10 00 B2 51 FF");
        assert_eq!(raw_line(&bytes[..2], true), "00010000 00000000");
        assert_eq!(parse_hex("abc"), None);
        assert_eq!(parse_hex("zz"), None);
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Renders the debugger with a sample PTH-660 report in the light, dark
    /// and high-contrast palettes, then at the minimum size in binary mode
    /// without the visualizer and with the driver stopped, to
    /// $OTD_PREVIEW_DIR/debugger-*.bmp without opening a window.
    #[test]
    #[ignore = "writes preview images; set OTD_PREVIEW_DIR"]
    fn render_debugger_preview() {
        let directory = std::env::var_os("OTD_PREVIEW_DIR").expect("OTD_PREVIEW_DIR");
        let raw = format!("1000b25100ed120000003ffa0900003f{}", "00".repeat(345));
        for (name, palette, size) in [
            ("light", theme::Palette::light(), (940, 590)),
            ("dark", theme::Palette::dark(), (940, 590)),
            ("contrast", theme::Palette::high_contrast(), (940, 590)),
            ("compact", theme::Palette::dark(), (760, 480)),
            ("stopped", theme::Palette::light(), (760, 480)),
        ] {
            let fonts = FontSet::new(96);
            let style = draw::Style {
                palette,
                fonts: fonts.fonts,
                scale: 1.0,
            };
            let (_, updates) = mpsc::sync_channel(1);
            let mut debugger = Debugger::new(
                ptr::null_mut(),
                96,
                palette,
                updates,
                Arc::new(AtomicBool::new(false)),
            );
            debugger.raw = parse_hex(&raw).unwrap();
            debugger.latest = Some(DebugReport {
                tablet: Some("Wacom PTH-660".into()),
                parser: Some(
                    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser"
                        .into(),
                ),
                sequence: 1000,
                raw_hex: raw.clone(),
                values: serde_json::json!({"kind": "data", "values": {
                    "position": [20914.0, 4845.0], "pressure": 0, "tilt": [-6.0, 9.0],
                    "eraser": false, "near_proximity": false, "hover_distance": 63,
                    "pen_buttons": [false, false]}}),
            });
            debugger.spec = Some(("Wacom PTH-660".into(), Some(TabletSpec::PTH_660)));
            debugger.menu_hot = name == "dark";
            debugger.visualizer = name != "compact";
            debugger.binary = name == "compact";
            if name == "stopped" {
                debugger.latest = None;
                debugger.raw.clear();
                debugger.spec = None;
                debugger.error = Some("The driver is not running. Start it to see reports.".into());
            }
            let now = Instant::now();
            debugger
                .history
                .push_back((now - Duration::from_millis(1000), 284));
            debugger.history.push_back((now, 1000));
            let area = rect(0, 0, size.0, size.1);
            let screen = unsafe { GetDC(ptr::null_mut()) };
            let mut canvas = canvas::Canvas::new(screen, area).unwrap();
            debugger.draw(&mut canvas, area, &style);
            std::fs::write(
                std::path::Path::new(&directory).join(format!("debugger-{name}.bmp")),
                canvas.to_bmp(),
            )
            .unwrap();
            unsafe { ReleaseDC(ptr::null_mut(), screen) };
        }
    }
}
