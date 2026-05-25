//! `EntryList` C-ABI: an opaque heap-allocated `Vec<Entry>` plus small per-
//! accessor caches that back the `lofi_entries_get_*` borrow contracts and an
//! optional fuzzy-filter index built from a current query.
//!
//! ## Borrow contract
//!
//! Every `lofi_entries_get_*(list, idx)` returns a `*const c_char` that the
//! caller may read until the next mutation of the list. Mutations are:
//! - any `lofi_entries_push_*` call,
//! - `lofi_entries_set_query` (it can shuffle the filter and invalidates the
//!   meaning of every previously-handed-out pointer),
//! - `lofi_entries_apply_mru` (reorders the underlying vector in place and
//!   clears every cache so any previously-handed-out pointer is invalid),
//!   and
//! - `lofi_entries_free`.
//!
//! Callers (Swift, in particular) must copy the bytes into their own storage
//! before doing anything else with the list.
//!
//! Two accessors are exempt from the "copy before the next mutation" rule:
//! - `lofi_entries_get_window_id` returns a `u64` by value (no borrow at all).
//! - `lofi_entries_get_command_id` returns a `*const c_char` that points at a
//!   `&'static CStr` (a `c"..."` literal selected by `command_id_cstr`), so its
//!   lifetime is the *whole process*, not "until the next mutation". A mutation
//!   never invalidates it. Swift still copies it for uniformity with the other
//!   string accessors.
//! - `lofi_entries_get_command_geometry` writes its result into caller-owned
//!   out-params by value (no borrow), so it too is outside the borrow contract.
//!
//! Each cache is a `RefCell<Vec<Option<CString>>>` keyed on the **underlying**
//! `entries` index (not the filtered index — see `EntryList::resolved`). We
//! lazily fill slot `i` on demand and clear the whole cache on every mutation.
//! The cache lives behind `RefCell` rather than raw `UnsafeCell` because every
//! entry into Rust from the FFI is single-threaded per call by contract; the
//! only `RefCell` borrow we take is a short `borrow_mut()` confined to a
//! single FFI call, so there is no reentrancy hazard.
//!
//! ## Filtering
//!
//! `query` is the active search string. `filter` is `None` when the query is
//! empty (or whitespace-only) — that's the passthrough case. When non-empty,
//! `filter` is `Some(indices)` where each index points into `entries`. The
//! filter is recomputed on every mutation that could change membership: a
//! `push_*` (the new entry may or may not match the active query) and a
//! `set_query` (the predicate itself changed).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use fuzzy_matcher::skim::SkimMatcherV2;

use crate::compute_geometry;
use crate::matcher;
use crate::mru::MruStore;
use crate::{
    Application, Command, CommandKind, Entry, EntryKind, EntryRef, PowerCommand,
    PowerCommandKind, Window, WorkArea,
};

/// Stable English category label for `EntryKind::Application`. The UI displays
/// these as-is; localization is a UI-layer concern.
const CATEGORY_APPLICATION: &str = "Application";
/// Stable English category label for `EntryKind::Window`.
const CATEGORY_WINDOW: &str = "Window";
/// Stable English category label for `EntryKind::Workspace`.
const CATEGORY_WORKSPACE: &str = "Workspace";
/// Stable English category label for `EntryKind::Command`.
const CATEGORY_COMMAND: &str = "Command";
/// Stable English category label for `EntryKind::PowerCommand`.
const CATEGORY_POWER_COMMAND: &str = "PowerCommand";

/// Opaque handle owning a vector of `Entry` values plus a current query and
/// per-accessor caches. Construction is via `lofi_entries_new`; teardown is
/// via `lofi_entries_free`. The Rust-side layout is intentionally not exposed
/// to C — cbindgen emits this as an opaque forward declaration.
pub struct EntryList {
    pub(super) entries: Vec<Entry>,
    /// Current search string. Empty (or whitespace-only) means "no filter".
    /// Stored as an owned `String` so the FFI's borrowed `*const c_char` does
    /// not have to outlive the call.
    pub(super) query: String,
    /// `None` when `query` is the passthrough case (empty / whitespace-only);
    /// `Some(indices)` when a real filter is active. Each entry indexes into
    /// `entries`. Built by `recompute_filter`.
    pub(super) filter: Option<Vec<usize>>,
    /// Number of entries known to the MRU store, set by `apply_mru`.
    /// `apply_mru` sorts `entries` so the first `mru_count` slots are
    /// MRU-known (in recency order); the remaining slots are entries
    /// the user has never launched, in push order. The filter uses
    /// this boundary to enforce "MRU always wins": matching entries
    /// at idx < mru_count stay in MRU order, and matching entries at
    /// idx >= mru_count are sorted by descending fuzzy-match score
    /// so the highest-quality match in the never-used tier comes
    /// first. 0 before `apply_mru` ever runs (no MRU known →
    /// everything sorts by score).
    pub(super) mru_count: usize,
    /// Lazily-built C strings backing the pointers returned by
    /// `lofi_entries_get_name`. Indexed parallel to `entries` (NOT to
    /// `filter`). Cleared on every mutation.
    pub(super) name_cache: RefCell<Vec<Option<CString>>>,
    /// Parallel to `name_cache`; backs `lofi_entries_get_bundle_id`.
    pub(super) bundle_id_cache: RefCell<Vec<Option<CString>>>,
    /// Parallel to `name_cache`; backs `lofi_entries_get_category`. Each slot
    /// caches the C-string form of the entry's stable English category label.
    pub(super) category_cache: RefCell<Vec<Option<CString>>>,
    /// Parallel to `name_cache`; backs `lofi_entries_get_icon`. A `None` slot
    /// here means "not yet cached"; the wrapped `Option<CString>` is itself
    /// `None` when the underlying entry has no icon (we encode "no icon" via
    /// a `None` *value* and a `Some` cache slot, distinguishing it from "not
    /// yet computed").
    pub(super) icon_cache: RefCell<Vec<Option<Option<CString>>>>,
}

