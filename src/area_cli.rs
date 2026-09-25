//! Offline area operations shared with the graphical editor.
use otd_core::areas::{Bounds, Conversion, fit_aspect, full_area, validate_area};
use otd_core::mapping::OtdArea;
use otd_core::tablets::Database;
use std::path::PathBuf;

pub fn usage() -> &'static str {
    "Area commands (offline; results are millimetres):
  area convert FORMAT A B C D [--tablet NAME] [--configurations DIRECTORY]
  area full [--tablet NAME] [--configurations DIRECTORY]
  area fit WIDTH_MM HEIGHT_MM ASPECT_RATIO

Converters and input order:
  percentage           Up Left Down Right, fractions (1 = 100%)
  wacom-veikk          Top Left Bottom Right, tablet report units
  xp-pen               W H X Y, XP Pen driver units
  gaomon-v2-otd067      Width Height X Y, report units (pinned X-for-Y quirk)
  gaomon-v2-corrected   Width Height X Y, report units (uses Y for Y offset)

The default tablet is Wacom PTH-660. Conversion does not select or start a tablet.
Results are JSON previews; profiles and active settings are not changed."
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        println!("{}", usage());
        return Ok(());
    };
    if matches!(command.as_str(), "--help" | "-h" | "help") {
        println!("{}", usage());
        return Ok(());
    }
    if command == "fit" {
        let width = number(args.next(), "WIDTH_MM")?;
        let height = number(args.next(), "HEIGHT_MM")?;
        let ratio = number(args.next(), "ASPECT_RATIO")?;
        if args.next().is_some() {
            return Err(usage().into());
        }
        if width <= 0.0 || height <= 0.0 || ratio <= 0.0 {
            return Err("width, height and aspect ratio must be positive".into());
        }
        let (fitted_width, fitted_height) = fit_aspect(
            Bounds {
                left: 0.0,
                top: 0.0,
                right: width,
                bottom: height,
            },
            ratio,
        );
        let area = OtdArea {
            width: fitted_width,
            height: fitted_height,
            x: width / 2.0,
            y: height / 2.0,
            rotation: 0.0,
        };
        validate_area(area)?;
        return print(serde_json::json!({"units":"mm", "operation":"fit", "area":area}));
    }
    let conversion = match command.as_str() {
        "full" => None,
        "convert" => {
            let format = args
                .next()
                .ok_or("convert needs a format; see area --help")?;
            let kind: Conversion = format.parse()?;
            let labels = kind.labels();
            let values = [
                number(args.next(), labels[0])?,
                number(args.next(), labels[1])?,
                number(args.next(), labels[2])?,
                number(args.next(), labels[3])?,
            ];
            Some((format, kind, values))
        }
        _ => return Err(format!("unknown area command {command:?}\n{}", usage())),
    };
    let mut tablet = None;
    let mut directory = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--tablet" if tablet.is_none() => {
                tablet = Some(args.next().ok_or("--tablet needs a name")?)
            }
            "--configurations" if directory.is_none() => {
                directory = Some(PathBuf::from(
                    args.next().ok_or("--configurations needs a directory")?,
                ))
            }
            _ => return Err(format!("unknown or repeated area option: {flag}")),
        }
    }
    let tablet = tablet.as_deref().unwrap_or("Wacom PTH-660");
    let (custom, _) = crate::load_tablets(directory.as_deref())?;
    let database = custom.as_ref().unwrap_or_else(|| Database::builtin());
    let configuration = database
        .entries()
        .iter()
        .filter_map(|entry| entry.usable())
        .find(|configuration| configuration.name == tablet)
        .ok_or_else(|| {
            format!("no valid tablet configuration named {tablet:?}; use tablets --list")
        })?;
    let digitizer = configuration
        .specifications
        .as_ref()
        .and_then(|specification| specification.digitizer.as_ref())
        .ok_or("selected tablet has no digitizer specification")?;
    match conversion {
        Some((format, kind, values)) => print(serde_json::json!({
            "units":"mm", "tablet":tablet, "converter":format,
            "input_labels":kind.labels(), "input_units":kind.input_units(), "input":values,
            "area":kind.convert(digitizer, values)?, "notice":kind.notice(),
            "source_revision":otd_core::tablets::source_revision()
        })),
        None => {
            print(serde_json::json!({"units":"mm", "tablet":tablet, "area":full_area(digitizer)?}))
        }
    }
}

fn number(text: Option<String>, name: &str) -> Result<f64, String> {
    text.ok_or_else(|| format!("missing {name}"))?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{name} must be a finite number"))
}

fn print(value: serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}
