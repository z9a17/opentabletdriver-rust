//! What the mapping needs to know about the desktop: the virtual screen, the
//! monitor rectangles, and a cheap fingerprint for noticing changes. The
//! platform reads them; this module turns a snapshot into a `Mapper`.

use crate::config::Profile;
use crate::mapping::{Crop, Mapper, Rect};

/// The virtual screen and the monitor count: enough to notice a resolution or
/// monitor change from the report thread without enumerating monitors or
/// allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayFingerprint {
    pub virtual_screen: Rect,
    pub monitors: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaySnapshot {
    pub virtual_screen: Rect,
    /// Sorted by left, top, right, bottom.
    pub monitors: Vec<Rect>,
}

impl DisplaySnapshot {
    pub fn fingerprint(&self) -> DisplayFingerprint {
        DisplayFingerprint {
            virtual_screen: self.virtual_screen,
            monitors: i32::try_from(self.monitors.len()).unwrap_or(i32::MAX),
        }
    }

    pub fn mapper(&self, profile: &Profile) -> Result<Mapper, String> {
        if let Some(settings) = profile.otd_mapping {
            return Mapper::from_otd_for(settings, self.virtual_screen, profile.tablet)
                .ok_or_else(|| "invalid OpenTabletDriver absolute-area mapping".into());
        }
        let dest = if let Some(index) = profile.monitor {
            *self
                .monitors
                .get(index)
                .ok_or_else(|| format!("monitor {index} is not present"))?
        } else {
            self.virtual_screen
        };
        // The default crop means the whole digitizer, whatever its size.
        let crop = if profile.crop == Crop::default() {
            Crop::full(profile.tablet)
        } else {
            profile.crop
        };
        Mapper::new_for(
            crop,
            profile.rotation,
            dest,
            self.virtual_screen,
            profile.tablet,
        )
        .ok_or_else(|| "invalid tablet-to-display mapping".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_monitor_is_an_error_and_fingerprint_counts_monitors() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let snapshot = DisplaySnapshot {
            virtual_screen: screen,
            monitors: vec![screen],
        };
        let profile = Profile {
            monitor: Some(1),
            ..Profile::default()
        };
        assert_eq!(
            snapshot.mapper(&profile).unwrap_err(),
            "monitor 1 is not present"
        );
        assert!(snapshot.mapper(&Profile::default()).is_ok());
        assert_eq!(
            snapshot.fingerprint(),
            DisplayFingerprint {
                virtual_screen: screen,
                monitors: 1
            }
        );
    }
}
