//! Tablet debugger, like OpenTabletDriver's: the tablet, its parser, the
//! report rate, the latest raw packet and its decoded values, and the pen on
//! an outline of the tablet. A background thread polls the daemon; the report
//! thread only copies packets while this window is open.
use super::*;
use crate::control::{self, Command, DebugReport, Reply, Request};
use otd_core::spec::TabletSpec;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
use std::time::Instant;

const CLASS: &str = "OpenTabletDriverRustDebugger";
const WM_DEBUG_REPORT: u32 = WM_APP + 20;
const POLL: Duration = Duration::from_millis(33);

struct Debugger {
    window: HWND,
    updates: Receiver<Result<DebugReport, String>>,
    cancelled: Arc<AtomicBool>,
    latest: Option<DebugReport>,
    error: Option<String>,
    /// (time, packet counter) samples for the report rate.
    history: VecDeque<(Instant, u64)>,
    spec: Option<(String, TabletSpec)>,
}

thread_local! {
    static DEBUGGER: RefCell<Option<Debugger>> = const { RefCell::new(None) };
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
            wide("Tablet debugger").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            560,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let (sender, updates) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&cancelled);
    let target = window as isize;
    std::thread::Builder::new()
        .name("tablet-debugger".into())
        .spawn(move || poll(sender, stop, target))
        .map_err(|error| error.to_string())?;
    DEBUGGER.with(|slot| {
        *slot.borrow_mut() = Some(Debugger {
            window,
            updates,
            cancelled,
            latest: None,
            error: None,
            history: VecDeque::new(),
            spec: None,
        })
    });
    if let Some(dark) = with_look(|look| look.style.palette.dark) {
        theme::DarkMode::load().apply_title_bar(window, dark);
    }
    unsafe {
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
    }
    Ok(())
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
                    Reply::Debug { report } => Ok(report),
                    Reply::Error { error } => Err(error.message),
                    _ => Err("unexpected daemon reply".into()),
                },
                Err(_) => Err("The driver is not running. Start it to see reports.".into()),
            };
        // The window takes the newest update; a full channel means it has
        // not drawn the previous one yet.
        if sender.try_send(update).is_ok() {
            unsafe { PostMessageW(window as HWND, WM_DEBUG_REPORT, 0, 0) };
        }
        std::thread::sleep(POLL);
    }
}

