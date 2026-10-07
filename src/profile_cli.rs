//! Offline profile inspection and migration commands. No device or plugin loads.
use std::path::{Path, PathBuf};
use std::time::Duration;

use otd_core::config::{
    ImportOptions, OutputKind, MAX_PEN_BUTTONS, NativeProfileCollection, OtdSettingsDocument, Profile,
};
use otd_core::output::buttons::{ButtonAction, WheelBinding};
use otd_core::reports::MAX_WHEELS;
use otd_core::storage;
use serde_json::{Value, json};

pub fn usage() -> &'static str {
    "Profile commands (offline; indexes start at zero):
  profiles list INPUT.json|INPUT.toml
  profiles preview INPUT --profile INDEX [--legacy-force-radial-follow]
  profiles import INPUT --profile INDEX --output PROFILE.toml [--legacy-force-radial-follow]
  profiles export PROFILE.toml --output SETTINGS.json
  profiles select COLLECTION.toml --name NAME --output COLLECTION.toml
  profiles recover PROFILE.toml --output RECOVERED.toml
  profiles get INPUT [--profile INDEX] [--section SECTION]
  profiles set PROFILE.toml --output NEW.toml [--sensitivity X,Y]
      [--relative-rotation DEGREES] [--reset-time MS] [--pen-button NUMBER=ACTION]
      [--aux-button NUMBER=ACTION] [--wheel-clockwise WHEEL=ACTION]
      [--wheel-counter-clockwise WHEEL=ACTION] [--wheel-threshold WHEEL=DEGREES]
      [--mouse-button NUMBER=ACTION] [--mouse-scroll-up ACTION] [--mouse-scroll-down ACTION]
      [--output-mode absolute|relative|pen]
      [--monitor all|INDEX] [--display-area W,H,X,Y] [--tablet-area W,H,X,Y[,ROTATION]]
      [--clipping BOOL] [--limiting BOOL] [--tip-enabled BOOL] [--eraser-enabled BOOL]
      [--tip-threshold PERCENT] [--eraser-threshold PERCENT] [--drag-only BOOL]
      [--disable-pressure BOOL] [--disable-tilt BOOL] [--plugin-enabled NUMBER=BOOL]
      [--radial-follow enable|disable|reset]
  profiles paths

Areas use centered coordinates: display pixels and tablet millimeters. Simple
profiles require both areas together; --monitor requires a simple absolute profile.
Plugin indexes start at 1 and include tools. Disabling native Radial Follow
retains one configured entry; ambiguous multiple native entries are rejected.
Output files must not already exist. The source file is never overwritten.
Importing a tablet profile does not establish runtime support for that tablet.
The legacy flag explicitly activates disabled Radial Follow stores during OTD import.
Recovery reads the sibling .bak into a new file; it never replaces the source.
Get prints JSON for one section: all, output, areas, sensitivity, bindings,
filters, tools or misc (default all). A collection defaults to its selected profile and
a single Rust profile to index 0; OTD JSON requires --profile.
Set writes a new profile with the next settings revision; it neither contacts
the daemon nor applies the result. Relative options require relative output.
Pen button and express key numbers start at 1 (maximum 64). Repeat
--pen-button or --aux-button for distinct buttons. Actions: none, barrel:1..3,
mouse:left|right|middle|backward|forward, or keys:Control+Shift+Z. Quote key
chords when required by your shell. Scroll actions: scroll:up|down|left|right or
scroll:vertical|horizontal:AMOUNT[:INTERVAL_MS]. Amount is the upstream signed
amount, emitted with its sign inverted; interval defaults to 300 ms. Scroll
presses once then repeats while held; release cancels repetition. Mouse scroll
up/down use the report Y sign as an edge-triggered binding, as upstream does.
Wheel numbers start at 1 (maximum 8); each
rotation threshold step presses and releases the wheel's action once, and
--wheel-threshold sets both directions in degrees (default: one wheel step).
OTD_RUST_PORTABLE_DIR selects an explicit absolute portable settings directory."
}

/// Profile keys shown by `profiles get --section`, grouped like the upstream
/// console getters (`getareas`, `getsensitivity`, `getbindings`, `getfilters`,
/// `getmiscsettings`). `output` and `all` are handled separately.
const SECTIONS: [(&str, &[&str]); 6] = [
    ("areas", &["monitor", "rotation", "crop", "absolute"]),
    ("sensitivity", &["relative"]),
    ("tools", &["plugins"]),
    ("bindings", &["bindings", "pen_buttons", "aux_buttons", "mouse_buttons", "mouse_scroll_up", "mouse_scroll_down", "wheels"]),
    ("filters", &["radial_follow", "disabled_radial_follow", "plugins"]),
    (
        "misc",
        &["device_path", "schema_version", "settings_revision"],
    ),
];

#[derive(Default)]
struct Options {
    profile: Option<usize>,
    output: Option<PathBuf>,
    name: Option<String>,
    legacy: bool,
    section: Option<String>,
    sensitivity: Option<(f64, f64)>,
    relative_rotation: Option<f64>,
    reset_time_ms: Option<u64>,
    pen_buttons: Vec<(usize, ButtonAction)>,
    aux_buttons: Vec<(usize, ButtonAction)>,
    wheel_clockwise: Vec<(usize, ButtonAction)>,
    wheel_counter_clockwise: Vec<(usize, ButtonAction)>,
    wheel_thresholds: Vec<(usize, f32)>,
    mouse_buttons: Vec<(usize, ButtonAction)>,
    mouse_scroll_up: Option<ButtonAction>,
    mouse_scroll_down: Option<ButtonAction>,
    mode: Option<String>,
    monitor: Option<Option<usize>>,
    display_area: Option<otd_core::mapping::OtdArea>,
    tablet_area: Option<otd_core::mapping::OtdArea>,
    clipping: Option<bool>,
    limiting: Option<bool>,
    tip_enabled: Option<bool>,
    eraser_enabled: Option<bool>,
    tip_threshold: Option<f64>,
    eraser_threshold: Option<f64>,
    drag_only: Option<bool>,
    disable_pressure: Option<bool>,
    disable_tilt: Option<bool>,
    plugin_states: Vec<(usize, bool)>,
    radial_state: Option<String>,
}

