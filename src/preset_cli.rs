//! Offline named presets. These commands never contact the daemon or load DLLs.
use std::path::PathBuf;

use otd_core::config::Profile;
use otd_core::presets::{PresetName, PresetStore};
use otd_core::storage;
use serde_json::{Value, json};

pub fn usage() -> &'static str {
    "Named presets (offline; does not select or apply a running configuration):
  presets list
  presets show NAME
  presets save NAME --config FILE [--replace]
  presets export NAME --output NEWFILE

Names preserve case and spacing: 1-64 ASCII letters/digits, internal spaces,
hyphens, underscores or parentheses; Windows device names are rejected.
Use quotes around names with spaces. Case-only aliases are not substituted.
Save creates a new preset by default; --replace requires an existing valid preset.
Show includes full profile values and plugin paths; review before sharing.
Presets live below the selected data directory (OTD_RUST_PORTABLE_DIR if set).
No GUI integration, runtime activation or delete command is provided here."
}

enum Command {
    List,
    Show(PresetName),
    Save {
        name: PresetName,
        config: PathBuf,
        replace: bool,
    },
    Export {
        name: PresetName,
        output: PathBuf,
    },
    Help,
}

fn parse(args: Vec<String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Command::Help);
    };
    if matches!(command.as_str(), "list" | "help" | "--help" | "-h") {
        if args.next().is_some() {
            return Err(format!("presets {command} takes no arguments"));
        }
        return Ok(if command == "list" {
            Command::List
        } else {
            Command::Help
        });
    }
    if !matches!(command.as_str(), "show" | "save" | "export") {
        return Err(format!("unknown preset command {command:?}\n{}", usage()));
    }
    let name = PresetName::parse(&args.next().ok_or("preset command needs a NAME")?)?;
    let mut config = None;
    let mut output = None;
    let mut replace = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--replace" if command == "save" && !replace => replace = true,
            "--config" if command == "save" && config.is_none() => {
                config = Some(PathBuf::from(value(&mut args, &flag)?));
            }
            "--output" if command == "export" && output.is_none() => {
                output = Some(PathBuf::from(value(&mut args, &flag)?));
            }
            _ => {
                return Err(format!(
                    "unknown, repeated or incompatible preset option {flag:?}"
                ));
            }
        }
    }
    match command.as_str() {
        "show" => Ok(Command::Show(name)),
        "save" => Ok(Command::Save {
            name,
            config: config.ok_or("presets save requires --config FILE")?,
            replace,
        }),
        "export" => Ok(Command::Export {
            name,
            output: output.ok_or("presets export requires --output NEWFILE")?,
        }),
        _ => unreachable!("validated command"),
    }
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.is_empty() && !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a path"))
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let command = parse(args)?;
    if matches!(command, Command::Help) {
        println!("{}", usage());
        return Ok(());
    }
    let store = PresetStore::user()?;
    match command {
        Command::List => {
            let listing = store.list()?;
            print(
                json!({ "directory": store.directory(), "presets": listing.presets, "warnings": listing.warnings }),
            )
        }
        Command::Show(name) => {
            let preset = store.load(&name)?;
            print(json!({"name": preset.name(), "path": preset.path(),
                "settings_revision": preset.profile().settings_revision,
                "profile_toml": preset.profile().to_toml()?}))
        }
        Command::Save {
            name,
            config,
            replace,
        } => {
            // Capture the replacement target before loading/relocating the source.
            // Concurrent changes during preparation must conflict at publication.
            let previous = replace.then(|| store.load(&name)).transpose()?;
            let path = std::path::absolute(config).map_err(|error| error.to_string())?;
            let source = storage::read_utf8(&path)?;
            let profile = Profile::from_toml_text(&source.text, &path)?;
            let preset = store.save(&name, &profile, previous.as_ref())?;
            print(json!({"name": preset.name(), "path": preset.path(),
                "settings_revision": preset.profile().settings_revision, "replaced": replace,
                "backup": if replace { Some(storage::backup_path(preset.path())?) } else { None }}))
        }
        Command::Export { name, output } => {
            let snapshot = store.export(&name, &output)?;
            print(json!({"name": name.as_str(), "output": snapshot.path()}))
        }
        Command::Help => unreachable!("help returned before selecting storage"),
    }
}

fn print(value: Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}