impl EntryList {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            query: String::new(),
            filter: None,
            mru_count: 0,
            name_cache: RefCell::new(Vec::new()),
            bundle_id_cache: RefCell::new(Vec::new()),
            category_cache: RefCell::new(Vec::new()),
            icon_cache: RefCell::new(Vec::new()),
        }
    }

    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        // Any cached `CString` is suspect once the underlying `Vec<Entry>`
        // has moved (a reallocation could change every `&str` source); clear
        // wholesale rather than try to preserve indices.
        self.clear_caches();
        // The new entry may or may not pass the active query; rebuild the
        // filter so it shows up in `len`/`get_*` only when it should.
        self.recompute_filter();
    }

    /// Map a filtered index (the index callers pass to `get_*`) to the
    /// underlying `entries` index. Returns `None` if the filtered index is
    /// out of bounds — that's how every accessor signals null.
    fn resolved(&self, filtered_idx: usize) -> Option<usize> {
        match &self.filter {
            Some(indices) => indices.get(filtered_idx).copied(),
            None => {
                if filtered_idx < self.entries.len() {
                    Some(filtered_idx)
                } else {
                    None
                }
            }
        }
    }

    /// Resolve a filtered index to a borrow of the underlying entry. Used by
    /// `lofi_mru_bump_entry` to walk filtered_idx -> &Entry -> EntryRef
    /// without leaking the cache-key (underlying) index space across the
    /// module boundary. Returns `None` when the filtered index is out of
    /// bounds — same null-signaling semantics as `resolved`.
    pub(super) fn resolve_filtered_index(&self, idx: usize) -> Option<&Entry> {
        self.resolved(idx).and_then(|i| self.entries.get(i))
    }

    /// Rebuild `filter` from `entries` + `query`. Empty / whitespace-only
    /// query becomes the passthrough case (`filter = None`). Non-empty query
    /// is tokenized on whitespace; every entry whose haystack matches every
    /// token (intersection semantics) ends up in the filter index vector.
    ///
    /// Ordering rule — *MRU always wins, then score*:
    ///   - Matching entries with underlying idx `< mru_count` keep their
    ///     existing entries-vec order. That order was set by `apply_mru`
    ///     and reflects recency (rank 0 first), so the user's most-used
    ///     hits stay at the top of the result set regardless of how good
    ///     the fuzzy match against their name was.
    ///   - Matching entries with idx `>= mru_count` (apps the user has
    ///     never launched) are sorted by descending fuzzy-match score
    ///     from `matcher::score`. This is what keeps `"Visual Studio
    ///     Code"` above `"Acrobat"` when the user types `"Code"`:
    ///     `"Visual Studio Code"` is a near-perfect substring match
    ///     while `"Acrobat"` only matches via scattered letters from
    ///     `"com.adobe.Acrobat"`.
    pub(super) fn recompute_filter(&mut self) {
        if self.query.trim().is_empty() {
            self.filter = None;
            return;
        }
        let tokens: Vec<&str> = self.query.split_whitespace().collect();
        let matcher = SkimMatcherV2::default().ignore_case();

        let mut mru_part: Vec<usize> = Vec::new();
        let mut scored_part: Vec<(i64, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(score) = matcher::score(e, &tokens, &matcher) else {
                continue;
            };
            if i < self.mru_count {
                // MRU-known: position in `entries` already encodes
                // recency, so just append in iteration order.
                mru_part.push(i);
            } else {
                scored_part.push((score, i));
            }
        }
        // Stable sort descending by score; equal-score ties keep push
        // order (which mirrors GNOME's `.desktop` enumeration or
        // macOS's `AppDiscovery` order).
        scored_part.sort_by(|a, b| b.0.cmp(&a.0));

        let mut indices = Vec::with_capacity(mru_part.len() + scored_part.len());
        indices.extend(mru_part);
        indices.extend(scored_part.iter().map(|(_, i)| *i));
        self.filter = Some(indices);
    }

    /// Clear every per-accessor cache. Called from every mutation so a stale
    /// pointer (already a no-no per the borrow contract) cannot point into
    /// freed or relocated memory either.
    pub(super) fn clear_caches(&mut self) {
        self.name_cache.borrow_mut().clear();
        self.bundle_id_cache.borrow_mut().clear();
        self.category_cache.borrow_mut().clear();
        self.icon_cache.borrow_mut().clear();
    }

    /// Reset the list to its just-constructed state — no entries, no
    /// query, no filter, no MRU bookkeeping, no caches. The borrow contract
    /// applies the same as for `push`: any pointer previously returned by
    /// `get_*` is invalid after this call. Used by the macOS daemon on
    /// each global-hotkey summon to rebuild the list from scratch
    /// (`AppDelegate.summonPanel`), so the command target reflects the
    /// frontmost-non-LoFi window *at summon time*, not at process-start
    /// time.
    fn clear(&mut self) {
        self.entries.clear();
        self.query.clear();
        self.filter = None;
        self.mru_count = 0;
        self.clear_caches();
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

/// Reset the list to its just-constructed state: no entries, no
/// active query, no filter, no MRU bookkeeping, every per-accessor
/// cache emptied. Passing null is a safe no-op.
///
/// The borrow contract from `push_*` applies the same way: every
/// pointer previously returned by `lofi_entries_get_*` is invalidated
/// by this call. Callers (e.g. the macOS daemon's
/// `AppDelegate.summonPanel`) use this to rebuild the list from
/// scratch on each global-hotkey summon so the command target
/// reflects the frontmost-non-LoFi window *at summon time*, not at
/// process-start time.
///
/// # Safety
///
/// `list` must be null or a pointer obtained from `lofi_entries_new`
/// that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_clear(list: *mut EntryList) {
    if list.is_null() {
        return;
    }
    // SAFETY: non-null precondition above; single-threaded FFI contract
    // gives exclusive access for the duration of this call.
    unsafe { &mut *list }.clear();
}

