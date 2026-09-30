//! Pure endpoint selection for the pinned OpenTabletDriver v0.6.7 contract.
//! See `OpenTabletDriver/Driver.cs` at revision 736003ed. No endpoint is opened
//! or initialized here; transport adapters supply snapshots from discovery.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;

use crate::tablets::{Database, DeviceIdentifier, Match, ParserSupport, Role};

const DOTNET_DATE_PREFIX_EXCLUSION: &str = r"^(?!202\d-\d{2}-\d{2})";
static DATE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A202\d-\d{2}-\d{2}").expect("static date regex is valid"));

enum DeviceStringPattern {
    Rust(Regex),
    ExcludeDatePrefix,
}

impl DeviceStringPattern {
    fn compile(pattern: &str) -> Result<Self, regex::Error> {
        if pattern == DOTNET_DATE_PREFIX_EXCLUSION {
            Ok(Self::ExcludeDatePrefix)
        } else {
            Regex::new(pattern).map(Self::Rust)
        }
    }

    fn is_match(&self, value: &str) -> bool {
        match self {
            Self::Rust(regex) => regex.is_match(value),
            Self::ExcludeDatePrefix => !DATE_PREFIX.is_match(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    UsbHid,
    Other,
}

/// One discovered collection. `physical_id` must identify the parent device,
/// not the collection path; a serial number alone is not sufficient.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub path: String,
    pub physical_id: String,
    pub transport: Transport,
    pub vendor_id: u16,
    pub product_id: u16,
    pub can_open: bool,
    pub input_length: u32,
    pub output_length: u32,
    pub feature_length: u32,
    pub strings: BTreeMap<u8, String>,
    /// Discovery attributes. A constrained interface must be present; a HID
    /// report pattern is checked only on backends providing HID_REPORTS.
    pub attributes: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    Unavailable,
    WrongLength,
    MissingString(u8),
    InvalidPattern(String),
    WrongString(u8),
    WrongCollection,
    WrongHidReports,
    WrongInterface,
    UnsupportedTransport,
}

/// Apply the same predicate order as upstream `Driver.GetMatchingDevices`.
/// Configuration attributes supply defaults; identifier attributes win.
pub fn matches(endpoint: &Endpoint, candidate: &Match<'_>) -> Result<(), Rejection> {
    let id = candidate.identifier;
    if endpoint.transport != Transport::UsbHid {
        return Err(Rejection::UnsupportedTransport);
    }
    if endpoint.vendor_id != id.vendor_id().unwrap_or_default()
        || endpoint.product_id != id.product_id().unwrap_or_default()
        || !endpoint.can_open
    {
        return Err(Rejection::Unavailable);
    }
    if !matches_report_lengths(endpoint, id) {
        return Err(Rejection::WrongLength);
    }
    if let Some(strings) = &id.device_strings {
        for (index, pattern) in strings {
            let index = index
                .parse::<u8>()
                .map_err(|_| Rejection::MissingString(0))?;
            let value = endpoint
                .strings
                .get(&index)
                .ok_or(Rejection::MissingString(index))?;
            let predicate = DeviceStringPattern::compile(pattern)
                .map_err(|_| Rejection::InvalidPattern(pattern.clone()))?;
            if !predicate.is_match(value) {
                return Err(Rejection::WrongString(index));
            }
        }
    }
    let attribute = |key: &str| {
        id.attributes
            .as_ref()
            .and_then(|map| map.get(key))
            .or_else(|| {
                candidate
                    .configuration
                    .attributes
                    .as_ref()
                    .and_then(|map| map.get(key))
            })
    };
    // Upstream checks this Windows-only attribute on Windows only.
    if cfg!(target_os = "windows")
        && let Some(usage) = attribute("WinUsage")
    {
        let pattern = format!("&col{usage}");
        let regex = Regex::new(&pattern).map_err(|_| Rejection::InvalidPattern(pattern))?;
        if !regex.is_match(&endpoint.path) {
            return Err(Rejection::WrongCollection);
        }
    }
    if let Some(pattern) = attribute("HidReports")
        && let Some(reports) = endpoint.attributes.as_ref().and_then(|a| a.get("HID_REPORTS"))
    {
        let regex = Regex::new(pattern).map_err(|_| Rejection::InvalidPattern(pattern.clone()))?;
        if !regex.is_match(reports) { return Err(Rejection::WrongHidReports); }
    }
    if let Some(interface) = attribute("Interface")
        && endpoint.attributes.as_ref().and_then(|a| a.get("USB_INTERFACE_NUMBER")) != Some(interface)
    {
        return Err(Rejection::WrongInterface);
    }
    Ok(())
}

/// Shared by descriptor discovery and selection so impossible collections
/// never need indexed USB string requests. Omitted sizes remain wildcards.
pub fn matches_report_lengths(endpoint: &Endpoint, id: &DeviceIdentifier) -> bool {
    id.input_report_length.is_none_or(|n| n == endpoint.input_length)
        && id.output_report_length.is_none_or(|n| n == endpoint.output_length)
        && id.feature_report_length.is_none_or(|n| n == endpoint.feature_length)
}

/// Regex patterns in a loaded database which Rust cannot parse. Run at setup
/// before selection so a .NET-only pattern cannot disappear as a mere miss.
pub fn unsupported_patterns(database: &Database) -> Vec<(String, String)> {
    let mut unsupported = Vec::new();
    for entry in database.entries() {
        let Some(config) = entry.usable() else {
            continue;
        };
        for id in config
            .digitizer_identifiers
            .iter()
            .chain(config.auxiliary_identifiers())
        {
            if let Some(strings) = &id.device_strings {
                for pattern in strings.values() {
                    if DeviceStringPattern::compile(pattern).is_err() {
                        unsupported.push((entry.path.clone(), pattern.clone()));
                    }
                }
            }
            if let Some(pattern) = id.attributes.as_ref().and_then(|a| a.get("HidReports"))
                .or_else(|| config.attributes.as_ref().and_then(|a| a.get("HidReports")))
                && Regex::new(pattern).is_err()
            {
                unsupported.push((entry.path.clone(), pattern.clone()));
            }
            let usage = id
                .attributes
                .as_ref()
                .and_then(|a| a.get("WinUsage"))
                .or_else(|| config.attributes.as_ref().and_then(|a| a.get("WinUsage")));
            if let Some(usage) = usage {
                let pattern = format!("&col{usage}");
                if Regex::new(&pattern).is_err() {
                    unsupported.push((entry.path.clone(), pattern));
                }
            }
        }
    }
    unsupported
}

pub struct Selection<'a> {
    pub digitizer: Match<'a>,
    pub digitizer_endpoint: &'a Endpoint,
    pub auxiliary: Option<(Match<'a>, &'a Endpoint)>,
    /// More than one collection met the chosen identifier.
    pub ambiguous_digitizer: bool,
}

/// Choose the first usable configuration, then its first matching digitizer
/// identifier and endpoint, as upstream does. Auxiliary endpoints must belong
/// to the same physical device. Selection does not imply parser support.
pub fn select<'a>(database: &'a Database, endpoints: &'a [Endpoint]) -> Option<Selection<'a>> {
    for entry in database.entries() {
        let Some(config) = entry.usable() else {
            continue;
        };
        for id in &config.digitizer_identifiers {
            let Some((vendor, product)) = id.vendor_id().zip(id.product_id()) else {
                continue;
            };
            let Some(candidate) = database.find(vendor, product).find(|m| {
                m.role == Role::Digitizer
                    && std::ptr::eq(m.entry, entry)
                    && std::ptr::eq(m.identifier, id)
            }) else {
                continue;
            };
            let mut found = endpoints
                .iter()
                .filter(|ep| matches(ep, &candidate).is_ok());
            let Some(digitizer_endpoint) = found.next() else {
                continue;
            };
            let ambiguous_digitizer = found.next().is_some();
            let auxiliary = config.auxiliary_identifiers().iter().find_map(|aux_id| {
                let (vendor, product) = aux_id.vendor_id().zip(aux_id.product_id())?;
                let aux = database.find(vendor, product).find(|m| {
                    m.role == Role::Auxiliary
                        && std::ptr::eq(m.entry, entry)
                        && std::ptr::eq(m.identifier, aux_id)
                })?;
                endpoints
                    .iter()
                    .find(|ep| {
                        ep.path != digitizer_endpoint.path
                            && !ep.physical_id.is_empty()
                            && ep.physical_id == digitizer_endpoint.physical_id
                            && matches(ep, &aux).is_ok()
                    })
                    .map(|ep| (aux, ep))
            });
            return Some(Selection {
                digitizer: candidate,
                digitizer_endpoint,
                auxiliary,
                ambiguous_digitizer,
            });
        }
    }
    None
}

impl Selection<'_> {
    /// Return the parser's actual capability; partial decoding is not full
    /// device support, and a missing parser is never replaced implicitly.
    pub fn parser_support(&self) -> ParserSupport {
        self.digitizer.parser
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(path: &str, physical: &str, input: u32) -> Endpoint {
        Endpoint {
            path: path.into(),
            physical_id: physical.into(),
            transport: Transport::UsbHid,
            vendor_id: 0x056a,
            product_id: 0x0357,
            can_open: true,
            input_length: input,
            output_length: 0,
            feature_length: 0,
            strings: BTreeMap::new(),
            attributes: Some(BTreeMap::new()),
        }
    }

    const SPECS: &str = r#""Specifications":{"Digitizer":{"Width":1,"Height":1,"MaxX":1,"MaxY":1},"Pen":{"MaxPressure":1,"ButtonCount":0}}"#;

    #[test]
    fn pairs_only_same_physical_tablet_and_reports_ambiguity() {
        let endpoints = [
            endpoint("pen-a", "a", 192),
            endpoint("pen-b", "b", 192),
            endpoint("aux-b", "b", 44),
            endpoint("aux-a", "a", 44),
        ];
        let selected = select(Database::builtin(), &endpoints).unwrap();
        assert_eq!(selected.digitizer.configuration.name, "Wacom PTH-660");
        assert_eq!(selected.digitizer_endpoint.path, "pen-a");
        assert_eq!(selected.auxiliary.unwrap().1.path, "aux-a");
        assert!(selected.ambiguous_digitizer);
        assert!(matches!(
            selected.parser_support(),
            ParserSupport::Partial(_)
        ));
    }

    #[test]
    fn rejects_wrong_collection_and_denied_access() {
        let mut wrong = endpoint("wrong", "a", 44);
        assert!(select(Database::builtin(), &[wrong.clone()]).is_none());
        wrong.input_length = 192;
        wrong.can_open = false;
        assert!(select(Database::builtin(), &[wrong]).is_none());
    }

    #[test]
    fn string_and_attribute_predicates_are_applied() {
        let json = format!(
            r#"{{"Name":"Fixture tablet",{SPECS},"DigitizerIdentifiers":[{{"VendorID":1386,"ProductID":855,"InputReportLength":192,"DeviceStrings":{{"2":"^Pen$"}},"Attributes":{{"WinUsage":"01","Interface":"2"}}}}]}}"#
        );
        let db = Database::with_overrides(&[("fixture.json".into(), json)]);
        let mut ep = endpoint("device&col01", "a", 192);
        ep.strings.insert(2, "Pen".into());
        ep.attributes
            .as_mut()
            .unwrap()
            .insert("USB_INTERFACE_NUMBER".into(), "2".into());
        // Built-in PTH-660 precedes an added override, so inspect the fixture's predicate directly.
        let candidate = db
            .find(0x056a, 0x0357)
            .find(|m| m.configuration.name == "Fixture tablet")
            .unwrap();
        assert_eq!(matches(&ep, &candidate), Ok(()));
        ep.path = "device&col02".into();
        // Upstream applies WinUsage on Windows only.
        let expected = if cfg!(target_os = "windows") {
            Err(Rejection::WrongCollection)
        } else {
            Ok(())
        };
        assert_eq!(matches(&ep, &candidate), expected);
        ep.path = "device&col01".into();
        ep.attributes.as_mut().unwrap().clear();
        assert_eq!(matches(&ep, &candidate), Err(Rejection::WrongInterface));
        ep.attributes = None;
        assert_eq!(matches(&ep, &candidate), Err(Rejection::WrongInterface));
        ep.attributes = Some(BTreeMap::new());
        ep.attributes
            .as_mut()
            .unwrap()
            .insert("USB_INTERFACE_NUMBER".into(), "2".into());
        ep.strings.insert(2, "Other".into());
        assert_eq!(matches(&ep, &candidate), Err(Rejection::WrongString(2)));
    }

    #[test]
    fn pinned_regex_catalog_pattern_support_is_audited() {
        let (mut strings, mut usages) = (0, 0);
        for entry in Database::builtin().entries() {
            let Some(config) = entry.usable() else {
                continue;
            };
            for id in config
                .digitizer_identifiers
                .iter()
                .chain(config.auxiliary_identifiers())
            {
                strings += id.device_strings.as_ref().map_or(0, BTreeMap::len);
                if id
                    .attributes
                    .as_ref()
                    .and_then(|a| a.get("WinUsage"))
                    .or_else(|| config.attributes.as_ref().and_then(|a| a.get("WinUsage")))
                    .is_some()
                {
                    usages += 1;
                }
            }
        }
        assert_eq!(
            (strings, usages),
            (216, 0),
            "review changed pinned regex inventory"
        );
        let unsupported = unsupported_patterns(Database::builtin());
        assert!(
            unsupported.is_empty(),
            "review unsupported .NET patterns: {unsupported:?}"
        );
    }

    #[test]
    fn pinned_deco_01_v2_excludes_date_shaped_serial_prefixes() {
        let database = Database::builtin();
        let candidate = database
            .find(10_429, 2_309)
            .find(|candidate| {
                candidate.configuration.name == "XP-Pen Deco 01 V2"
                    && candidate
                        .identifier
                        .device_strings
                        .as_ref()
                        .is_some_and(|strings| {
                            strings
                                .get("5")
                                .is_some_and(|pattern| pattern == DOTNET_DATE_PREFIX_EXCLUSION)
                        })
            })
            .expect("pinned Deco 01 V2 identifier and pattern");
        let mut ep = endpoint("deco-v2", "deco-v2", 12);
        ep.vendor_id = 10_429;
        ep.product_id = 2_309;
        ep.output_length = 10;
        ep.strings.insert(4, "UG901_BPU1002".into());

        for serial in ["2024-01-31", "2024-01-31-extra"] {
            ep.strings.insert(5, serial.into());
            assert_eq!(matches(&ep, &candidate), Err(Rejection::WrongString(5)));
        }

        for serial in ["", "2024-1-31", "2024-01-3", "ABC-2024-01-31"] {
            ep.strings.insert(5, serial.into());
            assert_eq!(matches(&ep, &candidate), Ok(()), "serial {serial:?}");
        }
    }

    #[test]
    fn unsupported_transport_and_parser_are_explicit() {
        let mut ep = endpoint("pen", "a", 192);
        ep.transport = Transport::Other;
        assert!(select(Database::builtin(), &[ep]).is_none());
        let json = format!(
            r#"{{"Name":"Wacom PTH-660",{SPECS},"DigitizerIdentifiers":[{{"VendorID":1386,"ProductID":855,"InputReportLength":192,"ReportParser":"Example.MissingParser"}}]}}"#
        );
        let db = Database::with_overrides(&[("Wacom/PTH-660.json".into(), json)]);
        let endpoints = [endpoint("pen", "a", 192)];
        let selected = select(&db, &endpoints).unwrap();
        assert_eq!(selected.parser_support(), ParserSupport::Missing);
    }
}
