//! Integration tests for the `lofi-core` C-ABI FFI surface.
//!
//! These exercise the FFI from a foreign-caller perspective: every symbol is
//! reached via an `extern "C"` declaration rather than through the
//! `lofi_core::ffi` Rust module. That keeps the test honest about what Swift
//! (or any other consumer) actually sees — the linker name, the C signature,
//! and the null/UTF-8/lifetime contracts spelled out in the plan.
//!
//! The whole file is gated on `feature = "ffi"` so the default
//! `cargo test -p lofi-core` invocation (no features) is unaffected, and
//! `cargo test -p lofi-core --features ffi` runs the suite.

#![cfg(feature = "ffi")]

// Forces the linker to bring the lofi_core rlib in, so the
// #[unsafe(no_mangle)] FFI symbols below are present at link time.
// Without this, rustc registers the rlib as a dep but the linker
// drops it because no Rust-side item from lofi_core is referenced.
extern crate lofi_core as _;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

/// Opaque type matching the Rust-side `EntryList` newtype. The tests only
/// ever traffic in `*mut EntryList` / `*const EntryList`; the inside is
/// inaccessible by design (mirrors the `typedef struct lofi_EntryList
/// lofi_EntryList;` that cbindgen emits for Swift).
#[repr(C)]
struct EntryList {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn lofi_entries_new() -> *mut EntryList;
    fn lofi_entries_free(list: *mut EntryList);
    fn lofi_entries_push_application(
        list: *mut EntryList,
        name: *const c_char,
        bundle_id: *const c_char,
        icon: *const c_char,
    ) -> bool;
    fn lofi_entries_len(list: *const EntryList) -> usize;
    fn lofi_entries_get_name(list: *const EntryList, idx: usize) -> *const c_char;
    fn lofi_entries_set_query(list: *mut EntryList, query: *const c_char) -> bool;
    fn lofi_entries_get_bundle_id(list: *const EntryList, idx: usize) -> *const c_char;
    fn lofi_entries_get_category(list: *const EntryList, idx: usize) -> *const c_char;
    fn lofi_entries_get_icon(list: *const EntryList, idx: usize) -> *const c_char;

    // MRU FFI surface (added in the macOS MRU slice). Mirrors the C
    // signatures cbindgen emits for the new `lofi_core::ffi::mru` module:
    //
    //     typedef struct MruStore MruStore;
    //     struct MruStore *lofi_mru_open(const char *path);
    //     void             lofi_mru_free(struct MruStore *store);
    //     bool             lofi_mru_bump_entry(const struct MruStore *store,
    //                                          const struct EntryList *list,
    //                                          uintptr_t idx);
    //     bool             lofi_entries_apply_mru(struct EntryList *list,
    //                                             const struct MruStore *store);
    //
    // The tests below only ever traffic in raw pointers to `MruStore`; the
    // inside is opaque by design, same shape as `EntryList`.
    fn lofi_mru_open(path: *const c_char) -> *mut MruStore;
    fn lofi_mru_free(store: *mut MruStore);
    fn lofi_mru_bump_entry(
        store: *const MruStore,
        list: *const EntryList,
        idx: usize,
    ) -> bool;
    fn lofi_entries_apply_mru(list: *mut EntryList, store: *const MruStore) -> bool;

    // Window FFI surface (added in the macOS windows slice). Mirrors the C
    // signatures cbindgen emits for the two new symbols in
    // `lofi_core::ffi::entries`:
    //
    //     bool     lofi_entries_push_window(struct EntryList *list,
    //                                        uint64_t id,
    //                                        const char *title,
    //                                        const char *app_name,
    //                                        const char *icon,
    //                                        int32_t workspace,
    //                                        const char *app_desktop_id);
    //     uint64_t lofi_entries_get_window_id(const struct EntryList *list,
    //                                          uintptr_t idx);
    //
    // `app_name`, `icon`, and `app_desktop_id` are nullable per the plan;
    // `title` is required. Invalid UTF-8 in any non-null string causes
    // push_window to return false. get_window_id returns 0 for null list,
    // non-Window variant, or out-of-bounds idx — 0 is a safe sentinel
    // because real CGWindowIDs on macOS are always > 0 for app windows.
    fn lofi_entries_push_window(
        list: *mut EntryList,
        id: u64,
        title: *const c_char,
        app_name: *const c_char,
        icon: *const c_char,
        workspace: i32,
        app_desktop_id: *const c_char,
    ) -> bool;
    fn lofi_entries_get_window_id(list: *const EntryList, idx: usize) -> u64;

    // Command FFI surface (added in the macOS window-commands slice). Mirrors
    // the C signatures cbindgen emits for the three new symbols in
    // `lofi_core::ffi::entries`:
    //
    //     bool        lofi_entries_push_command(struct EntryList *list,
    //                                            const char *kind_id,
    //                                            uint64_t target_window_id,
    //                                            int32_t wa_x, int32_t wa_y,
    //                                            int32_t wa_w, int32_t wa_h,
    //                                            int32_t frame_x, int32_t frame_y,
    //                                            int32_t frame_w, int32_t frame_h);
    //     const char *lofi_entries_get_command_id(const struct EntryList *list,
    //                                              uintptr_t idx);
    //     bool        lofi_entries_get_command_geometry(const struct EntryList *list,
    //                                                    uintptr_t idx,
    //                                                    int32_t *out_x,
    //                                                    int32_t *out_y,
    //                                                    int32_t *out_w,
    //                                                    int32_t *out_h);
    //
    // `kind_id` is required and must be a valid `CommandKind::as_id` string
    // (unknown / null / invalid-UTF-8 ids cause push_command to return false
    // with no push). `get_command_id` returns `CommandKind::as_id` (a
    // process-lifetime `&'static CStr`) for Command entries and null for any
    // other variant / OOB / null list. `get_command_geometry` runs
    // `compute_geometry`: it writes the four out-params and returns true only
    // for geometry kinds; for state-toggle kinds (minimize / toggle_maximize /
    // toggle_fullscreen), non-Command variants, OOB idx, or a null list it
    // returns false AND leaves all four out-params untouched (documented
    // contract). Null out-pointers are guarded (false).
    fn lofi_entries_push_command(
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
    ) -> bool;
    fn lofi_entries_get_command_id(list: *const EntryList, idx: usize) -> *const c_char;
    fn lofi_entries_get_command_geometry(
        list: *const EntryList,
        idx: usize,
        out_x: *mut i32,
        out_y: *mut i32,
        out_w: *mut i32,
        out_h: *mut i32,
    ) -> bool;
}

/// Opaque type matching the Rust-side `MruStore` newtype the FFI hands out
/// via `lofi_mru_open`. Same zero-sized-private-field shape as `EntryList`:
/// the inside is unreachable by design, mirroring the
/// `typedef struct MruStore MruStore;` cbindgen emits for Swift.
#[repr(C)]
struct MruStore {
    _private: [u8; 0],
}

/// Test helper: open a fresh `MruStore` backed by a SQLite file inside a
/// brand-new `tempfile::tempdir()`. Returns the `TempDir` alongside the
/// store pointer so the caller can keep the directory alive for the
/// lifetime of the test by holding the tuple; dropping the tuple's first
/// element cleans up the directory and its contents.
fn open_temp_store() -> (tempfile::TempDir, *mut MruStore) {
    let dir = tempfile::tempdir().expect("tempdir should succeed");
    let path = dir.path().join("mru.sqlite");
    let path_str = path.to_str().expect("tempdir path should be UTF-8");
    let cstr = CString::new(path_str).expect("tempdir path should have no NUL");
    // SAFETY: `cstr` lives across the call; `lofi_mru_open` copies what it
    // needs out of the borrowed C string before returning.
    let store = unsafe { lofi_mru_open(cstr.as_ptr()) };
    assert!(
        !store.is_null(),
        "lofi_mru_open should succeed in a fresh tempdir"
    );
    (dir, store)
}

#[test]
fn mru_open_creates_file_and_can_be_freed() {
    // Opening a store under a fresh tempdir must succeed, return a non-null
    // handle, and leave a SQLite file on disk at the requested path. After
    // free, the file should still exist (free closes the connection but
    // does not delete the backing store).
    let dir = tempfile::tempdir().expect("tempdir should succeed");
    let path = dir.path().join("mru.sqlite");
    let path_str = path.to_str().expect("tempdir path should be UTF-8");
    let cstr = CString::new(path_str).expect("tempdir path should have no NUL");

    // SAFETY: standard FFI lifecycle: open -> free. `cstr` lives across
    // the open call.
    unsafe {
        let store = lofi_mru_open(cstr.as_ptr());
        assert!(
            !store.is_null(),
            "lofi_mru_open should succeed in a fresh tempdir"
        );
        lofi_mru_free(store);
    }

    assert!(
        path.exists(),
        "the SQLite file should still exist on disk after lofi_mru_free"
    );
}

