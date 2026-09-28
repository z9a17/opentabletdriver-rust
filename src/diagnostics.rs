//! Explicit diagnostic export, with conservative privacy defaults. This module
//! never opens a tablet, loads a plugin or subscribes to the report thread.
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Profile;
use crate::control::{self, Command, Reply, Request};
use serde_json::{Value, json};

pub fn usage() -> &'static str {
    "Diagnostic bundle:
  diagnostics --output NEW_FILE.json [--config PROFILE.toml] [--include-private-details]

The default bundle omits paths, plugin property values, imported settings and log text.
Private details include paths and free-form daemon messages; review before sharing.
This command does not open HID devices, load plugins or change the running driver."
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let (mut output, mut config, mut private) = (None, None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" | "help" => {
                println!("{}", usage());
                return Ok(());
            }
            "--output" if output.is_none() => output = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--config" if config.is_none() => config = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--include-private-details" if !private => private = true,
            _ => {
                return Err(format!(
                    "unknown or repeated diagnostic option: {arg}\n{}",
                    usage()
                ));
            }
        }
    }
    let output =
        output.ok_or_else(|| format!("diagnostics needs --output NEW_FILE.json\n{}", usage()))?;
    let profile = config
        .as_deref()
        .map(|path| Profile::load(Some(path)))
        .transpose()?;
    write_new(
        &output,
        &serde_json::to_vec_pretty(&bundle(profile.as_ref(), private)?)
            .map_err(|error| error.to_string())?,
    )?;
    println!("Saved {}", output.display());
    Ok(())
}

/// The diagnostic bundle, as `diagnostics` writes it and the panel's Help >
/// Export diagnostics saves or copies it.
pub fn bundle(profile: Option<&Profile>, private: bool) -> Result<Value, String> {
    Ok(json!({
        "format":"opentabletdriver-rust-diagnostics", "schema_version":1,
        "privacy":if private { "private_details_included" } else { "redacted" },
        "build":{"version":env!("CARGO_PKG_VERSION"), "os":std::env::consts::OS,
            "architecture":std::env::consts::ARCH, "control_protocol":control::PROTOCOL_VERSION,
            "upstream_revision":otd_core::tablets::source_revision()},
        "backend":{"transport":"windows_usb_hid", "runtime_tablet":"any configuration with a supported parser",
            "output":"SendInput mouse", "hid_inspected":false, "plugins_loaded":false},
        "displays":display_summary(private),
        "profile":profile.map(|profile| profile_summary(profile, private)).transpose()?,
        "daemon":daemon_summary(private),
        "crashes":crash_summary(private),
        "excluded":["raw tablet reports", "plugin property values", "imported settings archive", "environment variables"],
    }))
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.is_empty() && !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn rectangle(rect: crate::mapping::Rect) -> Value {
    json!({"left":rect.left, "top":rect.top, "right":rect.right, "bottom":rect.bottom})
}

fn display_summary(private: bool) -> Value {
    match crate::display::read_snapshot() {
        Ok(displays) => json!({"available":true, "units":"pixels",
            "virtual_screen":rectangle(displays.virtual_screen),
            "monitors":displays.monitors.into_iter().map(rectangle).collect::<Vec<_>>()}),
        Err(error) => json!({"available":false, "error":private.then_some(error)}),
    }
}

fn profile_summary(profile: &Profile, private: bool) -> Result<Value, String> {
    let native: toml::Value =
        toml::from_str(&profile.to_toml()?).map_err(|error| error.to_string())?;
    let mut settings = serde_json::Map::new();
    // A whitelist avoids accidentally exporting a future unknown field/store.
    for key in [
        "monitor",
        "rotation",
        "crop",
        "relative",
        "absolute",
        "bindings",
        "radial_follow",
    ] {
        if let Some(value) = native.get(key) {
            settings.insert(
                key.into(),
                serde_json::to_value(value).map_err(|error| error.to_string())?,
            );
        }
    }
    let plugins: Vec<_> = profile
        .plugins
        .iter()
        .map(|plugin| {
            let mut result = json!({"kind":plugin.kind, "enabled":plugin.enabled,
            "has_settings":plugin.settings_json != "{}"});
            if private {
                result["path"] = json!(plugin.path);
                result["type"] = json!(plugin.type_name);
            }
            result
        })
        .collect();
    let mut result = json!({"schema_version":profile.schema_version,
        "settings_revision":profile.settings_revision, "settings":settings, "plugins":plugins,
        "diagnostic_count":profile.diagnostics.len(), "preserved_field_count":profile.preserved_fields.len(),
        "has_imported_archive":profile.imported_otd.is_some()});
    if private {
        result["source"] = json!(profile.source);
        result["device_path"] = json!(profile.device_path);
        // Diagnostic messages may contain arbitrary imported names or paths.
        result["diagnostics"] = json!(profile.diagnostics);
    }
    Ok(result)
}

fn daemon_summary(private: bool) -> Value {
    match control::request(&Request::new(1, Command::Status), Duration::from_secs(2)) {
        Ok(response) => match response.reply {
            Reply::Status { status } => {
                if private {
                    return json!({"available":true,"status":status});
                }
                json!({"available":true, "state":status.state, "generation":status.generation,
                    "has_profile":status.profile.is_some(), "has_error":status.last_error.is_some(),
                    "retained_log_lines":status.logs.len()})
            }
            Reply::Error { error } => json!({"available":false, "code":error.code,
                "message":private.then_some(error.message)}),
            _ => json!({"available":false, "reason":"unexpected_status_response"}),
        },
        Err(error) => json!({"available":false, "error_kind":format!("{:?}",error.kind()),
            "message":private.then(|| error.to_string())}),
    }
}

/// The newest crash records. Their messages may name files, so only the
/// private bundle includes them.
fn crash_summary(private: bool) -> Value {
    let records: Vec<_> = otd_core::crash::recent(10)
        .into_iter()
        .map(|record| {
            let mut result = json!({"time":record.time, "version":record.version,
                "role":record.role, "kind":record.kind, "thread":record.thread,
                "location":record.location});
            if private {
                result["message"] = json!(record.message);
            }
            result
        })
        .collect();
    json!({"recent":records})
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    otd_core::storage::save(path, data, otd_core::storage::SaveMode::CreateNew).map(|_| ())
}
