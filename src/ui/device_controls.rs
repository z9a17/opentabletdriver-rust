//! The panel edits an explicit physical session; debugger selection only
//! changes when that session is running. All requests run on its client worker.
use super::*;

impl App {
    pub(super) fn choose_device(&mut self, id: String) {
        if self.control_busy || self.closing || self.update_restart_pending { return; }
        if self.dirty || !self.invalid.is_empty() {
            self.log(Level::Warning, "Settings", "Save the current edits before selecting another tablet.");
            return;
        }
        let Some(device) = self.device_sessions.iter().find(|device| device.id == id).cloned() else { return; };
        if device.pending_generation.is_some() || device.device_generation == 0 {
            self.log(Level::Info, "Tablet", "This tablet is still preparing its settings. Wait for its device state to settle.");
            return;
        }
        let Some(expected) = self.running.as_ref().map(|running| running.identity.clone()) else { return; };
        if self.submit_control(client::ClientCommand::SelectDevice { expected, id,
            device_generation: device.device_generation,
            running: device.state == crate::device_sessions::SessionState::Running }) {
            self.log(Level::Info, "Tablet", format!("Selecting {} ({}).", device.tablet, device.id));
        }
    }

    pub(super) fn device_lifecycle(&mut self, start: bool) {
        if self.control_busy || self.closing || self.update_restart_pending { return; }
        let Some(device) = self.selected_device.clone() else { return; };
        let Some(expected) = self.running.as_ref().map(|running| running.identity.clone()) else { return; };
        if device.pending_generation.is_some() {
            self.log(Level::Info, "Tablet", "This tablet is already changing state.");
            return;
        }
        if self.submit_control(client::ClientCommand::DeviceLifecycle { expected, id: device.id.clone(),
            device_generation: device.device_generation, start }) {
            self.log(Level::Info, "Tablet", format!("{} requested for {} ({}). Other tablets keep running.",
                if start { "Start" } else { "Stop" }, device.tablet, device.id));
        }
    }
}
