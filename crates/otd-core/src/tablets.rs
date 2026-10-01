//! OpenTabletDriver's tablet configuration database (D01). The pinned upstream
//! files in `crates/otd-core/tablets` are embedded unchanged, parsed into
//! typed specifications and identifiers, validated, and indexed by USB vendor
//! and product ID. Files from a configuration directory override built-in
//! configurations by name, as in OpenTabletDriver. This module describes
//! devices; user profiles are in `config`. Nothing here runs per report.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/tablets.rs"));
}

/// Where the built-in configurations come from: repository, revision, path
/// and license.
pub const SOURCE: &str = include_str!("../tablets/SOURCE");

/// The upstream revision the built-in configurations were copied at.
pub fn source_revision() -> &'static str {
    SOURCE
        .lines()
        .find_map(|line| line.strip_prefix("Revision: "))
        .unwrap_or("unknown")
}

/// The parser OpenTabletDriver uses when an identifier names none.
pub const DEFAULT_PARSER: &str = "OpenTabletDriver.Plugin.Tablet.PassthroughReportParser";

/// Fields this schema does not know, kept as read.
pub type Unknown = BTreeMap<String, Value>;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TabletConfiguration {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specifications: Option<TabletSpecifications>,
    #[serde(default)]
    pub digitizer_identifiers: Vec<DeviceIdentifier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auxiliary_device_identifiers: Option<Vec<DeviceIdentifier>>,
    /// The misspelled legacy name, still read by OpenTabletDriver.
    #[serde(default, rename = "AuxilaryDeviceIdentifiers", skip_serializing)]
    pub legacy_auxiliary_device_identifiers: Option<Vec<DeviceIdentifier>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TabletSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digitizer: Option<DigitizerSpecifications>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pen: Option<PenSpecifications>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auxiliary_buttons: Option<ButtonSpecifications>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_buttons: Option<ButtonSpecifications>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wheels: Option<Vec<WheelSpecifications>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strips: Option<Vec<AnalogSpecifications>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch: Option<DigitizerSpecifications>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

/// Physical size in millimetres and the raw coordinate range.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DigitizerSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, rename = "MaxX", skip_serializing_if = "Option::is_none")]
    pub max_x: Option<f64>,
    #[serde(default, rename = "MaxY", skip_serializing_if = "Option::is_none")]
    pub max_y: Option<f64>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PenSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pressure: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_count: Option<u32>,
    /// The legacy `Buttons` object, still read by OpenTabletDriver.
    #[serde(default, rename = "Buttons", skip_serializing)]
    pub legacy_buttons: Option<ButtonSpecifications>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

impl PenSpecifications {
    /// The button count, from the current field or the legacy object.
    pub fn buttons(&self) -> Option<u32> {
        self.button_count
            .or_else(|| self.legacy_buttons.as_ref().and_then(|b| b.button_count))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ButtonSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_count: Option<u32>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

/// A wheel reports either an absolute position (`AbsoluteWheelMax`) or
/// relative steps (`RelativeWheelSteps`), never both.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct WheelSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_wheel_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_wheel_max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_of_zero_reading: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_count: Option<u32>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AnalogSpecifications {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_relative: Option<bool>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

/// Bytes written as base64, as OpenTabletDriver's JSON stores `byte[]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let digits = text.trim_end_matches('=');
    if !text.len().is_multiple_of(4) || text.len() - digits.len() > 2 {
        return Err(format!("{text:?} is not padded base64"));
    }
    let mut bytes = Vec::with_capacity(digits.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for character in digits.bytes() {
        let value = BASE64
            .iter()
            .position(|&digit| digit == character)
            .ok_or_else(|| format!("{text:?} is not base64"))?;
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    Ok(bytes)
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (i, &byte)| {
            value | u32::from(byte) << (16 - 8 * i)
        });
        for i in 0..4 {
            text.push(if i <= chunk.len() {
                char::from(BASE64[(value >> (18 - 6 * i) & 63) as usize])
            } else {
                '='
            });
        }
    }
    text
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_base64(&text).map(Bytes).map_err(de::Error::custom)
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_base64(&self.0))
    }
}

/// One USB interface of a tablet: how to recognize it and how to read it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceIdentifier {
    /// Kept wide so an out-of-range value is reported, not rejected unread.
    #[serde(default, rename = "VendorID", skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<i64>,
    #[serde(default, rename = "ProductID", skip_serializing_if = "Option::is_none")]
    pub product_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_report_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_report_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_report_length: Option<u32>,
    /// `None` means OpenTabletDriver's passthrough parser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_parser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_init_report: Option<Vec<Bytes>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_init_report: Option<Vec<Bytes>>,
    /// USB string descriptor index to a .NET regular expression the string
    /// must match. Matching belongs to device selection (D02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_strings: Option<BTreeMap<String, String>>,
    /// String descriptor indices to read while initializing the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_strings: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub unknown: Unknown,
}

impl DeviceIdentifier {
    pub fn vendor_id(&self) -> Option<u16> {
        self.vendor_id.and_then(|id| u16::try_from(id).ok())
    }

    pub fn product_id(&self) -> Option<u16> {
        self.product_id.and_then(|id| u16::try_from(id).ok())
    }

    pub fn parser(&self) -> &str {
        self.report_parser.as_deref().unwrap_or(DEFAULT_PARSER)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The configuration still loads; review it.
    Warning,
    /// The configuration cannot describe a usable device.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{label}: {}", self.message)
    }
}

/// How much of a report parser this driver implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParserSupport {
    /// Some of the parser's reports are decoded; the text says which.
    Partial(&'static str),
    /// Not implemented. A device that needs it is not supported, and no
    /// other parser stands in for it.
    Missing,
}