impl Debugger {
    fn take_updates(&mut self) {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Ok(report) => {
                    let now = Instant::now();
                    self.history.push_back((now, report.sequence));
                    while self
                        .history
                        .front()
                        .is_some_and(|(time, _)| now - *time > Duration::from_secs(1))
                    {
                        self.history.pop_front();
                    }
                    if let Some(name) = &report.tablet
                        && self.spec.as_ref().is_none_or(|(known, _)| known != name)
                    {
                        self.spec = otd_core::config::runtime_tablet(name)
                            .ok()
                            .map(|spec| (name.clone(), spec));
                    }
                    self.error = None;
                    self.latest = Some(report);
                }
                Err(error) => {
                    self.error = Some(error);
                    self.history.clear();
                }
            }
        }
    }

    fn rate(&self) -> f64 {
        match (self.history.front(), self.history.back()) {
            (Some((start, first)), Some((end, last))) if end > start => {
                last.saturating_sub(*first) as f64 / (*end - *start).as_secs_f64()
            }
            _ => 0.0,
        }
    }

    fn lines(&self) -> Vec<(String, String)> {
        let mut lines = Vec::new();
        if let Some(error) = &self.error {
            lines.push(("Status".into(), error.clone()));
        }
        let Some(report) = &self.latest else {
            lines.push(("Status".into(), "Waiting for the driver...".into()));
            return lines;
        };
        let text = |value: &Option<String>| value.clone().unwrap_or_else(|| "none".into());
        lines.push(("Tablet".into(), text(&report.tablet)));
        lines.push((
            "Parser".into(),
            report.parser.as_deref().map_or("none".into(), |parser| {
                parser.rsplit('.').next().unwrap_or(parser).into()
            }),
        ));
        lines.push(("Reports".into(), format!("{:.0} per second", self.rate())));
        lines.push(("Raw".into(), spaced_hex(&report.raw_hex)));
        match report
            .values
            .get("values")
            .and_then(|values| values.as_object())
        {
            Some(values) => {
                if let Some(kind) = report.values.get("kind").and_then(|kind| kind.as_str()) {
                    lines.push(("Kind".into(), label(kind)));
                }
                for (key, value) in values {
                    if !value.is_null() {
                        lines.push((label(key), show(value)));
                    }
                }
            }
            None => {
                if let Some(error) = report.values.get("error") {
                    lines.push(("Decode error".into(), error.to_string()));
                }
            }
        }
        lines
    }

    fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.window, &mut ps) };
        let client = client_rect(self.window);
        let style = with_look(|look| look.style);
        if let (Some(style), Some(mut canvas)) = (style, canvas::Canvas::new(hdc, client)) {
            self.draw(&mut canvas, client, &style);
            canvas.present(hdc);
        }
        unsafe { EndPaint(self.window, &ps) };
    }

    fn draw(&self, canvas: &mut canvas::Canvas, client: RECT, style: &draw::Style) {
        let p = style.palette;
        let fonts = style.fonts;
        let s = |value: f32| style.ipx(value);
        canvas.fill(client, p.page);
        let diagram_width = ((client.right - client.left) * 2 / 5).max(s(160.0));
        let text_right = client.right - diagram_width - s(16.0);
        let line = s(22.0);
        let mut top = client.top + s(12.0);
        for (label, value) in self.lines() {
            let label_rect = rect(
                client.left + s(12.0),
                top,
                client.left + s(140.0),
                top + line,
            );
            canvas.text(
                label_rect,
                &label,
                fonts.bold,
                p.text,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
            );
            let value_rect = rect(client.left + s(144.0), top, text_right, top + line * 4);
            let height =
                canvas.wrapped_height(fonts.mono, &value, value_rect.right - value_rect.left);
            canvas.text(
                rect(
                    value_rect.left,
                    top,
                    value_rect.right,
                    top + height.max(line),
                ),
                &value,
                fonts.mono,
                p.text,
                DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
            );
            top += height.max(line) + s(2.0);
            if top > client.bottom {
                break;
            }
        }
        self.draw_tablet(
            canvas,
            rect(
                text_right + s(8.0),
                client.top + s(12.0),
                client.right - s(12.0),
                client.bottom - s(12.0),
            ),
            style,
        );
    }

    /// The pen on an outline of the tablet's active area, with a pressure bar.
    fn draw_tablet(&self, canvas: &mut canvas::Canvas, area: RECT, style: &draw::Style) {
        let p = style.palette;
        let Some((_, spec)) = &self.spec else {
            return;
        };
        let bar = style.ipx(18.0);
        let width = f64::from(area.right - area.left);
        let height = f64::from(area.bottom - area.top - bar * 2);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let aspect = spec.width_mm / spec.height_mm;
        let (w, h) = if width / height > aspect {
            (height * aspect, height)
        } else {
            (width, width / aspect)
        };
        let outline = rect(
            area.left,
            area.top,
            area.left + w as i32,
            area.top + h as i32,
        );
        canvas.round_rect(
            outline,
            [style.px(4.0); 4],
            Some(p.bounds_fill),
            Some((p.bounds_border, 1.0)),
        );
        let values = self
            .latest
            .as_ref()
            .and_then(|report| report.values.get("values"));
        let number = |key: &str, index: usize| {
            values
                .and_then(|values| values.get(key))
                .and_then(|value| value.get(index))
                .and_then(serde_json::Value::as_f64)
        };
        if let (Some(x), Some(y)) = (number("position", 0), number("position", 1)) {
            let px =
                outline.left as f32 + (x / f64::from(spec.max_x)).clamp(0.0, 1.0) as f32 * w as f32;
            let py =
                outline.top as f32 + (y / f64::from(spec.max_y)).clamp(0.0, 1.0) as f32 * h as f32;
            canvas.circle((px, py), style.px(5.0), Some(p.accent), None);
        }
        let pressure = values
            .and_then(|values| values.get("pressure"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let fraction = (pressure / f64::from(spec.max_pressure.max(1))).clamp(0.0, 1.0);
        let track = rect(
            outline.left,
            outline.bottom + bar / 2,
            outline.right,
            outline.bottom + bar + bar / 2,
        );
        canvas.round_rect(
            track,
            [style.px(3.0); 4],
            Some(p.field),
            Some((p.field_border, 1.0)),
        );
        let filled = rect(
            track.left,
            track.top,
            track.left + ((track.right - track.left) as f64 * fraction) as i32,
            track.bottom,
        );
        if filled.right > filled.left {
            canvas.round_rect(filled, [style.px(3.0); 4], Some(p.accent), None);
        }
        canvas.text(
            rect(
                track.left,
                track.bottom + style.ipx(4.0),
                track.right,
                track.bottom + bar * 2,
            ),
            &format!("Pressure {pressure:.0} / {}", spec.max_pressure),
            style.fonts.small,
            p.muted,
            draw::TEXT_LEFT | DT_NOPREFIX,
        );
    }
}

/// "near_proximity" as "Near proximity".
fn label(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A decoded value for reading: whole numbers without a fraction, lists
/// separated by commas.
fn show(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(number) => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.abs() < 1e15 => format!("{float:.0}"),
            _ => number.to_string(),
        },
        serde_json::Value::Array(items) => items.iter().map(show).collect::<Vec<_>>().join(", "),
        serde_json::Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| format!("{} {}", label(key), show(value)))
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// "a1b2c3" as "a1 b2 c3".
/// Trailing zero bytes are summarized: most packets are padded to the
/// collection's report length.
fn spaced_hex(hex: &str) -> String {
    let bytes: Vec<&str> = (0..hex.len() / 2).map(|i| &hex[i * 2..i * 2 + 2]).collect();
    let used = bytes
        .iter()
        .rposition(|byte| *byte != "00")
        .map_or(0, |last| last + 1);
    let zeros = bytes.len() - used;
    if zeros > 4 {
        format!("{} (+{zeros} zero bytes)", bytes[..used].join(" "))
            .trim_start()
            .to_owned()
    } else {
        bytes.join(" ")
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DEBUG_REPORT => {
            DEBUGGER.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut()
                    && let Some(debugger) = slot.as_mut()
                {
                    debugger.take_updates();
                }
            });
            unsafe { InvalidateRect(window, ptr::null(), 0) };
            0
        }
        WM_PAINT => {
            DEBUGGER.with(|slot| {
                if let Ok(slot) = slot.try_borrow()
                    && let Some(debugger) = slot.as_ref()
                {
                    debugger.paint();
                }
            });
            0
        }
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            DEBUGGER.with(|slot| {
                if let Some(debugger) = slot.borrow_mut().take() {
                    debugger.cancelled.store(true, Ordering::Release);
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
    fn hex_is_spaced_by_byte() {
        assert_eq!(spaced_hex("0a1bff"), "0a 1b ff");
        assert_eq!(spaced_hex("1364000000000000"), "13 64 (+6 zero bytes)");
        assert_eq!(spaced_hex("0a0000"), "0a 00 00");
        assert_eq!(label("near_proximity"), "Near proximity");
        assert_eq!(show(&serde_json::json!([25981.0, 3939.5])), "25981, 3939.5");
        assert_eq!(spaced_hex(""), "");
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Renders the debugger with a sample PTH-660 report to
    /// $OTD_PREVIEW_DIR/debugger.bmp without opening a window.
    #[test]
    #[ignore = "writes a preview image; set OTD_PREVIEW_DIR"]
    fn render_debugger_preview() {
        let directory = std::env::var_os("OTD_PREVIEW_DIR").expect("OTD_PREVIEW_DIR");
        let fonts = FontSet::new(96);
        let style = draw::Style {
            palette: theme::Palette::light(),
            fonts: fonts.fonts,
            scale: 1.0,
        };
        let (_, updates) = mpsc::sync_channel(1);
        let mut debugger = Debugger {
            window: ptr::null_mut(),
            updates,
            cancelled: Arc::new(AtomicBool::new(false)),
            latest: Some(DebugReport {
                tablet: Some("Wacom PTH-660".into()),
                parser: Some(
                    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser"
                        .into(),
                ),
                sequence: 1000,
                raw_hex: "10617d6500630f009319f8070000000013f2ae8025020811".into(),
                values: serde_json::json!({"kind": "data", "values": {
                    "position": [25981.0, 3939.0], "pressure": 6547, "tilt": [-8.0, 7.0],
                    "eraser": false, "near_proximity": true, "pen_buttons": [false, false]}}),
            }),
            error: None,
            history: VecDeque::new(),
            spec: Some(("Wacom PTH-660".into(), TabletSpec::PTH_660)),
        };
        let now = Instant::now();
        debugger
            .history
            .push_back((now - Duration::from_millis(500), 500));
        debugger.history.push_back((now, 1000));
        let area = rect(0, 0, 760, 520);
        let screen = unsafe { GetDC(ptr::null_mut()) };
        let mut canvas = canvas::Canvas::new(screen, area).unwrap();
        debugger.draw(&mut canvas, area, &style);
        std::fs::write(
            std::path::Path::new(&directory).join("debugger.bmp"),
            canvas.to_bmp(),
        )
        .unwrap();
        unsafe { ReleaseDC(ptr::null_mut(), screen) };
    }
}
