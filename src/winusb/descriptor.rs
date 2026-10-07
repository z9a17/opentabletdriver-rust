//! Bounded HID descriptor metadata for the pinned Windows WinUSB backend.
//! Lengths include the report-ID byte even for an unnumbered feature report,
//! as HidSharp's ReportDescriptor does. USB input/output sizes come from pipes.

use std::collections::BTreeMap;

#[derive(Default, Clone, Copy)]
struct Globals { page: u32, size: u32, count: u32, id: u8 }

#[derive(Debug, PartialEq, Eq)]
pub struct Metadata {
    pub feature_length: u32,
    pub reports: String,
    pub usage_page: u16,
    pub usage: u16,
}

/// Reject malformed, excessive, or ambiguous layouts instead of inventing
/// metadata that could select the wrong configuration. This runs at discovery.
pub fn parse(data: &[u8]) -> Option<Metadata> {
    if data.is_empty() || data.len() > 4096 { return None; }
    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut collections: Vec<Option<usize>> = Vec::new();
    let mut devices: Vec<Vec<u32>> = Vec::new();
    let mut usages = Vec::new();
    let mut usage_min = None;
    let mut usage_max = None;
    let mut input: Vec<(u8, usize)> = Vec::new();
    let mut feature = BTreeMap::<u8, u32>::new();
    let mut at = 0;
    while at < data.len() {
        let prefix = data[at];
        if prefix == 0xfe {
            let size = usize::from(*data.get(at + 1)?);
            at = at.checked_add(3 + size)?;
            if at > data.len() { return None; }
            continue;
        }
        let size = if prefix & 3 == 3 { 4 } else { usize::from(prefix & 3) };
        let bytes = data.get(at + 1..at + 1 + size)?;
        let value = bytes.iter().rev().fold(0u32, |n, b| (n << 8) | u32::from(*b));
        at += 1 + size;
        let extended = if size == 4 { value } else { (globals.page << 16) | value };
        match ((prefix >> 2) & 3, prefix >> 4) {
            (1, 0) => { if value > u16::MAX as u32 { return None; } globals.page = value; }
            (1, 7) => globals.size = value,
            (1, 8) => { if value == 0 || value > 255 { return None; } globals.id = value as u8; }
            (1, 9) => globals.count = value,
            (1, 10) => { if stack.len() >= 32 { return None; } stack.push(globals); }
            (1, 11) => globals = stack.pop()?,
            (2, 0) => { if usages.len() >= 256 { return None; } usages.push(extended); }
            (2, 1) => usage_min = Some(extended),
            (2, 2) => usage_max = Some(extended),
            (2, 10) => return None, // Delimiter alternatives require HidSharp semantics.
            (0, tag) => {
                if tag == 10 {
                    if collections.len() >= 32 { return None; }
                    let owner = if collections.is_empty() {
                        if devices.len() >= 64 { return None; }
                        if let (Some(first), Some(last)) = (usage_min, usage_max) {
                            if last < first || last - first > 255 { return None; }
                            usages.extend(first..=last);
                        } else if usage_min.is_some() || usage_max.is_some() { return None; }
                        let id = devices.len();
                        devices.push(usages.clone());
                        Some(id)
                    } else { *collections.last()? };
                    collections.push(owner);
                } else if tag == 12 {
                    collections.pop()?;
                } else if tag == 8 {
                    let owner = (*collections.last()?)?;
                    if input.iter().any(|(id, device)| *id == globals.id && *device != owner) {
                        return None;
                    }
                    if !input.contains(&(globals.id, owner)) { input.push((globals.id, owner)); }
                } else if tag == 11 {
                    let bits = globals.size.checked_mul(globals.count)?;
                    let total = feature.entry(globals.id).or_default();
                    *total = total.checked_add(bits)?;
                    if *total > (u16::MAX as u32 - 1) * 8 { return None; }
                }
                usages.clear(); usage_min = None; usage_max = None;
            }
            _ => {}
        }
    }
    if !collections.is_empty() || !stack.is_empty() { return None; }
    let mut entries = Vec::new();
    for (id, owner) in input {
        for usage in &devices[owner] {
            entries.push(format!("{id:02X}:{:04X}:{:04X}", usage >> 16, usage & 0xffff));
        }
    }
    let usage = devices.iter().flatten().next().copied().unwrap_or(0);
    Some(Metadata {
        feature_length: feature.values().map(|n| n.div_ceil(8) + 1).max().unwrap_or(0),
        reports: entries.join(", "),
        usage_page: (usage >> 16) as u16,
        usage: usage as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_device_items_and_pushed_globals_keep_report_identity() {
        let descriptor = [
            0x05, 0x0d, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x01,
            0x75, 8, 0x95, 4, 0x81, 2, 0xa4, 0x85, 3, 0x95, 2,
            0xb1, 2, 0xb4, 0xc0,
            0x05, 1, 0x09, 2, 0xa1, 1, 0x85, 2, 0x81, 2, 0xc0,
        ];
        let metadata = parse(&descriptor).unwrap();
        assert_eq!(metadata.feature_length, 3);
        assert_eq!(metadata.reports, "01:000D:0002, 02:0001:0002");
    }
    #[test]
    fn unnumbered_features_count_the_id_and_invalid_layouts_are_rejected() {
        assert_eq!(parse(&[0x75, 8, 0x95, 2, 0xb1, 2]).unwrap().feature_length, 3);
        for bad in [&[0x75][..], &[0xb4], &[0xc0], &[0xfe, 4, 0, 1], &[0x85, 0]] {
            assert_eq!(parse(bad), None);
        }
        assert_eq!(parse(&[0x77, 255, 255, 255, 255, 0x95, 2, 0xb1, 2]), None);
    }
}
