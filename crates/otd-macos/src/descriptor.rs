//! Report lengths from a HID report descriptor, as Windows reports them in
//! `HIDP_CAPS`: the largest report of each kind in bytes, plus the report ID
//! byte, which Windows counts even when the device uses no report IDs.
//! OpenTabletDriver matches configurations on these lengths.
//! Copied from crates/otd-linux/src/descriptor.rs for the same HidSharp
//! normalized report contract; macOS native unnumbered reports omit byte 0.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lengths {
    pub input: u32,
    pub output: u32,
    pub feature: u32,
    /// Reports start with a report ID. Without IDs, hidraw returns reports
    /// without the leading zero byte Windows and HidSharp put there.
    pub uses_report_ids: bool,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    size: u32,
    count: u32,
    id: u8,
}

/// Parses the short and long items of a report descriptor. Returns `None`
/// for a truncated descriptor.
pub fn lengths(descriptor: &[u8]) -> Option<Lengths> {
    // Bits per report ID for input, output and feature reports.
    let mut bits: [BTreeMap<u8, u32>; 3] = Default::default();
    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut at = 0;
    while at < descriptor.len() {
        let prefix = descriptor[at];
        if prefix == 0xFE {
            // Long item: size byte, tag byte, data.
            let size = usize::from(*descriptor.get(at + 1)?);
            at += 3 + size;
            if at > descriptor.len() {
                return None;
            }
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            size => usize::from(size),
        };
        let data = descriptor.get(at + 1..at + 1 + size)?;
        let value = data
            .iter()
            .rev()
            .fold(0u32, |value, byte| (value << 8) | u32::from(*byte));
        at += 1 + size;
        match ((prefix >> 2) & 3, prefix >> 4) {
            // Main items: Input, Output, Feature.
            (0, tag @ (0x8 | 0x9 | 0xB)) => {
                let kind = match tag {
                    0x8 => 0,
                    0x9 => 1,
                    _ => 2,
                };
                let current = bits[kind].entry(globals.id).or_default();
                *current = current.saturating_add(globals.size.saturating_mul(globals.count));
            }
            (1, 0x7) => globals.size = value,
            (1, 0x8) => globals.id = value as u8,
            (1, 0x9) => globals.count = value,
            (1, 0xA) => stack.push(globals),
            (1, 0xB) => globals = stack.pop().unwrap_or_default(),
            _ => {}
        }
    }
    let longest = |kind: &BTreeMap<u8, u32>| {
        kind.values()
            .map(|bits| bits.div_ceil(8) + 1)
            .max()
            .unwrap_or(0)
    };
    Some(Lengths {
        input: longest(&bits[0]),
        output: longest(&bits[1]),
        feature: longest(&bits[2]),
        uses_report_ids: bits.iter().any(|kind| kind.keys().any(|id| *id != 0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_count_the_largest_report_and_its_id_byte() {
        // Report 2: 8 + 16 + 16 bits of input; report 3: 64 bits of input,
        // 2 bytes of feature; pushed globals do not leak.
        let descriptor = [
            0x05, 0x0D, // Usage Page (Digitizer)
            0x09, 0x02, // Usage (Pen)
            0xA1, 0x01, // Collection (Application)
            0x85, 0x02, // Report ID 2
            0x75, 0x08, 0x95, 0x01, 0x81, 0x02, // 1 x 8 bits input
            0x75, 0x10, 0x95, 0x02, 0x81, 0x02, // 2 x 16 bits input
            0xA4, // Push
            0x85, 0x03, 0x75, 0x08, 0x95, 0x08, 0x81, 0x02, // report 3: 8 bytes
            0x95, 0x02, 0xB1, 0x02, // report 3: 2 feature bytes
            0xB4, // Pop
            0x95, 0x01, 0x91, 0x02, // report 2: one 16-bit output field
            0xC0, // End Collection
        ];
        assert_eq!(
            lengths(&descriptor),
            Some(Lengths {
                input: 9,
                output: 3,
                feature: 3,
                uses_report_ids: true,
            })
        );
    }

    #[test]
    fn descriptors_without_report_ids_still_count_the_id_byte() {
        let descriptor = [0x75, 0x08, 0x95, 0x09, 0x81, 0x02];
        assert_eq!(lengths(&descriptor).unwrap().input, 10);
        assert_eq!(lengths(&descriptor).unwrap().output, 0);
        assert!(!lengths(&descriptor).unwrap().uses_report_ids);
    }

    #[test]
    fn long_items_are_skipped_and_truncation_is_rejected() {
        let descriptor = [
            0xFE, 0x02, 0x10, 0xAA, 0xBB, 0x75, 0x08, 0x95, 0x01, 0x81, 0x02,
        ];
        assert_eq!(lengths(&descriptor).unwrap().input, 2);
        assert_eq!(lengths(&[0x75]), None);
    }
}