impl Options {
    fn sets_controls(&self) -> bool {
        self.mode.is_some() || self.monitor.is_some() || self.display_area.is_some()
            || self.tablet_area.is_some() || self.clipping.is_some() || self.limiting.is_some()
            || self.tip_enabled.is_some() || self.eraser_enabled.is_some()
            || self.tip_threshold.is_some() || self.eraser_threshold.is_some()
            || self.drag_only.is_some() || self.disable_pressure.is_some() || self.disable_tilt.is_some()
            || !self.plugin_states.is_empty() || self.radial_state.is_some()
    }
    fn sets_bindings(&self) -> bool {
        !(self.mouse_scroll_up.is_none()
            && self.mouse_scroll_down.is_none()
            && self.mouse_buttons.is_empty()
            && self.pen_buttons.is_empty()
            && self.aux_buttons.is_empty()
            && self.wheel_clockwise.is_empty()
            && self.wheel_counter_clockwise.is_empty()
            && self.wheel_thresholds.is_empty())
    }

    fn sets_relative(&self) -> bool {
        self.sensitivity.is_some()
            || self.relative_rotation.is_some()
            || self.reset_time_ms.is_some()
    }
}

enum Input {
    Otd(OtdSettingsDocument),
    Native(Box<Profile>),
    Collection(NativeProfileCollection),
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        println!("{}", usage());
        return Ok(());
    };
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        println!("{}", usage());
        return Ok(());
    }
    if command == "paths" {
        if args.next().is_some() {
            return Err("profiles paths takes no arguments".into());
        }
        let directory = storage::data_directory()?;
        return print_json(
            &json!({"directory": directory, "profile": directory.join("driver.toml"),
            "preferences": directory.join("ui.toml"), "portable": std::env::var_os("OTD_RUST_PORTABLE_DIR").is_some()}),
        );
    }
    if !matches!(
        command.as_str(),
        "list" | "preview" | "import" | "export" | "select" | "recover" | "get" | "set"
    ) {
        return Err(format!("unknown profile command {command:?}\n{}", usage()));
    }
    let input_path = PathBuf::from(
        args.next()
            .ok_or_else(|| format!("{command} requires an input file\n{}", usage()))?,
    );
    let options = parse_options(args)?;
    validate_options(&command, &options)?;
    if command == "recover" {
        let backup = storage::read_backup(&input_path)?;
        let output = options
            .output
            .as_deref()
            .ok_or("recover requires --output FILE")?;
        let contents = match parse_input(&backup.text, &input_path)? {
            Input::Native(profile) => profile.to_toml_at(output)?,
            Input::Collection(collection) => collection.to_toml_at(output)?,
            Input::Otd(document) => document.original_json().to_owned(),
        };
        return write_output(output, &contents);
    }
    let input = read_input(&input_path)?;
    if options.legacy && !matches!(input, Input::Otd(_)) {
        return Err("--legacy-force-radial-follow applies only to OTD JSON imports; native profiles retain their stored filter behavior".into());
    }
    match command.as_str() {
        "list" => print_json(&list(&input)?),
        "preview" => {
            let index = options.profile.ok_or("preview requires --profile INDEX")?;
            let value = match &input {
                Input::Otd(document) => serde_json::to_value(document.preview(
                    index,
                    ImportOptions {
                        legacy_force_radial_follow: options.legacy,
                    },
                )?)
                .map_err(|error| error.to_string())?,
                _ => {
                    let profile = select_profile(&input, index, false)?;
                    json!({"profile": native_summary(index, None, &profile)?,
                        "diagnostics": profile.diagnostics, "import_error": null})
                }
            };
            print_json(&value)
        }
        "import" => {
            let index = options.profile.ok_or("import requires --profile INDEX")?;
            let profile = select_profile(&input, index, options.legacy)?;
            let output = options
                .output
                .as_deref()
                .ok_or("import requires --output FILE")?;
            write_output(output, &profile.to_toml_at(output)?)?;
            for diagnostic in &profile.diagnostics {
                eprintln!(
                    "{} [{}]: {}",
                    diagnostic.kind, diagnostic.location, diagnostic.message
                );
            }
            Ok(())
        }
        "export" => {
            let Input::Native(profile) = input else {
                return Err("export requires a single Rust profile TOML file. Extract a collection entry with profiles import first; OTD JSON is already in export format.".into());
            };
            write_output(
                options
                    .output
                    .as_deref()
                    .ok_or("export requires --output FILE")?,
                &profile.to_otd_json()?,
            )
        }
        "select" => {
            let Input::Collection(mut collection) = input else {
                return Err("select requires a native profile collection TOML file".into());
            };
            let name = options
                .name
                .as_deref()
                .ok_or("select requires --name NAME")?;
            let index = collection
                .profiles
                .iter()
                .position(|profile| profile.name == name)
                .ok_or_else(|| {
                    format!("no profile named {name:?}; use profiles list to see names")
                })?;
            collection.select(index)?;
            let output = options
                .output
                .as_deref()
                .ok_or("select requires --output FILE")?;
            write_output(output, &collection.to_toml_at(output)?)
        }
        "get" => {
            let index = match (&input, options.profile) {
                (_, Some(index)) => index,
                (Input::Native(_), None) => 0,
                (Input::Collection(collection), None) => collection.selected_profile,
                (Input::Otd(_), None) => {
                    return Err("get from OTD JSON requires --profile INDEX".into());
                }
            };
            let profile = select_profile(&input, index, options.legacy)?;
            print_json(&profile_section(
                &profile,
                options.section.as_deref().unwrap_or("all"),
            )?)
        }
        "set" => {
            let Input::Native(mut profile) = input else {
                return Err("set requires a single Rust profile TOML file. Extract a profile with profiles import first.".into());
            };
            apply_controls(&mut profile, &options)?;
            if options.sets_relative() {
                let mut relative = profile
                    .relative
                    .ok_or("relative setting options require a profile with relative output")?;
                if let Some(sensitivity) = options.sensitivity {
                    relative.sensitivity = sensitivity;
                }
                if let Some(rotation) = options.relative_rotation {
                    relative.rotation = rotation;
                }
                if let Some(ms) = options.reset_time_ms {
                    relative.reset_delay = Duration::from_millis(ms);
                }
                profile.relative = Some(relative.validate_for(profile.tablet)?);
            }
            if let Some(action) = &options.mouse_scroll_up { profile.mouse_scroll_up = action.clone(); }
            if let Some(action) = &options.mouse_scroll_down { profile.mouse_scroll_down = action.clone(); }
            for (list, edits) in [
                (&mut profile.pen_buttons, &options.pen_buttons),
                (&mut profile.aux_buttons, &options.aux_buttons),
                (&mut profile.mouse_buttons, &options.mouse_buttons),
            ] {
                for (index, action) in edits {
                    if list.len() <= *index {
                        list.resize(*index + 1, ButtonAction::None);
                    }
                    list[*index] = action.clone();
                }
            }
            // Edited wheels past the current list start unbound.
            let last = options
                .wheel_clockwise
                .iter()
                .chain(&options.wheel_counter_clockwise)
                .map(|(index, _)| *index)
                .chain(options.wheel_thresholds.iter().map(|(index, _)| *index))
                .max();
            if let Some(last) = last
                && profile.wheels.len() <= last
            {
                profile.wheels.resize(last + 1, WheelBinding::default());
            }
            for (index, action) in &options.wheel_clockwise {
                profile.wheels[*index].clockwise = action.clone();
            }
            for (index, action) in &options.wheel_counter_clockwise {
                profile.wheels[*index].counter_clockwise = action.clone();
            }
            for (index, degrees) in &options.wheel_thresholds {
                let wheel = &mut profile.wheels[*index];
                wheel.clockwise_threshold = Some(*degrees);
                wheel.counter_clockwise_threshold = Some(*degrees);
            }
            profile.advance_revision()?;
            let output = options
                .output
                .as_deref()
                .ok_or("set requires --output FILE")?;
            let text = profile.to_toml_at(output)?;
            Profile::from_toml_text(&text, output)?;
            write_output(output, &text)
        }
        _ => unreachable!("command was validated"),
    }
}


