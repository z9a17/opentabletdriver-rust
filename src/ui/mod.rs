//! Native Windows control panel laid out like OpenTabletDriver's UX: a menu
//! bar, Output / Filters / Pen Settings / Console tabs with graphical area
//! editors, and a Save / Apply bar. It follows the Windows light or dark
//! setting unless the user picks one, and uses system colors in high
//! contrast. The message loop stays off the driver thread; stop requests use
//! a duplicated Windows event handle.
//!
//! Like OpenTabletDriver's UX, which starts its daemon, the panel starts the
//! driver when it opens; it runs in-process on a background thread. The panel
//! keeps an icon in the notification area and minimizes into it. Opening the
//! panel again brings the running one forward.
//!
//! Child controls are real Win32 controls (keyboard focus, accessibility)
//! painted through custom draw. Their painting reads the `LOOK` state, which
//! is only ever borrowed briefly, so controls that repaint synchronously
//! while an `App` handler runs still get the right colors.
mod app;
mod area;
mod canvas;
mod commands;
mod draw;
mod layout;
mod model;
mod paint;
mod theme;
mod tray;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::Write;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HWND, LPARAM,
    LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetStartupInfoW, OpenMutexW, STARTF_USESHOWWINDOW, STARTUPINFOW,
    SYNCHRONIZATION_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, CDDS_PREPAINT, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED,
    CDIS_SHOWKEYBOARDCUES, CDRF_DODEFAULT, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, EM_SETCUEBANNER,
    EM_SETMARGINS, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, MEASUREITEMSTRUCT, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, ODS_FOCUS,
    ODS_SELECTED, ODT_LISTBOX, TBS_BOTH, TBS_HORZ, TBS_NOTICKS, TTF_IDISHWND, TTF_SUBCLASS,
    TTM_ADDTOOLW, TTM_DELTOOLW, TTM_NEWTOOLRECTW, TTM_SETMAXTIPWIDTH, TTM_UPDATETIPTEXTW,
    TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    GetSystemMetricsForDpi, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, ReleaseCapture, SetCapture, SetFocus,
    VK_CONTROL, VK_F1, VK_NEXT, VK_PRIOR, VK_RETURN, VK_TAB,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::Profile;
use crate::display::DisplaySnapshot;
use crate::dotnet::FilterMetadata;
use crate::hid::{Event, OwnedHandle};
use crate::mapping::{OtdArea, OtdMapping};
use crate::plugins::{PluginConfig, PluginKind};
use crate::radial_follow::RadialFollowSettings;
use area::AreaView;
use commands::{
    accelerator_table, append, checked, confirm_discard, copy_to_clipboard, on_command,
};
use draw::{Glyph, State, Style};
use model::{Align, AspectSource, Bounds, Editor, FilterRef, OutputMode, PropertyValue};
use paint::{ctl_color, custom_draw, draw_item};
use theme::{Palette, Rgb, ThemeMode, UiPrefs};

// Menu bar, tabs and commands. Button controls use their command's ID.
const ID_MENU: u16 = 100;
const ID_TAB: u16 = 110;
const CMD_LOAD: u16 = 200;
const CMD_SAVE: u16 = 201;
const CMD_SAVE_AS: u16 = 202;
const CMD_RESET: u16 = 203;
const CMD_APPLY: u16 = 204;
const CMD_IMPORT: u16 = 205;
const CMD_OPEN_FOLDER: u16 = 206;
const CMD_QUIT: u16 = 207;
const CMD_DETECT: u16 = 210;
const CMD_ADD_DOTNET: u16 = 220;
const CMD_ADD_NATIVE: u16 = 221;
const CMD_REMOVE_FILTER: u16 = 222;
const CMD_FILTER_UP: u16 = 223;
const CMD_FILTER_DOWN: u16 = 224;
const CMD_FILTER_DEFAULTS: u16 = 225;
const CMD_THEME_SYSTEM: u16 = 230;
const CMD_THEME_LIGHT: u16 = 231;
const CMD_THEME_DARK: u16 = 232;
const CMD_DOCS: u16 = 240;
const CMD_ABOUT: u16 = 241;
const CMD_NEXT_TAB: u16 = 250;
const CMD_PREV_TAB: u16 = 251;
const CMD_START_STOP: u16 = 260;
const CMD_COPY_LOG: u16 = 261;
const CMD_CLEAR_LOG: u16 = 262;
const CMD_AUTOSTART: u16 = 263;
const CMD_SHOW: u16 = 270;
const ID_MODE: u16 = 300;
const ID_DISPLAY: u16 = 310;
const ID_TABLET: u16 = 320;
const ID_RELATIVE: u16 = 330;
const ID_FILTER_LIST: u16 = 400;
const ID_FILTER_ENABLE: u16 = 401;
const ID_FILTER_JSON: u16 = 402;
const ID_PROPERTY: u16 = 2000;
const MAX_PROPERTY_ROWS: u16 = 1000;
const ID_TIP_BINDING: u16 = 500;
const ID_TIP_SLIDER: u16 = 501;
const ID_TIP_FIELD: u16 = 502;
const ID_ERASER_BINDING: u16 = 510;
const ID_ERASER_SLIDER: u16 = 511;
const ID_ERASER_FIELD: u16 = 512;
const ID_LOG: u16 = 600;
// Area context menu.
const AREA_ALIGN: u16 = 800;
const AREA_FULL: u16 = 810;
const AREA_QUARTER: u16 = 811;
const AREA_FLIP_H: u16 = 820;
const AREA_FLIP_V: u16 = 821;
const AREA_HANDEDNESS: u16 = 822;
const AREA_LOCK_USABLE: u16 = 830;
const AREA_LOCK_ASPECT: u16 = 831;
const AREA_CLIPPING: u16 = 832;
const AREA_LIMITING: u16 = 833;
const AREA_DISPLAY: u16 = 840;

