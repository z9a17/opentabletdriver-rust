//! Canonical stores/defaults from pinned Desktop Settings/Profiles sources.
//! This constructs an export copy only; it never reads/writes user settings.
use serde_json::{Value, json};
use otd_core::config::{ImportedOtdSettings, Profile};
use otd_core::mapping::Rect;
use otd_core::spec::TabletSpec;
use otd_core::tablets::TabletConfiguration;
use super::protocol::Error;

fn adaptive(action: &str) -> Value {
    json!({"Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,
        "Settings":[{"Property":"Binding","Value":action}]})
}
/// ProfileCollection.GetProfile creates defaults for a detected tablet whose
/// name is missing. Existing profile order/unknown inactive rows are retained.
pub fn for_detected(settings:&Value,tablets:&[TabletConfiguration],screen:Rect) -> Result<Value,Error> {
    if settings.is_null() { return defaults(tablets,screen); }
    let mut document = settings.clone();
    let profiles = document.get_mut("Profiles").and_then(Value::as_array_mut)
        .ok_or_else(|| Error::invalid("settings must contain Profiles"))?;
    for tablet in tablets {
        if profiles.iter().any(|profile| profile["Tablet"] == tablet.name) { continue; }
        let generated = defaults(std::slice::from_ref(tablet),screen)?;
        profiles.push(generated["Profiles"][0].clone());
    }
    Ok(document)
}

pub fn defaults(tablets: &[TabletConfiguration], screen: Rect) -> Result<Value, Error> {
    if !screen.valid() { return Err(Error::failed("default settings need a valid virtual screen")); }
    let mut profiles = Vec::new();
    for tablet in tablets {
        let spec = TabletSpec::from_configuration(tablet)?;
        let controls = spec.controls;
        let declared = tablet.specifications.as_ref().ok_or_else(|| Error::failed("tablet specifications are missing"))?;
        if declared.pen.as_ref().and_then(|pen| pen.buttons()).unwrap_or(0) != u32::from(controls.pen_buttons)
            || declared.auxiliary_buttons.as_ref().and_then(|buttons| buttons.button_count).unwrap_or(0) != u32::from(controls.aux_buttons)
            || declared.mouse_buttons.as_ref().and_then(|buttons| buttons.button_count).unwrap_or(0) != u32::from(controls.mouse_buttons)
            || declared.wheels.as_ref().map_or(0,Vec::len) != controls.wheels().len() {
            return Err(Error::unsupported("ResetSettings", "declared controls exceed the native report representation"));
        }
        let pen: Vec<_> = (0..controls.pen_buttons).map(|index| match index {
            0 => adaptive("Button 1"), 1 => adaptive("Button 2"),
            2 => adaptive("Button 3"), _ => Value::Null,
        }).collect();
        let mut wheels = Vec::new();
        for wheel in controls.wheels() {
            let step = wheel.degrees_per_step().ok_or_else(|| Error::unsupported("ResetSettings", "detected wheel has no defined step count"))?;
            wheels.push(json!({"WheelButtons":vec![Value::Null;usize::from(wheel.buttons)],
                "ClockwiseRotation":null,"CounterClockwiseRotation":null,
                "ClockwiseActivationThreshold":step as f32,"CounterClockwiseActivationThreshold":step as f32,"StepSize":step}));
        }
        // Pinned AreaSettings.GetDefaults(IVirtualScreen) uses dimensions / 2;
        // it does not add the virtual screen origin. Preserve that default.
        profiles.push(json!({"Tablet":tablet.name,
            "OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true,"Settings":[]},
            "AbsoluteModeSettings":{"Display":{"Width":screen.width(),"Height":screen.height(),
                "X":screen.width() as f64 / 2.0,"Y":screen.height() as f64 / 2.0,"Rotation":0},
                "Tablet":{"Width":spec.width_mm,"Height":spec.height_mm,"X":spec.width_mm / 2.0,"Y":spec.height_mm / 2.0,"Rotation":0},
                "EnableClipping":true,"EnableAreaLimiting":false,"LockAspectRatio":false},
            "RelativeModeSettings":{"XSensitivity":10,"YSensitivity":10,"RelativeRotation":0,"RelativeResetDelay":"00:00:00.1000000"},
            "Bindings":{"TipButton":adaptive("Tip"),"EraserButton":adaptive("Eraser"),
                "TipActivationThreshold":1,"EraserActivationThreshold":1,
                "PenButtons":pen,"AuxButtons":vec![Value::Null;usize::from(controls.aux_buttons)],
                "MouseButtons":vec![Value::Null;usize::from(controls.mouse_buttons)],"MouseScrollUp":null,"MouseScrollDown":null,
                "WheelBindings":wheels,"EnableDragBindings":false,"DisablePressure":false,"DisableTilt":false},"Filters":[]}));
    }
    Ok(json!({"Revision":"0.6.7.0","Profiles":profiles,"Tools":[],"LockUsableAreaDisplay":true,"LockUsableAreaTablet":true}))
}