fn parse_bool(text: &str, flag: &str) -> Result<bool, String> {
    match text { "true" => Ok(true), "false" => Ok(false), _ => Err(format!("{flag} requires true or false")) }
}

fn parse_area(text: &str, flag: &str) -> Result<otd_core::mapping::OtdArea, String> {
    let fields = text.split(',').collect::<Vec<_>>();
    if fields.len() != 4 && !(flag == "--tablet-area" && fields.len() == 5) {
        return Err(format!("{flag} requires W,H,X,Y{}", if flag == "--tablet-area" { "[,ROTATION]" } else { "" }));
    }
    let area = otd_core::mapping::OtdArea {
        width: finite(fields[0], flag)?, height: finite(fields[1], flag)?,
        x: finite(fields[2], flag)?, y: finite(fields[3], flag)?,
        rotation: if fields.len() == 5 { finite(fields[4], flag)? } else { 0.0 },
    };
    if area.width <= 0.0 || area.height <= 0.0 { return Err(format!("{flag} requires positive dimensions")); }
    Ok(area)
}

fn apply_controls(profile: &mut Profile, options: &Options) -> Result<(), String> {
    if let Some(mode) = options.mode.as_deref() {
        match mode {
            "relative" => {
                profile.output = OutputKind::Mouse;
                profile.relative = Some(profile.relative.unwrap_or(otd_core::relative::RelativeSettings {
                    sensitivity: (10.0, 10.0), rotation: 0.0, reset_delay: Duration::from_millis(100),
                }));
                profile.otd_mapping = None;
                profile.monitor = None;
                profile.crop = otd_core::mapping::Crop::full(profile.tablet);
                profile.rotation = 0;
            }
            "absolute" | "pen" => { profile.relative = None; profile.output = if mode == "pen" { OutputKind::Pen } else { OutputKind::Mouse }; }
            _ => unreachable!("mode validated"),
        }
    }
    if options.display_area.is_some() || options.tablet_area.is_some() || options.clipping.is_some() || options.limiting.is_some() {
        if profile.relative.is_some() { return Err("absolute area options require absolute output".into()); }
        let mut mapping = match profile.otd_mapping {
            Some(mapping) => mapping,
            None => otd_core::mapping::OtdMapping {
                display: options.display_area.ok_or("a simple profile needs --display-area and --tablet-area together")?,
                tablet: options.tablet_area.ok_or("a simple profile needs --display-area and --tablet-area together")?,
                clipping: true, limiting: false,
            },
        };
        if let Some(area) = options.display_area { mapping.display = area; }
        if let Some(area) = options.tablet_area { mapping.tablet = area; }
        if let Some(value) = options.clipping { mapping.clipping = value; }
        if let Some(value) = options.limiting { mapping.limiting = value; }
        profile.otd_mapping = Some(mapping);
        profile.monitor = None;
        profile.crop = otd_core::mapping::Crop::default();
        profile.rotation = 0;
    }
    if let Some(monitor) = options.monitor {
        if profile.relative.is_some() || profile.otd_mapping.is_some() { return Err("--monitor requires a simple absolute profile without explicit areas".into()); }
        profile.monitor = monitor;
    }
    if let Some(value) = options.tip_enabled { profile.contact.tip_enabled = value; }
    if let Some(value) = options.eraser_enabled { profile.contact.eraser_enabled = value; }
    if let Some(value) = options.tip_threshold { profile.contact.tip_threshold_percent = Some(value as f32); profile.contact.tip_threshold_raw = Some(otd_core::config::activation_raw_for(value, profile.tablet.max_pressure)?); }
    if let Some(value) = options.eraser_threshold { profile.contact.eraser_threshold_percent = Some(value as f32); profile.contact.eraser_threshold_raw = Some(otd_core::config::activation_raw_for(value, profile.tablet.max_pressure)?); }
    if let Some(value) = options.drag_only { profile.contact.drag_only = value; }
    if let Some(value) = options.disable_pressure { profile.contact.disable_pressure = value; }
    if let Some(value) = options.disable_tilt { profile.contact.disable_tilt = value; }
    for (index, enabled) in &options.plugin_states {
        profile.plugins.get_mut(*index).ok_or_else(|| format!("plugin {} does not exist", index + 1))?.enabled = *enabled;
    }
    if let Some(state) = options.radial_state.as_deref() {
        if profile.radial_follow.len() > 1 { return Err("native Radial Follow control requires at most one entry; edit multiple entries explicitly".into()); }
        match state {
            "enable" => if profile.radial_follow.is_empty() { profile.radial_follow.push(profile.disabled_radial_follow.take().unwrap_or_default()); },
            "disable" => if let Some(settings) = profile.radial_follow.pop() { profile.disabled_radial_follow = Some(settings); },
            "reset" => if let Some(settings) = profile.radial_follow.first_mut() { *settings = Default::default(); } else { profile.disabled_radial_follow = Some(Default::default()); },
            _ => unreachable!("radial state validated"),
        }
    }
    profile.validate_filter_execution()
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.peekable();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--profile" => {
                if options.profile.is_some() {
                    return Err("--profile was specified more than once".into());
                }
                let value = option_value(&mut args, &flag)?;
                options.profile =
                    Some(value.parse().map_err(
                        |_| "--profile requires a nonnegative zero-based integer index",
                    )?);
            }
            "--output" => {
                if options.output.is_some() {
                    return Err("--output was specified more than once".into());
                }
                options.output = Some(option_value(&mut args, &flag)?.into());
            }
            "--name" => {
                if options.name.is_some() {
                    return Err("--name was specified more than once".into());
                }
                options.name = Some(option_value(&mut args, &flag)?);
            }
            "--legacy-force-radial-follow" => {
                if options.legacy {
                    return Err("--legacy-force-radial-follow was specified more than once".into());
                }
                options.legacy = true;
            }
            "--section" => {
                if options.section.is_some() {
                    return Err("--section was specified more than once".into());
                }
                let section = option_value(&mut args, &flag)?;
                if !matches!(section.as_str(), "all" | "output")
                    && !SECTIONS.iter().any(|(name, _)| *name == section)
                {
                    return Err(format!(
                        "unknown section {section:?}; use all, output, areas, sensitivity, bindings, filters, tools or misc"
                    ));
                }
                options.section = Some(section);
            }
            "--sensitivity" => {
                if options.sensitivity.is_some() {
                    return Err("--sensitivity was specified more than once".into());
                }
                let value = option_value(&mut args, &flag)?;
                let (x, y) = value.split_once(',').ok_or("--sensitivity requires X,Y")?;
                options.sensitivity = Some((finite(x, &flag)?, finite(y, &flag)?));
            }
            "--relative-rotation" => {
                if options.relative_rotation.is_some() {
                    return Err("--relative-rotation was specified more than once".into());
                }
                options.relative_rotation = Some(finite(&option_value(&mut args, &flag)?, &flag)?);
            }
            "--reset-time" => {
                if options.reset_time_ms.is_some() {
                    return Err("--reset-time was specified more than once".into());
                }
                options.reset_time_ms = Some(
                    option_value(&mut args, &flag)?
                        .parse()
                        .map_err(|_| "--reset-time requires nonnegative whole milliseconds")?,
                );
            }
            "--output-mode" => {
                if options.mode.is_some() { return Err("--output-mode was specified more than once".into()); }
                let value = option_value(&mut args, &flag)?;
                if !matches!(value.as_str(), "absolute" | "relative" | "pen") { return Err("--output-mode requires absolute, relative or pen".into()); }
                options.mode = Some(value);
            }
            "--monitor" => {
                if options.monitor.is_some() { return Err("--monitor was specified more than once".into()); }
                let value = option_value(&mut args, &flag)?;
                options.monitor = Some(if value == "all" { None } else { Some(value.parse().map_err(|_| "--monitor requires all or a zero-based display index")?) });
            }
            "--display-area" | "--tablet-area" => {
                let area = parse_area(&option_value(&mut args, &flag)?, &flag)?;
                let target = if flag == "--display-area" { &mut options.display_area } else { &mut options.tablet_area };
                if target.replace(area).is_some() { return Err(format!("{flag} was specified more than once")); }
            }
            "--clipping" | "--limiting" | "--tip-enabled" | "--eraser-enabled" | "--drag-only" | "--disable-pressure" | "--disable-tilt" => {
                let value = parse_bool(&option_value(&mut args, &flag)?, &flag)?;
                let target = match flag.as_str() {
                    "--clipping" => &mut options.clipping, "--limiting" => &mut options.limiting,
                    "--tip-enabled" => &mut options.tip_enabled, "--eraser-enabled" => &mut options.eraser_enabled,
                    "--drag-only" => &mut options.drag_only, "--disable-pressure" => &mut options.disable_pressure,
                    _ => &mut options.disable_tilt,
                };
                if target.replace(value).is_some() { return Err(format!("{flag} was specified more than once")); }
            }
            "--tip-threshold" | "--eraser-threshold" => {
                let value = finite(&option_value(&mut args, &flag)?, &flag)?;
                if !(0.0..=100.0).contains(&value) { return Err(format!("{flag} requires a percentage from 0 to 100")); }
                let target = if flag == "--tip-threshold" { &mut options.tip_threshold } else { &mut options.eraser_threshold };
                if target.replace(value).is_some() { return Err(format!("{flag} was specified more than once")); }
            }
            "--plugin-enabled" => {
                let value = option_value(&mut args, &flag)?;
                let (index, enabled) = numbered(&value, &flag, 32)?;
                if options.plugin_states.iter().any(|(old, _)| *old == index) { return Err("plugin index was specified more than once".into()); }
                options.plugin_states.push((index, parse_bool(enabled, &flag)?));
            }
            "--radial-follow" => {
                if options.radial_state.is_some() { return Err("--radial-follow was specified more than once".into()); }
                let value = option_value(&mut args, &flag)?;
                if !matches!(value.as_str(), "enable" | "disable" | "reset") { return Err("--radial-follow requires enable, disable or reset".into()); }
                options.radial_state = Some(value);
            }
            "--mouse-scroll-up" | "--mouse-scroll-down" => {
                let action = option_value(&mut args, &flag)?.parse::<ButtonAction>()?;
                let target = if flag == "--mouse-scroll-up" { &mut options.mouse_scroll_up } else { &mut options.mouse_scroll_down };
                if target.replace(action).is_some() { return Err(format!("{flag} was specified more than once")); }
            }
            "--mouse-button" => {
                let value = option_value(&mut args, &flag)?;
                let (index, action) = numbered(&value, &flag, MAX_PEN_BUTTONS)?;
                if options.mouse_buttons.iter().any(|(old, _)| *old == index) { return Err("mouse button was specified more than once".into()); }
                options.mouse_buttons.push((index, action.parse()?));
            }
            "--pen-button" => {
                let value = option_value(&mut args, &flag)?;
                let (number, action) = value
                    .split_once('=')
                    .ok_or("--pen-button requires NUMBER=ACTION, for example 1=mouse:right")?;
                let number = number
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|number| (1..=MAX_PEN_BUTTONS).contains(number))
                    .ok_or("pen button number must be an integer from 1 to 64")?;
                let index = number - 1;
                if options.pen_buttons.iter().any(|(old, _)| *old == index) {
                    return Err(format!("pen button {number} was specified more than once"));
                }
                options.pen_buttons.push((index, action.parse()?));
            }
            "--aux-button" => {
                let value = option_value(&mut args, &flag)?;
                let (index, action) = numbered(&value, &flag, MAX_PEN_BUTTONS)?;
                if options.aux_buttons.iter().any(|(old, _)| *old == index) {
                    return Err(format!("express key {} was specified more than once", index + 1));
                }
                options.aux_buttons.push((index, action.parse()?));
            }
            "--wheel-clockwise" | "--wheel-counter-clockwise" => {
                let value = option_value(&mut args, &flag)?;
                let (index, action) = numbered(&value, &flag, MAX_WHEELS)?;
                let edits = if flag == "--wheel-clockwise" {
                    &mut options.wheel_clockwise
                } else {
                    &mut options.wheel_counter_clockwise
                };
                if edits.iter().any(|(old, _)| *old == index) {
                    return Err(format!("{flag} for wheel {} was specified more than once", index + 1));
                }
                edits.push((index, action.parse()?));
            }
            "--wheel-threshold" => {
                let value = option_value(&mut args, &flag)?;
                let (index, degrees) = numbered(&value, &flag, MAX_WHEELS)?;
                let degrees = degrees
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|degrees| degrees.is_finite() && *degrees > 0.0)
                    .ok_or("--wheel-threshold requires a positive number of degrees")?;
                if options.wheel_thresholds.iter().any(|(old, _)| *old == index) {
                    return Err(format!("--wheel-threshold for wheel {} was specified more than once", index + 1));
                }
                options.wheel_thresholds.push((index, degrees));
            }
            _ => return Err(format!("unknown profile option {flag:?}\n{}", usage())),
        }
    }
    Ok(options)
}