const WM_DRIVER_STATUS: u32 = WM_APP + 1;
const WM_DRIVER_EXITED: u32 = WM_APP + 2;
const WM_DETECT: u32 = WM_APP + 3;
const WM_AUTOSTART: u32 = WM_APP + 4;
/// Posted by a second launch of the panel.
const WM_SHOW_PANEL: u32 = WM_APP + 5;
const WM_TRAY: u32 = WM_APP + 6;

const PANEL_CLASS: &str = "OpenTabletDriverRustControlPanel";
const PANEL_MUTEX: &str = "Local\\OpenTabletDriverRustPanel";

const TBM_GETPOS: u32 = WM_USER;
const TBM_SETPOS: u32 = WM_USER + 5;
const TBM_SETRANGE: u32 = WM_USER + 6;
const TBM_SETPAGESIZE: u32 = WM_USER + 21;
const TBM_GETTHUMBRECT: u32 = WM_USER + 25;
const TBM_GETCHANNELRECT: u32 = WM_USER + 26;
const CF_UNICODETEXT: u32 = 13;
const EM_LIMITTEXT: u32 = 0x00C5;
const SS_LEFT: u32 = 0;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_ENDELLIPSIS: u32 = 0x4000;

const DISPLAY_FIELDS: [(&str, &str); 4] =
    [("Width", "px"), ("Height", "px"), ("X", "px"), ("Y", "px")];
const TABLET_FIELDS: [(&str, &str); 5] = [
    ("Width", "mm"),
    ("Height", "mm"),
    ("X", "mm"),
    ("Y", "mm"),
    ("Rotation", "°"),
];
const RELATIVE_FIELDS: [(&str, &str); 4] = [
    ("X Sensitivity", "px/mm"),
    ("Y Sensitivity", "px/mm"),
    ("Rotation", "°"),
    ("Reset Time", "ms"),
];
const MENUS: [&str; 5] = ["&File", "&Tablets", "&Plugins", "&View", "&Help"];
const DOCS_URL: &str = "https://github.com/z9a17/opentabletdriver-rust#readme";
const TABLET_NAME: &str = "Wacom PTH-660";
const LOG_LIMIT: usize = 1_000;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static LOOK: RefCell<Option<Look>> = const { RefCell::new(None) };
    static QUIET: Cell<u32> = const { Cell::new(0) };
    static LAST_FOCUS: Cell<isize> = const { Cell::new(0) };
    /// Explorer's "TaskbarCreated" message; its tray starts empty.
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// UTF-16 without the terminator, for length-counted APIs.
pub(crate) fn wide_text(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn text(hwnd: HWND) -> String {
    let count = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut buffer = vec![0; count + 1];
    let read =
        unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..read])
}

/// Sets control text without handling the resulting change notification.
fn set_text(hwnd: HWND, value: &str) {
    if text(hwnd) == value {
        return;
    }
    QUIET.with(|quiet| quiet.set(quiet.get() + 1));
    unsafe { SetWindowTextW(hwnd, wide(value).as_ptr()) };
    QUIET.with(|quiet| quiet.set(quiet.get() - 1));
}

fn scale(value: i32, dpi: u32) -> i32 {
    (i64::from(value) * i64::from(dpi) / 96) as i32
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) };
    rect
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        slot.as_mut().map(f)
    })
}

fn with_look<R>(f: impl FnOnce(&Look) -> R) -> Option<R> {
    LOOK.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        slot.as_ref().map(f)
    })
}

