//! `EntryList` C-ABI: an opaque heap-allocated `Vec<Entry>` plus a small name
//! cache that backs `lofi_entries_get_name`'s borrow contract.
//!
//! ## Borrow contract
//!
//! `lofi_entries_get_name(list, idx)` returns a `*const c_char` that the
//! caller may read until the next mutation of the list (any `push_*` call)
//! or `lofi_entries_free`. Callers (Swift, in particular) must copy the
//! bytes into their own storage before doing anything else with the list.
//!
//! The cache is a `RefCell<Vec<Option<CString>>>` parallel to `entries`. We
//! lazily fill slot `idx` on demand and clear the whole cache on every
//! `push_*`. The cache lives behind `RefCell` rather than raw `UnsafeCell`
//! because every entry into Rust from the FFI is single-threaded per call by
//! contract; the only `RefCell` borrow we take is a short `borrow_mut()`
//! confined to a single FFI call, so there is no reentrancy hazard.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use crate::{Application, Entry};

/// Opaque handle owning a vector of `Entry` values. Construction is via
/// `lofi_entries_new`; teardown is via `lofi_entries_free`. The Rust-side
/// layout is intentionally not exposed to C — cbindgen emits this as an
/// opaque forward declaration.
pub struct EntryList {
    entries: Vec<Entry>,
    /// Lazily-built C strings backing the pointers returned by
    /// `lofi_entries_get_name`. Indexed parallel to `entries`. Cleared
    /// whenever the list mutates so a stale pointer (already a no-no per the
    /// borrow contract) cannot point into freed memory either.
    name_cache: RefCell<Vec<Option<CString>>>,
}

impl EntryList {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            name_cache: RefCell::new(Vec::new()),
        }
    }

    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        // Any cached `CString` is suspect once the underlying `Vec<Entry>`
        // has moved (a reallocation could change every `&str` source); clear
        // wholesale rather than try to preserve indices.
        self.name_cache.borrow_mut().clear();
    }
}

/// Construct a fresh empty `EntryList` on the heap and hand its ownership
/// to the caller. Returns a non-null pointer; allocation failures abort the
/// process via Rust's default OOM handler, matching the rest of `Box`.
///
/// The caller must release the list with `lofi_entries_free` when finished.
#[unsafe(no_mangle)]
pub extern "C" fn lofi_entries_new() -> *mut EntryList {
    Box::into_raw(Box::new(EntryList::new()))
}

/// Release an `EntryList` previously returned by `lofi_entries_new`. Passing
/// a null pointer is a safe no-op (mirrors `free(NULL)` in C), which keeps
/// Swift `deinit` paths simple when the handle was never assigned.
///
/// # Safety
///
/// `list` must be either null or a pointer obtained from `lofi_entries_new`
/// that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_free(list: *mut EntryList) {
    if list.is_null() {
        return;
    }
    // SAFETY: precondition above — `list` came from `Box::into_raw` and has
    // not been freed.
    unsafe {
        drop(Box::from_raw(list));
    }
}

/// Append an application entry to the list. Copies every string in; the
/// caller's C buffers may be reused or freed as soon as the call returns.
///
/// Returns `true` on success, `false` if any of:
/// - `list` is null
/// - `name` or `bundle_id` is null
/// - any non-null string is not valid UTF-8 (this includes `icon`)
///
/// `icon` may be null to mean "no icon"; the resulting `Application::icon`
/// is `None`. A non-null `icon` that is invalid UTF-8 is rejected (strict
/// rather than silent-`None` so a Swift bug is easier to spot).
///
/// # Safety
///
/// Pointers must be null or point at NUL-terminated C strings whose buffers
/// remain valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_push_application(
    list: *mut EntryList,
    name: *const c_char,
    bundle_id: *const c_char,
    icon: *const c_char,
) -> bool {
    if list.is_null() || name.is_null() || bundle_id.is_null() {
        return false;
    }

    // SAFETY: non-null and assumed-valid C strings per the function contract.
    let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return false,
    };
    // SAFETY: same as above.
    let bundle_str = match unsafe { CStr::from_ptr(bundle_id) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return false,
    };
    let icon_opt = if icon.is_null() {
        None
    } else {
        // SAFETY: non-null branch — caller's contract is the same as above.
        match unsafe { CStr::from_ptr(icon) }.to_str() {
            Ok(s) => Some(s.to_owned()),
            Err(_) => return false,
        }
    };

    // SAFETY: non-null `list` precondition; we have exclusive access for the
    // duration of this call by the single-threaded FFI contract.
    let list_ref = unsafe { &mut *list };
    list_ref.push(Entry::Application(Application {
        name: name_str,
        desktop_id: bundle_str,
        icon: icon_opt,
        recent_window_id: None,
    }));
    true
}

/// Return the number of entries currently in `list`. A null `list` returns
/// `0` (rather than crashing) so a defensive Swift caller can use the same
/// shape for both initialized and not-yet-initialized handles.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_len(list: *const EntryList) -> usize {
    if list.is_null() {
        return 0;
    }
    // SAFETY: non-null `list` per the precondition.
    unsafe { (*list).entries.len() }
}

/// Return a borrowed pointer to the entry-at-`idx`'s display name.
///
/// The returned pointer is valid until the next mutation of the list (any
/// `push_*` call) or `lofi_entries_free`. Callers must copy before doing
/// anything that could mutate or free the list. See the module-level borrow
/// contract.
///
/// Returns null when `list` is null or `idx >= len`.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_name(list: *const EntryList, idx: usize) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    if idx >= list_ref.entries.len() {
        return ptr::null();
    }

    let mut cache = list_ref.name_cache.borrow_mut();
    if cache.len() < list_ref.entries.len() {
        cache.resize(list_ref.entries.len(), None);
    }
    if cache[idx].is_none() {
        // `Entry::name()` returns the in-memory display name. We copy it
        // into a `CString` once and stash it in the cache slot; the pointer
        // we hand back lives inside `list_ref` and is therefore valid for
        // the documented borrow lifetime.
        let name = list_ref.entries[idx].name();
        let Ok(cstring) = CString::new(name) else {
            // `Entry::name()` returning a string containing an interior NUL
            // is unexpected — `Application::name` is built from a `&str`
            // that itself came through `CStr::from_ptr(...).to_str()` (no
            // NUL possible) or from internal display-name constants. Bail
            // out with null rather than panic.
            return ptr::null();
        };
        cache[idx] = Some(cstring);
    }
    cache[idx]
        .as_ref()
        .map_or(ptr::null(), |s| s.as_ptr())
}