/// Append an application entry to the list. Copies every string in; the
/// caller's C buffers may be reused or freed as soon as the call returns.
///
/// `is_running` is the boolean projection of `Application::is_running` — the
/// caller passes `true` when the application has at least one open window
/// at gather time. This drives the running-indicator dot in the UI. The macOS
/// platform layer derives it from a one-pass scan of the window list
/// (`AppDelegate.summonPanel`); GNOME populates the field through the Rust
/// `Application` struct directly and does not use this FFI. `recent_window_id`
/// is left `None` here — the macOS activation path is
/// `NSWorkspace.open(...)`, which finds an existing window itself, so there
/// is no use for a real window id Swift-side.
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
/// A successful push invalidates every pointer previously returned by
/// `lofi_entries_get_*` (the borrow contract). It also recomputes the filter
/// against the active query so the new entry appears in `len`/`get_*` only
/// when it matches.
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
    is_running: bool,
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
        is_running,
    }));
    true
}

/// Set the active search query. The list is filtered using whitespace-
/// tokenized, case-insensitive, intersection-semantics fuzzy matching (same
/// rules as `matcher::search`). An empty or whitespace-only query clears the
/// filter; a null `query` pointer is also treated as "clear the filter".
///
/// Returns `true` on success, `false` if `list` is null or the non-null
/// `query` pointer is invalid UTF-8.
///
/// Like every other mutating call, this invalidates every pointer previously
/// returned by `lofi_entries_get_*`. Callers must copy borrowed bytes into
/// their own storage before calling.
///
/// # Safety
///
/// `list` must be null or a pointer obtained from `lofi_entries_new`.
/// `query` must be null or a NUL-terminated C string whose buffer remains
/// valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_set_query(
    list: *mut EntryList,
    query: *const c_char,
) -> bool {
    if list.is_null() {
        return false;
    }
    let new_query = if query.is_null() {
        String::new()
    } else {
        // SAFETY: non-null branch — caller's contract is "NUL-terminated C
        // string". We reject invalid UTF-8 strictly.
        match unsafe { CStr::from_ptr(query) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => return false,
        }
    };

    // SAFETY: non-null `list` precondition; exclusive access per the
    // single-threaded FFI contract.
    let list_ref = unsafe { &mut *list };
    list_ref.query = new_query;
    list_ref.recompute_filter();
    // set_query is a mutating call — any pointers we handed out previously
    // are no longer valid.
    list_ref.clear_caches();
    true
}

/// Return the number of entries currently visible in `list` — i.e. the number
/// of entries that pass the active filter (or `entries.len()` when no filter
/// is active). A null `list` returns `0` (rather than crashing) so a
/// defensive Swift caller can use the same shape for both initialized and
/// not-yet-initialized handles.
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
    let list_ref = unsafe { &*list };
    match &list_ref.filter {
        Some(v) => v.len(),
        None => list_ref.entries.len(),
    }
}

