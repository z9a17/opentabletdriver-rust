//! Desktop geometry at startup. Compositor commands stay outside the report
//! loop. The pinned Linux providers retain construction-time monitor lists.

use std::process::Command;

use otd_core::display::DisplaySnapshot;
use otd_core::mapping::Rect;
use serde_json::Value;

pub fn explicit((width, height): (i32, i32)) -> DisplaySnapshot {
    let screen = Rect { left: 0, top: 0, right: width, bottom: height };
    DisplaySnapshot { virtual_screen: screen, monitors: vec![screen] }
}

pub fn discover() -> Result<DisplaySnapshot, String> {
    let mut errors = Vec::new();
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        match json_command("hyprctl", &["-j", "monitors"]).and_then(hyprland) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => errors.push(error),
        }
    }
    if std::env::var_os("SWAYSOCK").is_some() {
        match json_command("swaymsg", &["-t", "get_outputs", "-r"]).and_then(sway) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => errors.push(error),
        }
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var_os("DISPLAY").is_some()
    {
        match command("xrandr", &["--listmonitors"]).and_then(|text| xrandr(&text)) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => errors.push(error),
        }
    }
    // The actual pinned Wayland provider handles wl_output/xdg_output on
    // desktops with no compositor-specific CLI (GNOME/KDE, for example).
    // Only this cold fallback initializes the original managed display code;
    // successful native backends above never initialize CoreCLR.
    if std::env::var_os("WAYLAND_DISPLAY").is_some(){
        match original_wayland(){Ok(snapshot)=>return Ok(snapshot),Err(error)=>errors.push(format!("original Wayland provider: {error}"))}
    }
    Err(format!("could not discover the desktop; pass --screen WIDTHxHEIGHT in desktop coordinates{}",
        if errors.is_empty() { String::new() } else { format!(" ({})", errors.join("; ")) }))
}

fn original_wayland()->Result<DisplaySnapshot,String>{
    static SNAPSHOT:std::sync::OnceLock<std::sync::Mutex<Option<DisplaySnapshot>>>=std::sync::OnceLock::new();
    let mut snapshot=SNAPSHOT.get_or_init(||std::sync::Mutex::new(None)).lock().map_err(|_|"Original display snapshot poisoned")?;
    if let Some(snapshot)=snapshot.as_ref(){return Ok(snapshot.clone());}
    let actual=otd_platform::dotnet::original_display_snapshot()?;
    if !actual.virtual_screen.valid()||actual.monitors.is_empty()||actual.monitors.len()>256
        ||actual.monitors.iter().any(|monitor|!monitor.valid()) {return Err("Original Wayland provider returned invalid or excessive geometry".into());}
    *snapshot=Some(actual.clone());Ok(actual)
}

fn command(program: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(arguments).output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("{program}: {error}"))
}

fn json_command(program: &str, arguments: &[&str]) -> Result<Value, String> {
    serde_json::from_str(&command(program, arguments)?)
        .map_err(|error| format!("{program}: invalid display JSON: {error}"))
}

fn integer(value: &Value, key: &str) -> Result<i32, String> {
    value[key].as_i64().and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| format!("invalid display {key}"))
}

fn rect(left: i32, top: i32, width: i32, height: i32) -> Result<Rect, String> {
    if width <= 0 || height <= 0 { return Err("invalid display dimensions".into()); }
    Ok(Rect {
        left, top,
        right: left.checked_add(width).ok_or("display bounds overflow")?,
        bottom: top.checked_add(height).ok_or("display bounds overflow")?,
    })
}

fn snapshot(mut monitors: Vec<Rect>) -> Result<DisplaySnapshot, String> {
    let first = *monitors.first().ok_or("no active displays")?;
    let virtual_screen = monitors.iter().fold(first, |bounds, monitor| Rect {
        left: bounds.left.min(monitor.left), top: bounds.top.min(monitor.top),
        right: bounds.right.max(monitor.right), bottom: bounds.bottom.max(monitor.bottom),
    });
    if virtual_screen.right.checked_sub(virtual_screen.left).is_none()
        || virtual_screen.bottom.checked_sub(virtual_screen.top).is_none() {
        return Err("virtual desktop dimensions overflow".into());
    }
    monitors.sort_by_key(|monitor| (monitor.left, monitor.top, monitor.right, monitor.bottom));
    monitors.dedup();
    Ok(DisplaySnapshot { virtual_screen, monitors })
}

fn hyprland(value: Value) -> Result<DisplaySnapshot, String> {
    let mut monitors = Vec::new();
    for monitor in value.as_array().ok_or("expected Hyprland monitor array")? {
        if monitor["disabled"].as_bool() == Some(true)
            || monitor["mirrorOf"].as_str().is_some_and(|name| name != "none") { continue; }
        let scale = monitor["scale"].as_f64().ok_or("missing display scale")?;
        if !scale.is_finite() || scale <= 0.0 { return Err("invalid display scale".into()); }
        let (mut width, mut height) = (integer(monitor, "width")?, integer(monitor, "height")?);
        let transform = integer(monitor, "transform")?;
        if !(0..=7).contains(&transform) { return Err("invalid display transform".into()); }
        if transform % 2 != 0 { std::mem::swap(&mut width, &mut height); }
        let logical = |pixels: i32| -> Result<i32, String> {
            let size = (f64::from(pixels) / scale).round();
            if size < 1.0 || size > f64::from(i32::MAX) { return Err("invalid logical display size".into()); }
            Ok(size as i32)
        };
        monitors.push(rect(integer(monitor, "x")?, integer(monitor, "y")?, logical(width)?, logical(height)?)?);
    }
    snapshot(monitors)
}

fn sway(value: Value) -> Result<DisplaySnapshot, String> {
    let mut monitors = Vec::new();
    for monitor in value.as_array().ok_or("expected Sway output array")? {
        if monitor["active"].as_bool() != Some(true) { continue; }
        let area = &monitor["rect"];
        monitors.push(rect(integer(area, "x")?, integer(area, "y")?, integer(area, "width")?, integer(area, "height")?)?);
    }
    snapshot(monitors)
}

fn xrandr(text: &str) -> Result<DisplaySnapshot, String> {
    let mut monitors = Vec::new();
    for line in text.lines().skip(1) {
        // xrandr --listmonitors: 1920/340x1080/190+0+0, with signed offsets.
        let geometry = line.split_whitespace().nth(2).ok_or("missing xrandr geometry")?;
        let (width, rest) = geometry.split_once('x').ok_or("invalid xrandr geometry")?;
        let offset = rest.find(['+', '-']).ok_or("missing xrandr position")?;
        let (height, position) = rest.split_at(offset);
        let next = position[1..].find(['+', '-']).map(|n| n + 1).ok_or("invalid xrandr position")?;
        let number = |text: &str| text.split('/').next().unwrap_or(text).parse::<i32>()
            .map_err(|_| "invalid xrandr number".to_owned());
        monitors.push(rect(number(&position[..next])?, number(&position[next..])?, number(width)?, number(height)?)?);
    }
    snapshot(monitors)
}
