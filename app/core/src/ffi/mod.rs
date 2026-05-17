//! C-ABI surface exposed to the macOS Swift frontend.
//!
//! Gated on `feature = "ffi"`. When the feature is off this module does not
//! exist, so the GNOME build pays nothing for it and the default test run is
//! unaffected.
//!
//! The submodules each carry one logical chunk of the FFI; this `mod.rs`
//! re-exports them so the symbols all land in `lofi_core::ffi`.

pub mod entries;
pub mod mru;

pub use entries::*;
pub use mru::*;