/// Return a borrowed pointer to the entry-at-`idx`'s display name.
///
/// `idx` is the *filtered* index (0..len()). The returned pointer is valid
/// until the next mutation of the list (any `push_*` or `set_query` call) or
/// `lofi_entries_free`. Callers must copy before doing anything that could
/// mutate or free the list. See the module-level borrow contract.
///
/// Returns null when `list` is null or `idx >= len`.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_name(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(real_idx) = list_ref.resolved(idx) else {
        return ptr::null();
    };

    let mut cache = list_ref.name_cache.borrow_mut();
    if cache.len() < list_ref.entries.len() {
        cache.resize(list_ref.entries.len(), None);
    }
    if cache[real_idx].is_none() {
        // `Entry::name()` returns the in-memory display name. We copy it
        // into a `CString` once and stash it in the cache slot; the pointer
        // we hand back lives inside `list_ref` and is therefore valid for
        // the documented borrow lifetime.
        let name = list_ref.entries[real_idx].name();
        let Ok(cstring) = CString::new(name) else {
            // `Entry::name()` returning a string containing an interior NUL
            // is unexpected — `Application::name` is built from a `&str`
            // that itself came through `CStr::from_ptr(...).to_str()` (no
            // NUL possible) or from internal display-name constants. Bail
            // out with null rather than panic.
            return ptr::null();
        };
        cache[real_idx] = Some(cstring);
    }
    cache[real_idx]
        .as_ref()
        .map_or(ptr::null(), |s| s.as_ptr())
}

/// Return a borrowed pointer to the entry-at-`idx`'s bundle id. Only
/// `Entry::Application` carries a bundle id; for every other variant this
/// returns null. The match below is exhaustive on `Entry` so adding a new
/// variant is a compile error until this function is updated.
///
/// `idx` is the *filtered* index. Borrow lifetime: same as `get_name`.
///
/// Returns null when `list` is null, `idx >= len`, or the entry has no bundle
/// id (non-Application variants).
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_bundle_id(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(real_idx) = list_ref.resolved(idx) else {
        return ptr::null();
    };

    // Read the value the cache will hold. Non-Application variants get a
    // null pointer right back; only Application carries a bundle id today.
    let bundle: Option<&str> = match &list_ref.entries[real_idx] {
        Entry::Application(app) => Some(app.desktop_id.as_str()),
        Entry::Window(_)
        | Entry::Workspace(_)
        | Entry::Command(_)
        | Entry::PowerCommand(_) => None,
    };
    let Some(bundle_str) = bundle else {
        return ptr::null();
    };

    let mut cache = list_ref.bundle_id_cache.borrow_mut();
    if cache.len() < list_ref.entries.len() {
        cache.resize(list_ref.entries.len(), None);
    }
    if cache[real_idx].is_none() {
        let Ok(cstring) = CString::new(bundle_str) else {
            return ptr::null();
        };
        cache[real_idx] = Some(cstring);
    }
    cache[real_idx]
        .as_ref()
        .map_or(ptr::null(), |s| s.as_ptr())
}

/// Return a borrowed pointer to the entry-at-`idx`'s stable English category
/// label — one of `"Application"`, `"Window"`, `"Workspace"`, `"Command"`,
/// `"PowerCommand"`. The UI displays these as-is; localization is a UI-layer
/// concern.
///
/// `idx` is the *filtered* index. Borrow lifetime: same as `get_name`.
///
/// Returns null when `list` is null or `idx >= len`.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_category(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(real_idx) = list_ref.resolved(idx) else {
        return ptr::null();
    };

    let label: &str = match list_ref.entries[real_idx].kind() {
        EntryKind::Application => CATEGORY_APPLICATION,
        EntryKind::Window => CATEGORY_WINDOW,
        EntryKind::Workspace => CATEGORY_WORKSPACE,
        EntryKind::Command => CATEGORY_COMMAND,
        EntryKind::PowerCommand => CATEGORY_POWER_COMMAND,
    };

    let mut cache = list_ref.category_cache.borrow_mut();
    if cache.len() < list_ref.entries.len() {
        cache.resize(list_ref.entries.len(), None);
    }
    if cache[real_idx].is_none() {
        let Ok(cstring) = CString::new(label) else {
            return ptr::null();
        };
        cache[real_idx] = Some(cstring);
    }
    cache[real_idx]
        .as_ref()
        .map_or(ptr::null(), |s| s.as_ptr())
}