/// `NUMBER=VALUE` with a 1-based number up to `maximum`, as a 0-based index.
fn numbered<'a>(text: &'a str, flag: &str, maximum: usize) -> Result<(usize, &'a str), String> {
    let (number, value) = text
        .split_once('=')
        .ok_or_else(|| format!("{flag} requires NUMBER=VALUE, for example 1=keys:PageDown"))?;
    let number = number
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|number| (1..=maximum).contains(number))
        .ok_or_else(|| format!("{flag} number must be an integer from 1 to {maximum}"))?;
    Ok((number - 1, value))
}

fn option_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    flag: &str,
) -> Result<String, String> {
    if args.peek().is_none_or(|value| value.starts_with("--")) {
        return Err(format!("{flag} requires a value"));
    }
    args.next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{flag} requires a nonempty value"))
}

fn finite(text: &str, flag: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{flag} requires finite numbers"))
}

fn validate_options(command: &str, options: &Options) -> Result<(), String> {
    if options.sets_controls() && command != "set" {
        return Err("control setting options apply only to profiles set".into());
    }
    if options.section.is_some() && command != "get" {
        return Err(format!(
            "--section applies only to profiles get\n{}",
            usage()
        ));
    }
    if options.sets_relative() && command != "set" {
        return Err(format!(
            "relative setting options apply only to profiles set\n{}",
            usage()
        ));
    }
    if options.sets_bindings() && command != "set" {
        return Err("binding options apply only to profiles set".into());
    }
    let valid = match command {
        "list" => {
            options.profile.is_none()
                && options.output.is_none()
                && options.name.is_none()
                && !options.legacy
        }
        "preview" => {
            options.profile.is_some() && options.output.is_none() && options.name.is_none()
        }
        "import" => options.profile.is_some() && options.output.is_some() && options.name.is_none(),
        "export" | "recover" => {
            options.profile.is_none()
                && options.output.is_some()
                && options.name.is_none()
                && !options.legacy
        }
        "select" => {
            options.profile.is_none()
                && options.output.is_some()
                && options.name.is_some()
                && !options.legacy
        }
        "get" => options.output.is_none() && options.name.is_none(),
        "set" => {
            options.profile.is_none()
                && options.output.is_some()
                && options.name.is_none()
                && !options.legacy
                && (options.sets_relative() || options.sets_bindings() || options.sets_controls())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "missing or incompatible options for profiles {command}\n{}",
            usage()
        ))
    }
}