/// What the live driver does with a parser's reports.
pub const LIVE_SUPPORT: &str = "native decoder registered; the live driver uses pen position, pressure, tip, eraser and proximity, while separate auxiliary collections, buttons, wheels, strips and touch have no bindings yet; physical compatibility is unverified";

/// Passthrough produces raw reports only, so it cannot drive input.
pub fn parser_support(type_name: &str) -> ParserSupport {
    if type_name != DEFAULT_PARSER && crate::decoders::ReportParser::for_type(type_name).is_some() {
        ParserSupport::Partial(LIVE_SUPPORT)
    } else {
        ParserSupport::Missing
    }
}

/// `ReportParser` names are .NET type names: dot-separated identifiers,
/// OpenTabletDriver's `^([A-Za-z]+\w*)(\.[A-Za-z]+\w*)+$`.
fn valid_type_name(name: &str) -> bool {
    let mut segments = 0;
    for segment in name.split('.') {
        let mut characters = segment.chars();
        if !characters.next().is_some_and(|c| c.is_ascii_alphabetic())
            || !characters.all(|c| c.is_alphanumeric() || c == '_')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

struct Checker {
    diagnostics: Vec<Diagnostic>,
}

impl Checker {
    fn error(&mut self, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            message,
        });
    }

    fn warning(&mut self, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message,
        });
    }

    fn unknown(&mut self, at: &str, unknown: &Unknown) {
        if !unknown.is_empty() {
            let names: Vec<&str> = unknown.keys().map(String::as_str).collect();
            self.warning(format!(
                "{at}: unknown fields {}, kept as read",
                names.join(", ")
            ));
        }
    }

    fn count(&mut self, at: &str, value: Option<u32>) {
        if value.is_none() {
            self.error(format!("{at}.ButtonCount must be defined"));
        }
    }

    fn digitizer(&mut self, at: &str, digitizer: &DigitizerSpecifications) {
        for (name, value) in [
            ("Width", digitizer.width),
            ("Height", digitizer.height),
            ("MaxX", digitizer.max_x),
            ("MaxY", digitizer.max_y),
        ] {
            match value {
                None => self.error(format!("{at}.{name} must be defined")),
                Some(value) if !(value.is_finite() && value > 0.0) => {
                    self.error(format!("{at}.{name} must be positive, not {value}"))
                }
                Some(_) => {}
            }
        }
        self.unknown(at, &digitizer.unknown);
    }

    fn identifier(&mut self, at: &str, identifier: &DeviceIdentifier) {
        for (name, value) in [
            ("VendorID", identifier.vendor_id),
            ("ProductID", identifier.product_id),
        ] {
            match value {
                None => self.error(format!("{at}.{name} must be defined")),
                Some(value) if !(0..=0xffff).contains(&value) => {
                    self.error(format!("{at}.{name} {value} is outside 0..65535"))
                }
                Some(_) => {}
            }
        }
        if !valid_type_name(identifier.parser()) {
            self.error(format!(
                "{at}.ReportParser {:?} is not a type name",
                identifier.parser()
            ));
        }
        for key in identifier.device_strings.iter().flat_map(BTreeMap::keys) {
            if key.parse::<u8>().is_err() {
                self.error(format!("{at}.DeviceStrings index {key:?} is not 0..255"));
            }
        }
        self.unknown(at, &identifier.unknown);
    }
}