/// Return a borrowed pointer to the entry-at-`idx`'s icon identifier. For
/// `Entry::Application` and `Entry::Window` this is the `icon` field as pushed
/// in (or null when `None`); for every other variant the result is null today.
///
/// `idx` is the *filtered* index. Borrow lifetime: same as `get_name`.
///
/// Returns null when `list` is null, `idx >= len`, or the entry has no icon.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_icon(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(real_idx) = list_ref.resolved(idx) else {
        return ptr::null();
    };

    // Exhaustive match so future variants are a compile error until this is
    // updated. Application and Window both carry a caller-supplied icon
    // identifier; the other variants don't have one today.
    let icon: Option<&str> = match &list_ref.entries[real_idx] {
        Entry::Application(app) => app.icon.as_deref(),
        Entry::Window(w) => w.icon.as_deref(),
        Entry::Workspace(_) | Entry::Command(_) | Entry::PowerCommand(_) => None,
    };

    let mut cache = list_ref.icon_cache.borrow_mut();
    if cache.len() < list_ref.entries.len() {
        cache.resize(list_ref.entries.len(), None);
    }
    if cache[real_idx].is_none() {
        // Outer `Some` means "we've computed this"; inner `Option<CString>`
        // distinguishes "icon present" from "no icon". This lets us return a
        // stable null for absent icons without recomputing every call.
        let computed = match icon {
            Some(s) => CString::new(s).ok(),
            None => None,
        };
        cache[real_idx] = Some(computed);
    }
    match &cache[real_idx] {
        Some(Some(cs)) => cs.as_ptr(),
        _ => ptr::null(),
    }
}

/// Reorder the underlying entries in MRU order, most-recent-first. Reads
/// every row from `store` (via `MruStore::read_all`), builds a rank map
/// keyed by `EntryRef` (rank 0 = most recent), and stable-sorts the list
/// in place by `(rank, original_position)`. Entries with no MRU row fall
/// to the bottom in their original push order (the stable sort preserves
/// relative order for equal keys; we use `usize::MAX` for the unknown
/// rank).
///
/// Returns `true` on success. Returns `false` when:
/// - `list` or `store` is null,
/// - or `MruStore::read_all` fails. On read failure the list is left
///   untouched (no partial reordering) and the launcher proceeds with
///   degraded behavior.
///
/// Like every other mutating call, this invalidates every pointer
/// previously returned by `lofi_entries_get_*`. The filter is recomputed
/// against the freshly-reordered vec so an active query still works.
///
/// # Safety
///
/// `list` must be null or a pointer obtained from `lofi_entries_new`.
/// `store` must be null or a pointer obtained from `lofi_mru_open`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_apply_mru(
    list: *mut EntryList,
    store: *const MruStore,
) -> bool {
    if list.is_null() || store.is_null() {
        return false;
    }

    // SAFETY: non-null per the precondition; single-threaded FFI contract
    // gives us exclusive access to `list` and shared access to `store`.
    let store_ref = unsafe { &*store };
    let recent: Vec<EntryRef> = match store_ref.read_all() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lofi_entries_apply_mru: {e}");
            return false;
        }
    };
    let ranks: HashMap<EntryRef, usize> = recent
        .into_iter()
        .enumerate()
        .map(|(i, r)| (r, i))
        .collect();

    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &mut *list };

    // Stable-sort by (rank, original_position). `Vec::sort_by_key` is
    // stable, so equal keys keep their relative order. For entries with
    // no MRU row we use `usize::MAX`, which sinks them below all known
    // entries while preserving their push order.
    let mut paired: Vec<(usize, Entry)> = list_ref
        .entries
        .drain(..)
        .map(|e| {
            let rank = ranks.get(&e.reference()).copied().unwrap_or(usize::MAX);
            (rank, e)
        })
        .collect();
    paired.sort_by_key(|(rank, _)| *rank);
    // Count how many entries actually got a finite MRU rank. After the
    // sort those entries sit in the leading positions of `entries`; the
    // boundary lets `recompute_filter` keep MRU-known matches above
    // score-ranked ones. We count *before* dropping the rank tags.
    list_ref.mru_count = paired.iter().filter(|(r, _)| *r != usize::MAX).count();
    list_ref.entries = paired.into_iter().map(|(_, e)| e).collect();

    list_ref.clear_caches();
    list_ref.recompute_filter();
    true
}

