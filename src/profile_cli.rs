//! Offline profile inspection and migration commands. No device or plugin loads.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use otd_core::config::{ImportOptions, NativeProfileCollection, OtdSettingsDocument, Profile};
use serde_json::{Value, json};

pub fn usage() -> &'static str {
    "Profile commands (offline; indexes start at zero):
  profiles list INPUT.json|INPUT.toml
  profiles preview INPUT --profile INDEX [--legacy-force-radial-follow]
  profiles import INPUT --profile INDEX --output PROFILE.toml [--legacy-force-radial-follow]
  profiles export PROFILE.toml --output SETTINGS.json
  profiles select COLLECTION.toml --name NAME --output COLLECTION.toml

Output files must not already exist. The source file is never overwritten.
Importing a tablet profile does not establish runtime support for that tablet.
The legacy flag explicitly activates disabled Radial Follow stores during OTD import."
}

#[derive(Default)]
struct Options {
    profile: Option<usize>,
    output: Option<PathBuf>,
    name: Option<String>,
    legacy: bool,
}

enum Input {
    Otd(OtdSettingsDocument),
    Native(Profile),
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
    if !matches!(
        command.as_str(),
        "list" | "preview" | "import" | "export" | "select"
    ) {
        return Err(format!("unknown profile command {command:?}\n{}", usage()));
    }
    let input_path = PathBuf::from(
        args.next()
            .ok_or_else(|| format!("{command} requires an input file\n{}", usage()))?,
    );
    let options = parse_options(args)?;
    validate_options(&command, &options)?;
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
            write_output(
                options
                    .output
                    .as_deref()
                    .ok_or("import requires --output FILE")?,
                &profile.to_toml()?,
            )?;
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
            write_output(
                options
                    .output
                    .as_deref()
                    .ok_or("select requires --output FILE")?,
                &collection.to_toml()?,
            )
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

fn validate_options(command: &str, options: &Options) -> Result<(), String> {
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
        "export" => {
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
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))?;
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => OtdSettingsDocument::from_json(&text, path).map(Input::Otd),
        Some("toml") => {
            let document: toml::Value = toml::from_str(&text)
                .map_err(|error| format!("invalid TOML {}: {error}", path.display()))?;
            if document.get("format").and_then(toml::Value::as_str) == Some("profile_collection") {
                NativeProfileCollection::from_toml_text(&text, path).map(Input::Collection)
            } else {
                Profile::from_toml_text(&text, path).map(Input::Native)
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
        Input::Native(profile) if index == 0 => Ok(profile.clone()),
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

fn native_summary(index: usize, name: Option<&str>, profile: &Profile) -> Result<Value, String> {
    let tablet = profile
        .tablet_name()?
        .unwrap_or_else(|| "Wacom PTH-660".into());
    Ok(json!({"index": index, "name": name, "tablet": tablet,
        "runtime_tablet_supported": tablet == "Wacom PTH-660",
        "schema_version": profile.schema_version, "settings_revision": profile.settings_revision,
        "output_mode": if profile.relative.is_some() { "relative" } else { "absolute" }}))
}

fn print_json(value: &Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn write_output(path: &Path, text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("output {} already exists; choose a new path (source files are never overwritten)", path.display())
        } else { format!("cannot create output {}: {error}", path.display()) }
    })?;
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        // This branch is reachable only after this call created this exact file.
        // Never delete an existing output when create_new itself fails.
        return match fs::remove_file(path) {
            Ok(()) => Err(format!(
                "could not write {}; removed the newly created partial file: {error}",
                path.display()
            )),
            Err(cleanup) => Err(format!(
                "could not write {}: {error}; the newly created partial file remains because cleanup failed: {cleanup}",
                path.display()
            )),
        };
    }
    println!("Saved {}", path.display());
    Ok(())
}
