use crate::protocol::{MAX_X, MAX_Y};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn valid(self) -> bool {
        self.width() > 0 && self.height() > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: MAX_X,
            height: MAX_Y,
        }
    }
}

impl Crop {
    pub fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.x.checked_add(self.width).is_some_and(|v| v <= MAX_X)
            && self.y.checked_add(self.height).is_some_and(|v| v <= MAX_Y)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Mapper {
    crop: Crop,
    rotation: u16,
    destination: Rect,
    virtual_screen: Rect,
}

impl Mapper {
    pub fn new(crop: Crop, rotation: u16, destination: Rect, virtual_screen: Rect) -> Option<Self> {
        if !crop.valid()
            || !matches!(rotation, 0 | 90 | 180 | 270)
            || !destination.valid()
            || !virtual_screen.valid()
        {
            return None;
        }
        Some(Self {
            crop,
            rotation,
            destination,
            virtual_screen,
        })
    }

    #[inline]
    fn scale(value: u32, denom: u32, span: i32) -> i32 {
        let span = span as u64;
        ((u64::from(value) * span + u64::from(denom) / 2) / u64::from(denom)) as i32
    }

    /// Returns normalized virtual-desktop coordinates for SendInput.
    pub fn map(self, x: u32, y: u32) -> (i32, i32) {
        let crop = self.crop;
        let u = x.saturating_sub(crop.x).min(crop.width);
        let v = y.saturating_sub(crop.y).min(crop.height);
        let (u, v, w, h) = match self.rotation {
            90 => (v, crop.width - u, crop.height, crop.width),
            180 => (crop.width - u, crop.height - v, crop.width, crop.height),
            270 => (crop.height - v, u, crop.height, crop.width),
            _ => (u, v, crop.width, crop.height),
        };
        let px = self.destination.left + Self::scale(u, w, self.destination.width() - 1);
        let py = self.destination.top + Self::scale(v, h, self.destination.height() - 1);
        let nx = Self::scale(
            (px - self.virtual_screen.left) as u32,
            (self.virtual_screen.width() - 1).max(1) as u32,
            65_535,
        );
        let ny = Self::scale(
            (py - self.virtual_screen.top) as u32,
            (self.virtual_screen.height() - 1).max(1) as u32,
            65_535,
        );
        (nx.clamp(0, 65_535), ny.clamp(0, 65_535))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_reach_edges_with_negative_screen_origin() {
        let screen = Rect {
            left: -1920,
            top: -1080,
            right: 1920,
            bottom: 1080,
        };
        let m = Mapper::new(Crop::default(), 0, screen, screen).unwrap();
        assert_eq!(m.map(0, 0), (0, 0));
        assert_eq!(m.map(MAX_X, MAX_Y), (65_535, 65_535));
    }

    #[test]
    fn monitor_selection_and_rotation() {
        let screen = Rect {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let left = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let m = Mapper::new(Crop::default(), 90, left, screen).unwrap();
        assert_eq!(m.map(0, MAX_Y), (32_759, 65_535));
        assert_eq!(m.map(MAX_X, 0), (0, 0));
    }

    #[test]
    fn invalid_crop_is_rejected() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let crop = Crop {
            x: MAX_X,
            ..Crop::default()
        };
        assert!(Mapper::new(crop, 0, screen, screen).is_none());
    }
}