/// Append a window entry to the list. Copies every string in; the caller's C
/// buffers may be reused or freed as soon as the call returns.
///
/// Returns `true` on success, `false` if any of:
/// - `list` is null
/// - `title` is null
/// - any non-null string is not valid UTF-8 (this includes `app_name`,
///   `icon`, and `app_desktop_id`)
///
/// `app_name`, `icon`, and `app_desktop_id` may each be null to mean "field
/// absent"; the resulting `Window` carries a `None` for the corresponding
/// `Option<String>`. A non-null pointer that is invalid UTF-8 is rejected
/// (strict rather than silent-`None`, matching `push_application`).
///
/// A successful push invalidates every pointer previously returned by
/// `lofi_entries_get_*` (the borrow contract). The filter is recomputed
/// against the active query so the new entry appears in `len`/`get_*` only
/// when it matches.
///
/// # Safety
///
/// Pointers must be null or point at NUL-terminated C strings whose buffers
/// remain valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_push_window(
    list: *mut EntryList,
    id: u64,
    title: *const c_char,
    app_name: *const c_char,
    icon: *const c_char,
    workspace: i32,
    app_desktop_id: *const c_char,
) -> bool {
    if list.is_null() || title.is_null() {
        return false;
    }

    // SAFETY: non-null and assumed-valid C string per the function contract.
    let title_str = match unsafe { CStr::from_ptr(title) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return false,
    };
    let app_name_opt = if app_name.is_null() {
        None
    } else {
        // SAFETY: non-null branch — caller's contract is the same as above.
        match unsafe { CStr::from_ptr(app_name) }.to_str() {
            Ok(s) => Some(s.to_owned()),
            Err(_) => return false,
        }
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
    let app_desktop_id_opt = if app_desktop_id.is_null() {
        None
    } else {
        // SAFETY: non-null branch — caller's contract is the same as above.
        match unsafe { CStr::from_ptr(app_desktop_id) }.to_str() {
            Ok(s) => Some(s.to_owned()),
            Err(_) => return false,
        }
    };

    // SAFETY: non-null `list` precondition; we have exclusive access for the
    // duration of this call by the single-threaded FFI contract.
    let list_ref = unsafe { &mut *list };
    list_ref.push(Entry::Window(Window {
        id,
        title: title_str,
        app_name: app_name_opt,
        icon: icon_opt,
        workspace,
        app_desktop_id: app_desktop_id_opt,
    }));
    true
}

/// Return the `CGWindowID` (or platform-equivalent integer id) for the
/// `Entry::Window` at the filtered idx. Returns `0` for any other case:
/// - `list` is null
/// - `idx` is out of bounds
/// - the resolved entry is not an `Entry::Window`
///
/// The `0` sentinel is safe because real `CGWindowID`s on macOS are always
/// strictly greater than 0 for regular application windows. Callers are
/// expected to gate on `lofi_entries_get_category(...) == "Window"` before
/// reading; this is a robustness fallback rather than the primary signal.
///
/// Unlike the string accessors there is no cache: the `u64` round-trips
/// through the FFI by value, and no `CString` allocation is involved.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_window_id(
    list: *const EntryList,
    idx: usize,
) -> u64 {
    if list.is_null() {
        return 0;
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return 0;
    };
    match entry {
        Entry::Window(w) => w.id,
        Entry::Application(_)
        | Entry::Workspace(_)
        | Entry::Command(_)
        | Entry::PowerCommand(_) => 0,
    }
}

/// Return `true` when the entry at the filtered `idx` is an `Application`
/// whose `is_running` flag is set — i.e. the platform layer reported at
/// least one open window for that app at gather time. Returns `false` for
/// any other case:
/// - `list` is null
/// - `idx` is out of bounds
/// - the resolved entry is not an `Entry::Application`
/// - the Application's `is_running` field is `false`
///
/// Drives the running-indicator dot in the UI. The boolean return matches
/// the GNOME `recent_window_id.is_some()` shape but does not require the
/// macOS platform layer to plumb a real `CGWindowID` through the FFI; on
/// macOS the activation path is `NSWorkspace.open(...)` and the launcher
/// does not raise a specific existing window itself, so the window id
/// would never be read.
///
/// Like `get_window_id`, this accessor returns a value by copy and is not
/// subject to the borrow contract — no pointer into the list is handed out.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_is_running(
    list: *const EntryList,
    idx: usize,
) -> bool {
    if list.is_null() {
        return false;
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return false;
    };
    match entry {
        Entry::Application(app) => app.is_running,
        Entry::Window(_)
        | Entry::Workspace(_)
        | Entry::Command(_)
        | Entry::PowerCommand(_) => false,
    }
}

/// Stable C-string form of a `CommandKind`'s id, for `lofi_entries_get_command_id`.
///
/// Each arm returns a process-lifetime `&'static CStr` built from a `c"..."`
/// literal, so the pointer handed back across the FFI never dangles and is
/// never invalidated by a later mutation (unlike the lazily-cached string
/// accessors). No `RefCell`/cache slot is needed.
///
/// The bytes here MUST stay byte-for-byte equal to the corresponding
/// `CommandKind::as_id` (`app/core/src/lib.rs`) — `as_id` is the canonical
/// snake_case id that round-trips into `EntryRef::Command` / the persistent MRU
/// key. The match is exhaustive (no `_` arm) so adding a `CommandKind` variant
/// is a compile error here until both this map and `as_id` are extended in
/// lockstep. The `command_id_matches_as_id_for_all_kinds` FFI test guards
/// against silent drift.
fn command_id_cstr(kind: CommandKind) -> &'static CStr {
    match kind {
        CommandKind::Center => c"center",
        CommandKind::CenterThird => c"center_third",
        CommandKind::CenterHalf => c"center_half",
        CommandKind::CenterTwoThirds => c"center_two_thirds",
        CommandKind::LeftThird => c"left_third",
        CommandKind::LeftHalf => c"left_half",
        CommandKind::LeftTwoThirds => c"left_two_thirds",
        CommandKind::RightThird => c"right_third",
        CommandKind::RightHalf => c"right_half",
        CommandKind::RightTwoThirds => c"right_two_thirds",
        CommandKind::StandardSize => c"standard_size",
        CommandKind::Minimize => c"minimize",
        CommandKind::ToggleMaximize => c"toggle_maximize",
        CommandKind::ToggleFullscreen => c"toggle_fullscreen",
        CommandKind::NextDisplay => c"next_display",
        CommandKind::PreviousDisplay => c"previous_display",
    }
}

