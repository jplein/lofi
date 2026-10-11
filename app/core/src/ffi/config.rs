//! Configuration-file C-ABI: one call that reads the macOS launcher's
//! `config.toml` and returns its values by value.
//!
//! By value rather than as an opaque handle, unlike `MruStore`, because the
//! configuration is a handful of plain scalars with no state to keep alive
//! between calls: Swift reads it, keeps the struct, and there is nothing to
//! free. That also keeps it outside the `EntryList` borrow contract — no
//! pointer into Rust memory is handed out.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;

use crate::config;

/// The configuration file's values, flattened to C-representable scalars.
/// Mirrors `lofi_core::config::Config` field for field; the two are separate
/// types so the serde-facing struct can grow non-FFI-safe fields (strings,
/// nested tables) without forcing the ABI to follow.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LofiConfig {
    /// `gaps` — space between a command-placed window and the work-area edges,
    /// in points. `0` by default.
    pub gaps: u32,
}

impl From<config::Config> for LofiConfig {
    fn from(config: config::Config) -> Self {
        LofiConfig { gaps: config.gaps }
    }
}

/// Read and parse the configuration file at `path`, writing the commented
/// default file there first if none exists (`config::load_or_create`).
///
/// Never fails from the caller's point of view: a missing file is created and
/// yields the defaults; one that cannot be created, read, or parsed logs via
/// `eprintln!` and yields the defaults. A null or non-UTF-8 `path` also yields
/// the defaults (the Swift side always passes a real path, so this is a guard,
/// not a mode). The launcher therefore never has to handle "no configuration"
/// as a distinct state — there is always a `LofiConfig`.
///
/// # Safety
///
/// `path` must be null or point at a NUL-terminated C string whose buffer
/// remains valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_config_load(path: *const c_char) -> LofiConfig {
    if path.is_null() {
        return config::Config::default().into();
    }
    // SAFETY: non-null per the precondition; caller's contract is a
    // NUL-terminated C string.
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return config::Config::default().into(),
    };
    config::load_or_create(Path::new(path_str)).into()
}
