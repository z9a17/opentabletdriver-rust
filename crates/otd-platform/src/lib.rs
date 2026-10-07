//! Shared Unix runtime. Native input/output stays with its platform owner.
//! Dynamic plugin and CLR entry points are loaded only for explicit startup.
#[cfg(unix)]
pub mod library;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
