pub mod entities;
pub mod ids;
pub mod materialize;
pub mod rank;
pub mod resolve;

pub use entities::*;

/// Inventory dead-strip guard: binaries must call this before touching myko so
/// the entity registrations in this crate are linked in.
#[inline]
pub fn link() {}
