//! `MruStore` C-ABI: opaque handle around `crate::mru::MruStore` plus the
//! bump-on-activation entry point.
//!
//! See the module-level borrow contract in `entries.rs` for how
//! `lofi_entries_apply_mru` slots in alongside the other mutating calls.
//! `lofi_mru_bump_entry` is non-mutating with respect to the `EntryList`
//! (it only reads the resolved entry's reference) and so does not
//! invalidate any previously handed-out pointers.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;

use crate::mru::MruStore;

use super::entries::EntryList;

/// Open (or create) the SQLite-backed MRU store at `path`. The file and
/// any missing parent directories are created on demand; subsequent opens
/// against the same path reattach to the existing history.
///
/// Returns a non-null pointer on success. Returns null when:
/// - `path` is null,
/// - `path` is not valid UTF-8,
/// - or `MruStore::open` itself fails (permission denied, parent path is
///   not a directory, etc.). The underlying error is logged via
///   `eprintln!` for visibility during development; the launcher then
///   proceeds with degraded behavior (no MRU ordering).
///
/// The caller must release the store with `lofi_mru_free` when finished.
///
/// # Safety
///
/// `path` must be null or point at a NUL-terminated C string whose buffer
/// remains valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_mru_open(path: *const c_char) -> *mut MruStore {
    if path.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: non-null per the precondition; caller's contract is a
    // NUL-terminated C string.
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    match MruStore::open(Path::new(path_str)) {
        Ok(store) => Box::into_raw(Box::new(store)),
        Err(e) => {
            eprintln!("lofi_mru_open: {e}");
            ptr::null_mut()
        }
    }
}

/// Release an `MruStore` previously returned by `lofi_mru_open`. Passing a
/// null pointer is a safe no-op (mirrors `lofi_entries_free`).
///
/// # Safety
///
/// `store` must be either null or a pointer obtained from `lofi_mru_open`
/// that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_mru_free(store: *mut MruStore) {
    if store.is_null() {
        return;
    }
    // SAFETY: precondition above — `store` came from `Box::into_raw` and
    // has not been freed.
    unsafe {
        drop(Box::from_raw(store));
    }
}

/// Record an activation: resolve the entry at the filtered index `idx` in
/// `list`, take its `EntryRef`, and UPSERT it into the MRU store with the
/// current timestamp.
///
/// `idx` is the *filtered* index — the same coordinate space callers pass
/// to `lofi_entries_get_*`. Resolution goes through the active filter, so
/// a query-narrowed view's index maps to the right underlying entry.
///
/// Returns `true` on success. Returns `false` when:
/// - `store` or `list` is null,
/// - `idx` is out of bounds against the current filtered view,
/// - or the underlying `MruStore::bump` fails (SQL error, JSON encode
///   error, clock skew before the epoch). Errors are logged via
///   `eprintln!` but never panic.
///
/// # Safety
///
/// `store` must be null or a pointer obtained from `lofi_mru_open`.
/// `list` must be null or a pointer obtained from `lofi_entries_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_mru_bump_entry(
    store: *const MruStore,
    list: *const EntryList,
    idx: usize,
) -> bool {
    if store.is_null() || list.is_null() {
        return false;
    }
    // SAFETY: non-null `list` per the precondition; shared access is fine
    // because the single-threaded FFI contract means no other thread can
    // be mutating the list during this call.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return false;
    };
    let entry_ref = entry.reference();
    // SAFETY: non-null `store` per the precondition.
    let store_ref = unsafe { &*store };
    match store_ref.bump(&entry_ref) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("lofi_mru_bump_entry: {e}");
            false
        }
    }
}