fn read_input(path: &Path) -> Result<Input, String> {
    let loaded = storage::read_utf8(path)?;
    parse_input(&loaded.text, path)
}

fn parse_input(text: &str, path: &Path) -> Result<Input, String> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => OtdSettingsDocument::from_json(text, path).map(Input::Otd),
        Some("toml") => {
            let document: toml::Value = toml::from_str(text)
                .map_err(|error| format!("invalid TOML {}: {error}", path.display()))?;
            if document.get("format").and_then(toml::Value::as_str) == Some("profile_collection") {
                NativeProfileCollection::from_toml_text(text, path).map(Input::Collection)
            } else {
                Profile::from_toml_text(text, path).map(|profile| Input::Native(Box::new(profile)))
            }
        }
        _ => Err("input must be an OTD .json file or a Rust .toml profile/collection".into()),
    }
}

fn select_profile(input: &Input, index: usize, legacy: bool) -> Result<Profile, String> {
    match input {
        Input::Otd(document) => document.import(
            index,
            ImportOptions {
                legacy_force_radial_follow: legacy,
            },
        ),
        Input::Native(profile) if index == 0 => Ok((**profile).clone()),
        Input::Native(_) => Err("a single Rust profile has index 0".into()),
        Input::Collection(collection) => collection
            .profiles
            .get(index)
            .map(|entry| entry.profile.clone())
            .ok_or_else(|| {
                format!("native profile index {index} does not exist; use profiles list")
            }),
    }
}