/// Look updates must not call Win32: controls may be painting.
fn update_look(f: impl FnOnce(&mut Look)) {
    LOOK.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut()
            && let Some(look) = slot.as_mut()
        {
            f(look);
        }
    });
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(".otd-profile-{}-{stamp}.tmp", std::process::id()));
    let outcome = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(contents)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        let from = crate::plugins::wide(temporary.as_os_str())?;
        let to = crate::plugins::wide(path.as_os_str())?;
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome
}

pub fn save_profile(path: &Path, profile: &Profile) -> Result<(), String> {
    write_atomic(path, profile.to_toml()?.as_bytes())
}

pub(crate) fn create_font(pixels: i32, weight: i32, face: &str, escapement: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -pixels,
            0,
            escapement,
            escapement,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.into(),
            OUT_DEFAULT_PRECIS.into(),
            CLIP_DEFAULT_PRECIS.into(),
            CLEARTYPE_QUALITY.into(),
            DEFAULT_PITCH.into(),
            wide(face).as_ptr(),
        )
    }
}

/// Font sizes follow OpenTabletDriver: 9 pt text, bold 9 pt group titles and
/// 8 pt area labels.
struct FontSet {
    fonts: draw::Fonts,
    small_pixels: i32,
}

impl FontSet {
    fn new(dpi: u32) -> Self {
        let points = |pt: i32| (pt * dpi as i32 + 36) / 72;
        let small_pixels = points(8);
        Self {
            fonts: draw::Fonts {
                ui: create_font(points(9), 400, "Segoe UI", 0),
                bold: create_font(points(9), 600, "Segoe UI", 0),
                small: create_font(small_pixels, 400, "Segoe UI", 0),
                mono: create_font(points(9), 400, "Consolas", 0),
            },
            small_pixels,
        }
    }
}