/// Append a window-action command entry to the list. Mirrors the `Command`
/// struct field-for-field: `kind_id` selects the `CommandKind`,
/// `target_window_id` is the window the command will act on at activation, the
/// `wa_*` quadruple is the target window's monitor work area, and the `frame_*`
/// quadruple is the target window's current frame at gather time (read by
/// `CommandKind::Center` to recenter without resizing). All eight integers are
/// in the caller's coordinate space and are stored verbatim — the platform
/// layer is responsible for handing them in the convention `compute_geometry`
/// expects (on macOS: top-left global; see `app/macos`).
///
/// Returns `true` on success, `false` if any of:
/// - `list` is null
/// - `kind_id` is null
/// - `kind_id` is not valid UTF-8
/// - `kind_id` is not a recognized `CommandKind::as_id` (unknown id; nothing is
///   pushed, so the list length is unchanged)
///
/// A successful push invalidates every pointer previously returned by
/// `lofi_entries_get_*` (the borrow contract). The filter is recomputed against
/// the active query so the new command appears in `len`/`get_*` only when it
/// matches.
///
/// # Safety
///
/// `list` must be null or a pointer obtained from `lofi_entries_new`.
/// `kind_id` must be null or a NUL-terminated C string whose buffer remains
/// valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_push_command(
    list: *mut EntryList,
    kind_id: *const c_char,
    target_window_id: u64,
    wa_x: i32,
    wa_y: i32,
    wa_w: i32,
    wa_h: i32,
    frame_x: i32,
    frame_y: i32,
    frame_w: i32,
    frame_h: i32,
) -> bool {
    if list.is_null() || kind_id.is_null() {
        return false;
    }

    // SAFETY: non-null and assumed-valid C string per the function contract.
    let kind_str = match unsafe { CStr::from_ptr(kind_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Unknown ids are rejected (no garbage entry inserted) so a Swift typo or
    // a stale id surfaces as a `false` rather than a silently-broken row.
    let Some(kind) = CommandKind::from_id(kind_str) else {
        return false;
    };

    // SAFETY: non-null `list` precondition; we have exclusive access for the
    // duration of this call by the single-threaded FFI contract.
    let list_ref = unsafe { &mut *list };
    list_ref.push(Entry::Command(Command {
        kind,
        target_window_id,
        work_area: WorkArea {
            x: wa_x,
            y: wa_y,
            width: wa_w,
            height: wa_h,
        },
        current_frame: (frame_x, frame_y, frame_w, frame_h),
    }));
    true
}

/// Return a borrowed pointer to the entry-at-`idx`'s command id — the
/// `CommandKind::as_id` snake_case string for an `Entry::Command`, and null for
/// every other variant. The match below is exhaustive on `Entry` so a new
/// variant is a compile error until this function is updated.
///
/// `idx` is the *filtered* index. Unlike the other string accessors the
/// returned pointer is a process-lifetime `&'static CStr` (see
/// `command_id_cstr`): it is NOT invalidated by a later mutation. Swift still
/// copies it for uniformity.
///
/// Returns null when `list` is null, `idx >= len`, or the entry is not a
/// Command.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_command_id(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return ptr::null();
    };
    match entry {
        Entry::Command(c) => command_id_cstr(c.kind).as_ptr(),
        Entry::Application(_)
        | Entry::Window(_)
        | Entry::Workspace(_)
        | Entry::PowerCommand(_) => ptr::null(),
    }
}

/// Compute the target geometry for the command-at-`idx` and write it to the
/// four out-params. The geometry is `compute_geometry(kind, &work_area,
/// current_frame)` — the single source of geometry truth shared with the GNOME
/// frontend — so Swift never duplicates the half / two-thirds arithmetic.
///
/// Returns `true` and writes all four out-params only for an `Entry::Command`
/// whose kind is a *geometry* kind (`Center`, `CenterHalf`, `CenterTwoThirds`,
/// `LeftHalf`, `RightHalf`, `StandardSize`). Returns `false` and leaves ALL
/// FOUR out-params untouched (documented contract) for every other case:
/// - any out-pointer is null (guarded first)
/// - `list` is null
/// - `idx >= len`
/// - the entry is not a Command
/// - the command is a *state-toggle* kind (`Minimize`, `ToggleMaximize`,
///   `ToggleFullscreen`), where `compute_geometry` returns `None` — Swift
///   dispatches those by `lofi_entries_get_command_id` instead.
///
/// The result is by value through caller-owned out-params, so this accessor is
/// exempt from the borrow contract (no pointer into the list is handed out).
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer. Each non-null
/// out-pointer must reference writable `i32` storage that outlives the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_command_geometry(
    list: *const EntryList,
    idx: usize,
    out_x: *mut i32,
    out_y: *mut i32,
    out_w: *mut i32,
    out_h: *mut i32,
) -> bool {
    // Guard null out-pointers first: if we cannot write all four results we
    // must not write any, so reject before touching the list.
    if out_x.is_null() || out_y.is_null() || out_w.is_null() || out_h.is_null() {
        return false;
    }
    if list.is_null() {
        return false;
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return false;
    };
    let geometry = match entry {
        Entry::Command(c) => compute_geometry(c.kind, &c.work_area, c.current_frame),
        Entry::Application(_)
        | Entry::Window(_)
        | Entry::Workspace(_)
        | Entry::PowerCommand(_) => None,
    };
    let Some((x, y, w, h)) = geometry else {
        // State-toggle kind / non-Command: leave the out-params untouched.
        return false;
    };
    // SAFETY: each out-pointer is non-null (guarded at the top) and the caller
    // guarantees writable, live `i32` storage for the duration of the call.
    unsafe {
        *out_x = x;
        *out_y = y;
        *out_w = w;
        *out_h = h;
    }
    true
}