fn list(input: &Input) -> Result<Value, String> {
    Ok(match input {
        Input::Otd(document) => {
            json!({"format": "otd", "revision": document.revision(), "profiles": document.profiles()})
        }
        Input::Native(profile) => {
            json!({"format": "profile", "profiles": [native_summary(0, None, profile)?]})
        }
        Input::Collection(collection) => {
            let profiles = collection
                .profiles
                .iter()
                .enumerate()
                .map(|(index, entry)| native_summary(index, Some(&entry.name), &entry.profile))
                .collect::<Result<Vec<_>, _>>()?;
            json!({"format": "profile_collection", "selected_profile": collection.selected_profile,
                "settings_revision": collection.settings_revision, "profiles": profiles})
        }
    })
}

pub fn active_section(section: &str) -> Result<(), String> {
    print_json(&profile_section(&crate::daemon::active_profile()?, section)?)
}

pub fn binding_actions() -> Result<(), String> {
    print_json(&json!({"native": ["none", "barrel:1", "barrel:2", "barrel:3", "mouse:left", "mouse:right", "mouse:middle", "mouse:backward", "mouse:forward", "keys:KEY[+KEY...]", "scroll:up", "scroll:down", "scroll:left", "scroll:right", "scroll:vertical|horizontal:AMOUNT[:INTERVAL_MS]"],
        "managed_bindings": "not hosted", "preset_bindings": "not hosted"}))
}

fn output_mode(profile: &Profile) -> &'static str {
    if profile.relative.is_some() {
        "relative"
    } else {
        "absolute"
    }
}

