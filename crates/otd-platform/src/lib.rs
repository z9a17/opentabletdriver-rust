//! Shared Unix runtime. Native input/output stays with its platform owner.
//! Dynamic plugin and CLR entry points are loaded only for explicit startup.
#[cfg(unix)]
pub mod library;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod local_control;
#[cfg(unix)]
#[path = "../../../src/upstream_rpc/protocol.rs"]
pub mod original_rpc_protocol;
#[cfg(unix)]
pub mod upstream_settings;
#[cfg(unix)]
pub mod cli;
#[cfg(unix)]
pub mod shared_device_io;
#[cfg(unix)]
pub mod paired_source;
#[cfg(unix)]
#[path = "../../../src/shared_devices.rs"]
pub mod shared_devices;
#[cfg(unix)]
#[path = "../../../src/global_tools.rs"]
pub mod global_tools;
#[cfg(unix)]
#[path = "../../../src/binding_presets.rs"]
pub mod binding_presets;
#[cfg(unix)]
#[path = "../../../src/plugin_catalog.rs"]
pub mod plugin_catalog;
#[cfg(unix)]
#[path = "../../../src/download.rs"]
pub mod download;
#[cfg(unix)]
pub mod update;
#[cfg(unix)]
pub mod process;

#[cfg(unix)]
#[path = "../../../src/custom_devices.rs"]
pub mod custom_devices;
#[cfg(unix)]
pub mod managed_source;
#[cfg(unix)]
pub mod input_owner;
#[cfg(unix)]
#[path = "../../../src/plugin_manager.rs"]
pub mod plugin_manager;

#[cfg(unix)]
mod cold_services;
