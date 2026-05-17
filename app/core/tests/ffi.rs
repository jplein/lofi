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