/// The profile's stored settings for one `get` section. The archived OTD
/// document and preserved unknown fields are summarized rather than printed.
fn profile_section(profile: &Profile, section: &str) -> Result<Value, String> {
    let tablet = profile
        .tablet_name()?
        .unwrap_or_else(|| "Wacom PTH-660".into());
    if section == "output" {
        return Ok(
            json!({"tablet": tablet, "output_mode": output_mode(profile),
            "output": profile.output}),
        );
    }
    let document: toml::Value = toml::from_str(&profile.to_toml()?)
        .map_err(|error| format!("profile did not round-trip through TOML: {error}"))?;
    let Value::Object(mut settings) =
        serde_json::to_value(document).map_err(|error| error.to_string())?
    else {
        return Err("serialized profile is not a table".into());
    };
    settings.remove("imported_otd");
    settings.remove("preserved_fields");
    settings.insert("tablet".into(), json!(tablet));
    settings.insert("output_mode".into(), json!(output_mode(profile)));
    // Default barrel bindings are omitted from TOML, but must remain visible
    // to inspection commands just like explicitly configured actions.
    settings.insert(
        "pen_buttons".into(),
        json!(profile.pen_buttons.iter().map(ToString::to_string).collect::<Vec<_>>()),
    );
    settings.insert(
        "aux_buttons".into(),
        json!(profile.aux_buttons.iter().map(ToString::to_string).collect::<Vec<_>>()),
    );
    settings.insert("mouse_buttons".into(), json!(profile.mouse_buttons.iter().map(ToString::to_string).collect::<Vec<_>>()));
    settings.insert("mouse_scroll_up".into(), json!(profile.mouse_scroll_up.to_string()));
    settings.insert("mouse_scroll_down".into(), json!(profile.mouse_scroll_down.to_string()));
    // Every wheel with every field; an absent threshold is one wheel step.
    settings.insert(
        "wheels".into(),
        Value::Array(
            profile
                .wheels
                .iter()
                .map(|wheel| {
                    json!({
                        "clockwise": wheel.clockwise.to_string(),
                        "counter_clockwise": wheel.counter_clockwise.to_string(),
                        "clockwise_threshold": wheel.clockwise_threshold,
                        "counter_clockwise_threshold": wheel.counter_clockwise_threshold,
                        "buttons": wheel.buttons.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
    );
    if section == "all" {
        settings.insert(
            "imported_otd_archive".into(),
            json!(profile.imported_otd.is_some()),
        );
        settings.insert(
            "preserved_field_count".into(),
            json!(profile.preserved_fields.len()),
        );
        return Ok(Value::Object(settings));
    }
    if section == "tools" {
        settings.insert("plugins".into(), serde_json::to_value(profile.plugins.iter()
            .filter(|plugin| plugin.kind == otd_core::plugins::PluginKind::DotnetTool).collect::<Vec<_>>()).map_err(|error| error.to_string())?);
    }
    let (_, keys) = SECTIONS
        .iter()
        .find(|(name, _)| *name == section)
        .ok_or_else(|| format!("unknown section {section:?}"))?;
    let mut selected = serde_json::Map::new();
    for key in ["tablet", "output_mode"].iter().chain(keys.iter()) {
        selected.insert((*key).into(), settings.remove(*key).unwrap_or(Value::Null));
    }
    Ok(Value::Object(selected))
}

fn native_summary(index: usize, name: Option<&str>, profile: &Profile) -> Result<Value, String> {
    let tablet = profile
        .tablet_name()?
        .unwrap_or_else(|| "Wacom PTH-660".into());
    Ok(json!({"index": index, "name": name, "tablet": tablet,
        "runtime_tablet_supported": otd_core::config::runtime_tablet(&tablet).is_ok(),
        "schema_version": profile.schema_version, "settings_revision": profile.settings_revision,
        "output_mode": output_mode(profile), "output": profile.output}))
}

fn print_json(value: &Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn write_output(path: &Path, text: &str) -> Result<(), String> {
    storage::save(path, text.as_bytes(), storage::SaveMode::CreateNew)?;
    println!("Saved {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Options, String> {
        parse_options(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn bindings_getter_shows_default_and_explicit_pen_buttons() {
        let mut profile = Profile::default();
        let bindings = profile_section(&profile, "bindings").unwrap();
        assert_eq!(
            bindings["pen_buttons"],
            json!(["barrel:1", "barrel:2", "barrel:3"])
        );
        assert!(bindings["bindings"].is_object());
        profile.pen_buttons = vec!["keys:Control+Z".parse().unwrap(), ButtonAction::None];
        assert_eq!(
            profile_section(&profile, "all").unwrap()["pen_buttons"],
            json!(["keys:LeftControl+Z", "none"])
        );
    }

    #[test]
    fn pen_button_options_are_bounded_distinct_and_set_only() {
        for args in [
            vec!["--pen-button", "0=none"],
            vec!["--pen-button", "65=none"],
            vec!["--pen-button", "one=none"],
            vec!["--pen-button", "1"],
            vec!["--pen-button", "1="],
            vec!["--pen-button", "1=keys:Mute"],
            vec!["--pen-button", "1=barrel:4"],
            vec!["--pen-button", "1=none", "--pen-button", "1=mouse:right"],
        ] {
            assert!(options(&args).is_err(), "{args:?}");
        }
        let valid = options(&["--output", "new.toml", "--pen-button", "64=mouse:right"])
            .unwrap();
        assert_eq!(valid.pen_buttons[0].0, 63);
        assert_eq!(valid.pen_buttons[0].1.to_string(), "mouse:right");
        validate_options("set", &valid).unwrap();
        for command in [
            "get", "list", "import", "export", "preview", "select", "recover",
        ] {
            assert!(validate_options(command, &valid).is_err(), "{command}");
        }
        assert!(validate_options("set", &options(&["--output", "new.toml"]).unwrap()).is_err());
    }

    #[test]
    fn set_pen_buttons_writes_absolute_copy_and_preserves_source() {
        let directory = std::env::temp_dir().join(format!(
            "otd-pen-buttons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("source.toml");
        let output = directory.join("edited.toml");
        let rejected = directory.join("rejected.toml");
        let original = Profile::default().to_toml().unwrap();
        std::fs::write(&source, &original).unwrap();
        let args = vec![
            "set".into(),
            source.to_string_lossy().into_owned(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
            "--pen-button".into(),
            "1=keys:Control+Z".into(),
            "--pen-button".into(),
            "5=mouse:forward".into(),
        ];
        run(args.clone()).unwrap();
        let Input::Native(edited) = read_input(&output).unwrap() else {
            panic!("native copy")
        };
        assert_eq!(edited.settings_revision, 1);
        assert!(edited.relative.is_none());
        assert_eq!(
            edited.pen_buttons.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "keys:LeftControl+Z", "barrel:2", "barrel:3", "none", "mouse:forward",
            ]
        );
        let saved = std::fs::read(&output).unwrap();
        assert!(run(args).is_err(), "existing output must be refused");
        assert_eq!(std::fs::read(&output).unwrap(), saved);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert!(
            run(vec![
                "set".into(),
                source.to_string_lossy().into_owned(),
                "--output".into(),
                rejected.to_string_lossy().into_owned(),
                "--pen-button".into(),
                "1=none".into(),
                "--sensitivity".into(),
                "1,1".into(),
            ])
            .is_err()
        );
        assert!(!rejected.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn set_edits_express_keys_and_wheels_and_get_shows_them() {
        let directory = std::env::temp_dir().join(format!(
            "otd-aux-buttons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("source.toml");
        let output = directory.join("edited.toml");
        std::fs::write(&source, Profile::default().to_toml().unwrap()).unwrap();
        run(vec![
            "set".into(),
            source.to_string_lossy().into_owned(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
            "--aux-button".into(),
            "2=keys:Control+Z".into(),
            "--wheel-clockwise".into(),
            "1=keys:PageDown".into(),
            "--wheel-counter-clockwise".into(),
            "1=keys:PageUp".into(),
            "--wheel-threshold".into(),
            "1=10".into(),
        ])
        .unwrap();
        let Input::Native(edited) = read_input(&output).unwrap() else {
            panic!("native copy")
        };
        let bindings = profile_section(&edited, "bindings").unwrap();
        assert_eq!(bindings["aux_buttons"], json!(["none", "keys:LeftControl+Z"]));
        assert_eq!(
            bindings["wheels"],
            json!([{
                "clockwise": "keys:PageDown",
                "counter_clockwise": "keys:PageUp",
                "clockwise_threshold": 10.0,
                "counter_clockwise_threshold": 10.0,
                "buttons": [],
            }])
        );
        assert_eq!(edited.pen_buttons, otd_core::output::buttons::default_pen_buttons());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn auxiliary_options_are_bounded_distinct_and_set_only() {
        for invalid in [
            vec!["--aux-button", "0=none"],
            vec!["--aux-button", "65=none"],
            vec!["--aux-button", "1=keys:Mute"],
            vec!["--aux-button", "1=none", "--aux-button", "1=mouse:right"],
            vec!["--wheel-clockwise", "9=none"],
            vec!["--wheel-clockwise", "1=wheel:up"],
            vec!["--wheel-threshold", "1=0"],
            vec!["--wheel-threshold", "1=nan"],
            vec!["--wheel-threshold", "1=5", "--wheel-threshold", "1=6"],
        ] {
            assert!(options(&invalid).is_err(), "{invalid:?}");
        }
        let get = options(&["--aux-button", "1=none"]).unwrap();
        assert!(validate_options("get", &get).is_err());
    }
    #[test]
    fn controls_edit_areas_output_and_policies_with_loader_validation() {
        let edits = options(&["--output", "new.toml", "--display-area", "1920,1080,960,540",
            "--tablet-area", "100,60,50,30,15", "--clipping", "false", "--limiting", "true",
            "--drag-only", "true", "--tip-threshold", "100", "--disable-tilt", "true"]).unwrap();
        validate_options("set", &edits).unwrap();
        assert!(validate_options("get", &edits).is_err());
        let mut profile = Profile::default();
        apply_controls(&mut profile, &edits).unwrap();
        assert!(profile.contact.drag_only && profile.contact.disable_tilt);
        assert_eq!(profile.contact.tip_threshold_raw, Some(profile.tablet.max_pressure));
        let mapping = profile.otd_mapping.unwrap();
        assert!(!mapping.clipping && mapping.limiting);
        assert_eq!(mapping.tablet.rotation, 15.0);
        let text = profile.to_toml().unwrap();
        Profile::from_toml_text(&text, Path::new("new.toml")).unwrap();
        assert!(apply_controls(&mut profile, &options(&["--monitor", "1"]).unwrap()).is_err());
        apply_controls(&mut profile, &options(&["--output-mode", "relative"]).unwrap()).unwrap();
        assert!(profile.relative.is_some() && profile.otd_mapping.is_none());
        assert!(apply_controls(&mut profile, &options(&["--tablet-area", "100,60,50,30"]).unwrap()).is_err());
    }

    #[test]
    fn controls_reject_invalid_or_duplicate_values() {
        for args in [vec!["--clipping", "yes"], vec!["--tip-threshold", "-1"],
            vec!["--eraser-threshold", "101"], vec!["--tablet-area", "0,10,0,0"],
            vec!["--display-area", "10,10,nan,0"], vec!["--output-mode", "unknown"],
            vec!["--drag-only", "true", "--drag-only", "false"],
            vec!["--plugin-enabled", "1=true", "--plugin-enabled", "1=false"],
            vec!["--mouse-button", "65=none"]] {
            assert!(options(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn radial_disable_enable_retains_values_and_reset_preserves_disabled_state() {
        let mut profile = Profile::default();
        let settings = otd_core::radial_follow::RadialFollowSettings { outer_radius: 23.0, ..Default::default() };
        profile.radial_follow.push(settings);
        apply_controls(&mut profile, &options(&["--radial-follow", "disable"]).unwrap()).unwrap();
        assert!(profile.radial_follow.is_empty());
        assert_eq!(profile.disabled_radial_follow.unwrap().outer_radius, 23.0);
        apply_controls(&mut profile, &options(&["--radial-follow", "enable"]).unwrap()).unwrap();
        assert_eq!(profile.radial_follow[0].outer_radius, 23.0);
        apply_controls(&mut profile, &options(&["--radial-follow", "disable"]).unwrap()).unwrap();
        apply_controls(&mut profile, &options(&["--radial-follow", "reset"]).unwrap()).unwrap();
        assert!(profile.radial_follow.is_empty());
        assert_eq!(profile.disabled_radial_follow.unwrap().outer_radius, 1.0);
    }

}