pub fn canonical_copy(profile: &Profile, tablet: &TabletConfiguration, screen: Rect) -> Result<Profile, Error> {
    if profile.imported_otd.is_some() { return Ok(profile.clone()); }
    if !profile.plugins.is_empty() || !profile.preserved_fields.is_empty() {
        return Err(Error::unsupported("GetSettings", "standalone DLL stores/unknown native extensions have no exact OTD representation"));
    }
    if profile.otd_mapping.is_none() && profile.relative.is_none() {
        return Err(Error::unsupported("GetSettings", "native pixel-span crop mapping requires a behavior-changing OTD conversion; use explicit absolute areas or relative settings"));
    }
    if profile.contact.tip_threshold_raw.is_none() || profile.contact.eraser_threshold_raw.is_none() {
        return Err(Error::unsupported("GetSettings", "hardware tip-switch contact has no exact OTD pressure-threshold representation"));
    }
    let mut document = defaults(std::slice::from_ref(tablet), screen)?;
    let selected = &mut document["Profiles"][0];
    // Identity/order is known for these native built-ins. Seed each actual RF
    // slot before using the strict core differential serializer for all edits.
    let mut filters = Vec::new();
    for (settings, enabled) in profile.radial_follow.iter().map(|settings| (settings,true))
        .chain(profile.disabled_radial_follow.iter().map(|settings| (settings,false))) {
        filters.push(json!({"Path":otd_core::radial_follow::FILTER_PATH,"Enable":enabled,"Settings":[
            {"Property":"OuterRadius","Value":settings.outer_radius},
            {"Property":"InnerRadius","Value":settings.inner_radius},
            {"Property":"SmoothingCoefficient","Value":settings.smoothing_coefficient},
            {"Property":"SoftKneeScale","Value":settings.soft_knee_scale},
            {"Property":"SmoothingLeakCoefficient","Value":settings.smoothing_leak_coefficient}]}));
    }
    selected["Filters"] = json!(filters);
    // Seed null arrays with the actual runtime lengths. This avoids claiming
    // discarded default bindings when a native profile intentionally has fewer.
    selected["Bindings"]["PenButtons"] = json!(vec![Value::Null;profile.pen_buttons.len()]);
    selected["Bindings"]["AuxButtons"] = json!(vec![Value::Null;profile.aux_buttons.len()]);
    selected["Bindings"]["MouseButtons"] = json!(vec![Value::Null;profile.mouse_buttons.len()]);
    selected["Bindings"]["WheelBindings"] = json!([]);
    let mut copy = profile.for_tablet(TabletSpec::from_configuration(tablet)?)?;
    copy.target_tablet = Some(tablet.name.clone());
    copy.imported_otd = Some(ImportedOtdSettings { source_path: "upstream-rpc-canonical.json".into(),
        settings_json: serde_json::to_string(&document).map_err(|error| Error::failed(error.to_string()))?,
        selected_profile:0, legacy_force_radial_follow:false });
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tablet() -> TabletConfiguration {
        let database = otd_core::config::configured_tablets().unwrap();
        database.entries().iter().filter_map(|entry| entry.usable())
            .find(|configuration| configuration.name == "Wacom PTH-660").unwrap().clone()
    }
    #[test]
    fn pinned_defaults_use_detected_controls_and_original_virtual_screen_origin_rule() {
        let tablet = tablet();
        let screen = Rect { left:-1920,top:0,right:1920,bottom:1080 };
        let settings = defaults(std::slice::from_ref(&tablet), screen).unwrap();
        let profile = &settings["Profiles"][0];
        assert_eq!(profile["Tablet"],tablet.name);
        assert_eq!(profile["AbsoluteModeSettings"]["Display"]["X"],1920.0);
        assert_eq!(profile["Bindings"]["TipActivationThreshold"],1);
        assert_eq!(profile["Bindings"]["PenButtons"].as_array().unwrap().len(),
            usize::from(TabletSpec::from_configuration(&tablet).unwrap().controls.pen_buttons));
        let imported = Profile::from_otd_text(&settings.to_string(),std::path::Path::new("defaults.json")).unwrap();
        assert!(imported.radial_follow.is_empty());
        assert_eq!(imported.contact.tip_threshold_percent,Some(1.0));
    }
    #[test]
    fn missing_detected_profiles_generate_defaults_without_dropping_unknown_rows() {
        let tablet = tablet();
        let screen = Rect { left:0,top:0,right:2560,bottom:1440 };
        let original = json!({"Profiles":[{"Tablet":"Disconnected model","Opaque":[null,17]}],"Tools":[],"Extra":"keep"});
        let generated = for_detected(&original,std::slice::from_ref(&tablet),screen).unwrap();
        assert_eq!(generated["Profiles"][0],original["Profiles"][0]);
        assert_eq!(generated["Extra"],"keep");
        assert_eq!(generated["Profiles"][1]["Tablet"],tablet.name);
        assert_eq!(for_detected(&generated,std::slice::from_ref(&tablet),screen).unwrap(),generated);
        assert_eq!(for_detected(&Value::Null,std::slice::from_ref(&tablet),screen).unwrap(),defaults(&[tablet],screen).unwrap());
    }
    #[test]
    fn standalone_explicit_areas_export_through_strict_serializer_and_refuse_native_extensions() {
        let tablet = tablet();
        let screen = Rect { left:0,top:0,right:2560,bottom:1440 };
        let settings = defaults(std::slice::from_ref(&tablet),screen).unwrap();
        let mut profile = Profile::from_otd_text(&settings.to_string(),std::path::Path::new("defaults.json")).unwrap();
        profile.imported_otd = None;
        profile.radial_follow.push(Default::default());
        let text = canonical_copy(&profile,&tablet,screen).unwrap().to_otd_json().unwrap();
        let roundtrip = Profile::from_otd_text(&text,std::path::Path::new("canonical.json")).unwrap();
        assert_eq!(roundtrip.otd_mapping,profile.otd_mapping);
        assert_eq!(roundtrip.radial_follow.len(),1);
        assert_eq!(roundtrip.pen_buttons,profile.pen_buttons);
        assert_eq!(roundtrip.contact.tip_threshold_raw,profile.contact.tip_threshold_raw);
        profile.contact.tip_threshold_raw = None;
        assert!(canonical_copy(&profile,&tablet,screen).is_err());
        profile.otd_mapping = None;
        assert!(canonical_copy(&profile,&tablet,screen).is_err());
    }
}
