//! Literal cases transcribed from the current 0.6.x parser source at a126f7b.
//! These are separate from the unchanged-family 0.6.7 differential corpus.
use otd_core::{config::Profile, decoders::ReportParser, reports::*};
use std::{path::Path, time::Duration};

fn metadata() -> ReportMetadata {
    ReportMetadata {
        device: DeviceId(0), session: SessionId(0), endpoint: EndpointId(0),
        received_at: Duration::ZERO, sequence: 0,
    }
}

fn parser(name: &str) -> ReportParser {
    ReportParser::for_type(&format!("OpenTabletDriver.Configurations.Parsers.{name}")).unwrap()
}

#[test]
fn high_pressure_automatic_profiles_validate_against_selected_hardware() {
    let text = "tablet = \"*\"\n[bindings]\ntip_threshold_raw = 12288\n";
    let profile = Profile::from_toml_text(text, Path::new("memory.toml")).unwrap();
    assert_eq!(profile.tablet_name().unwrap(), None);
    let gaomon = otd_core::config::spec_for_tablet("Gaomon M7").unwrap();
    assert_eq!(profile.for_tablet(gaomon).unwrap().contact.tip_threshold_raw, Some(12288));
    assert!(profile.for_tablet(otd_core::spec::TabletSpec::PTH_660).is_err());
    let reloaded = Profile::from_toml_text(&profile.to_toml().unwrap(), Path::new("memory.toml")).unwrap();
    assert_eq!(reloaded.tablet_name().unwrap(), None);
}

#[test]
fn explicit_automatic_target_preserves_the_imported_document() {
    let original = r#"{"Profiles":[{"Tablet":"Wacom PTH-660"}]}"#;
    let mut profile = Profile {
        imported_otd: Some(otd_core::config::ImportedOtdSettings {
            source_path: "original.json".into(),
            settings_json: original.into(),
            selected_profile: 0,
            legacy_force_radial_follow: false,
        }),
        ..Profile::default()
    };
    assert_eq!(profile.tablet_name().unwrap().as_deref(), Some("Wacom PTH-660"));
    profile.target_tablet = Some("*".into());
    let reloaded = Profile::from_toml_text(&profile.to_toml().unwrap(), Path::new("memory.toml")).unwrap();
    assert_eq!(reloaded.tablet_name().unwrap(), None);
    assert_eq!(reloaded.imported_otd.unwrap().settings_json, original);
}

#[test]
fn kamvas_offset_coordinates_pressure_and_signed_tilt() {
    let raw = [8, 0x8e, 0x45, 0x23, 1, 0x45, 0x23, 0x56, 0x34, 0, 0x80, 1];
    let mut decoder = parser("Huion.KamvasOffsetReportParser");
    let (_, report) = decoder.parse(&raw, metadata()).unwrap();
    assert_eq!(report.values.position, Some([74565.0, 9029.0]));
    assert_eq!(report.values.pressure, Some(13398));
    assert_eq!(report.values.tilt, Some([128.0, -1.0]));
    for index in 0..3 { assert_eq!(report.values.pen_buttons.unwrap().get(index), Some(true)); }
    for length in 0..raw.len() { assert!(decoder.parse(&raw[..length], metadata()).is_err()); }
}

#[test]
fn xp_pen_status_bits_do_not_maximize_pressure() {
    let mut raw = [0u8; 14]; raw[1] = 0xa0; raw[6] = 0xff; raw[7] = 0xff; raw[13] = 1;
    let (_, report) = parser("XP_Pen.XP_PenGen2ReportParser").parse(&raw, metadata()).unwrap();
    assert_eq!(report.values.pressure, Some(16383));
    raw[13] = 0;
    let (_, report) = parser("XP_Pen.XP_PenGen2ReportParser").parse(&raw, metadata()).unwrap();
    assert_eq!(report.values.pressure, Some(8191));
}

#[test]
fn current_wheel_dispatch_retains_signed_deltas_and_buttons() {
    let raw = [8, 0xf0, 0, 0, 3, 0xfe, 2];
    let (_, report) = parser("Huion.GianoReportParser").parse(&raw, metadata()).unwrap();
    assert_eq!(report.values.relative_analog.unwrap().deltas.as_slice().get(0).copied(), Some(-2));
    assert_eq!(report.values.relative_analog.unwrap().deltas.as_slice().get(1).copied(), Some(2));
    assert_eq!(report.values.aux_buttons.unwrap().get(0), Some(true));
    let (_, report) = parser("UCLogic.UCLogicV1ReportParser").parse(&[8, 0xe0, 0, 0x10, 0xfe], metadata()).unwrap();
    assert_eq!(report.values.relative_analog.unwrap().deltas.as_slice().get(0).copied(), Some(-2));
    let (_, report) = parser("Veikk.VeikkTiltReportParser").parse(&[8, 0x42, 3, 1, 2], metadata()).unwrap();
    assert_eq!(report.values.relative_analog.unwrap().deltas.as_slice().get(0).copied(), Some(1));
    let (_, report) = parser("XP_Pen.XP_PenDeco03ReportParser").parse(&[8, 0xf0, 3, 0, 0, 0, 0, 0x40], metadata()).unwrap();
    assert_eq!(report.values.aux_buttons.unwrap().get(1), Some(true));
    assert_eq!(report.values.relative_analog.unwrap().deltas.as_slice().get(0).copied(), Some(1));
}

#[test]
fn wacom_rotation_survives_the_following_position_report() {
    let mut decoder = parser("Wacom.IntuosV1.IntuosV1ReportParser");
    let rotation = [2, 0xea, 0, 0, 0, 0, 2, 0x20, 0, 0];
    let (_, report) = decoder.parse(&rotation, metadata()).unwrap();
    assert_eq!(report.values.rotation, Some(8));
    let (_, report) = decoder.parse(&[2, 0xe0, 0, 0, 0, 0, 0, 0, 0, 0], metadata()).unwrap();
    assert_eq!(report.values.rotation, Some(8));
    decoder.reset();
    let (_, report) = decoder.parse(&[2, 0xe0, 0, 0, 0, 0, 0, 0, 0, 0], metadata()).unwrap();
    assert_eq!(report.values.rotation, Some(0));
}

#[test]
fn bamboo_wheel_requires_its_byte_but_mouse_does_not() {
    let mut decoder = parser("Wacom.Bamboo.BambooReportParser");
    assert!(decoder.parse(&[2, 0, 0, 0, 0, 0, 0, 0], metadata()).is_err());
    let (_, report) = decoder.parse(&[2, 0, 0, 0, 0, 0, 0, 0, 0x85], metadata()).unwrap();
    assert_eq!(report.values.absolute_analog.unwrap().positions.as_slice(), &[Some(5)]);
    let (_, report) = decoder.parse(&[2, 0xc0, 1, 0, 0, 0, 0, 0], metadata()).unwrap();
    assert!(report.values.absolute_analog.is_none());
    assert_eq!(report.values.position, Some([1.0, 0.0]));
}
