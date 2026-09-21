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

    fn from_colorref(value: u32) -> Self {
        Self(value as u8, (value >> 8) as u8, (value >> 16) as u8)
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

pub fn palette_for(mode: ThemeMode) -> Palette {
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
    pub lock_aspect_ratio: bool,
    pub lock_display_to_usable_area: bool,
    pub lock_tablet_to_usable_area: bool,
    /// Normal window size in 96-DPI units.
    pub window_size: Option<(i32, i32)>,
    pub maximized: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            lock_aspect_ratio: false,
            lock_display_to_usable_area: true,
            lock_tablet_to_usable_area: true,
            window_size: None,
            maximized: false,
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
    }

    #[test]
    fn colors_convert_to_gdi_order() {
        assert_eq!(Rgb::hex(0x0078D7).colorref(), 0x00D7_7800);
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(200, 100, 50), 0.5), Rgb(100, 50, 25));
    }
}