/// Stable C-string form of a `PowerCommandKind`'s id, for
/// `lofi_entries_get_power_command_id`. Mirrors `command_id_cstr` for the
/// window-action commands.
///
/// Each arm returns a process-lifetime `&'static CStr` built from a `c"..."`
/// literal, so the pointer handed back across the FFI never dangles and is
/// never invalidated by a later mutation.
///
/// The bytes here MUST stay byte-for-byte equal to the corresponding
/// `PowerCommandKind::as_id` (`app/core/src/lib.rs`). The match is exhaustive
/// (no `_` arm) so adding a variant is a compile error here until both maps
/// are extended in lockstep.
fn power_command_id_cstr(kind: PowerCommandKind) -> &'static CStr {
    match kind {
        PowerCommandKind::LockSession => c"lock_session",
        PowerCommandKind::Logout => c"logout",
        PowerCommandKind::Suspend => c"suspend",
        PowerCommandKind::Restart => c"restart",
        PowerCommandKind::Shutdown => c"shutdown",
    }
}

/// Append a system-level power-command entry to the list. Unlike the window-
/// action commands, power commands have no target window, work area, or
/// current frame — the command always applies — so `kind_id` is the only
/// input. Mirrors `PowerCommandKind::as_id` (e.g. `"lock_session"`,
/// `"shutdown"`).
///
/// Returns `true` on success, `false` if any of:
/// - `list` is null
/// - `kind_id` is null
/// - `kind_id` is not valid UTF-8
/// - `kind_id` is not a recognized `PowerCommandKind::as_id` (unknown id;
///   nothing is pushed)
///
/// A successful push invalidates every pointer previously returned by
/// `lofi_entries_get_*` (the borrow contract). The filter is recomputed
/// against the active query so the new command appears in `len`/`get_*` only
/// when it matches.
///
/// # Safety
///
/// `list` must be null or a pointer obtained from `lofi_entries_new`.
/// `kind_id` must be null or a NUL-terminated C string whose buffer remains
/// valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_push_power_command(
    list: *mut EntryList,
    kind_id: *const c_char,
) -> bool {
    if list.is_null() || kind_id.is_null() {
        return false;
    }

    // SAFETY: non-null and assumed-valid C string per the function contract.
    let kind_str = match unsafe { CStr::from_ptr(kind_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let Some(kind) = PowerCommandKind::from_id(kind_str) else {
        return false;
    };

    // SAFETY: non-null `list` precondition; we have exclusive access for the
    // duration of this call by the single-threaded FFI contract.
    let list_ref = unsafe { &mut *list };
    list_ref.push(Entry::PowerCommand(PowerCommand { kind }));
    true
}

/// Return a borrowed pointer to the entry-at-`idx`'s power-command id — the
/// `PowerCommandKind::as_id` snake_case string for an `Entry::PowerCommand`,
/// and null for every other variant. Mirrors `lofi_entries_get_command_id`.
///
/// `idx` is the *filtered* index. The returned pointer is a process-lifetime
/// `&'static CStr` (see `power_command_id_cstr`): it is NOT invalidated by a
/// later mutation. Swift still copies it for uniformity.
///
/// Returns null when `list` is null, `idx >= len`, or the entry is not a
/// PowerCommand.
///
/// # Safety
///
/// `list` must be null or a valid `EntryList` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lofi_entries_get_power_command_id(
    list: *const EntryList,
    idx: usize,
) -> *const c_char {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: non-null `list` per the precondition.
    let list_ref = unsafe { &*list };
    let Some(entry) = list_ref.resolve_filtered_index(idx) else {
        return ptr::null();
    };
    match entry {
        Entry::PowerCommand(c) => power_command_id_cstr(c.kind).as_ptr(),
        Entry::Application(_)
        | Entry::Window(_)
        | Entry::Workspace(_)
        | Entry::Command(_) => ptr::null(),
    }
}