#[test]
fn mru_open_invalid_path_returns_null() {
    // `/dev/null/cannot_create` has a parent (`/dev/null`) that is a
    // character device, not a directory. `MruStore::open`'s parent-dir
    // create_dir_all call will fail; the FFI swallows that into null.
    let cstr = CString::new("/dev/null/cannot_create").expect("path has no NUL");
    // SAFETY: deliberately pass an unwritable path; FFI must return null,
    // not panic.
    unsafe {
        let store = lofi_mru_open(cstr.as_ptr());
        assert!(
            store.is_null(),
            "lofi_mru_open under an unwritable path must return null"
        );
        // Belt and braces: freeing null must be a safe no-op.
        lofi_mru_free(store);
    }
}

#[test]
fn mru_bump_then_apply_promotes_entry() {
    // Three apps in alphabetical push order. Bumping the third (Chrome)
    // must promote it to idx 0 after apply_mru. The other two retain their
    // relative input order (Alpha < Beta) because they have no MRU rank
    // and fall through the stable-sort by original position.
    let (_dir, store) = open_temp_store();

    // SAFETY: standard FFI lifecycle: new -> push -> bump -> apply -> read
    // -> free for both the list and the store.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Alpha", "com.example.alpha", None));
        assert!(push_app(list, "Beta", "com.example.beta", None));
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));

        assert!(
            lofi_mru_bump_entry(store, list, 2),
            "bumping idx 2 (Chrome) should return true"
        );
        assert!(
            lofi_entries_apply_mru(list, store),
            "apply_mru should succeed"
        );

        assert_eq!(
            lofi_entries_len(list),
            3,
            "len should be unchanged at 3 after apply_mru"
        );
        assert_eq!(
            name_at(list, 0),
            "Chrome",
            "Chrome should be at the top after bump+apply_mru"
        );

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn apply_mru_with_empty_store_preserves_input_order() {
    // Empty MRU file on first run: every entry has rank `usize::MAX`, so
    // the stable-sort by (rank, original_position) leaves the input order
    // untouched. Documents the "no special case needed" point from the
    // plan's risks section.
    const EXPECTED: [&str; 3] = ["Alpha", "Beta", "Chrome"];
    let (_dir, store) = open_temp_store();

    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();
        for name in EXPECTED.iter() {
            let bundle = format!("com.example.{name}");
            assert!(push_app(list, name, &bundle, None));
        }

        assert!(
            lofi_entries_apply_mru(list, store),
            "apply_mru against an empty store should succeed"
        );

        assert_eq!(
            lofi_entries_len(list),
            EXPECTED.len(),
            "len should match the push count"
        );
        for (i, expected) in EXPECTED.iter().enumerate() {
            assert_eq!(
                name_at(list, i),
                *expected,
                "empty-store apply_mru should preserve push order at idx {i}"
            );
        }

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn mru_persists_across_open() {
    // Open store A under a tempdir path, push apps, bump idx 1, free A.
    // Open store B at the same path with a fresh list pushed in the same
    // order. After apply_mru, idx 0 must be the previously bumped entry —
    // the proof that MRU state survives the close/reopen cycle through
    // the SQLite file on disk.
    let dir = tempfile::tempdir().expect("tempdir should succeed");
    let path = dir.path().join("mru.sqlite");
    let path_str = path.to_str().expect("tempdir path should be UTF-8");
    let cstr = CString::new(path_str).expect("tempdir path should have no NUL");

    // First open: push three apps, bump idx 1 (Beta), close.
    // SAFETY: standard FFI lifecycle for store A and its list.
    unsafe {
        let store_a = lofi_mru_open(cstr.as_ptr());
        assert!(!store_a.is_null(), "first open should succeed");

        let list_a = lofi_entries_new();
        assert!(push_app(list_a, "Alpha", "com.example.alpha", None));
        assert!(push_app(list_a, "Beta", "com.example.beta", None));
        assert!(push_app(list_a, "Chrome", "com.google.Chrome", None));

        assert!(
            lofi_mru_bump_entry(store_a, list_a, 1),
            "bumping idx 1 (Beta) should succeed"
        );

        lofi_entries_free(list_a);
        lofi_mru_free(store_a);
    }

    // Second open: same path, fresh list, same push order. After apply_mru,
    // Beta should be promoted from the persisted MRU state.
    // SAFETY: standard FFI lifecycle for store B and its list.
    unsafe {
        let store_b = lofi_mru_open(cstr.as_ptr());
        assert!(!store_b.is_null(), "second open at the same path should succeed");

        let list_b = lofi_entries_new();
        assert!(push_app(list_b, "Alpha", "com.example.alpha", None));
        assert!(push_app(list_b, "Beta", "com.example.beta", None));
        assert!(push_app(list_b, "Chrome", "com.google.Chrome", None));

        assert!(
            lofi_entries_apply_mru(list_b, store_b),
            "apply_mru on the reopened store should succeed"
        );

        assert_eq!(
            name_at(list_b, 0),
            "Beta",
            "Beta should be at idx 0 after reopen+apply_mru thanks to persisted MRU"
        );

        lofi_entries_free(list_b);
        lofi_mru_free(store_b);
    }
}

#[test]
fn apply_mru_invalidates_caches() {
    // The borrow contract: `apply_mru` is a mutation, so any `*const c_char`
    // handed out by `get_*` before the call is invalidated. The new call to
    // `get_name(0)` after apply_mru must reflect the *new* top entry, not
    // a cached pointer to the previously-top entry. Mirrors the
    // borrow-lifetime contract test for push/set_query.
    let (_dir, store) = open_temp_store();

    // SAFETY: standard FFI lifecycle. We deliberately copy the bytes out
    // of the pre-mutation pointer and never dereference it after apply_mru.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Alpha", "com.example.alpha", None));
        assert!(push_app(list, "Beta", "com.example.beta", None));
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));

        // Warm the name cache for idx 0 and copy the bytes out.
        let p = lofi_entries_get_name(list, 0);
        assert!(!p.is_null(), "pre-mutation get_name(0) should be non-null");
        let before: Vec<u8> = CStr::from_ptr(p).to_bytes().to_vec();
        assert_eq!(
            before, b"Alpha",
            "pre-mutation idx 0 should be the first pushed name"
        );

        // Bump Chrome (idx 2) and apply. This is the mutation; `p` is now
        // not guaranteed valid and we do not touch it again.
        assert!(lofi_mru_bump_entry(store, list, 2), "bump should succeed");
        assert!(lofi_entries_apply_mru(list, store), "apply_mru should succeed");

        // Fresh borrow: must reflect the new top entry, not the cached one.
        let p2 = lofi_entries_get_name(list, 0);
        assert!(!p2.is_null(), "post-mutation get_name(0) should be non-null");
        let after: Vec<u8> = CStr::from_ptr(p2).to_bytes().to_vec();
        assert_eq!(
            after, b"Chrome",
            "post-apply_mru idx 0 should be the bumped entry, not the cached pre-mutation value"
        );

        // The owned pre-mutation copy is, of course, independent.
        assert_eq!(before, b"Alpha", "owned pre-mutation copy is unchanged");

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn apply_mru_with_query_active_keeps_filter() {
    // With a query that narrows to two entries, apply_mru must recompute
    // the filter against the freshly reordered underlying vec so the
    // visible count stays at 2. The two Firefox variants both contain the
    // "fire" subsequence; Chrome does not.
    let (_dir, store) = open_temp_store();

    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(
            list,
            "Firefox",
            "org.mozilla.firefox",
            None,
        ));
        assert!(push_app(
            list,
            "Firefox Developer Edition",
            "org.mozilla.firefoxdeveloperedition",
            None,
        ));
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));

        // Narrow to the two Firefox entries.
        let q = CString::new("fire").expect("query is valid C string");
        assert!(
            lofi_entries_set_query(list, q.as_ptr()),
            "set_query should succeed"
        );
        assert_eq!(
            lofi_entries_len(list),
            2,
            "sanity: \"fire\" should narrow to both Firefox variants"
        );

        // Bump the developer edition. In the filtered view it lives at
        // idx 1 (insertion order: Firefox = filtered idx 0, Developer
        // Edition = filtered idx 1, since Chrome was filtered out). The
        // bump must go through the filtered-index resolver and reach the
        // correct underlying entry.
        assert!(
            lofi_mru_bump_entry(store, list, 1),
            "bumping filtered idx 1 (Developer Edition) should succeed"
        );
        assert!(
            lofi_entries_apply_mru(list, store),
            "apply_mru with active query should succeed"
        );

        assert_eq!(
            lofi_entries_len(list),
            2,
            "filter should still match both Firefox variants after apply_mru"
        );

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn mru_bump_entry_null_args_return_false() {
    // Null-pointer contract for bump_entry: any null argument returns false
    // and does not crash. Three sub-cases in one test.
    let (_dir, store) = open_temp_store();

    // SAFETY: deliberately pass nulls; FFI must short-circuit to false.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Solo", "com.example.solo", None));

        // (a) null store + valid list.
        assert!(
            !lofi_mru_bump_entry(ptr::null(), list, 0),
            "bump with null store must return false"
        );

        // (b) valid store + null list.
        assert!(
            !lofi_mru_bump_entry(store, ptr::null(), 0),
            "bump with null list must return false"
        );

        // (c) both null.
        assert!(
            !lofi_mru_bump_entry(ptr::null(), ptr::null(), 0),
            "bump with both null must return false"
        );

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn mru_apply_null_args_return_false() {
    // Null-pointer contract for apply_mru: any null argument returns false
    // and does not crash. Symmetric with bump_entry.
    let (_dir, store) = open_temp_store();

    // SAFETY: deliberately pass nulls; FFI must short-circuit to false.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Solo", "com.example.solo", None));

        // (a) null list + valid store.
        assert!(
            !lofi_entries_apply_mru(ptr::null_mut(), store),
            "apply_mru with null list must return false"
        );

        // (b) valid list + null store.
        assert!(
            !lofi_entries_apply_mru(list, ptr::null()),
            "apply_mru with null store must return false"
        );

        // (c) both null.
        assert!(
            !lofi_entries_apply_mru(ptr::null_mut(), ptr::null()),
            "apply_mru with both null must return false"
        );

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

#[test]
fn mru_bump_out_of_bounds_returns_false() {
    // Out-of-bounds idx must return false (no entry to resolve a reference
    // for) and must not perturb the underlying list. After a follow-up
    // apply_mru against the still-empty store, the entry order must be
    // unchanged — proving that no entry was actually bumped on the OOB
    // call.
    const FAR_OUT_OF_BOUNDS: usize = 999;
    const EXPECTED: [&str; 3] = ["Alpha", "Beta", "Chrome"];
    let (_dir, store) = open_temp_store();

    // SAFETY: standard FFI lifecycle with one deliberately OOB bump call.
    unsafe {
        let list = lofi_entries_new();
        for name in EXPECTED.iter() {
            let bundle = format!("com.example.{name}");
            assert!(push_app(list, name, &bundle, None));
        }

        assert!(
            !lofi_mru_bump_entry(store, list, FAR_OUT_OF_BOUNDS),
            "bump with out-of-bounds idx must return false"
        );

        // Apply against the (still-empty) store; the entry order must be
        // unchanged.
        assert!(
            lofi_entries_apply_mru(list, store),
            "apply_mru should succeed after OOB bump"
        );
        for (i, expected) in EXPECTED.iter().enumerate() {
            assert_eq!(
                name_at(list, i),
                *expected,
                "entry order at idx {i} must be unchanged after OOB bump"
            );
        }

        lofi_entries_free(list);
        lofi_mru_free(store);
    }
}

/// Push a `(name, bundle_id, icon)` triple where every string is a valid
/// UTF-8 C string. Returns the boolean from the FFI call so each test can
/// assert on it. Keeping this as a helper avoids re-typing the `CString`
/// dance in every test.
fn push_app(list: *mut EntryList, name: &str, bundle_id: &str, icon: Option<&str>) -> bool {
    let name_c = CString::new(name).expect("name must be valid for CString");
    let bundle_c = CString::new(bundle_id).expect("bundle_id must be valid for CString");
    let icon_c = icon.map(|s| CString::new(s).expect("icon must be valid for CString"));
    let icon_ptr: *const c_char = match &icon_c {
        Some(s) => s.as_ptr(),
        None => ptr::null(),
    };
    // SAFETY: the C strings are owned by this function and live across the
    // call; `lofi_entries_push_application` copies their contents per the
    // FFI contract.
    unsafe { lofi_entries_push_application(list, name_c.as_ptr(), bundle_c.as_ptr(), icon_ptr) }
}

/// Read the name at `idx` and return it as an owned `String`. Panics if the
/// FFI returns null or non-UTF-8 — tests that want to assert on null call
/// `lofi_entries_get_name` directly.
fn name_at(list: *const EntryList, idx: usize) -> String {
    // SAFETY: caller is responsible for `idx` being in bounds; the borrowed
    // pointer is valid until the next mutation or free, both of which we
    // avoid before this `to_owned`.
    unsafe {
        let p = lofi_entries_get_name(list, idx);
        assert!(
            !p.is_null(),
            "lofi_entries_get_name returned null for in-bounds idx={idx}"
        );
        CStr::from_ptr(p)
            .to_str()
            .expect("name bytes should be UTF-8")
            .to_owned()
    }
}

#[test]
fn round_trip_push_len_and_get_name() {
    // SAFETY: standard FFI lifecycle: new -> push -> read -> free.
    unsafe {
        let list = lofi_entries_new();
        assert!(!list.is_null(), "lofi_entries_new should not return null");

        assert!(
            push_app(list, "Safari", "com.apple.Safari", Some("safari-icon")),
            "first push should return true"
        );
        assert!(
            push_app(list, "Terminal", "com.apple.Terminal", Some("terminal-icon")),
            "second push should return true"
        );

        assert_eq!(
            lofi_entries_len(list),
            2,
            "len should equal the number of successful pushes"
        );

        assert_eq!(
            name_at(list, 0),
            "Safari",
            "index 0 should be the first push"
        );
        assert_eq!(
            name_at(list, 1),
            "Terminal",
            "index 1 should be the second push"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_null_icon_succeeds() {
    // A null `icon` argument is explicitly allowed (maps to `None` on the
    // Rust side). The name must still be retrievable afterwards.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            push_app(list, "Calculator", "com.apple.Calculator", None),
            "push with null icon should return true"
        );
        assert_eq!(
            lofi_entries_len(list),
            1,
            "len should be 1 after one successful push"
        );
        assert_eq!(
            name_at(list, 0),
            "Calculator",
            "name should still be retrievable with a null icon"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_null_name_returns_false() {
    // SAFETY: deliberately pass a null `name` pointer; required-args contract
    // says push must return false without crashing.
    unsafe {
        let list = lofi_entries_new();

        let bundle = CString::new("com.example.NoName").expect("bundle id valid");
        let icon = CString::new("icon").expect("icon valid");
        let ok = lofi_entries_push_application(
            list,
            ptr::null(),
            bundle.as_ptr(),
            icon.as_ptr(),
        );
        assert!(!ok, "push with null name must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push fails on a null name"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_null_bundle_id_returns_false() {
    // SAFETY: deliberately pass a null `bundle_id` pointer.
    unsafe {
        let list = lofi_entries_new();

        let name = CString::new("Anonymous").expect("name valid");
        let icon = CString::new("icon").expect("icon valid");
        let ok = lofi_entries_push_application(
            list,
            name.as_ptr(),
            ptr::null(),
            icon.as_ptr(),
        );
        assert!(!ok, "push with null bundle_id must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push fails on a null bundle_id"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_null_list_returns_false() {
    // SAFETY: deliberately pass a null `list` pointer; this is the most
    // important "do not crash" case because there's no list to even read
    // `len` from afterwards.
    unsafe {
        let name = CString::new("Orphan").expect("name valid");
        let bundle = CString::new("com.example.Orphan").expect("bundle valid");
        let ok = lofi_entries_push_application(
            ptr::null_mut(),
            name.as_ptr(),
            bundle.as_ptr(),
            ptr::null(),
        );
        assert!(!ok, "push with null list must return false");
    }
}

#[test]
fn push_with_invalid_utf8_name_returns_false() {
    // 0xFF is never a valid UTF-8 byte; the FFI must reject it without
    // crashing. We cannot use `CString::new` on this (it accepts any non-NUL
    // bytes, which is what we want), then take a pointer into it. Note the
    // trailing NUL we add manually since we're not going through `CString`'s
    // validating constructor for the *content*.
    unsafe {
        let list = lofi_entries_new();

        // Bytes: 0xFF (invalid UTF-8), then NUL terminator.
        let bad_name: [u8; 2] = [0xFF, 0x00];
        let bundle = CString::new("com.example.Bad").expect("bundle valid");

        let ok = lofi_entries_push_application(
            list,
            bad_name.as_ptr().cast::<c_char>(),
            bundle.as_ptr(),
            ptr::null(),
        );
        assert!(!ok, "push with invalid UTF-8 name must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push fails on invalid UTF-8"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_invalid_utf8_bundle_id_returns_false() {
    // Strict-rejection contract for `bundle_id`: the FFI must refuse invalid
    // UTF-8 just like it does for `name`. Same shape as
    // `push_with_invalid_utf8_name_returns_false`, but the bad bytes are in
    // the bundle id; the name is a normal valid C string and icon is null.
    unsafe {
        let list = lofi_entries_new();

        let name = CString::new("ValidName").expect("name valid");
        // Bytes: 0xFF (invalid UTF-8), then NUL terminator.
        let bad_bundle: [u8; 2] = [0xFF, 0x00];

        let ok = lofi_entries_push_application(
            list,
            name.as_ptr(),
            bad_bundle.as_ptr().cast::<c_char>(),
            ptr::null(),
        );
        assert!(!ok, "push with invalid UTF-8 bundle_id must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push fails on invalid UTF-8 bundle_id"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_with_invalid_utf8_icon_returns_false() {
    // The plan calls out icon-invalid-UTF-8 as "be strict, not silent None":
    // a non-null `icon` pointer that contains invalid UTF-8 must cause the
    // push to fail outright, not be silently treated as `None`.
    unsafe {
        let list = lofi_entries_new();

        let name = CString::new("ValidName").expect("name valid");
        let bundle = CString::new("com.example.ValidBundle").expect("bundle valid");
        // Bytes: 0xFF (invalid UTF-8), then NUL terminator.
        let bad_icon: [u8; 2] = [0xFF, 0x00];

        let ok = lofi_entries_push_application(
            list,
            name.as_ptr(),
            bundle.as_ptr(),
            bad_icon.as_ptr().cast::<c_char>(),
        );
        assert!(!ok, "push with invalid UTF-8 icon must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push fails on invalid UTF-8 icon"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn ordering_preserved_across_multiple_pushes() {
    // Three pushes; get_name must return them in insertion order.
    const EXPECTED: [&str; 3] = ["Alpha", "Beta", "Gamma"];

    unsafe {
        let list = lofi_entries_new();

        for (i, name) in EXPECTED.iter().enumerate() {
            let bundle = format!("com.example.{name}");
            assert!(
                push_app(list, name, &bundle, None),
                "push #{i} ({name}) should return true"
            );
        }

        assert_eq!(
            lofi_entries_len(list),
            EXPECTED.len(),
            "len should match the number of pushes"
        );

        for (i, expected) in EXPECTED.iter().enumerate() {
            assert_eq!(
                name_at(list, i),
                *expected,
                "name at index {i} should match insertion order"
            );
        }

        lofi_entries_free(list);
    }
}

#[test]
fn out_of_bounds_get_name_returns_null() {
    // `get_name(list, len)` is exactly one past the end. Must return null,
    // must not panic, must not crash.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Only", "com.example.Only", None));

        let len = lofi_entries_len(list);
        let p = lofi_entries_get_name(list, len);
        assert!(
            p.is_null(),
            "get_name at idx==len must return null; got non-null"
        );

        // A much larger index must also return null, not crash.
        const FAR_OUT_OF_BOUNDS: usize = 1_000_000;
        let p2 = lofi_entries_get_name(list, FAR_OUT_OF_BOUNDS);
        assert!(
            p2.is_null(),
            "get_name at a far out-of-bounds idx must return null"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn free_of_null_is_noop() {
    // Calling `lofi_entries_free(null)` must be a safe no-op — mirrors
    // `free(NULL)` in C and supports Swift's `deinit` running on an EntryList
    // whose handle was never assigned.
    unsafe {
        lofi_entries_free(ptr::null_mut());
    }
}

#[test]
fn borrow_lifetime_contract_copy_before_mutation() {
    // The plan documents the returned `*const c_char` from get_name as
    // "borrow valid until next mutation or free." This test honors that
    // contract: read the borrowed bytes into an owned `Vec<u8>`, THEN
    // mutate (push another entry). The owned copy must still match the
    // original, and the freshly pushed entry must be reachable.
    //
    // We intentionally do NOT use the original pointer after the push — that
    // would be UB and is not what we are testing.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "First", "com.example.First", None));

        let p = lofi_entries_get_name(list, 0);
        assert!(!p.is_null(), "name at idx 0 should be non-null");

        // Copy the C string contents out before doing anything else with the
        // list.
        let owned: Vec<u8> = CStr::from_ptr(p).to_bytes().to_vec();
        assert_eq!(
            owned, b"First",
            "copied bytes should match the pushed name verbatim"
        );

        // Now mutate. After this point `p` is no longer guaranteed valid;
        // we do not touch it again.
        assert!(
            push_app(list, "Second", "com.example.Second", None),
            "post-mutation push should succeed"
        );

        assert_eq!(
            lofi_entries_len(list),
            2,
            "len should reflect both pushes after the mutation"
        );

        // The owned copy is unchanged regardless of what the list does.
        assert_eq!(
            owned, b"First",
            "owned copy must be independent of the list's storage"
        );

        // Re-read the freshly inserted entry through a new borrow; this is
        // safe because it's a new call to get_name with no intervening
        // mutation.
        let p1 = lofi_entries_get_name(list, 1);
        assert!(!p1.is_null(), "name at idx 1 should be non-null");
        let second = CStr::from_ptr(p1)
            .to_str()
            .expect("name should be UTF-8")
            .to_owned();
        assert_eq!(
            second, "Second",
            "second push should be retrievable after mutation"
        );

        lofi_entries_free(list);
    }
}

/// Test helper: set the current query on `list` via `lofi_entries_set_query`,
/// taking ownership of the `CString` for the duration of the call.
fn set_query(list: *mut EntryList, query: &str) -> bool {
    let q = CString::new(query).expect("query must be valid for CString");
    // SAFETY: `q` lives across the FFI call; the FFI is documented to copy
    // the bytes into the list's owned `query` field before returning.
    unsafe { lofi_entries_set_query(list, q.as_ptr()) }
}

/// Test helper: push three apps used by several of the `set_query_*` tests.
/// The names are deliberately picked so that `"fire"` matches only `"Firefox"`
/// and nothing else in this set (Calculator has no 'f', Terminal has no 'f').
fn push_three_apps(list: *mut EntryList) {
    assert!(
        push_app(list, "Firefox", "org.mozilla.firefox", None),
        "push Firefox should succeed"
    );
    assert!(
        push_app(list, "Calculator", "com.apple.calculator", None),
        "push Calculator should succeed"
    );
    assert!(
        push_app(list, "Terminal", "com.apple.Terminal", None),
        "push Terminal should succeed"
    );
}

/// Read the bundle id at `idx` and return it as an owned `String`. Panics on
/// null or non-UTF-8 — mirrors `name_at` for the new accessor.
fn bundle_id_at(list: *const EntryList, idx: usize) -> String {
    // SAFETY: caller is responsible for `idx` being in bounds; pointer is
    // valid until the next mutation or free.
    unsafe {
        let p = lofi_entries_get_bundle_id(list, idx);
        assert!(
            !p.is_null(),
            "lofi_entries_get_bundle_id returned null for in-bounds idx={idx}"
        );
        CStr::from_ptr(p)
            .to_str()
            .expect("bundle_id bytes should be UTF-8")
            .to_owned()
    }
}

/// Read the category at `idx` and return it as an owned `String`. Panics on
/// null or non-UTF-8 — mirrors `name_at` for the new accessor.
fn category_at(list: *const EntryList, idx: usize) -> String {
    // SAFETY: caller is responsible for `idx` being in bounds.
    unsafe {
        let p = lofi_entries_get_category(list, idx);
        assert!(
            !p.is_null(),
            "lofi_entries_get_category returned null for in-bounds idx={idx}"
        );
        CStr::from_ptr(p)
            .to_str()
            .expect("category bytes should be UTF-8")
            .to_owned()
    }
}

#[test]
fn set_query_filters_to_match() {
    // Filtering down to a single matching entry. With the three apps from
    // `push_three_apps`, the substring "fire" only fuzzy-matches "Firefox".
    unsafe {
        let list = lofi_entries_new();
        push_three_apps(list);

        assert!(set_query(list, "fire"), "set_query should return true");

        assert_eq!(
            lofi_entries_len(list),
            1,
            "len should reflect the single match for query \"fire\""
        );
        assert_eq!(
            name_at(list, 0),
            "Firefox",
            "the surviving entry under \"fire\" should be Firefox"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn set_query_empty_restores_all() {
    // Empty query is the passthrough case: after filtering down, setting the
    // query back to "" must restore the original count and insertion order.
    const EXPECTED: [&str; 3] = ["Firefox", "Calculator", "Terminal"];

    unsafe {
        let list = lofi_entries_new();
        push_three_apps(list);

        assert!(set_query(list, "fire"), "narrowing set_query should succeed");
        assert_eq!(
            lofi_entries_len(list),
            1,
            "sanity: \"fire\" narrows to one entry before restoring"
        );

        assert!(set_query(list, ""), "set_query(\"\") should succeed");

        assert_eq!(
            lofi_entries_len(list),
            EXPECTED.len(),
            "empty query should restore the full count"
        );
        for (i, expected) in EXPECTED.iter().enumerate() {
            assert_eq!(
                name_at(list, i),
                *expected,
                "empty query should restore insertion order at idx {i}"
            );
        }

        lofi_entries_free(list);
    }
}

#[test]
fn set_query_intersection_semantics() {
    // Multi-token queries require every whitespace-separated token to match.
    // "fire" matches all three Firefox variants; "dev" only matches the
    // entry whose haystack contains a d-e-v subsequence — "Firefox Developer
    // Edition". The plain "Firefox" entry has bundle id "org.mozilla.firefox"
    // (no 'd'), so it must be excluded.
    unsafe {
        let list = lofi_entries_new();

        assert!(push_app(
            list,
            "Firefox Developer Edition",
            "org.mozilla.firefoxdeveloperedition",
            None,
        ));
        assert!(push_app(list, "Firefox", "org.mozilla.firefox", None));
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));

        assert!(set_query(list, "fire dev"), "set_query should succeed");

        assert_eq!(
            lofi_entries_len(list),
            1,
            "only Firefox Developer Edition should satisfy both \"fire\" and \"dev\""
        );
        assert_eq!(
            name_at(list, 0),
            "Firefox Developer Edition",
            "the surviving entry under \"fire dev\" should be the developer edition"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn set_query_case_insensitive() {
    // The matcher is case-insensitive (skim's `.ignore_case()`); an
    // uppercase query must still match a mixed-case name.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Firefox", "org.mozilla.firefox", None));
        assert!(push_app(list, "Calculator", "com.apple.calculator", None));

        assert!(set_query(list, "FIRE"), "set_query should succeed");

        assert_eq!(
            lofi_entries_len(list),
            1,
            "uppercase \"FIRE\" should match Firefox only"
        );
        assert_eq!(
            name_at(list, 0),
            "Firefox",
            "case-insensitive match should still return Firefox"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn set_query_invalidates_get_name_borrow() {
    // The borrow contract: pointers returned by `get_*` are valid only until
    // the next mutating call. `set_query` is a mutating call. This test
    // documents the contract by example: take a borrow, copy the bytes out,
    // mutate via set_query, and only then assert against the owned copy. The
    // original pointer is NEVER dereferenced after the set_query call.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Firefox", "org.mozilla.firefox", None));
        assert!(push_app(list, "Calculator", "com.apple.calculator", None));

        let p = lofi_entries_get_name(list, 0);
        assert!(!p.is_null(), "name at idx 0 should be non-null pre-mutation");

        // Copy the bytes out BEFORE the mutating call.
        let owned: Vec<u8> = CStr::from_ptr(p).to_bytes().to_vec();
        assert_eq!(
            owned, b"Firefox",
            "copied bytes should match the pushed name verbatim"
        );

        // Mutate. After this point, `p` is no longer guaranteed valid and we
        // do not touch it again.
        assert!(set_query(list, "nomatch"), "set_query should succeed");

        // The owned copy is independent of the list's storage and unchanged.
        assert_eq!(
            owned, b"Firefox",
            "owned copy must survive set_query mutation intact"
        );

        // Sanity: the filter actually narrowed (no entry has a fuzzy match
        // for "nomatch" — none of the names or ids contain the n-o-m-a-t-c-h
        // subsequence).
        assert_eq!(
            lofi_entries_len(list),
            0,
            "query \"nomatch\" should filter out everything"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn get_bundle_id_round_trips() {
    // The new get_bundle_id accessor must return the bundle id verbatim, and
    // must return null for an out-of-bounds idx (the same null-on-OOB contract
    // as get_name).
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Foo", "org.example.foo", None));

        assert_eq!(
            bundle_id_at(list, 0),
            "org.example.foo",
            "get_bundle_id should return the pushed bundle id verbatim"
        );

        const FAR_OUT_OF_BOUNDS: usize = 999;
        let p = lofi_entries_get_bundle_id(list, FAR_OUT_OF_BOUNDS);
        assert!(
            p.is_null(),
            "get_bundle_id must return null for an out-of-bounds idx"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn get_category_returns_application() {
    // Application entries must report the stable category string
    // "Application" (the plan calls out this constant for the Application
    // variant; other variants get their own constants once they're wired up).
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Foo", "org.example.foo", None));

        assert_eq!(
            category_at(list, 0),
            "Application",
            "Application entries should report category \"Application\""
        );

        lofi_entries_free(list);
    }
}

#[test]
fn get_icon_returns_pushed_value() {
    // A non-null `icon` pushed in must come back out byte-for-byte via
    // get_icon. A null `icon` pushed in must come back as a null pointer
    // (no silent empty-string substitution).
    const ICON_PATH: &str = "/Applications/Foo.app";

    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Foo", "org.example.foo", Some(ICON_PATH)));
        assert!(push_app(list, "Bar", "org.example.bar", None));

        let p0 = lofi_entries_get_icon(list, 0);
        assert!(
            !p0.is_null(),
            "get_icon should return non-null for an entry pushed with a non-null icon"
        );
        let icon0 = CStr::from_ptr(p0)
            .to_str()
            .expect("icon bytes should be UTF-8");
        assert_eq!(
            icon0, ICON_PATH,
            "get_icon should return the pushed icon path verbatim"
        );

        let p1 = lofi_entries_get_icon(list, 1);
        assert!(
            p1.is_null(),
            "get_icon should return null for an entry pushed with a null icon"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn set_query_null_clears_filter() {
    // A null `query` pointer is documented as equivalent to the empty string
    // (passthrough — no filter). Symmetric with `set_query("")`.
    unsafe {
        let list = lofi_entries_new();
        push_three_apps(list);

        assert!(set_query(list, "fire"), "narrowing set_query should succeed");
        assert_eq!(
            lofi_entries_len(list),
            1,
            "sanity: \"fire\" narrows to one entry before clearing"
        );

        // Null query pointer: per the plan, must be treated as no filter.
        let ok = lofi_entries_set_query(list, ptr::null());
        assert!(ok, "set_query(null) should succeed and return true");

        assert_eq!(
            lofi_entries_len(list),
            3,
            "null query should clear the filter, restoring all entries"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_recomputes_filter() {
    // With a query active, pushing a new matching entry must make it visible
    // in `len`/`get_name` without a follow-up `set_query` call. Pushing a
    // non-matching entry must leave the visible count untouched.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Alpha", "alpha", None));
        assert!(push_app(list, "Beta", "beta", None));

        assert!(set_query(list, "ome"), "set_query should succeed");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "neither Alpha nor Beta should match \"ome\""
        );

        // Now push a matching entry; the filter must be recomputed.
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));

        assert_eq!(
            lofi_entries_len(list),
            1,
            "Chrome should appear in len after push under active query \"ome\""
        );
        assert_eq!(
            name_at(list, 0),
            "Chrome",
            "the visible entry after push should be the freshly pushed Chrome"
        );

        lofi_entries_free(list);
    }
}

/// Test helper: push a Window with all-valid UTF-8 strings for the required
/// `title` and the three optional string fields. Mirrors `push_app` for
/// terseness in the Window FFI tests. `workspace` is always 0 (macOS has no
/// Mutter-style workspaces; the field exists for cross-platform parity).
fn push_window_named(
    list: *mut EntryList,
    id: u64,
    title: &str,
    app_name: Option<&str>,
    icon: Option<&str>,
    app_desktop_id: Option<&str>,
) -> bool {
    let title_c = CString::new(title).expect("title must be valid for CString");
    let app_name_c = app_name.map(|s| CString::new(s).expect("app_name must be valid for CString"));
    let icon_c = icon.map(|s| CString::new(s).expect("icon must be valid for CString"));
    let app_desktop_id_c = app_desktop_id
        .map(|s| CString::new(s).expect("app_desktop_id must be valid for CString"));
    let app_name_ptr: *const c_char = match &app_name_c {
        Some(s) => s.as_ptr(),
        None => ptr::null(),
    };
    let icon_ptr: *const c_char = match &icon_c {
        Some(s) => s.as_ptr(),
        None => ptr::null(),
    };
    let app_desktop_id_ptr: *const c_char = match &app_desktop_id_c {
        Some(s) => s.as_ptr(),
        None => ptr::null(),
    };
    // SAFETY: every `CString` is owned by this function and lives across the
    // FFI call; the FFI is documented to copy what it needs out before
    // returning. `workspace = 0` is the cross-platform-default sentinel.
    unsafe {
        lofi_entries_push_window(
            list,
            id,
            title_c.as_ptr(),
            app_name_ptr,
            icon_ptr,
            0,
            app_desktop_id_ptr,
        )
    }
}

#[test]
fn push_window_round_trips() {
    // Push a fully populated window. Title is the user-visible row label
    // (matches `Entry::name()` for the Window variant); category is the
    // stable English "Window"; window_id round-trips the pushed u64.
    const WINDOW_ID: u64 = 42;

    // SAFETY: standard FFI lifecycle: new -> push -> read -> free.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            push_window_named(
                list,
                WINDOW_ID,
                "Untitled — TextEdit",
                Some("TextEdit"),
                Some("/Applications/TextEdit.app"),
                Some("com.apple.TextEdit"),
            ),
            "push_window with all-valid args should return true"
        );

        assert_eq!(
            lofi_entries_len(list),
            1,
            "len should be 1 after one successful push_window"
        );
        assert_eq!(
            name_at(list, 0),
            "Untitled — TextEdit",
            "Window's get_name should return the title verbatim"
        );
        assert_eq!(
            category_at(list, 0),
            "Window",
            "Window entries should report category \"Window\""
        );
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            WINDOW_ID,
            "get_window_id should round-trip the pushed CGWindowID"
        );

        // The Window icon field carries an icon-resolution input (on macOS
        // the owning .app's bundle path); get_icon must round-trip it so
        // the UI can resolve a real icon at draw time. Regression guard:
        // an earlier pass through `lofi_entries_get_icon` only matched
        // `Entry::Application` and silently dropped Window icons,
        // producing iconless rows in the launcher.
        let icon_ptr = lofi_entries_get_icon(list, 0);
        assert!(
            !icon_ptr.is_null(),
            "get_icon should return non-null for a Window pushed with an icon"
        );
        let icon = CStr::from_ptr(icon_ptr).to_str().expect("UTF-8 icon");
        assert_eq!(
            icon, "/Applications/TextEdit.app",
            "get_icon should return the pushed Window icon path verbatim"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_window_with_nil_optionals() {
    // Null app_name, null icon, null app_desktop_id — all explicitly allowed
    // and mapped to None on the Rust side. Title is still required and
    // present. The four accessors that need to work on Window entries
    // (name, category, window_id, icon) must all behave correctly:
    // - name returns the title
    // - category returns "Window"
    // - window_id returns the pushed id
    // - icon returns null (no silent empty-string substitution; mirrors the
    //   existing get_icon_returns_pushed_value contract for Applications)
    const WINDOW_ID: u64 = 7;

    // SAFETY: standard FFI lifecycle; the three optional pointers are null.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            push_window_named(list, WINDOW_ID, "Hello", None, None, None),
            "push_window with nil optionals should return true"
        );

        assert_eq!(
            lofi_entries_len(list),
            1,
            "len should be 1 after one successful push_window"
        );
        assert_eq!(
            name_at(list, 0),
            "Hello",
            "get_name should still return the title when optionals are null"
        );
        assert_eq!(
            category_at(list, 0),
            "Window",
            "category should be \"Window\" regardless of optional fields"
        );
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            WINDOW_ID,
            "get_window_id should round-trip the pushed CGWindowID"
        );

        let icon_ptr = lofi_entries_get_icon(list, 0);
        assert!(
            icon_ptr.is_null(),
            "get_icon should return null for a Window pushed with a null icon"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_window_null_title_returns_false() {
    // Null `title` is the required-args contract: push_window must return
    // false and not mutate the list. Other args are valid C strings so we're
    // testing the title-null path specifically, not a multi-null short-circuit.
    const WINDOW_ID: u64 = 11;

    // SAFETY: deliberately pass a null `title` pointer; FFI must short-circuit
    // to false without crashing.
    unsafe {
        let list = lofi_entries_new();

        let app_name = CString::new("TextEdit").expect("app_name valid");
        let app_desktop_id = CString::new("com.apple.TextEdit").expect("app_desktop_id valid");

        let ok = lofi_entries_push_window(
            list,
            WINDOW_ID,
            ptr::null(),
            app_name.as_ptr(),
            ptr::null(),
            0,
            app_desktop_id.as_ptr(),
        );
        assert!(!ok, "push_window with null title must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push_window fails on null title"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_window_invalid_utf8_title_returns_false() {
    // 0xFF is never a valid UTF-8 byte; the FFI must reject it strictly,
    // mirroring the existing push_application invalid-UTF-8 contract. Title
    // is constructed as raw bytes [0xFF, 0x00] (one invalid byte plus the
    // NUL terminator) and passed as a `*const c_char`.
    const WINDOW_ID: u64 = 13;

    // SAFETY: deliberately pass a non-UTF-8 title pointer; FFI must reject
    // without crashing.
    unsafe {
        let list = lofi_entries_new();

        let bad_title: [u8; 2] = [0xFF, 0x00];
        let app_name = CString::new("TextEdit").expect("app_name valid");

        let ok = lofi_entries_push_window(
            list,
            WINDOW_ID,
            bad_title.as_ptr().cast::<c_char>(),
            app_name.as_ptr(),
            ptr::null(),
            0,
            ptr::null(),
        );
        assert!(
            !ok,
            "push_window with invalid UTF-8 title must return false"
        );
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push_window fails on invalid UTF-8 title"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn get_window_id_returns_zero_for_application() {
    // The 0-sentinel contract: get_window_id returns 0 for anything that
    // isn't a Window at the given idx. Three sub-cases bundled here:
    //   (a) Application variant at the requested idx -> 0
    //   (b) Out-of-bounds idx -> 0
    //   (c) Null list pointer -> 0
    // Plus the positive case: after pushing a Window, its idx returns the
    // pushed u64 verbatim. The Application stays at idx 0 (insertion order
    // is preserved under no active query), the Window lands at idx 1.
    const WINDOW_ID: u64 = 99;
    const FAR_OUT_OF_BOUNDS: usize = 999;

    // SAFETY: standard FFI lifecycle plus a deliberate null-list call.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Calculator", "com.apple.Calculator", None));

        // (a) Application at idx 0 -> 0.
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            0,
            "get_window_id must return 0 for an Application variant"
        );

        // Now push a Window so we have a mixed list.
        assert!(
            push_window_named(list, WINDOW_ID, "Some Doc", Some("TextEdit"), None, None),
            "push_window should succeed for the mixed-list setup"
        );

        // Application still at 0, Window at 1.
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            0,
            "Application at idx 0 must still report 0 after a Window is pushed"
        );
        assert_eq!(
            lofi_entries_get_window_id(list, 1),
            WINDOW_ID,
            "Window at idx 1 must report its pushed CGWindowID"
        );

        // (b) Out-of-bounds idx -> 0.
        assert_eq!(
            lofi_entries_get_window_id(list, FAR_OUT_OF_BOUNDS),
            0,
            "get_window_id at an out-of-bounds idx must return 0"
        );

        // (c) Null list -> 0.
        assert_eq!(
            lofi_entries_get_window_id(ptr::null(), 0),
            0,
            "get_window_id with a null list must return 0"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn mixed_list_search_then_window_id() {
    // The filtered-index resolver must route through to the right underlying
    // entry on get_window_id. Push two apps and a window whose title is the
    // only haystack containing "cron"; narrow with set_query("cron"); the
    // single surviving entry at filtered idx 0 must be the Window, and
    // get_window_id(list, 0) must return that window's pushed id.
    const WINDOW_ID: u64 = 314;

    // SAFETY: standard FFI lifecycle with one set_query mutation; we read
    // get_window_id only after the mutation, so no stale-borrow concern.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Calculator", "com.apple.Calculator", None));
        assert!(push_app(list, "Calendar", "com.apple.iCal", None));
        assert!(
            push_window_named(
                list,
                WINDOW_ID,
                "Cron Job Notes",
                Some("Notes"),
                None,
                None,
            ),
            "push_window should succeed for the mixed-list search test"
        );

        assert!(set_query(list, "cron"), "set_query should succeed");

        assert_eq!(
            lofi_entries_len(list),
            1,
            "only the window's title \"Cron Job Notes\" should match \"cron\""
        );
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            WINDOW_ID,
            "filtered idx 0 must resolve through to the underlying window entry"
        );

        lofi_entries_free(list);
    }
}

// ---------------------------------------------------------------------------
// Command FFI surface (macOS window-commands slice).
//
// Constants reused from `commands.rs`'s `compute_geometry` unit tests so the
// expected rects below are pinned to the same arithmetic the launcher relies
// on. `WA` has non-zero `x`/`y` to catch relative-vs-absolute bugs; the
// dimensions divide cleanly for the half / two-thirds cases. `FRAME` is the
// target window's current frame at gather time — only `center` reads it, so
// its size (800x600) must survive into the computed `center` rect.
// ---------------------------------------------------------------------------

/// Work area `(x, y, w, h)` shared by the command tests.
const WA: (i32, i32, i32, i32) = (100, 50, 1800, 1000);

/// Current frame `(x, y, w, h)` shared by the command tests. Only `center`
/// reads it (recenter without resize), so its 800x600 size must reappear in
/// the `center` geometry.
const FRAME: (i32, i32, i32, i32) = (200, 60, 800, 600);

/// Sentinel pre-filled into the four geometry out-params before a call so the
/// "untouched on false" contract can be asserted: if the FFI returns false it
/// must leave every out-param at this value.
const GEOMETRY_SENTINEL: i32 = -12345;

/// Push a Command of `kind_id` carrying `target_window_id`, a work area, and a
/// current frame (both `(x, y, w, h)` tuples). Returns the boolean from the
/// FFI call so each test can assert on it. Mirrors `push_app` /
/// `push_window_named` for terseness.
fn push_command_kind(
    list: *mut EntryList,
    kind_id: &str,
    target_window_id: u64,
    wa: (i32, i32, i32, i32),
    frame: (i32, i32, i32, i32),
) -> bool {
    let kind_c = CString::new(kind_id).expect("kind_id must be valid for CString");
    // SAFETY: `kind_c` is owned by this function and lives across the FFI
    // call; `lofi_entries_push_command` copies what it needs (the kind id is
    // parsed into a `CommandKind`, never retained) before returning.
    unsafe {
        lofi_entries_push_command(
            list,
            kind_c.as_ptr(),
            target_window_id,
            wa.0,
            wa.1,
            wa.2,
            wa.3,
            frame.0,
            frame.1,
            frame.2,
            frame.3,
        )
    }
}

/// Read the command id at `idx` and return it as an owned `String`. Panics if
/// the FFI returns null or non-UTF-8 — tests that want to assert on null call
/// `lofi_entries_get_command_id` directly. Mirrors `name_at`.
fn command_id_at(list: *const EntryList, idx: usize) -> String {
    // SAFETY: caller is responsible for `idx` being in bounds; the returned
    // pointer is a process-lifetime `&'static CStr` (per the FFI contract) so
    // it is never invalidated by a later mutation, but we copy it out anyway
    // for uniformity with the other accessor helpers.
    unsafe {
        let p = lofi_entries_get_command_id(list, idx);
        assert!(
            !p.is_null(),
            "lofi_entries_get_command_id returned null for in-bounds Command idx={idx}"
        );
        CStr::from_ptr(p)
            .to_str()
            .expect("command id bytes should be UTF-8")
            .to_owned()
    }
}

/// Read the computed geometry at `idx`. Pre-fills the four out-params with
/// `GEOMETRY_SENTINEL`, calls the FFI, and returns `Some((x, y, w, h))` on
/// true / `None` on false. Tests asserting the "untouched on false" contract
/// drive the raw FFI directly so they can inspect the sentinel after the call.
fn command_geometry_at(list: *const EntryList, idx: usize) -> Option<(i32, i32, i32, i32)> {
    let mut x = GEOMETRY_SENTINEL;
    let mut y = GEOMETRY_SENTINEL;
    let mut w = GEOMETRY_SENTINEL;
    let mut h = GEOMETRY_SENTINEL;
    // SAFETY: the four out-pointers reference live stack locals that outlive
    // the call; the FFI either writes all four (true) or none (false).
    let ok = unsafe {
        lofi_entries_get_command_geometry(list, idx, &mut x, &mut y, &mut w, &mut h)
    };
    if ok {
        Some((x, y, w, h))
    } else {
        None
    }
}

#[test]
fn push_command_round_trips_geometry_kind() {
    // A geometry-kind Command must round-trip its polymorphic accessors:
    // name == CommandKind::display_name ("Center half"), category == "Command",
    // command_id == CommandKind::as_id ("center_half"), and geometry ==
    // compute_geometry's rect for CenterHalf over WA. The shared accessors that
    // are meaningless for a Command must report their sentinels: get_icon ==
    // null, get_window_id == 0, get_bundle_id == null.
    const EXPECTED_GEOMETRY: (i32, i32, i32, i32) = (550, 50, 900, 1000);
    const TARGET_WINDOW_ID: u64 = 4242;

    // SAFETY: standard FFI lifecycle: new -> push -> read -> free.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            push_command_kind(list, "center_half", TARGET_WINDOW_ID, WA, FRAME),
            "push_command for a known geometry kind should return true"
        );

        assert_eq!(
            lofi_entries_len(list),
            1,
            "len should be 1 after one successful push_command"
        );
        assert_eq!(
            name_at(list, 0),
            "Center half",
            "Command's get_name should return the kind's display_name"
        );
        assert_eq!(
            category_at(list, 0),
            "Command",
            "Command entries should report category \"Command\""
        );
        assert_eq!(
            command_id_at(list, 0),
            "center_half",
            "get_command_id should return CommandKind::as_id"
        );
        assert_eq!(
            command_geometry_at(list, 0),
            Some(EXPECTED_GEOMETRY),
            "get_command_geometry should return compute_geometry's CenterHalf rect"
        );

        // Polymorphic accessors that don't apply to a Command must report
        // their documented sentinels rather than crash or leak Window/App data.
        assert!(
            lofi_entries_get_icon(list, 0).is_null(),
            "get_icon must be null for a Command (Command is in the None icon arm)"
        );
        assert_eq!(
            lofi_entries_get_window_id(list, 0),
            0,
            "get_window_id must be 0 for a Command (not a Window variant)"
        );
        assert!(
            lofi_entries_get_bundle_id(list, 0).is_null(),
            "get_bundle_id must be null for a Command (not an Application variant)"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_command_center_uses_current_frame() {
    // Center is the only kind that reads `current_frame`: it keeps the
    // window's current size and recenters it within the work area. With
    // WA={100,50,1800,1000} and FRAME size 800x600, x = 100 + (1800-800)/2 =
    // 600, y = 50 + (1000-600)/2 = 250. Proves the frame is plumbed through
    // push_command into compute_geometry (a wrong/zero frame would change the
    // size and origin).
    const EXPECTED_GEOMETRY: (i32, i32, i32, i32) = (600, 250, 800, 600);
    const TARGET_WINDOW_ID: u64 = 1;

    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            push_command_kind(list, "center", TARGET_WINDOW_ID, WA, FRAME),
            "push_command(center) should return true"
        );
        assert_eq!(
            command_geometry_at(list, 0),
            Some(EXPECTED_GEOMETRY),
            "center geometry must reflect the pushed current_frame size, recentered"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn command_geometry_false_for_state_toggle_kinds() {
    // The three state-toggle kinds (minimize, toggle_maximize,
    // toggle_fullscreen) produce no rectangle: compute_geometry returns None,
    // so get_command_geometry returns false. They must still surface
    // command_id and name so Swift can dispatch them by id.
    const TARGET_WINDOW_ID: u64 = 5;
    const STATE_KINDS: [(&str, &str); 3] = [
        ("minimize", "Minimize"),
        ("toggle_maximize", "Toggle maximize"),
        ("toggle_fullscreen", "Toggle fullscreen"),
    ];

    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();

        for (id, _display) in STATE_KINDS.iter() {
            assert!(
                push_command_kind(list, id, TARGET_WINDOW_ID, WA, FRAME),
                "push_command({id}) should return true"
            );
        }
        assert_eq!(
            lofi_entries_len(list),
            STATE_KINDS.len(),
            "all three state-toggle kinds should be pushed"
        );

        for (idx, (id, display)) in STATE_KINDS.iter().enumerate() {
            assert_eq!(
                command_geometry_at(list, idx),
                None,
                "state-toggle kind {id} at idx {idx} must have no geometry"
            );
            assert_eq!(
                command_id_at(list, idx),
                *id,
                "state-toggle kind at idx {idx} should still surface its command_id"
            );
            assert_eq!(
                name_at(list, idx),
                *display,
                "state-toggle kind at idx {idx} should still surface its display name"
            );
        }

        lofi_entries_free(list);
    }
}

#[test]
fn command_geometry_leaves_out_params_untouched_on_false() {
    // The explicit contract: get_command_geometry must leave ALL FOUR
    // out-params untouched on every false path. We pre-fill the outs with a
    // sentinel and drive the raw FFI directly (not the helper) so we can
    // inspect the sentinel after each call. Four false paths are exercised:
    //   (a) state-toggle kind (minimize) at a valid Command idx
    //   (b) non-Command variant (an Application)
    //   (c) out-of-bounds idx
    //   (d) null list pointer
    const TARGET_WINDOW_ID: u64 = 9;
    const FAR_OUT_OF_BOUNDS: usize = 999;

    // SAFETY: standard FFI lifecycle plus a deliberate null-list call. The
    // four out-pointers reference live stack locals across every call.
    unsafe {
        let list = lofi_entries_new();
        // idx 0: a state-toggle Command (no geometry).
        assert!(
            push_command_kind(list, "minimize", TARGET_WINDOW_ID, WA, FRAME),
            "push_command(minimize) should return true"
        );
        // idx 1: a non-Command variant.
        assert!(
            push_app(list, "Calculator", "com.apple.Calculator", None),
            "push_app should succeed for the non-Command false-path case"
        );

        // Each sub-case re-arms the sentinel, calls the raw FFI, asserts
        // false, then asserts every out-param is still the sentinel.
        let check_untouched = |list: *const EntryList, idx: usize, case: &str| {
            let mut x = GEOMETRY_SENTINEL;
            let mut y = GEOMETRY_SENTINEL;
            let mut w = GEOMETRY_SENTINEL;
            let mut h = GEOMETRY_SENTINEL;
            // SAFETY: out-pointers reference live stack locals; this is the
            // false path under test, so no write should occur.
            let ok = lofi_entries_get_command_geometry(
                list, idx, &mut x, &mut y, &mut w, &mut h,
            );
            assert!(!ok, "get_command_geometry should return false for {case}");
            assert_eq!(x, GEOMETRY_SENTINEL, "out_x must be untouched for {case}");
            assert_eq!(y, GEOMETRY_SENTINEL, "out_y must be untouched for {case}");
            assert_eq!(w, GEOMETRY_SENTINEL, "out_w must be untouched for {case}");
            assert_eq!(h, GEOMETRY_SENTINEL, "out_h must be untouched for {case}");
        };

        check_untouched(list, 0, "state-toggle kind (minimize)");
        check_untouched(list, 1, "non-Command variant (Application)");
        check_untouched(list, FAR_OUT_OF_BOUNDS, "out-of-bounds idx");
        check_untouched(ptr::null(), 0, "null list pointer");

        lofi_entries_free(list);
    }
}

#[test]
fn command_geometry_false_for_non_command() {
    // An Application entry has no command geometry: get_command_geometry must
    // return None (false). Companion to the bundled false-path test above but
    // expressed through the helper for the common case.
    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Calculator", "com.apple.Calculator", None));

        assert_eq!(
            command_geometry_at(list, 0),
            None,
            "get_command_geometry must be None for a non-Command (Application)"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_command_unknown_kind_id_rejected() {
    // An unrecognized kind_id (not a CommandKind::as_id) must be rejected:
    // push_command returns false and the list stays empty (no garbage Command
    // entry is inserted).
    const TARGET_WINDOW_ID: u64 = 3;

    // SAFETY: standard FFI lifecycle with one deliberately unknown kind id.
    unsafe {
        let list = lofi_entries_new();

        assert!(
            !push_command_kind(list, "not_a_command", TARGET_WINDOW_ID, WA, FRAME),
            "push_command with an unknown kind_id must return false"
        );
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must stay 0 when push_command rejects an unknown kind_id"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn push_command_null_kind_id_returns_false() {
    // Null kind_id is the required-args contract: push_command returns false
    // and does not mutate the list. A null list pointer must also short-circuit
    // to false without crashing.
    const TARGET_WINDOW_ID: u64 = 8;

    // SAFETY: deliberately pass a null kind_id, then a null list; FFI must
    // short-circuit to false without crashing.
    unsafe {
        let list = lofi_entries_new();

        let ok = lofi_entries_push_command(
            list,
            ptr::null(),
            TARGET_WINDOW_ID,
            WA.0,
            WA.1,
            WA.2,
            WA.3,
            FRAME.0,
            FRAME.1,
            FRAME.2,
            FRAME.3,
        );
        assert!(!ok, "push_command with a null kind_id must return false");
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push_command fails on a null kind_id"
        );

        // Null list with an otherwise-valid kind id must also be false.
        let kind = CString::new("center").expect("kind id valid");
        let ok_null_list = lofi_entries_push_command(
            ptr::null_mut(),
            kind.as_ptr(),
            TARGET_WINDOW_ID,
            WA.0,
            WA.1,
            WA.2,
            WA.3,
            FRAME.0,
            FRAME.1,
            FRAME.2,
            FRAME.3,
        );
        assert!(!ok_null_list, "push_command with a null list must return false");

        lofi_entries_free(list);
    }
}

#[test]
fn push_command_invalid_utf8_kind_id_returns_false() {
    // 0xFF is never a valid UTF-8 byte; push_command must reject a non-UTF-8
    // kind_id strictly (the to_str() check) rather than crashing or coercing.
    // Bytes are [0xFF, 0x00] (one invalid byte plus the NUL terminator).
    const TARGET_WINDOW_ID: u64 = 6;

    // SAFETY: deliberately pass a non-UTF-8 kind_id pointer; FFI must reject
    // without crashing.
    unsafe {
        let list = lofi_entries_new();

        let bad_kind: [u8; 2] = [0xFF, 0x00];
        let ok = lofi_entries_push_command(
            list,
            bad_kind.as_ptr().cast::<c_char>(),
            TARGET_WINDOW_ID,
            WA.0,
            WA.1,
            WA.2,
            WA.3,
            FRAME.0,
            FRAME.1,
            FRAME.2,
            FRAME.3,
        );
        assert!(
            !ok,
            "push_command with an invalid-UTF-8 kind_id must return false"
        );
        assert_eq!(
            lofi_entries_len(list),
            0,
            "len must not change when push_command fails on invalid UTF-8 kind_id"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn get_command_id_null_for_non_command() {
    // get_command_id returns null for anything that isn't a Command at the
    // given idx. Sub-cases: an Application (idx 0), a Window (idx 1), an
    // out-of-bounds idx, and a null list. The mirror of get_window_id's
    // 0-sentinel contract, but the sentinel here is a null pointer.
    const WINDOW_ID: u64 = 21;
    const FAR_OUT_OF_BOUNDS: usize = 999;

    // SAFETY: standard FFI lifecycle plus a deliberate null-list call.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Calculator", "com.apple.Calculator", None));
        assert!(
            push_window_named(list, WINDOW_ID, "Some Doc", Some("TextEdit"), None, None),
            "push_window should succeed for the get_command_id null test"
        );

        assert!(
            lofi_entries_get_command_id(list, 0).is_null(),
            "get_command_id must be null for an Application variant"
        );
        assert!(
            lofi_entries_get_command_id(list, 1).is_null(),
            "get_command_id must be null for a Window variant"
        );
        assert!(
            lofi_entries_get_command_id(list, FAR_OUT_OF_BOUNDS).is_null(),
            "get_command_id must be null for an out-of-bounds idx"
        );
        assert!(
            lofi_entries_get_command_id(ptr::null(), 0).is_null(),
            "get_command_id must be null for a null list"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn mixed_list_filtered_command_geometry_and_id() {
    // The filtered-index resolver must route through to a Command on both
    // get_command_id and get_command_geometry. Push two apps whose names do
    // NOT contain an l-e-f-t subsequence ("Chrome", "Notes") plus one
    // left_half command (display name "Left half" — contains "left"). Narrow
    // with set_query("left"): only the command survives, and its command_id
    // and geometry must still resolve through the filtered idx 0.
    const EXPECTED_GEOMETRY: (i32, i32, i32, i32) = (100, 50, 900, 1000);
    const TARGET_WINDOW_ID: u64 = 77;

    // SAFETY: standard FFI lifecycle with one set_query mutation; geometry/id
    // are read only after the mutation, so no stale-borrow concern.
    unsafe {
        let list = lofi_entries_new();
        assert!(push_app(list, "Chrome", "com.google.Chrome", None));
        assert!(push_app(list, "Notes", "com.apple.Notes", None));
        assert!(
            push_command_kind(list, "left_half", TARGET_WINDOW_ID, WA, FRAME),
            "push_command(left_half) should return true"
        );

        assert!(set_query(list, "left"), "set_query should succeed");

        assert_eq!(
            lofi_entries_len(list),
            1,
            "only the \"Left half\" command should match query \"left\""
        );
        assert_eq!(
            category_at(list, 0),
            "Command",
            "the surviving filtered entry should be the Command"
        );
        assert_eq!(
            command_id_at(list, 0),
            "left_half",
            "get_command_id must resolve through the filtered idx to the Command"
        );
        assert_eq!(
            command_geometry_at(list, 0),
            Some(EXPECTED_GEOMETRY),
            "get_command_geometry must resolve through the filtered idx to the Command"
        );

        lofi_entries_free(list);
    }
}

#[test]
fn command_id_matches_as_id_for_all_kinds() {
    // Push all nine command kinds in display order and assert each
    // command_id equals its expected snake_case id. This guards the FFI's
    // command_id_cstr map against drift from CommandKind::as_id: if a new kind
    // is added or an id string changes on one side only, this test fails.
    const TARGET_WINDOW_ID: u64 = 100;
    const ALL_KIND_IDS: [&str; 9] = [
        "center",
        "center_half",
        "center_two_thirds",
        "left_half",
        "right_half",
        "standard_size",
        "minimize",
        "toggle_maximize",
        "toggle_fullscreen",
    ];

    // SAFETY: standard FFI lifecycle.
    unsafe {
        let list = lofi_entries_new();

        for id in ALL_KIND_IDS.iter() {
            assert!(
                push_command_kind(list, id, TARGET_WINDOW_ID, WA, FRAME),
                "push_command({id}) should return true"
            );
        }
        assert_eq!(
            lofi_entries_len(list),
            ALL_KIND_IDS.len(),
            "all nine command kinds should be pushed"
        );

        for (idx, expected_id) in ALL_KIND_IDS.iter().enumerate() {
            assert_eq!(
                command_id_at(list, idx),
                *expected_id,
                "command_id at idx {idx} must equal its CommandKind::as_id"
            );
        }

        lofi_entries_free(list);
    }
}
