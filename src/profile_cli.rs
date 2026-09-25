//! Offline profile inspection and migration commands. No device or plugin loads.
use std::path::{Path, PathBuf};
use std::time::Duration;

use otd_core::config::{ImportOptions, NativeProfileCollection, OtdSettingsDocument, Profile};
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
      [--relative-rotation DEGREES] [--reset-time MS]
  profiles paths

Output files must not already exist. The source file is never overwritten.
Importing a tablet profile does not establish runtime support for that tablet.
The legacy flag explicitly activates disabled Radial Follow stores during OTD import.
Recovery reads the sibling .bak into a new file; it never replaces the source.
Get prints JSON for one section: all, output, areas, sensitivity, bindings,
filters or misc (default all). A collection defaults to its selected profile and
a single Rust profile to index 0; OTD JSON requires --profile.
Set changes relative-mode settings only and writes a new profile with the next
settings revision; it neither contacts the daemon nor applies the result.
OTD_RUST_PORTABLE_DIR selects an explicit absolute portable settings directory."
}

/// Profile keys shown by `profiles get --section`, grouped like the upstream
/// console getters (`getareas`, `getsensitivity`, `getbindings`, `getfilters`,
/// `getmiscsettings`). `output` and `all` are handled separately.
const SECTIONS: [(&str, &[&str]); 5] = [
    ("areas", &["monitor", "rotation", "crop", "absolute"]),
    ("sensitivity", &["relative"]),
    ("bindings", &["bindings"]),
    ("filters", &["radial_follow", "plugins"]),
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
}

impl Options {
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
            let mut relative = profile
                .relative
                .ok_or("set changes relative-mode settings; this profile uses absolute output")?;
            if let Some(sensitivity) = options.sensitivity {
                relative.sensitivity = sensitivity;
            }
            if let Some(rotation) = options.relative_rotation {
                relative.rotation = rotation;
            }
            if let Some(ms) = options.reset_time_ms {
                relative.reset_delay = Duration::from_millis(ms);
            }
            profile.relative = Some(relative.validate()?);
            profile.advance_revision()?;
            let output = options
                .output
                .as_deref()
                .ok_or("set requires --output FILE")?;
            write_output(output, &profile.to_toml_at(output)?)
        }
        _ => unreachable!("command was validated"),
    }
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
                        "unknown section {section:?}; use all, output, areas, sensitivity, bindings, filters or misc"
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
            _ => return Err(format!("unknown profile option {flag:?}\n{}", usage())),
        }
    }
    Ok(options)
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
                && options.sets_relative()
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
        return Ok(json!({"tablet": tablet, "output_mode": output_mode(profile)}));
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
        "runtime_tablet_supported": tablet == "Wacom PTH-660",
        "schema_version": profile.schema_version, "settings_revision": profile.settings_revision,
        "output_mode": output_mode(profile)}))
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