impl Drop for FontSet {
    fn drop(&mut self) {
        let f = self.fonts;
        for font in [f.ui, f.bold, f.small, f.mono] {
            unsafe { DeleteObject(font) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Output,
    Filters,
    Pen,
    Console,
}

const TABS: [(Tab, &str); 4] = [
    (Tab::Output, "Output"),
    (Tab::Filters, "Filters"),
    (Tab::Pen, "Pen Settings"),
    (Tab::Console, "Console"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Window,
    Page,
    Group,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Menu,
    Tab(Tab),
    Button,
    StartStop,
    Dropdown,
    Check,
    Field,
    Memo,
    List,
    Log,
    Slider,
    Label,
}

#[derive(Clone, Copy, Debug)]
struct ControlInfo {
    kind: Kind,
    surface: Surface,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Level {
    #[default]
    Info,
    Warning,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "Info",
            Level::Warning => "Warning",
            Level::Error => "Error",
        }
    }
}

struct LogEntry {
    time: String,
    level: Level,
    group: &'static str,
    message: String,
}

/// Everything child-control painting needs.
struct Look {
    style: Style,
    controls: HashMap<isize, ControlInfo>,
    tab: Tab,
    menu_open: isize,
    running: bool,
    filters: Vec<model::FilterItem>,
    log: VecDeque<LogEntry>,
    log_columns: [i32; 3],
    brushes: RefCell<Vec<(u32, HBRUSH)>>,
}

impl Look {
    fn surface(&self, surface: Surface) -> Rgb {
        let p = &self.style.palette;
        match surface {
            Surface::Window => p.window,
            Surface::Page => p.page,
            Surface::Group => p.group,
        }
    }

    fn brush(&self, color: Rgb) -> HBRUSH {
        let key = color.colorref();
        let mut brushes = self.brushes.borrow_mut();
        if let Some((_, brush)) = brushes.iter().find(|(k, _)| *k == key) {
            return *brush;
        }
        let brush = unsafe { CreateSolidBrush(key) };
        brushes.push((key, brush));
        brush
    }

    fn info(&self, hwnd: HWND) -> Option<ControlInfo> {
        self.controls.get(&(hwnd as isize)).copied()
    }
}

impl Drop for Look {
    fn drop(&mut self) {
        for (_, brush) in self.brushes.get_mut().drain(..) {
            unsafe { DeleteObject(brush) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AreaKind {
    Display,
    Tablet,
}

impl AreaKind {
    fn area(self, mapping: &OtdMapping) -> OtdArea {
        match self {
            AreaKind::Display => mapping.display,
            AreaKind::Tablet => mapping.tablet,
        }
    }

    fn area_mut(self, mapping: &mut OtdMapping) -> &mut OtdArea {
        match self {
            AreaKind::Display => &mut mapping.display,
            AreaKind::Tablet => &mut mapping.tablet,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Text,
    Muted,
    Warning,
    Error,
}

/// Display list built by `layout` and drawn by `paint`.
enum Item {
    Page(RECT),
    Title(RECT, String),
    Group(RECT),
    Unit(RECT),
    Label(RECT, String, Tone, u32),
    Frame(RECT, HWND),
    Area(RECT, AreaKind),
    Status(RECT),
    Header(RECT),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RadialField {
    Outer,
    Inner,
    Coefficient,
    Knee,
    Leak,
}

// Names, units and tool tips from AbstractQbit's RadialFollow 0.3.0 plugin
// (GPL-3.0-only), which the built-in filter ports.
const RADIAL_FIELDS: [(RadialField, &str, &str, &str); 5] = [
    (
        RadialField::Outer,
        "Outer Radius",
        "mm",
        "Outer radius defines the max distance the cursor can lag behind the actual reading.\n\nUnit of measurement is millimetres.\nThe value should be >= 0 and inner radius.\nIf smoothing leak is used, defines the point at which smoothing will be reduced,\ninstead of hard clamping the max distance between the tablet position and a cursor.\n\nDefault value is 1.0 mm",
    ),
    (
        RadialField::Inner,
        "Inner Radius",
        "mm",
        "Inner radius defines the max distance the tablet reading can deviate from the cursor without moving it.\nThis effectively creates a deadzone in which no movement is produced.\n\nUnit of measurement is millimetres.\nThe value should be >= 0 and <= outer radius.\n\nDefault value is 0.0 mm",
    ),
    (
        RadialField::Coefficient,
        "Initial Smoothing Coefficient",
        "",
        "Smoothing coefficient determines how fast or slow the cursor will descend from the outer radius to the inner.\n\nPossible value range is 0.0001..1, higher values mean more smoothing (slower descent to the inner radius).\n\nDefault value is 0.95",
    ),
    (
        RadialField::Knee,
        "Soft Knee Scale",
        "",
        "Soft knee scale determines how soft the transition between smoothing inside and outside the outer radius is.\n\nPossible value range is 0..100, higher values mean softer transition.\nThe effect is somewhat logarithmic, i.e. most of the change happens closer to zero.\n\nDefault value is 1.0",
    ),
    (
        RadialField::Leak,
        "Smoothing Leak Coefficient",
        "",
        "Smoothing leak coefficient allows for input smooting to continue past outer radius at a reduced rate.\n\nPossible value range is 0..1, 0 means no smoothing past outer radius, 1 means 100% of the smoothing gets through.\n\nDefault value is 0.0",
    ),
];

impl RadialField {
    fn get(self, s: &RadialFollowSettings) -> f64 {
        match self {
            RadialField::Outer => s.outer_radius,
            RadialField::Inner => s.inner_radius,
            RadialField::Coefficient => s.smoothing_coefficient,
            RadialField::Knee => s.soft_knee_scale,
            RadialField::Leak => s.smoothing_leak_coefficient,
        }
    }

    fn set(self, s: &mut RadialFollowSettings, value: f64) {
        match self {
            RadialField::Outer => s.outer_radius = value,
            RadialField::Inner => s.inner_radius = value,
            RadialField::Coefficient => s.smoothing_coefficient = value,
            RadialField::Knee => s.soft_knee_scale = value,
            RadialField::Leak => s.smoothing_leak_coefficient = value,
        }
    }
}

enum PropertyTarget {
    Radial(RadialField),
    Plugin(String, PropertyValue),
}

struct PropertyRow {
    hwnd: HWND,
    /// Static text before the field, which also names it for screen readers.
    label_control: Option<HWND>,
    label: String,
    unit: String,
    target: PropertyTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriverState {
    Stopped,
    Starting,
    Waiting,
    Connecting,
    Connected,
    Stopping,
    Failed,
}

impl DriverState {
    fn label(self) -> &'static str {
        match self {
            DriverState::Stopped => "Stopped",
            DriverState::Starting => "Starting",
            DriverState::Waiting => "Waiting for tablet",
            DriverState::Connecting => "Connecting",
            DriverState::Connected => "Running",
            DriverState::Stopping => "Stopping",
            DriverState::Failed => "Stopped with an error",
        }
    }
}

struct Running {
    stop: Event,
    thread: JoinHandle<Result<(), String>>,
    messages: Receiver<String>,
}

struct Drag {
    which: AreaKind,
    start: (i32, i32),
    center: (f64, f64),
}

/// Native control handles, grouped by page.
struct Controls {
    menus: Vec<HWND>,
    tabs: Vec<HWND>,
    mode: HWND,
    display: [HWND; 4],
    tablet: [HWND; 5],
    relative: [HWND; 4],
    filter_list: HWND,
    add_dotnet: HWND,
    add_native: HWND,
    remove_filter: HWND,
    filter_up: HWND,
    filter_down: HWND,
    filter_defaults: HWND,
    filter_enable: HWND,
    filter_json: HWND,
    tip_binding: HWND,
    tip_slider: HWND,
    tip_field: HWND,
    eraser_binding: HWND,
    eraser_slider: HWND,
    eraser_field: HWND,
    log: HWND,
    copy_log: HWND,
    clear_log: HWND,
    start: HWND,
    save: HWND,
    apply: HWND,
}

struct App {
    hwnd: HWND,
    dpi: u32,
    fonts: FontSet,
    dark_mode: theme::DarkMode,
    prefs: UiPrefs,
    prefs_path: PathBuf,
    process_dpi: isize,
    tooltip: HWND,
    accelerators: HACCEL,
    c: Controls,
    tab: Tab,
    items: Vec<Item>,
    display_view: Option<AreaView>,
    tablet_view: Option<AreaView>,
    displays: DisplaySnapshot,
    editor: Editor,
    profile_path: PathBuf,
    dirty: bool,
    selected_filter: usize,
    properties: Vec<PropertyRow>,
    /// Discovery results are UI-only; they are never written into profiles.
    plugin_metadata: HashMap<PathBuf, Result<Vec<FilterMetadata>, String>>,
    /// Label controls keyed by the control they name.
    labels: HashMap<isize, HWND>,
    json_visible: bool,
    json_error: Option<String>,
    invalid: HashSet<isize>,
    drag: Option<Drag>,
    context_area: AreaKind,
    running: Option<Running>,
    restart: Option<Profile>,
    closing: bool,
    driver: DriverState,
    tablet_present: Option<bool>,
    status: String,
    status_level: Level,
    validation_status: bool,
    tray_icon: HICON,
    /// Whether the notification area has the icon; without it minimizing
    /// keeps the taskbar button.
    in_tray: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        // Normal close waits asynchronously for this thread. This also covers
        // a message-loop failure without force-killing input or losing releases.
        if let Some(running) = self.running.take() {
            let _ = running.stop.signal();
            let _ = running.thread.join();
        }
        unsafe {
            DestroyAcceleratorTable(self.accelerators);
            if !self.tray_icon.is_null() {
                DestroyIcon(self.tray_icon);
            }
        }
    }
}

/// Monitors as the driver thread sees them. The panel itself is per-monitor
/// DPI aware, but the driver keeps the process default so its mapping stays
/// identical to the console daemon's.
fn displays_for_driver(process_dpi: isize) -> Result<DisplaySnapshot, String> {
    unsafe {
        let previous = SetThreadDpiAwarenessContext(process_dpi as DPI_AWARENESS_CONTEXT);
        let snapshot = crate::display::read_snapshot();
        if !previous.is_null() {
            SetThreadDpiAwarenessContext(previous);
        }
        snapshot
    }
}

fn fallback_displays() -> DisplaySnapshot {
    let screen = crate::mapping::Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    DisplaySnapshot {
        virtual_screen: screen,
        monitors: vec![screen],
    }
}

/// Compares a NUL-terminated UTF-16 string with `expected`.
fn wide_equals(value: *const u16, expected: &str) -> bool {
    let mut offset = 0;
    for unit in expected.encode_utf16() {
        if unsafe { *value.add(offset) } != unit {
            return false;
        }
        offset += 1;
    }
    unsafe { *value.add(offset) == 0 }
}

fn local_time() -> String {
    let mut time = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut time) };
    format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond)
}

fn profile_directory() -> Result<PathBuf, String> {
    Ok(std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?)
        .join("OpenTabletDriverRust"))
}

fn point_from(lp: LPARAM) -> (i32, i32) {
    (
        (lp & 0xFFFF) as i16 as i32,
        ((lp >> 16) & 0xFFFF) as i16 as i32,
    )
}

/// True while another process, such as the console daemon, runs the driver.
fn driver_instance_running() -> bool {
    let name = wide(crate::INSTANCE_MUTEX);
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        // An elevated driver's mutex exists but cannot be opened.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    unsafe { CloseHandle(handle) };
    true
}

/// Holds the panel's instance mutex, or brings the panel that already holds
/// it forward and returns `None`.
fn claim_panel() -> Result<Option<OwnedHandle>, String> {
    let name = wide(PANEL_MUTEX);
    let raw = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let handle = OwnedHandle::new(raw).map_err(|e| format!("panel instance guard failed: {e}"))?;
    if !exists {
        return Ok(Some(handle));
    }
    let class = wide(PANEL_CLASS);
    // The other panel may still be creating its window.
    for _ in 0..20 {
        let window = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
        if !window.is_null() {
            let mut process = 0;
            unsafe {
                GetWindowThreadProcessId(window, &mut process);
                AllowSetForegroundWindow(process);
                PostMessageW(window, WM_SHOW_PANEL, 0, 0);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(None)
}

/// Shows the new window. A launch that asks for a minimized window, such as
/// a shortcut set to run minimized, starts the panel in the tray. Windows
/// substitutes the launch's show command only for a plain `SW_SHOW`, so a
/// panel saved maximized asks for it here and still restores maximized.
fn show_initial(window: HWND, maximized: bool) {
    let mut startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    unsafe { GetStartupInfoW(&mut startup) };
    let minimized = startup.dwFlags & STARTF_USESHOWWINDOW != 0
        && [
            SW_MINIMIZE,
            SW_SHOWMINIMIZED,
            SW_SHOWMINNOACTIVE,
            SW_FORCEMINIMIZE,
        ]
        .contains(&i32::from(startup.wShowWindow));
    if !(minimized && maximized) {
        unsafe { ShowWindow(window, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOW }) };
        return;
    }
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe {
        GetWindowPlacement(window, &mut placement);
        placement.showCmd = SW_SHOWMINNOACTIVE as u32;
        placement.flags |= WPF_RESTORETOMAXIMIZED;
        SetWindowPlacement(window, &placement);
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    let default = || unsafe { DefWindowProcW(window, message, wp, lp) };
    match message {
        WM_DESTROY => {
            tray::remove(window);
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if with_app(App::paint).is_none() {
                return default();
            }
            0
        }
        WM_SIZE => {
            if wp as u32 == SIZE_MINIMIZED {
                // Upstream hides a minimized window into the tray.
                if with_app(|app| app.in_tray).unwrap_or(false) {
                    unsafe { ShowWindow(window, SW_HIDE) };
                }
                return 0;
            }
            with_app(App::layout);
            0
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(window) }.max(96);
            let mut frame = rect(0, 0, scale(820, dpi), scale(600, dpi));
            unsafe { AdjustWindowRectExForDpi(&mut frame, WS_OVERLAPPEDWINDOW, 0, 0, dpi) };
            let info = unsafe { &mut *(lp as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT {
                x: frame.right - frame.left,
                y: frame.bottom - frame.top,
            };
            0
        }
        WM_DPICHANGED => {
            let dpi = (wp & 0xFFFF) as u32;
            with_app(|app| app.set_dpi(dpi));
            let r = unsafe { &*(lp as *const RECT) };
            unsafe {
                SetWindowPos(
                    window,
                    ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            with_app(App::layout);
            0
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => {
            // Windows broadcasts many setting changes; only the app color mode,
            // high contrast and system colors affect the palette.
            let area = lp as *const u16;
            let relevant = message == WM_SYSCOLORCHANGE
                || wp == SPI_SETHIGHCONTRAST as usize
                || (!area.is_null() && wide_equals(area, "ImmersiveColorSet"));
            if relevant {
                with_app(App::apply_theme);
            }
            default()
        }
        WM_DISPLAYCHANGE => {
            with_app(|app| {
                if let Ok(displays) = displays_for_driver(app.process_dpi) {
                    app.displays = displays;
                    app.sync_areas(None);
                    app.layout();
                }
            });
            default()
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
            ctl_color(wp as HDC, lp as HWND).unwrap_or_else(default)
        }
        WM_NOTIFY => {
            let header = unsafe { &*(lp as *const NMHDR) };
            if header.code == NM_CUSTOMDRAW {
                return custom_draw(unsafe { &mut *(lp as *mut NMCUSTOMDRAW) });
            }
            default()
        }
        WM_DRAWITEM => {
            draw_item(unsafe { &*(lp as *const DRAWITEMSTRUCT) });
            1
        }
        WM_MEASUREITEM => {
            let item = unsafe { &mut *(lp as *mut MEASUREITEMSTRUCT) };
            let dpi = unsafe { GetDpiForWindow(window) }.max(96);
            item.itemHeight = scale(
                if item.CtlID == u32::from(ID_LOG) {
                    22
                } else {
                    44
                },
                dpi,
            ) as u32;
            1
        }
        WM_COMMAND => {
            on_command(
                window,
                (wp & 0xFFFF) as u16,
                ((wp >> 16) & 0xFFFF) as u32,
                lp as HWND,
            );
            0
        }
        WM_HSCROLL => {
            let slider = lp as HWND;
            with_app(|app| app.slider_moved(slider));
            0
        }
        WM_VKEYTOITEM => {
            let key = (wp & 0xFFFF) as u16;
            let control = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
            let is_log = with_app(|app| app.c.log == lp as HWND).unwrap_or(false);
            if is_log && control && key == u16::from(b'C') {
                with_app(|app| app.copy_log(false));
                return -2;
            }
            if is_log && control && key == u16::from(b'A') {
                unsafe { SendMessageW(lp as HWND, LB_SETSEL, 1, -1) };
                return -2;
            }
            -1
        }
        WM_LBUTTONDOWN => {
            if with_app(|app| app.mouse_down(point_from(lp))) == Some(true) {
                unsafe { SetCapture(window) };
            }
            0
        }
        WM_MOUSEMOVE => {
            with_app(|app| app.mouse_move(point_from(lp)));
            0
        }
        WM_LBUTTONUP => {
            if with_app(|app| app.drag.take().is_some()) == Some(true) {
                unsafe { ReleaseCapture() };
            }
            0
        }
        WM_CAPTURECHANGED => {
            with_app(|app| app.drag = None);
            0
        }
        WM_SETCURSOR => {
            let mut point = POINT::default();
            unsafe {
                GetCursorPos(&mut point);
                ScreenToClient(window, &mut point);
            }
            let over_area = (lp & 0xFFFF) as u32 == HTCLIENT
                && with_app(|app| {
                    app.drag.is_some()
                        || app.area_at((point.x, point.y)).is_some_and(|(_, hit)| hit)
                })
                .unwrap_or(false);
            if over_area {
                unsafe { SetCursor(LoadCursorW(ptr::null_mut(), IDC_SIZEALL)) };
                return 1;
            }
            default()
        }
        WM_CONTEXTMENU => {
            let mut point = POINT {
                x: point_from(lp).0,
                y: point_from(lp).1,
            };
            unsafe { ScreenToClient(window, &mut point) };
            if let Some(Some(menu)) = with_app(|app| app.area_menu((point.x, point.y))) {
                let (x, y) = point_from(lp);
                let command = unsafe {
                    TrackPopupMenuEx(
                        menu,
                        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
                        x,
                        y,
                        window,
                        ptr::null(),
                    )
                };
                unsafe { DestroyMenu(menu) };
                if command != 0 {
                    on_command(window, command as u16, 0, ptr::null_mut());
                }
                return 0;
            }
            default()
        }
        WM_DRIVER_STATUS => {
            with_app(App::driver_status);
            0
        }
        WM_DRIVER_EXITED => {
            if with_app(App::driver_exited) == Some(true) {
                unsafe { DestroyWindow(window) };
            }
            0
        }
        WM_DETECT => {
            with_app(App::detect_tablet);
            0
        }
        WM_AUTOSTART => {
            with_app(App::auto_start);
            0
        }
        WM_SHOW_PANEL => {
            tray::show_panel(window);
            0
        }
        WM_TRAY => {
            tray::notify(window, wp, lp);
            0
        }
        WM_ACTIVATE => {
            // Dialog-style focus memory: return to the last focused control.
            if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                let focus = unsafe { GetFocus() };
                if !focus.is_null() && unsafe { IsChild(window, focus) } != 0 {
                    LAST_FOCUS.with(|last| last.set(focus as isize));
                }
            } else {
                let focus = LAST_FOCUS.with(Cell::get) as HWND;
                if !focus.is_null()
                    && unsafe { IsWindow(focus) } != 0
                    && unsafe { IsWindowVisible(focus) } != 0
                {
                    unsafe { SetFocus(focus) };
                    return 0;
                }
            }
            default()
        }
        WM_CLOSE => {
            if with_app(|app| app.closing).unwrap_or(false) {
                return 0;
            }
            // Close from the tray asks about unsaved edits with the panel shown.
            if with_app(|app| app.dirty).unwrap_or(false) && unsafe { IsWindowVisible(window) } == 0
            {
                tray::show_panel(window);
            }
            if !confirm_discard(window) {
                return 0;
            }
            if with_app(App::begin_close).unwrap_or(true) {
                unsafe { DestroyWindow(window) };
            }
            0
        }
        _ if message != 0 && message == TASKBAR_CREATED.with(Cell::get) => {
            let in_tray = with_app(|app| {
                app.add_tray();
                app.in_tray
            });
            // Never leave the panel hidden without an icon to restore it.
            if in_tray == Some(false) && unsafe { IsWindowVisible(window) } == 0 {
                tray::show_panel(window);
            }
            0
        }
        _ => default(),
    }
}

/// Centers the window on the monitor under the cursor, as upstream does.
fn place_window(window: HWND, prefs: &UiPrefs) {
    unsafe {
        let dpi = GetDpiForWindow(window).max(96);
        let (width, height) = prefs.window_size.unwrap_or((960, 760));
        let mut frame = rect(
            0,
            0,
            scale(width.max(820), dpi),
            scale(height.max(600), dpi),
        );
        let style = GetWindowLongW(window, GWL_STYLE) as u32;
        let size_is_client = prefs.window_size.is_none();
        if size_is_client {
            AdjustWindowRectExForDpi(&mut frame, style, 0, 0, dpi);
        }
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let width = (frame.right - frame.left).min(work.right - work.left);
        let height = (frame.bottom - frame.top).min(work.bottom - work.top);
        SetWindowPos(
            window,
            ptr::null_mut(),
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn run() -> Result<(), String> {
    let Some(_panel) = claim_panel()? else {
        return Ok(());
    };
    // The panel renders at the monitor's DPI; the driver thread and display
    // snapshots keep using the process default (see displays_for_driver).
    let process_dpi = unsafe { GetThreadDpiAwarenessContext() } as isize;
    unsafe {
        if SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_null() {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
        }
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_BAR_CLASSES | ICC_TAB_CLASSES,
        };
        InitCommonControlsEx(&controls);
    }
    let directory = profile_directory()?;
    let prefs_path = UiPrefs::path(&directory);
    let prefs = UiPrefs::load(&prefs_path);

    let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    TASKBAR_CREATED.with(|message| message.set(taskbar_created));

    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class_name = wide(PANEL_CLASS);
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let window = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class_name.as_ptr(),
            wide(&format!(
                "OpenTabletDriver Rust v{}",
                env!("CARGO_PKG_VERSION")
            ))
            .as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960,
            760,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let maximized = prefs.maximized;
    place_window(window, &prefs);
    let app = App::create(window, prefs.clone(), prefs_path, process_dpi);
    let mut app = match app {
        Ok(app) => app,
        Err(error) => {
            unsafe { DestroyWindow(window) };
            return Err(error);
        }
    };
    let path = directory.join("driver.toml");
    let (profile, loaded) = if path.exists() {
        match Profile::load(Some(&path)) {
            Ok(profile) => (profile, Ok(format!("Loaded {}.", path.display()))),
            Err(error) => (
                Profile::default(),
                Err(format!("Could not load profile: {error}")),
            ),
        }
    } else {
        match Profile::load(None) {
            Ok(profile) => {
                let message = if profile.source.starts_with("OpenTabletDriver") {
                    "Imported the active OpenTabletDriver mapping. Save writes it as a separate Rust profile."
                } else {
                    "No saved profile; using the built-in full-area defaults."
                };
                (profile, Ok(message.to_owned()))
            }
            Err(error) => (
                Profile::default(),
                Err(format!(
                    "Could not import OpenTabletDriver settings: {error}"
                )),
            ),
        }
    };
    app.replace_profile(profile, Some(path), false);
    app.set_driver_state(DriverState::Stopped);
    // A profile that failed to load is replaced by defaults; never drive
    // the tablet with those unasked.
    let auto_start = prefs.start_driver_on_launch && loaded.is_ok();
    match loaded {
        Ok(message) => app.log(Level::Info, "Settings", message),
        Err(error) => app.log(Level::Error, "Settings", error),
    }
    if !auto_start {
        app.log(
            Level::Info,
            "UI",
            if prefs.start_driver_on_launch {
                "The driver was not started because the settings could not be loaded. Start driver uses the settings shown."
            } else {
                "The driver is stopped. Start driver begins reading the tablet with these settings."
            },
        );
    }
    let accelerators = app.accelerators;
    let first_tab = app.c.tabs[0];
    app.set_icons();
    // Before the window is shown, so a minimized launch goes to the tray.
    app.add_tray();
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    with_app(App::layout);
    show_initial(window, maximized);
    unsafe {
        SetFocus(first_tab);
        PostMessageW(window, WM_DETECT, 0, 0);
        if auto_start {
            PostMessageW(window, WM_AUTOSTART, 0, 0);
        }
    }

    let mut message = MSG::default();
    let result = loop {
        let status = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if status <= 0 {
            break if status < 0 {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                Ok(())
            };
        }
        unsafe {
            if TranslateAcceleratorW(window, accelerators, &message) != 0 {
                continue;
            }
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    };
    APP.with(|slot| drop(slot.borrow_mut().take()));
    LOOK.with(|slot| drop(slot.borrow_mut().take()));
    result
}
