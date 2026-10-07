//! Deferred native binding requests. The report path copies a bounded name;
//! the control owner reads settings and performs the guarded replacement.
use std::io;
use otd_core::presets::{PresetName, PresetStore, MAX_PRESET_NAME_BYTES};
use otd_core::output::buttons::{ActionSink, ScrollPulse};
use otd_core::actions::Action;
use otd_core::reports::{ReportKind, ReportValues};

#[derive(Clone, Copy, Debug)]
pub struct Request { bytes: [u8; MAX_PRESET_NAME_BYTES], length: u16, pub owner: u32 }
impl Request {
    pub fn new(owner: u32, name: &PresetName) -> Self {
        let mut bytes = [0; MAX_PRESET_NAME_BYTES];
        bytes[..name.as_str().len()].copy_from_slice(name.as_str().as_bytes());
        Self { bytes, length: name.as_str().len() as u16, owner }
    }
    pub fn name(&self) -> Result<PresetName, String> {
        let text = std::str::from_utf8(&self.bytes[..usize::from(self.length)]).map_err(|error| error.to_string())?;
        PresetName::parse(text)
    }
}

pub type Callback = Box<dyn FnMut(u32, &PresetName) -> io::Result<()>>;
struct Sink { inner: Box<dyn ActionSink>, request: Callback, inhibit: Option<u32> }
pub fn wrap(inner: Box<dyn ActionSink>, request: Callback, inhibit: Option<u32>) -> Box<dyn ActionSink> { Box::new(Sink { inner, request, inhibit }) }
impl ActionSink for Sink {
    fn pointer_attributes(&mut self, attributes: otd_core::output::MouseAttributes) -> io::Result<()> { self.inner.pointer_attributes(attributes) }
    fn flush_pointer(&mut self) -> io::Result<()> { self.inner.flush_pointer() }
    fn inhibited_binding(&self) -> Option<u32> { self.inhibit }
    fn supports_presets(&self) -> bool { true }
    fn preset(&mut self, owner: u32, name: &PresetName) -> io::Result<()> { (self.request)(owner, name) }
    fn has_managed(&self) -> bool { self.inner.has_managed() }
    fn managed_next_tick(&self) -> Option<std::time::Duration> { self.inner.managed_next_tick() }
    fn managed_tick(&mut self) -> io::Result<()> { self.inner.managed_tick() }
    fn supports_managed(&self, config: &otd_core::plugins::PluginConfig) -> bool { self.inner.supports_managed(config) }
    fn set_report(&mut self, kind: ReportKind, values: &ReportValues, raw: &[u8]) -> io::Result<()> { self.inner.set_report(kind, values, raw) }
    fn managed_binding(&mut self, owner: u32, config: &otd_core::plugins::PluginConfig, pressed: bool) -> io::Result<()> { self.inner.managed_binding(owner, config, pressed) }
    fn supports(&self, action: Action) -> bool { self.inner.supports(action) }
    fn supports_scroll(&self) -> bool { self.inner.supports_scroll() }
    fn scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> { self.inner.scroll(pulse) }
    fn next_managed_command(&mut self) -> Option<otd_core::plugins::ManagedCommand> { self.inner.next_managed_command() }
    fn hold(&mut self, owner: u32, action: Action, held: bool) -> io::Result<()> { self.inner.hold(owner, action, held) }
    fn flush(&mut self) -> io::Result<usize> { self.inner.flush() }
    fn release_all(&mut self) -> io::Result<usize> { self.inner.release_all() }
}

/// Native TOML presets select one physical session. Original JSON preset
/// collections are not reduced to one profile: their routing is handled by the
/// daemon's collection service (or reported unavailable).
pub fn load(request: Request) -> Result<crate::config::Profile, String> {
    let name = request.name()?;
    let store = PresetStore::user()?;
    let mut profile = store.load(&name)?.profile().clone();
    // Rotation owners are impulse actions, not held physical buttons.
    let rotation_start = 128 + 64 * otd_core::reports::MAX_WHEELS as u32;
    if !(rotation_start..rotation_start + 2 * otd_core::reports::MAX_WHEELS as u32).contains(&request.owner) {
        profile.binding_inhibit = Some(request.owner);
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_preset_notice_copies_unicode_without_report_allocation() {
        let name = PresetName::parse("Écriture.2").unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        crate::test_alloc::assert_no_allocations(|| {
            sender.try_send(Request::new(65, &name)).unwrap();
        });
        let request = receiver.try_recv().unwrap();
        assert_eq!(request.owner, 65);
        assert_eq!(request.name().unwrap(), name);
    }
}
