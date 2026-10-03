//! Light, dark and high-contrast palettes, Windows theme detection and the
//! small amount of window theming the native controls need in dark mode.
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::SetWindowTheme;
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};

use super::wide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(value: u32) -> Self {
        Self((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    pub fn colorref(self) -> u32 {
        u32::from(self.0) | (u32::from(self.1) << 8) | (u32::from(self.2) << 16)
    }

    pub fn from_colorref(value: u32) -> Self {
        Self(value as u8, (value >> 8) as u8, (value >> 16) as u8)
    }

    /// Relative luminance (WCAG), 0 for black to 1 for white.
    pub fn luminance(self) -> f32 {
        let linear = |channel: u8| {
            let value = f32::from(channel) / 255.0;
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }

    /// `#RRGGBB`.
    pub fn text(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.trim().strip_prefix('#')?;
        (digits.len() == 6)
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
            .map(Self::hex)
    }

    pub fn mix(self, other: Self, amount: f32) -> Self {
        let channel =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
        Self(
            channel(self.0, other.0),
            channel(self.1, other.1),
            channel(self.2, other.2),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// The color of selection, focus, hot and pressed buttons, tabs, sliders and
/// the area highlight. Blue is the original Windows blue; the others replace
/// every blue of the light and dark palettes. High contrast keeps the system
/// colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Blue,
    Teal,
    Green,
    Gold,
    Orange,
    Red,
    Pink,
    Purple,
    Violet,
    Slate,
    /// The Windows personalization accent color, read when the theme applies.
    Windows,
    Custom(Rgb),
}

impl Accent {
    /// The menu's presets, in order, with their names.
    pub const PRESETS: [(Accent, &str); 10] = [
        (Accent::Blue, "Blue (default)"),
        (Accent::Teal, "Teal"),
        (Accent::Green, "Green"),
        (Accent::Gold, "Gold"),
        (Accent::Orange, "Orange"),
        (Accent::Red, "Red"),
        (Accent::Pink, "Pink"),
        (Accent::Purple, "Purple"),
        (Accent::Violet, "Violet"),
        (Accent::Slate, "Slate"),
    ];

    /// The light palette's accent; the dark palette lightens it.
    pub fn base(self) -> Rgb {
        match self {
            Accent::Blue => Rgb::hex(0x0078D7),
            Accent::Teal => Rgb::hex(0x038387),
            Accent::Green => Rgb::hex(0x107C10),
            Accent::Gold => Rgb::hex(0x986F0B),
            Accent::Orange => Rgb::hex(0xCA5010),
            Accent::Red => Rgb::hex(0xD13438),
            Accent::Pink => Rgb::hex(0xC30052),
            Accent::Purple => Rgb::hex(0x8764B8),
            Accent::Violet => Rgb::hex(0x5C2E91),
            Accent::Slate => Rgb::hex(0x515C6B),
            Accent::Windows => windows_accent().unwrap_or(Rgb::hex(0x0078D7)),
            Accent::Custom(color) => color,
        }
    }

    fn name(self) -> String {
        match self {
            Accent::Windows => "windows".into(),
            Accent::Custom(color) => color.text(),
            preset => format!("{preset:?}").to_ascii_lowercase(),
        }
    }

    fn from_name(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("windows") {
            return Some(Accent::Windows);
        }
        Accent::PRESETS
            .iter()
            .map(|(accent, _)| *accent)
            .find(|accent| accent.name().eq_ignore_ascii_case(text))
            .or_else(|| Rgb::parse(text).map(Accent::Custom))
    }
}

impl Serialize for Accent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for Accent {
    /// An unknown value falls back to blue rather than discarding every
    /// other panel preference.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Accent::from_name(&text).unwrap_or_default())
    }
}

/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`, stored as ABGR.
pub fn windows_accent() -> Option<Rgb> {
    registry_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\DWM",
        "AccentColor",
    )
    .map(|abgr| Rgb::from_colorref(abgr & 0x00FF_FFFF))
}

/// White text, as Windows uses on accents, while it keeps the 3:1 contrast
/// of a UI component on `background`; black otherwise. The original blues
/// get white in the light palette and black in the dark one.
fn text_on(background: Rgb) -> Rgb {
    if 1.05 / (background.luminance() + 0.05) >= 3.0 {
        Rgb::hex(0xFFFFFF)
    } else {
        Rgb::hex(0x000000)
    }
}

/// Colors follow OpenTabletDriver's WPF controls on Windows: gray window,
/// white pages and group boxes, Windows blue selection and area highlight.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub high_contrast: bool,
    pub window: Rgb,
    pub page: Rgb,
    pub group: Rgb,
    pub border: Rgb,
    pub tab_border: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub disabled: Rgb,
    pub button: Rgb,
    pub button_border: Rgb,
    pub button_hot: Rgb,
    pub button_hot_border: Rgb,
    pub button_pressed: Rgb,
    pub button_pressed_border: Rgb,
    pub field: Rgb,
    pub field_border: Rgb,
    pub accent: Rgb,
    pub accent_text: Rgb,
    pub selection: Rgb,
    pub hover: Rgb,
    pub error: Rgb,
    pub warning: Rgb,
    pub success: Rgb,
    pub bounds_fill: Rgb,
    pub bounds_border: Rgb,
    pub area_fill: Rgb,
    pub area_alpha: f32,
    pub area_border: Rgb,
}

impl Palette {
    pub fn light() -> Self {
        Self {
            dark: false,
            high_contrast: false,
            window: Rgb::hex(0xF0F0F0),
            page: Rgb::hex(0xFFFFFF),
            group: Rgb::hex(0xFFFFFF),
            border: Rgb::hex(0xD5DFE5),
            tab_border: Rgb::hex(0xACACAC),
            text: Rgb::hex(0x1B1B1B),
            muted: Rgb::hex(0x5D5D5D),
            disabled: Rgb::hex(0xA0A0A0),
            button: Rgb::hex(0xE1E1E1),
            button_border: Rgb::hex(0xADADAD),
            button_hot: Rgb::hex(0xE5F1FB),
            button_hot_border: Rgb::hex(0x0078D7),
            button_pressed: Rgb::hex(0xCCE4F7),
            button_pressed_border: Rgb::hex(0x005499),
            field: Rgb::hex(0xFFFFFF),
            field_border: Rgb::hex(0xABADB3),
            accent: Rgb::hex(0x0078D7),
            accent_text: Rgb::hex(0xFFFFFF),
            selection: Rgb::hex(0xCCE8FF),
            hover: Rgb::hex(0xE5F3FF),
            error: Rgb::hex(0xC42B1C),
            warning: Rgb::hex(0xB86E00),
            success: Rgb::hex(0x0F7B0F),
            bounds_fill: Rgb::hex(0xFFFFFF),
            bounds_border: Rgb::hex(0x404040),
            area_fill: Rgb::hex(0x0078D7),
            area_alpha: 0.5,
            area_border: Rgb::hex(0x000000),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            high_contrast: false,
            window: Rgb::hex(0x202020),
            page: Rgb::hex(0x272727),
            group: Rgb::hex(0x2D2D2D),
            border: Rgb::hex(0x3E3E3E),
            tab_border: Rgb::hex(0x4A4A4A),
            text: Rgb::hex(0xF2F2F2),
            muted: Rgb::hex(0xABABAB),
            disabled: Rgb::hex(0x6D6D6D),
            button: Rgb::hex(0x373737),
            button_border: Rgb::hex(0x4F4F4F),
            button_hot: Rgb::hex(0x3F3F3F),
            button_hot_border: Rgb::hex(0x4CA0E0),
            button_pressed: Rgb::hex(0x1D3B55),
            button_pressed_border: Rgb::hex(0x4CA0E0),
            field: Rgb::hex(0x1C1C1C),
            field_border: Rgb::hex(0x5A5A5A),
            accent: Rgb::hex(0x4CA0E0),
            accent_text: Rgb::hex(0x000000),
            selection: Rgb::hex(0x1F3F5C),
            hover: Rgb::hex(0x333A41),
            error: Rgb::hex(0xFF99A4),
            warning: Rgb::hex(0xFCE100),
            success: Rgb::hex(0x6CCB5F),
            bounds_fill: Rgb::hex(0x3A3A3A),
            bounds_border: Rgb::hex(0x9A9A9A),
            area_fill: Rgb::hex(0x2F8FE0),
            area_alpha: 0.5,
            area_border: Rgb::hex(0xF2F2F2),
        }
    }

    /// High-contrast themes replace every color with a system color.
    pub fn high_contrast() -> Self {
        let color = |index| Rgb::from_colorref(unsafe { GetSysColor(index) });
        let window = color(COLOR_WINDOW);
        let text = color(COLOR_WINDOWTEXT);
        let highlight = color(COLOR_HIGHLIGHT);
        let highlight_text = color(COLOR_HIGHLIGHTTEXT);
        let gray = color(COLOR_GRAYTEXT);
        let face = color(COLOR_BTNFACE);
        let button_text = color(COLOR_BTNTEXT);
        let hot = color(COLOR_HOTLIGHT);
        Self {
            dark: false,
            high_contrast: true,
            window,
            page: window,
            group: window,
            border: text,
            tab_border: text,
            text,
            muted: text,
            disabled: gray,
            button: face,
            button_border: button_text,
            button_hot: highlight,
            button_hot_border: highlight,
            button_pressed: highlight,
            button_pressed_border: button_text,
            field: window,
            field_border: text,
            accent: highlight,
            accent_text: highlight_text,
            selection: highlight,
            hover: window,
            error: hot,
            warning: hot,
            success: text,
            bounds_fill: window,
            bounds_border: text,
            area_fill: highlight,
            area_alpha: 0.6,
            area_border: text,
        }
    }

    /// This palette with `accent` in place of every blue. The proportions
    /// are those of the blue palettes (for example the light pressed border
    /// is the accent 30% toward black), so Blue keeps its exact colors.
    pub fn with_accent(self, accent: Accent) -> Self {
        if self.high_contrast || accent == Accent::Blue {
            return self;
        }
        let base = accent.base();
        let white = Rgb::hex(0xFFFFFF);
        if self.dark {
            let light = base.mix(white, 0.3);
            Self {
                accent: light,
                accent_text: text_on(light),
                button_hot_border: light,
                button_pressed: Rgb::hex(0x202020).mix(base, 0.35),
                button_pressed_border: light,
                selection: Rgb::hex(0x202020).mix(base, 0.35),
                hover: Rgb::hex(0x373737).mix(base, 0.08),
                area_fill: base.mix(white, 0.18),
                ..self
            }
        } else {
            Self {
                accent: base,
                accent_text: text_on(base),
                button_hot: base.mix(white, 0.9),
                button_hot_border: base,
                button_pressed: base.mix(white, 0.8),
                button_pressed_border: base.mix(Rgb::hex(0x000000), 0.3),
                selection: base.mix(white, 0.8),
                hover: base.mix(white, 0.9),
                area_fill: base,
                ..self
            }
        }
    }

    /// Text shown on `selection` backgrounds.
    pub fn selection_text(&self) -> Rgb {
        if self.high_contrast {
            self.accent_text
        } else {
            self.text
        }
    }
}

fn registry_dword(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            wide(key).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

pub fn system_prefers_dark() -> bool {
    registry_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    ) == Some(0)
}

pub fn high_contrast_active() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            (&mut info as *mut HIGHCONTRASTW).cast(),
            0,
        ) != 0
            && info.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

pub fn palette_for(mode: ThemeMode, accent: Accent) -> Palette {
    if high_contrast_active() {
        return Palette::high_contrast();
    }
    let dark = match mode {
        ThemeMode::System => system_prefers_dark(),
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
    };
    if dark {
        Palette::dark()
    } else {
        Palette::light()
    }
    .with_accent(accent)
}

fn windows_build() -> u32 {
    let mut buffer = [0u16; 16];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion").as_ptr(),
            wide("CurrentBuildNumber").as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return 0;
    }
    let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
        .parse()
        .unwrap_or(0)
}

/// Dark popup menus and scrollbars rely on uxtheme exports that Windows only
/// exposes by ordinal. They have been stable since Windows 10 1903; older
/// builds keep light menus and scrollbars.
pub struct DarkMode {
    set_preferred_app_mode: Option<unsafe extern "system" fn(i32) -> i32>,
    flush_menu_themes: Option<unsafe extern "system" fn()>,
    allow_dark_mode_for_window: Option<unsafe extern "system" fn(HWND, i32) -> i32>,
}

impl DarkMode {
    pub fn load() -> Self {
        let none = Self {
            set_preferred_app_mode: None,
            flush_menu_themes: None,
            allow_dark_mode_for_window: None,
        };
        if windows_build() < 18_362 {
            return none;
        }
        let module = unsafe {
            LoadLibraryExW(
                wide("uxtheme.dll").as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return none;
        }
        // The module stays loaded for the life of the process.
        let ordinal = |number: usize| unsafe { GetProcAddress(module, number as *const u8) };
        unsafe {
            Self {
                set_preferred_app_mode: ordinal(135).map(|f| std::mem::transmute(f)),
                flush_menu_themes: ordinal(136).map(|f| std::mem::transmute(f)),
                allow_dark_mode_for_window: ordinal(133).map(|f| std::mem::transmute(f)),
            }
        }
    }

    /// Popup and context menus follow this app-wide preference.
    pub fn apply_app(&self, dark: bool) {
        unsafe {
            if let Some(set) = self.set_preferred_app_mode {
                // ForceDark = 2, ForceLight = 3.
                set(if dark { 2 } else { 3 });
            }
            if let Some(flush) = self.flush_menu_themes {
                flush();
            }
        }
    }

    pub fn apply_title_bar(&self, window: HWND, dark: bool) {
        let value: i32 = dark.into();
        unsafe {
            DwmSetWindowAttribute(
                window,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&value as *const i32).cast::<c_void>(),
                size_of::<i32>() as u32,
            );
        }
    }

    /// Scrollbars of list and edit controls, and tooltips.
    pub fn apply_control(&self, control: HWND, dark: bool) {
        unsafe {
            if let Some(allow) = self.allow_dark_mode_for_window {
                allow(control, dark.into());
            }
            let theme = wide(if dark {
                "DarkMode_Explorer"
            } else {
                "Explorer"
            });
            SetWindowTheme(control, theme.as_ptr(), std::ptr::null());
        }
    }
}

/// Per-user panel preferences, kept apart from driver profiles because the
/// profile format rejects unknown keys.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct UiPrefs {
    pub theme: ThemeMode,
    /// Replaces the panel's blue; blue by default.
    pub accent: Accent,
    pub lock_aspect_ratio: bool,
    pub lock_display_to_usable_area: bool,
    pub lock_tablet_to_usable_area: bool,
    /// Normal window size in 96-DPI units.
    pub window_size: Option<(i32, i32)>,
    pub maximized: bool,
    /// Start the driver when the panel opens, as OpenTabletDriver's UX
    /// starts its daemon.
    pub start_driver_on_launch: bool,
    /// Check GitHub for a newer release when the panel opens.
    pub check_for_updates: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            accent: Accent::Blue,
            lock_aspect_ratio: false,
            lock_display_to_usable_area: true,
            lock_tablet_to_usable_area: true,
            window_size: None,
            maximized: false,
            start_driver_on_launch: true,
            check_for_updates: true,
        }
    }
}

impl UiPrefs {
    pub fn path(directory: &Path) -> PathBuf {
        directory.join("ui.toml")
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        super::write_atomic(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_and_tolerate_missing_keys() {
        let prefs = UiPrefs {
            theme: ThemeMode::Dark,
            lock_aspect_ratio: true,
            window_size: Some((960, 760)),
            ..UiPrefs::default()
        };
        let text = toml::to_string_pretty(&prefs).unwrap();
        assert_eq!(toml::from_str::<UiPrefs>(&text).unwrap(), prefs);
        let partial: UiPrefs = toml::from_str("theme = 'light'").unwrap();
        assert_eq!(partial.theme, ThemeMode::Light);
        assert!(partial.lock_tablet_to_usable_area);
        assert!(partial.start_driver_on_launch);
    }

    #[test]
    fn accents_round_trip_and_unknown_values_keep_other_preferences() {
        for accent in Accent::PRESETS
            .iter()
            .map(|(accent, _)| *accent)
            .chain([Accent::Windows, Accent::Custom(Rgb::hex(0x12AB9F))])
        {
            let prefs = UiPrefs {
                accent,
                ..UiPrefs::default()
            };
            let text = toml::to_string_pretty(&prefs).unwrap();
            assert_eq!(toml::from_str::<UiPrefs>(&text).unwrap(), prefs, "{text}");
        }
        assert!(toml::to_string_pretty(&UiPrefs::default()).unwrap().contains("accent = \"blue\""));
        let odd: UiPrefs = toml::from_str("theme = 'dark'\naccent = 'chartreuse'").unwrap();
        assert_eq!((odd.theme, odd.accent), (ThemeMode::Dark, Accent::Blue));
        assert_eq!(toml::from_str::<UiPrefs>("").unwrap().accent, Accent::Blue);
    }

    #[test]
    fn blue_keeps_the_original_palettes_and_other_accents_replace_every_blue() {
        let fields = |p: &Palette| {
            [
                p.accent,
                p.accent_text,
                p.button_hot,
                p.button_hot_border,
                p.button_pressed,
                p.button_pressed_border,
                p.selection,
                p.hover,
                p.area_fill,
            ]
        };
        for palette in [Palette::light(), Palette::dark()] {
            assert_eq!(fields(&palette.with_accent(Accent::Blue)), fields(&palette));
            // The derivation reproduces the blue palette closely, so other
            // accents keep its relationships.
            let derived = palette.with_accent(Accent::Custom(Accent::Blue.base()));
            for (made, original) in fields(&derived).iter().zip(fields(&palette)) {
                let distance = [made.0.abs_diff(original.0), made.1.abs_diff(original.1), made.2.abs_diff(original.2)];
                assert!(distance.iter().all(|d| *d <= 20), "{made:?} vs {original:?}");
            }
            for (accent, name) in Accent::PRESETS.iter().skip(1) {
                let themed = palette.with_accent(*accent);
                // No field still uses a blue from the original palette;
                // neutral grays such as the dark hot button stay.
                for (color, original) in fields(&themed).iter().zip(fields(&palette)) {
                    let gray = original.0 == original.1 && original.1 == original.2;
                    if !gray {
                        assert_ne!(*color, original, "{name}");
                    }
                }
                // Accent text stays readable: WCAG contrast at least 3:1.
                let contrast = |a: Rgb, b: Rgb| {
                    let (l1, l2) = (a.luminance().max(b.luminance()), a.luminance().min(b.luminance()));
                    (l1 + 0.05) / (l2 + 0.05)
                };
                assert!(contrast(themed.accent, themed.accent_text) >= 3.0, "{name} accent text");
                // Everything that is not an accent is unchanged.
                assert_eq!((themed.window, themed.text, themed.error), (palette.window, palette.text, palette.error));
            }
        }
        let contrast = Palette::high_contrast();
        assert_eq!(contrast.with_accent(Accent::Red).accent, contrast.accent, "system colors win");
    }

    #[test]
    fn colors_convert_to_gdi_order() {
        assert_eq!(Rgb::hex(0x0078D7).colorref(), 0x00D7_7800);
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(200, 100, 50), 0.5), Rgb(100, 50, 25));
        assert_eq!(Rgb::parse("#0078d7"), Some(Rgb::hex(0x0078D7)));
        assert_eq!(Rgb::hex(0x0078D7).text(), "#0078D7");
        assert_eq!(Rgb::parse("0078D7"), None);
    }
}
