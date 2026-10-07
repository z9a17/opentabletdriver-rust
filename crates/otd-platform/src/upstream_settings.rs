pub use crate::original_rpc_protocol as protocol;
pub mod collection { pub fn empty() -> serde_json::Value { serde_json::json!({"Revision":"0.6.7.0","Profiles":[],"Tools":[],"LockUsableAreaDisplay":true,"LockUsableAreaTablet":true}) } }
#[path = "../../../src/upstream_rpc/settings.rs"]
mod common;
pub fn defaults(tablets: &[otd_core::tablets::TabletConfiguration], screen: otd_core::mapping::Rect) -> Result<serde_json::Value, String> {
    let mut value = common::defaults(tablets, screen).map_err(|error| error.message)?;
    // Pinned Profile.DefaultOutputModeType selects LinuxArtistMode on Linux.
    #[cfg(target_os = "linux")]
    for profile in value["Profiles"].as_array_mut().into_iter().flatten() {
        profile["OutputMode"]["Path"] = otd_core::config::LINUX_ARTIST_MODE.into();
    }
    Ok(value)
}
pub fn canonical_copy(profile: &otd_core::config::Profile, tablet: &otd_core::tablets::TabletConfiguration, screen: otd_core::mapping::Rect) -> Result<otd_core::config::Profile,String> {
    common::canonical_copy(profile,tablet,screen).map_err(|error| error.message)
}