impl TabletConfiguration {
    /// Checks OpenTabletDriver's required fields and ranges, legacy fields
    /// and unknown fields.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut check = Checker {
            diagnostics: Vec::new(),
        };
        if self.name.trim().is_empty() {
            check.error("Name is required".into());
        }
        match &self.specifications {
            None => check.error("Specifications are required".into()),
            Some(specifications) => {
                match &specifications.digitizer {
                    None => check.error("Specifications.Digitizer must be defined".into()),
                    Some(digitizer) => check.digitizer("Specifications.Digitizer", digitizer),
                }
                match &specifications.pen {
                    None => check.error("Specifications.Pen must be defined".into()),
                    Some(pen) => {
                        if pen.max_pressure.is_none() {
                            check.error("Specifications.Pen.MaxPressure must be defined".into());
                        }
                        match (pen.button_count, &pen.legacy_buttons) {
                            (None, None) => check.count("Specifications.Pen", None),
                            (Some(count), Some(legacy)) if legacy.button_count != Some(count) => check
                                .error("Specifications.Pen: ButtonCount and the legacy Buttons disagree".into()),
                            (_, Some(_)) => check.warning(
                                "Specifications.Pen uses the legacy Buttons object; OpenTabletDriver now uses ButtonCount"
                                    .into(),
                            ),
                            _ => {}
                        }
                        check.unknown("Specifications.Pen", &pen.unknown);
                    }
                }
                for (name, buttons) in [
                    ("AuxiliaryButtons", &specifications.auxiliary_buttons),
                    ("MouseButtons", &specifications.mouse_buttons),
                ] {
                    if let Some(buttons) = buttons {
                        let at = format!("Specifications.{name}");
                        check.count(&at, buttons.button_count);
                        check.unknown(&at, &buttons.unknown);
                    }
                }
                for (index, wheel) in specifications.wheels.iter().flatten().enumerate() {
                    let at = format!("Specifications.Wheels[{index}]");
                    if wheel.absolute_wheel_max.is_some() == wheel.relative_wheel_steps.is_some() {
                        check.error(format!(
                            "{at}: exactly one of AbsoluteWheelMax and RelativeWheelSteps must be defined"
                        ));
                    }
                    if let Some(angle) = wheel.angle_of_zero_reading {
                        if !(0.0..=360.0).contains(&angle) {
                            check.error(format!(
                                "{at}.AngleOfZeroReading {angle} is outside 0..360"
                            ));
                        } else if wheel.relative_wheel_steps.is_some() {
                            check.warning(format!(
                                "{at}: a relative wheel should not define AngleOfZeroReading"
                            ));
                        }
                    }
                    check.count(&at, wheel.button_count);
                    check.unknown(&at, &wheel.unknown);
                }
                for (index, strip) in specifications.strips.iter().flatten().enumerate() {
                    let at = format!("Specifications.Strips[{index}]");
                    if strip.step_count.is_none() || strip.is_relative.is_none() {
                        check.error(format!("{at}: StepCount and IsRelative must be defined"));
                    }
                    check.unknown(&at, &strip.unknown);
                }
                if let Some(touch) = &specifications.touch {
                    check.digitizer("Specifications.Touch", touch);
                }
                check.unknown("Specifications", &specifications.unknown);
            }
        }
        if self.digitizer_identifiers.is_empty() {
            check.error("DigitizerIdentifiers needs at least one identifier".into());
        }
        match (&self.auxiliary_device_identifiers, &self.legacy_auxiliary_device_identifiers) {
            (Some(current), Some(legacy)) if current != legacy => check.error(
                "AuxiliaryDeviceIdentifiers and the legacy AuxilaryDeviceIdentifiers disagree".into(),
            ),
            (_, Some(_)) => check.warning(
                "uses the legacy AuxilaryDeviceIdentifiers; OpenTabletDriver now uses AuxiliaryDeviceIdentifiers".into(),
            ),
            _ => {}
        }
        for (role, identifiers) in [
            (
                "DigitizerIdentifiers",
                self.digitizer_identifiers.as_slice(),
            ),
            ("AuxiliaryDeviceIdentifiers", self.auxiliary_identifiers()),
        ] {
            for (index, identifier) in identifiers.iter().enumerate() {
                check.identifier(&format!("{role}[{index}]"), identifier);
            }
        }
        check.unknown("configuration", &self.unknown);
        check.diagnostics
    }

    /// The auxiliary identifiers under either spelling.
    pub fn auxiliary_identifiers(&self) -> &[DeviceIdentifier] {
        self.auxiliary_device_identifiers
            .as_deref()
            .or(self.legacy_auxiliary_device_identifiers.as_deref())
            .unwrap_or_default()
    }

    /// The fields in which `other` differs from this configuration, as paths
    /// such as `Specifications.Pen.MaxPressure`. Legacy field names compare
    /// as their current form.
    pub fn changed_fields(&self, other: &Self) -> Vec<String> {
        fn current(configuration: &TabletConfiguration) -> Value {
            let mut configuration = configuration.clone();
            if configuration.auxiliary_device_identifiers.is_none() {
                configuration.auxiliary_device_identifiers =
                    configuration.legacy_auxiliary_device_identifiers.take();
            }
            if let Some(pen) = configuration
                .specifications
                .as_mut()
                .and_then(|s| s.pen.as_mut())
            {
                pen.button_count = pen.buttons();
            }
            serde_json::to_value(configuration).expect("configurations serialize")
        }
        fn walk(path: String, a: &Value, b: &Value, changed: &mut Vec<String>) {
            match (a, b) {
                (Value::Object(a), Value::Object(b)) => {
                    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
                    for key in keys {
                        let at = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        let missing = &Value::Null;
                        walk(
                            at,
                            a.get(key).unwrap_or(missing),
                            b.get(key).unwrap_or(missing),
                            changed,
                        );
                    }
                }
                (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                    for (index, (a, b)) in a.iter().zip(b).enumerate() {
                        walk(format!("{path}[{index}]"), a, b, changed);
                    }
                }
                _ if a != b => changed.push(path),
                _ => {}
            }
        }
        let mut changed = Vec::new();
        walk(String::new(), &current(self), &current(other), &mut changed);
        changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Embedded from `crates/otd-core/tablets`; the path is relative to it.
    BuiltIn,
    /// Read from a configuration directory.
    File,
}

/// One configuration file and what loading it found.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub origin: Origin,
    /// `None` when the file is not a configuration object.
    pub configuration: Option<TabletConfiguration>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Entry {
    fn parse(path: String, origin: Origin, text: &str) -> Self {
        match serde_json::from_str::<TabletConfiguration>(text.trim_start_matches('\u{feff}')) {
            Ok(configuration) => Self {
                diagnostics: configuration.validate(),
                configuration: Some(configuration),
                path,
                origin,
            },
            Err(error) => Self {
                path,
                origin,
                configuration: None,
                diagnostics: vec![Diagnostic {
                    severity: Severity::Error,
                    message: format!("not a tablet configuration: {error}"),
                }],
            },
        }
    }

    /// Parsed and free of errors.
    pub fn usable(&self) -> Option<&TabletConfiguration> {
        let errors = self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error);
        self.configuration.as_ref().filter(|_| !errors)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Digitizer,
    Auxiliary,
}

/// A configured interface with the given USB IDs.
#[derive(Clone, Copy, Debug)]
pub struct Match<'a> {
    pub entry: &'a Entry,
    pub configuration: &'a TabletConfiguration,
    pub identifier: &'a DeviceIdentifier,
    pub role: Role,
    pub parser: ParserSupport,
}

/// An interface in the index: the entry's position, the identifier list and
/// the identifier's position in it.
type Slot = (usize, Role, usize);

/// The loaded configurations and a (vendor, product) index over the usable
/// ones.
#[derive(Clone)]
pub struct Database {
    entries: Vec<Entry>,
    index: BTreeMap<(u16, u16), Vec<Slot>>,
}

