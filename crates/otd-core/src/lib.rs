//! Portable core of the Rust OpenTabletDriver port: report decoding, pen
//! state, profiles and OpenTabletDriver settings import, absolute and relative
//! mapping, the built-in Radial Follow filter, the per-report pipeline and the
//! device-session loop. Report processing does not call operating-system APIs; the
//! platform supplies the device (`session::ReportSource`), the desktop layout
//! (`session::Displays`), DLL filters (`plugins::Filters`) and the output sink.
//! The separate storage module provides filesystem persistence and guarded
//! platform-specific atomic file publication.

pub mod actions;
pub mod areas;
pub mod config;
pub mod decoders;
pub mod display;
pub mod endpoint_match;
pub mod mapping;
pub mod output;
pub mod pipeline;
pub mod plugins;
pub mod presets;
pub mod protocol;
pub mod radial_follow;
pub mod relative;
pub mod reports;
pub mod session;
pub mod spec;
pub mod state;
pub mod storage;
pub mod tablets;
#[cfg(any(test, feature = "test-alloc"))]
pub mod test_alloc;

#[cfg(test)]
mod differential;
#[cfg(test)]
mod golden;
#[cfg(test)]
mod properties;
