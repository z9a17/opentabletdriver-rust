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
#[path = "../../../src/shared_devices.rs"]
pub mod shared_devices;
#[cfg(unix)]
#[path = "../../../src/global_tools.rs"]
pub mod global_tools;
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