impl Database {
    /// The embedded upstream configurations, parsed once.
    pub fn builtin() -> &'static Database {
        static BUILTIN: OnceLock<Database> = OnceLock::new();
        BUILTIN.get_or_init(|| Self::with_overrides(&[]))
    }

    /// The embedded configurations with `files` (path, contents) applied in
    /// the given order, as OpenTabletDriver applies its configuration
    /// directory: the first file with a `Name` takes the place of the
    /// built-in configuration with that name, or is added after the built-in
    /// ones, and later files with the same name are ignored. A file that does
    /// not parse is kept with its error and replaces nothing; in
    /// OpenTabletDriver it stops detection (BC-25).
    pub fn with_overrides(files: &[(String, String)]) -> Database {
        let mut entries: Vec<Entry> = embedded::FILES
            .iter()
            .map(|(path, text)| Entry::parse((*path).to_owned(), Origin::BuiltIn, text))
            .collect();
        let mut first_files: BTreeMap<String, &str> = BTreeMap::new();
        for (path, text) in files {
            let mut entry = Entry::parse(path.clone(), Origin::File, text);
            if let Some(name) = entry.configuration.as_ref().map(|c| c.name.clone()) {
                if let Some(first) = first_files.get(&name) {
                    entry.diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!("ignored: {first} has the same Name and is read first"),
                    });
                } else {
                    first_files.insert(name.clone(), path);
                    let built_in = entries.iter().position(|e| {
                        e.origin == Origin::BuiltIn
                            && e.configuration.as_ref().is_some_and(|c| c.name == name)
                    });
                    if let Some(position) = built_in {
                        entries[position] = entry;
                        continue;
                    }
                }
            }
            entries.push(entry);
        }
        let mut database = Database {
            entries,
            index: BTreeMap::new(),
        };
        database.build_index();
        database
    }

    fn build_index(&mut self) {
        let mut shared: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (position, entry) in self.entries.iter().enumerate() {
            let Some(configuration) = entry.usable() else {
                continue;
            };
            for (role, identifiers) in [
                (
                    Role::Digitizer,
                    configuration.digitizer_identifiers.as_slice(),
                ),
                (Role::Auxiliary, configuration.auxiliary_identifiers()),
            ] {
                for (index, identifier) in identifiers.iter().enumerate() {
                    let (Some(vendor), Some(product)) =
                        (identifier.vendor_id(), identifier.product_id())
                    else {
                        continue;
                    };
                    self.index
                        .entry((vendor, product))
                        .or_default()
                        .push((position, role, index));
                    let key = format!(
                        "{vendor:04x}:{product:04x}, {:?} bytes, strings {:?}",
                        identifier.input_report_length, identifier.device_strings
                    );
                    shared
                        .entry(key)
                        .or_default()
                        .push(configuration.name.clone());
                }
            }
        }
        // Two tablets that declare the same interface cannot be told apart by
        // the declaration alone; device selection (D02) must choose.
        for (key, mut names) in shared {
            names.dedup();
            if names.len() > 1 {
                for entry in &mut self.entries {
                    if let Some(name) = entry.configuration.as_ref().map(|c| c.name.clone())
                        && names.contains(&name)
                    {
                        let others: Vec<&str> = names
                            .iter()
                            .filter(|n| **n != name)
                            .map(String::as_str)
                            .collect();
                        entry.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "interface {key} is also declared by {}",
                                others.join(", ")
                            ),
                        });
                    }
                }
            }
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every usable configured interface with these USB IDs, in file order.
    pub fn find(&self, vendor_id: u16, product_id: u16) -> impl Iterator<Item = Match<'_>> {
        self.index
            .get(&(vendor_id, product_id))
            .into_iter()
            .flatten()
            .map(move |&(position, role, index)| {
                let entry = &self.entries[position];
                let configuration = entry.configuration.as_ref().expect("indexed entries parse");
                let identifier = match role {
                    Role::Digitizer => &configuration.digitizer_identifiers[index],
                    Role::Auxiliary => &configuration.auxiliary_identifiers()[index],
                };
                Match {
                    entry,
                    configuration,
                    identifier,
                    role,
                    parser: parser_support(identifier.parser()),
                }
            })
    }

    /// Every report parser type the usable configurations name, with the
    /// number of identifiers that use it.
    pub fn parsers(&self) -> BTreeMap<&str, usize> {
        let mut parsers = BTreeMap::new();
        for configuration in self.entries.iter().filter_map(Entry::usable) {
            for identifier in configuration
                .digitizer_identifiers
                .iter()
                .chain(configuration.auxiliary_identifiers())
            {
                *parsers.entry(identifier.parser()).or_default() += 1;
            }
        }
        parsers
    }
}

/// Reads the `*.json` files under `directory` in the order OpenTabletDriver
/// enumerates its configuration directory: breadth first, each directory's
/// files before its subdirectories' files. Names within a directory are taken
/// in NTFS order, ignoring case; OpenTabletDriver takes the file system's
/// order, which on NTFS is the same. As in .NET, the extension ignores case
/// except on Linux. Returns (path, text) pairs for
/// [`Database::with_overrides`].
pub fn read_directory(directory: &Path) -> io::Result<Vec<(String, String)>> {
    let context = |path: &Path, error: io::Error| {
        io::Error::new(error.kind(), format!("{}: {error}", path.display()))
    };
    let mut files = Vec::new();
    let mut pending = VecDeque::from([directory.to_path_buf()]);
    while let Some(directory) = pending.pop_front() {
        let mut paths = std::fs::read_dir(&directory)
            .and_then(|entries| {
                entries
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<io::Result<Vec<_>>>()
            })
            .map_err(|error| context(&directory, error))?;
        paths.sort_by_cached_key(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            (name.to_uppercase(), name.into_owned())
        });
        for path in paths {
            if path.is_dir() {
                pending.push_back(path);
            } else if is_json(&path) {
                let bytes = std::fs::read(&path).map_err(|error| context(&path, error))?;
                files.push((path.display().to_string(), decode(&bytes)));
            }
        }
    }
    Ok(files)
}

