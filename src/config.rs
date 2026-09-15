use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::mapping::Crop;

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub monitor: Option<usize>,
    pub crop: Crop,
    pub rotation: u16,
    pub device_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    monitor: Option<usize>,
    rotation: Option<u16>,
    crop: Option<RawCrop>,
    device_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Profile {
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = fs::read_to_string(path)
            .map_err(|e| format!("cannot read profile {}: {e}", path.display()))?;
        let raw: RawProfile = toml::from_str(&text)
            .map_err(|e| format!("invalid profile {}: {e}", path.display()))?;
        let profile = Self {
            monitor: raw.monitor,
            crop: raw.crop.map_or_else(Crop::default, |c| Crop {
                x: c.x,
                y: c.y,
                width: c.width,
                height: c.height,
            }),
            rotation: raw.rotation.unwrap_or(0),
            device_path: raw.device_path,
        };
        if !profile.crop.valid() {
            return Err("crop must be nonzero and within 0..44800 X, 0..29600 Y".into());
        }
        if !matches!(profile.rotation, 0 | 90 | 180 | 270) {
            return Err("rotation must be 0, 90, 180, or 270".into());
        }
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_valid() {
        let p = Profile::load(None).unwrap();
        assert!(p.crop.valid());
        assert_eq!(p.rotation, 0);
    }
}
