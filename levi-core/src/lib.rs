pub mod entities;
#[cfg(not(target_arch = "wasm32"))]
pub mod hub;
pub mod ids;
pub mod materialize;
pub mod merkle;
pub mod rank;
pub mod resolve;

pub use entities::*;

/// Binaries that receive levi entities over the wire (the hub) must call
/// this: without any referenced symbol from this crate, the linker drops its
/// object files — including the inventory registrations for entities,
/// commands, and sagas.
pub fn link() {}