/// Whether .NET's `*.json` pattern matches the file name.
fn is_json(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let extension = name
        .len()
        .checked_sub(5)
        .and_then(|start| name.get(start..))
        .unwrap_or_default();
    if cfg!(target_os = "linux") {
        extension == ".json"
    } else {
        extension.eq_ignore_ascii_case(".json")
    }
}

/// Decodes a file as .NET's `StreamReader` does by default: a UTF-8, UTF-16
/// or UTF-32 byte order mark selects the encoding, otherwise UTF-8, with
/// invalid bytes replaced.
fn decode(bytes: &[u8]) -> String {
    fn units<const N: usize, T>(bytes: &[u8], unit: fn([u8; N]) -> T) -> (Vec<T>, bool) {
        let (chunks, rest) = bytes.as_chunks::<N>();
        (
            chunks.iter().map(|chunk| unit(*chunk)).collect(),
            !rest.is_empty(),
        )
    }
    fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
        let (units, partial) = units(bytes, unit);
        let mut text: String = char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        if partial {
            text.push(char::REPLACEMENT_CHARACTER);
        }
        text
    }
    fn utf32(bytes: &[u8], unit: fn([u8; 4]) -> u32) -> String {
        let (units, partial) = units(bytes, unit);
        let mut text: String = units
            .into_iter()
            .map(|u| char::from_u32(u).unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        if partial {
            text.push(char::REPLACEMENT_CHARACTER);
        }
        text
    }
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xff, 0xfe, 0, 0, rest @ ..] => utf32(rest, u32::from_le_bytes),
        [0, 0, 0xfe, 0xff, rest @ ..] => utf32(rest, u32::from_be_bytes),
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PTH_660: &str =
        "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser";

    /// Git's object ID for a blob: SHA-1 over a "blob <length>\0" header and
    /// the contents.
    fn git_blob_id(contents: &[u8]) -> String {
        let mut message = format!("blob {}\0", contents.len()).into_bytes();
        message.extend_from_slice(contents);
        let length = (message.len() as u64) * 8;
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&length.to_be_bytes());
        let mut state: [u32; 5] = [
            0x6745_2301,
            0xefcd_ab89,
            0x98ba_dcfe,
            0x1032_5476,
            0xc3d2_e1f0,
        ];
        for block in message.chunks(64) {
            let mut words = [0u32; 80];
            for (word, bytes) in words.iter_mut().zip(block.chunks(4)) {
                *word = u32::from_be_bytes(bytes.try_into().unwrap());
            }
            for i in 16..80 {
                words[i] =
                    (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
            }
            let [mut a, mut b, mut c, mut d, mut e] = state;
            for (i, word) in words.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                    20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                    _ => (b ^ c ^ d, 0xca62_c1d6),
                };
                let next = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(*word);
                (e, d, c, b, a) = (d, c, b.rotate_left(30), a, next);
            }
            for (value, add) in state.iter_mut().zip([a, b, c, d, e]) {
                *value = value.wrapping_add(add);
            }
        }
        state.iter().map(|value| format!("{value:08x}")).collect()
    }

    fn inventory() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/parity/device-catalog.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn git_blob_id_matches_git() {
        assert_eq!(git_blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            git_blob_id(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [
            &b""[..],
            b"\x02",
            b"\x02\x04",
            b"\x02\x04\x00",
            b"\t\n\xe1J\x00\x00l=\x00",
        ] {
            assert_eq!(decode_base64(&encode_base64(bytes)).unwrap(), bytes);
        }
        assert_eq!(
            decode_base64("CQrhSgAAbD0A").unwrap(),
            b"\t\n\xe1J\x00\x00l=\x00"
        );
        assert!(decode_base64("CQE").is_err() && decode_base64("C*E=").is_err());
    }

    /// Every pinned file is embedded unchanged and loads with an explicit
    /// result that agrees with the inventory's independent reading.
    #[test]
    fn every_pinned_configuration_loads_as_the_inventory_describes_it() {
        let inventory = inventory();
        let records = inventory["configurations"].as_array().unwrap();
        let database = Database::builtin();
        assert_eq!(database.entries().len(), records.len());
        assert_eq!(records.len(), 357);
        let revision = inventory["upstream"]["revision"].as_str().unwrap();
        assert_eq!(
            source_revision(),
            revision,
            "tablets/SOURCE names another revision"
        );
        let files: BTreeMap<&str, &str> = embedded::FILES.iter().copied().collect();
        for record in records {
            let path = record["path"].as_str().unwrap();
            let relative = path
                .strip_prefix("OpenTabletDriver.Configurations/Configurations/")
                .unwrap();
            assert_eq!(
                git_blob_id(files[relative].as_bytes()),
                record["git_blob"].as_str().unwrap(),
                "{relative} was changed"
            );
            let entry = database
                .entries()
                .iter()
                .find(|e| e.path == relative)
                .unwrap();
            let errors: Vec<_> = entry
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .collect();
            assert!(errors.is_empty(), "{relative}: {errors:?}");
            let configuration = entry.usable().unwrap();
            assert_eq!(
                configuration.name,
                record["name"].as_str().unwrap(),
                "{relative}"
            );
            for (role, identifiers) in [
                ("digitizer", configuration.digitizer_identifiers.as_slice()),
                ("auxiliary", configuration.auxiliary_identifiers()),
            ] {
                let expected = record[role].as_array().unwrap();
                assert_eq!(identifiers.len(), expected.len(), "{relative} {role}");
                for (identifier, want) in identifiers.iter().zip(expected) {
                    let count = |list: &Option<Vec<Bytes>>| list.as_ref().map_or(0, Vec::len);
                    let mut indices: Vec<u64> = identifier
                        .device_strings
                        .iter()
                        .flat_map(BTreeMap::keys)
                        .map(|key| key.parse().unwrap())
                        .collect();
                    indices.sort_unstable();
                    let mut attributes: Vec<&str> = identifier
                        .attributes
                        .iter()
                        .flat_map(BTreeMap::keys)
                        .map(String::as_str)
                        .collect();
                    attributes.sort_unstable();
                    let ours = serde_json::json!({
                        "vendor_id": identifier.vendor_id,
                        "product_id": identifier.product_id,
                        "parser": identifier.parser(),
                        "input_report_length": identifier.input_report_length,
                        "output_report_length": identifier.output_report_length,
                        "feature_report_length": identifier.feature_report_length,
                        "feature_init_reports": count(&identifier.feature_init_report),
                        "output_init_reports": count(&identifier.output_init_report),
                        "initialization_strings": identifier.initialization_strings.as_ref().map_or(0, Vec::len),
                        "device_string_indices": indices,
                        "attribute_names": attributes,
                    });
                    // The inventory writes indices as strings.
                    let mut want = want.clone();
                    let mut indices: Vec<u64> = want["device_string_indices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|index| index.as_str().unwrap().parse().unwrap())
                        .collect();
                    indices.sort_unstable();
                    want["device_string_indices"] = serde_json::json!(indices);
                    assert_eq!(ours, want, "{relative} {role}");
                }
            }
        }
    }

    /// Every one of the 53 referenced parser types has a decoder.
    #[test]
    fn referenced_parsers_resolve_to_partial_or_missing() {
        let inventory = inventory();
        let mut referenced: Vec<&str> = inventory["referenced_parsers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parser| parser["type"].as_str().unwrap())
            .collect();
        referenced.sort_unstable();
        assert_eq!(referenced.len(), 53);
        let database = Database::builtin();
        let used: Vec<&str> = database.parsers().into_keys().collect();
        assert_eq!(used, referenced);
        let missing: Vec<&str> = referenced
            .iter()
            .copied()
            .filter(|parser| parser_support(parser) == ParserSupport::Missing)
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn the_pth_660_resolves_to_its_parser_and_others_never_fall_back_to_it() {
        let database = Database::builtin();
        let matches: Vec<Match> = database.find(0x056a, 0x0357).collect();
        let pen: Vec<_> = matches
            .iter()
            .filter(|m| m.role == Role::Digitizer && m.identifier.input_report_length == Some(192))
            .collect();
        assert_eq!(pen.len(), 1, "{matches:?}");
        assert_eq!(pen[0].configuration.name, "Wacom PTH-660");
        assert!(matches!(pen[0].parser, ParserSupport::Partial(_)));
        // Another vendor retains its own parser and implementation classification.
        let huion: Vec<Match> = database
            .entries()
            .iter()
            .filter_map(Entry::usable)
            .filter(|c| c.name.starts_with("Huion"))
            .flat_map(|c| {
                let id = &c.digitizer_identifiers[0];
                database
                    .find(id.vendor_id().unwrap(), id.product_id().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(!huion.is_empty());
        for m in huion {
            assert_ne!(m.identifier.parser(), PTH_660);
            assert_eq!(
                m.parser,
                parser_support(m.identifier.parser()),
                "{}",
                m.configuration.name
            );
        }
        // An identifier that names no parser gets OpenTabletDriver's
        // passthrough parser, which has no registered decoder, even with
        // the PTH-660's USB IDs. This differs from TabletReportParser.
        let unnamed: DeviceIdentifier =
            serde_json::from_str(r#"{"VendorID": 1386, "ProductID": 855}"#).unwrap();
        assert_eq!(unnamed.parser(), DEFAULT_PARSER);
        assert_eq!(parser_support(unnamed.parser()), ParserSupport::Missing);
    }

    #[test]
    fn shared_interfaces_are_reported() {
        let entry = Database::builtin()
            .entries()
            .iter()
            .find(|e| {
                e.configuration
                    .as_ref()
                    .is_some_and(|c| c.name == "XP-Pen Star 03")
            })
            .unwrap();
        assert!(
            entry
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("XP-Pen Star 03 V2")),
            "{:?}",
            entry.diagnostics
        );
    }

    #[test]
    fn legacy_and_unknown_fields_load_with_warnings() {
        let text = r#"{
            "Name": "Legacy Tablet",
            "Specifications": {
                "Digitizer": {"Width": 100, "Height": 60, "MaxX": 20000, "MaxY": 12000, "Skew": 0.5},
                "Pen": {"MaxPressure": 2047, "Buttons": {"ButtonCount": 2}}
            },
            "DigitizerIdentifiers": [{"VendorID": 1, "ProductID": 2, "InputReportLength": 8,
                "ReportParser": "Vendor.Parser", "FeatureInitReport": ["AgQA"], "Firmware": "1.0"}],
            "AuxilaryDeviceIdentifiers": [{"VendorID": 1, "ProductID": 3}],
            "Color": "Blue"
        }"#;
        let entry = Entry::parse("legacy.json".into(), Origin::File, text);
        let configuration = entry.usable().unwrap();
        assert_eq!(
            configuration
                .specifications
                .as_ref()
                .unwrap()
                .pen
                .as_ref()
                .unwrap()
                .buttons(),
            Some(2)
        );
        assert_eq!(configuration.auxiliary_identifiers().len(), 1);
        assert_eq!(
            configuration.auxiliary_identifiers()[0].parser(),
            DEFAULT_PARSER
        );
        assert_eq!(
            configuration.digitizer_identifiers[0]
                .feature_init_report
                .as_ref()
                .unwrap()[0]
                .0,
            [2, 4, 0]
        );
        assert_eq!(configuration.unknown["Color"], "Blue");
        let warnings: Vec<&str> = entry
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        for expected in [
            "legacy Buttons",
            "legacy AuxilaryDeviceIdentifiers",
            "Skew",
            "Firmware",
            "Color",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(expected)),
                "{expected}: {warnings:?}"
            );
        }
        assert!(
            entry
                .diagnostics
                .iter()
                .all(|d| d.severity == Severity::Warning)
        );
        // Unknown fields survive serialization; legacy names are written in
        // their current form.
        let written = serde_json::to_value(configuration).unwrap();
        assert_eq!(written["Color"], "Blue");
        assert_eq!(written["DigitizerIdentifiers"][0]["Firmware"], "1.0");
        assert!(written.get("AuxilaryDeviceIdentifiers").is_none());
    }

    #[test]
    fn invalid_and_contradictory_declarations_are_errors() {
        let errors = |text: &str| -> Vec<String> {
            Entry::parse("bad.json".into(), Origin::File, text)
                .diagnostics
                .into_iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message)
                .collect()
        };
        let base = |specifications: &str, identifiers: &str| {
            format!(
                r#"{{"Name": "Bad", "Specifications": {specifications}, "DigitizerIdentifiers": {identifiers}}}"#
            )
        };
        let good_spec = r#"{"Digitizer": {"Width": 1, "Height": 1, "MaxX": 1, "MaxY": 1}, "Pen": {"MaxPressure": 1, "ButtonCount": 0}}"#;
        let good_id = r#"[{"VendorID": 1, "ProductID": 1, "ReportParser": "A.B"}]"#;
        assert!(errors(&base(good_spec, good_id)).is_empty());
        assert!(!errors("[1, 2]").is_empty());
        assert!(!errors(&base(good_spec, "[]")).is_empty());
        assert!(!errors(&base(good_spec, r#"[{"VendorID": 70000, "ProductID": 1}]"#)).is_empty());
        assert!(
            !errors(&base(
                good_spec,
                r#"[{"VendorID": 1, "ProductID": 1, "ReportParser": "NoNamespace"}]"#
            ))
            .is_empty()
        );
        assert!(
            !errors(&base(
                good_spec,
                r#"[{"VendorID": 1, "ProductID": 1, "DeviceStrings": {"300": "x"}}]"#
            ))
            .is_empty()
        );
        let no_width = r#"{"Digitizer": {"Height": 1, "MaxX": 1, "MaxY": 1}, "Pen": {"MaxPressure": 1, "ButtonCount": 0}}"#;
        assert!(
            errors(&base(no_width, good_id))
                .iter()
                .any(|e| e.contains("Width"))
        );
        let zero = r#"{"Digitizer": {"Width": 0, "Height": 1, "MaxX": 1, "MaxY": 1}, "Pen": {"MaxPressure": 1, "ButtonCount": 0}}"#;
        assert!(!errors(&base(zero, good_id)).is_empty());
        let both = r#"{"Digitizer": {"Width": 1, "Height": 1, "MaxX": 1, "MaxY": 1}, "Pen": {"MaxPressure": 1, "ButtonCount": 0},
            "Wheels": [{"AbsoluteWheelMax": 71, "RelativeWheelSteps": 3, "ButtonCount": 1}]}"#;
        assert!(
            errors(&base(both, good_id))
                .iter()
                .any(|e| e.contains("exactly one"))
        );
        let disagree = r#"{"Digitizer": {"Width": 1, "Height": 1, "MaxX": 1, "MaxY": 1},
            "Pen": {"MaxPressure": 1, "ButtonCount": 2, "Buttons": {"ButtonCount": 3}}}"#;
        assert!(
            errors(&base(disagree, good_id))
                .iter()
                .any(|e| e.contains("disagree"))
        );
    }

    #[test]
    fn overrides_replace_by_name_and_add_new_tablets() {
        let builtin = Database::builtin();
        let mut pth = builtin
            .entries()
            .iter()
            .find_map(|e| {
                e.configuration
                    .clone()
                    .filter(|c| c.name == "Wacom PTH-660")
            })
            .unwrap();
        pth.specifications
            .as_mut()
            .unwrap()
            .pen
            .as_mut()
            .unwrap()
            .max_pressure = Some(4095);
        let mut added = pth.clone();
        added.name = "Custom Tablet".into();
        added.digitizer_identifiers[0].product_id = Some(0x9999);
        let database = Database::with_overrides(&[
            (
                "C:/overrides/pth.json".into(),
                serde_json::to_string(&pth).unwrap(),
            ),
            (
                "C:/overrides/custom.json".into(),
                serde_json::to_string(&added).unwrap(),
            ),
            ("C:/overrides/broken.json".into(), "{".into()),
        ]);
        assert_eq!(database.entries().len(), builtin.entries().len() + 2);
        let found = database.find(0x056a, 0x0357).next().unwrap();
        assert_eq!(found.entry.origin, Origin::File);
        assert_eq!(
            found
                .configuration
                .specifications
                .as_ref()
                .unwrap()
                .pen
                .as_ref()
                .unwrap()
                .max_pressure,
            Some(4095)
        );
        assert_eq!(
            database
                .find(0x056a, 0x9999)
                .next()
                .unwrap()
                .configuration
                .name,
            "Custom Tablet"
        );
        let broken = database
            .entries()
            .iter()
            .find(|e| e.path.ends_with("broken.json"))
            .unwrap();
        assert!(broken.usable().is_none());
    }

    /// OpenTabletDriver keeps one configuration per name, so a duplicate name
    /// would hide a tablet.
    #[test]
    fn built_in_names_are_unique() {
        let mut names = BTreeSet::new();
        for entry in Database::builtin().entries() {
            let name = &entry.configuration.as_ref().unwrap().name;
            assert!(names.insert(name.as_str()), "{name} is declared twice");
        }
    }

    #[test]
    fn the_pth_660_declares_what_the_report_path_implements() {
        use crate::protocol::{HEIGHT_MM, MAX_PRESSURE, MAX_X, MAX_Y, WIDTH_MM};

        let pen = Database::builtin()
            .find(0x056a, 0x0357)
            .find(|m| m.role == Role::Digitizer)
            .unwrap();
        let specifications = pen.configuration.specifications.as_ref().unwrap();
        let digitizer = specifications.digitizer.as_ref().unwrap();
        assert_eq!(
            (
                digitizer.width,
                digitizer.height,
                digitizer.max_x,
                digitizer.max_y
            ),
            (
                Some(WIDTH_MM),
                Some(HEIGHT_MM),
                Some(f64::from(MAX_X)),
                Some(f64::from(MAX_Y))
            )
        );
        assert_eq!(
            specifications.pen.as_ref().unwrap().max_pressure,
            Some(u32::from(MAX_PRESSURE))
        );
        assert_eq!(pen.identifier.input_report_length, Some(192));
        assert_eq!(pen.identifier.parser(), PTH_660);
    }

    #[test]
    fn the_first_file_with_a_name_wins() {
        let builtin = Database::builtin();
        let original = builtin
            .find(0x056a, 0x0357)
            .next()
            .unwrap()
            .configuration
            .clone();
        let with_pressure = |pressure| {
            let mut pth = original.clone();
            let pen = pth.specifications.as_mut().unwrap().pen.as_mut().unwrap();
            pen.max_pressure = Some(pressure);
            serde_json::to_string(&pth).unwrap()
        };
        let mut custom = original.clone();
        custom.name = "Custom Tablet".into();
        custom.digitizer_identifiers[0].product_id = Some(0x9999);
        let custom = serde_json::to_string(&custom).unwrap();
        let database = Database::with_overrides(&[
            ("a/pth.json".into(), with_pressure(4095)),
            ("b/pth.json".into(), with_pressure(2047)),
            ("custom.json".into(), custom.clone()),
            ("custom copy.json".into(), custom),
        ]);
        assert_eq!(database.entries().len(), builtin.entries().len() + 3);
        let pens: Vec<Match> = database
            .find(0x056a, 0x0357)
            .filter(|m| m.role == Role::Digitizer)
            .collect();
        assert_eq!(pens.len(), 1);
        assert_eq!(pens[0].entry.path, "a/pth.json");
        assert_eq!(
            original.changed_fields(pens[0].configuration),
            ["Specifications.Pen.MaxPressure"]
        );
        assert_eq!(database.find(0x056a, 0x9999).count(), 1);
        for (path, first) in [
            ("b/pth.json", "a/pth.json"),
            ("custom copy.json", "custom.json"),
        ] {
            let entry = database.entries().iter().find(|e| e.path == path).unwrap();
            assert!(entry.usable().is_none());
            // Field warnings from the file itself may come first.
            assert!(
                entry
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error && d.message.contains(first)),
                "{entry:?}"
            );
        }
    }

    #[test]
    fn legacy_names_compare_as_their_current_form() {
        let current: TabletConfiguration = serde_json::from_str(
            r#"{"Name": "T", "Specifications": {"Pen": {"MaxPressure": 1, "ButtonCount": 2}},
                "AuxiliaryDeviceIdentifiers": [{"VendorID": 1, "ProductID": 2}]}"#,
        )
        .unwrap();
        let legacy: TabletConfiguration = serde_json::from_str(
            r#"{"Name": "T", "Specifications": {"Pen": {"MaxPressure": 1, "Buttons": {"ButtonCount": 2}}},
                "AuxilaryDeviceIdentifiers": [{"VendorID": 1, "ProductID": 2}]}"#,
        )
        .unwrap();
        assert!(current.changed_fields(&legacy).is_empty());
        let mut other = legacy.clone();
        other.name = "U".into();
        other.legacy_auxiliary_device_identifiers.as_mut().unwrap()[0].product_id = Some(3);
        assert_eq!(
            current.changed_fields(&other),
            ["AuxiliaryDeviceIdentifiers[0].ProductID", "Name"]
        );
    }

    #[test]
    fn directories_are_read_in_dotnet_order() {
        let root = std::env::temp_dir().join(format!("otd-tablets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["sub/deeper", "a2"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (path, contents) in [
            ("b.json", &b"b"[..]),
            ("A.JSON", b"a"),
            (".json", b"dot"),
            ("notes.txt", b"not read"),
            ("a2/e.json", b"\xef\xbb\xbfe"),
            ("sub/c.json", b"\xff\xfec\x00"),
            ("sub/deeper/d.json", b"\xfe\xff\x00d"),
        ] {
            std::fs::write(root.join(path), contents).unwrap();
        }
        let files = read_directory(&root);
        std::fs::remove_dir_all(&root).unwrap();
        let read: Vec<(String, String)> = files
            .unwrap()
            .into_iter()
            .map(|(path, text)| {
                let relative = Path::new(&path).strip_prefix(&root).unwrap();
                (relative.to_string_lossy().replace('\\', "/"), text)
            })
            .collect();
        let mut expected = vec![
            (".json", "dot"),
            ("A.JSON", "a"),
            ("b.json", "b"),
            ("a2/e.json", "e"),
            ("sub/c.json", "c"),
            ("sub/deeper/d.json", "d"),
        ];
        if cfg!(target_os = "linux") {
            expected.remove(1);
        }
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(path, text)| (path.to_owned(), text.to_owned()))
            .collect();
        assert_eq!(read, expected);
        assert!(read_directory(&root).is_err());
    }

    #[test]
    fn text_is_decoded_as_dotnet_reads_it() {
        assert_eq!(decode(b"\xff\xfe\x00\x00A\x00\x00\x00"), "A");
        assert_eq!(decode(b"\x00\x00\xfe\xff\x00\x00\x00A"), "A");
        assert_eq!(decode(b"\xff\xfeA\x00\x00\xd8"), "A\u{fffd}");
        assert_eq!(decode(b"\xff\xfeA"), "\u{fffd}");
        assert_eq!(decode(b"a\xffb"), "a\u{fffd}b");
    }
}
