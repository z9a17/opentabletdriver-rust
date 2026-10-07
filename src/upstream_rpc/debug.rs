//! Explicit compatibility-client subscription. Original managed decoders live
//! only on this background worker, never on the input owner or UI capture.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use otd_core::debug::stream::{self, SessionMetadata, Subscription};
use crate::device_sessions::SessionSnapshot;
use crate::dotnet::ManagedDebugDecoder;
use super::protocol;

struct Endpoint {
    metadata: SessionMetadata,
    tablet: Value,
    digitizer: ManagedDebugDecoder,
    auxiliary: Option<ManagedDebugDecoder>,
}
pub struct Capture {
    subscription: Subscription,
    endpoints: BTreeMap<u64, Endpoint>,
    boundary: Boundary,
    next_poll: Instant,
}
#[derive(Default)]
struct Boundary { sequence: Option<u64>, gap: Option<u64> }
impl Boundary {
    fn advance(&mut self, sequence: u64, gap: u64) -> bool {
        let reset = self.sequence.is_some_and(|last| last.checked_add(1) != Some(sequence))
            || self.gap.is_some_and(|last| last != gap);
        self.sequence = Some(sequence); self.gap = Some(gap);
        reset
    }
}
impl Capture {
    pub fn new(sessions: &[SessionSnapshot]) -> Result<Self, String> {
        let mut capture = Self { subscription: stream::subscribe()?, endpoints: BTreeMap::new(), boundary: Boundary::default(), next_poll: Instant::now() };
        // Preflight every active source before reporting success. Retired
        // endpoints are added only if this subscription actually retains one.
        for metadata in stream::sessions().into_iter().filter(|metadata| metadata.active) {
            capture.add(metadata, sessions)?;
        }
        Ok(capture)
    }
    fn add(&mut self, metadata: SessionMetadata, sessions: &[SessionSnapshot]) -> Result<(), String> {
        let id = metadata.key.as_deref().ok_or("debug source lacks an owned device session ID")?;
        let session = sessions.iter().find(|session| session.id == id && session.tablet == metadata.name)
            .ok_or("debug source has no exact device session/configuration match")?;
        let mut identifiers = vec![session.digitizer.clone()];
        if let Some(auxiliary) = &session.auxiliary { identifiers.push(auxiliary.clone()); }
        let tablet = json!({"Properties":session.properties,"Identifiers":identifiers});
        let digitizer = ManagedDebugDecoder::new(&metadata.parser)?;
        let auxiliary = metadata.auxiliary_parser.as_deref().map(ManagedDebugDecoder::new).transpose()?;
        self.endpoints.insert(metadata.session, Endpoint { metadata, tablet, digitizer, auxiliary });
        Ok(())
    }
    fn reset(&mut self) -> Result<(), String> {
        for endpoint in self.endpoints.values_mut() {
            endpoint.digitizer.reset()?;
            if let Some(auxiliary) = &mut endpoint.auxiliary { auxiliary.reset()?; }
        }
        Ok(())
    }
    pub fn poll(&mut self, sessions: Option<&[SessionSnapshot]>) -> Result<(Vec<Value>, Vec<String>), String> {
        if Instant::now() < self.next_poll { return Ok((Vec::new(),Vec::new())); }
        self.next_poll = Instant::now() + Duration::from_millis(50);
        let metadata = stream::sessions();
        // Retired metadata (bounded128) stays cached while final queued packets
        // can refer to it. Prune only identities no longer retained by the tap.
        self.endpoints.retain(|id, _| metadata.iter().any(|source| source.session == *id));
        let batch = self.subscription.read(64, 256 * 1024)?;
        let mut events = Vec::new();
        let mut diagnostics = Vec::new();
        if batch.lost != 0 { diagnostics.push(format!("Debug stream lost {} tap attempts/overwritten reports; parser state resets at marked boundaries", batch.lost)); }
        for packet in batch.packets {
            if self.boundary.advance(packet.sequence, packet.gap_generation) { self.reset()?; }
            if !self.endpoints.contains_key(&packet.session) {
                let source = metadata.iter().find(|source| source.session == packet.session)
                    .ok_or("queued debug source metadata expired")?.clone();
                let Some(sessions) = sessions else {
                    diagnostics.push("New debug source awaiting actual device session metadata; packet omitted".into());
                    continue;
                };
                if let Err(error) = self.add(source, sessions) {
                    diagnostics.push(format!("Debug source {} unavailable: {error}; packet omitted", packet.session));
                    continue;
                }
            }
            let endpoint = self.endpoints.get_mut(&packet.session).ok_or("debug endpoint missing")?;
            let decoder = if packet.auxiliary {
                endpoint.auxiliary.as_mut().ok_or("auxiliary report has no actual auxiliary parser")?
            } else { &mut endpoint.digitizer };
            match decoder.decode(&packet.bytes) {
                Ok(Some(report)) => {
                    let event = protocol::event("DeviceReport", json!({"Tablet":endpoint.tablet,"Path":report.path,"Data":report.data}));
                    if protocol::encode(&event).is_ok() { events.push(event); }
                    else { diagnostics.push(format!("Concrete debug report for {} exceeds RPC framing bound; omitted", endpoint.metadata.name)); }
                }
                Ok(None) => {},
                Err(error) => {
                    decoder.reset()?;
                    diagnostics.push(format!("Debug decoder for {} failed and reset: {error}", endpoint.metadata.name));
                }
            }
        }
        // Packet bound also bounds diagnostics. Avoid64 repeated notices on
        // every source failure; one representative plus count is enough.
        if diagnostics.len() > 4 { let omitted = diagnostics.len() - 3; diagnostics.truncate(3); diagnostics.push(format!("{omitted} additional debug diagnostics omitted")); }
        Ok((events, diagnostics))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resets_at_internal_drops_and_cursor_overwrites_only() {
        let mut boundary = Boundary::default();
        assert!(!boundary.advance(100, 7));
        assert!(!boundary.advance(101, 7));
        assert!(boundary.advance(102, 8));
        assert!(!boundary.advance(103, 8));
        assert!(boundary.advance(111, 8));
        assert!(!boundary.advance(112, 8));
    }
}
