//! Portable core of the Rust OpenTabletDriver port: report decoding, pen
//! state, profiles and OpenTabletDriver settings import, absolute and relative
//! mapping, the built-in Radial Follow filter, the per-report pipeline and the
//! device-session loop. Nothing here calls an operating-system API; the
//! platform supplies the device (`session::ReportSource`), the desktop layout
//! (`session::Displays`), DLL filters (`plugins::Filters`) and the output sink.

pub mod config;
pub mod display;
pub mod mapping;
pub mod output;
pub mod pipeline;
pub mod plugins;
pub mod protocol;
pub mod radial_follow;
pub mod relative;
pub mod session;
pub mod state;
#[cfg(any(test, feature = "test-alloc"))]
pub mod test_alloc;

#[cfg(test)]
mod golden;
