// Swift wrapper around the `lofi-core` C ABI.
//
// `EntryList` here mirrors the opaque handle the Rust side hands out
// via `lofi_entries_new`. The wrapper owns the handle through its
// lifetime: `deinit` frees it, and the borrow-on-read contract from the
// C API is honored by copying every name into a Swift `String` before
// returning.
//
// The C functions are pulled in via the `LoFiCore` Clang module
// declared in `app/core/include/module.modulemap`; cbindgen emits the
// prototypes from `app/core/src/ffi/entries.rs`. The opaque `EntryList`
// struct lands as `OpaquePointer` in Swift.

import Foundation
import LoFiCore

final class EntryList {
    /// Underlying C handle. `lofi_entries_new` never returns null in
    /// practice (the only failure mode is OOM, which aborts), but we
    /// still treat the pointer as optional to keep the bridge defensive
    /// — a future Rust-side change that returns null must not crash
    /// this constructor.
    private let handle: OpaquePointer

    init() {
        guard let p = lofi_entries_new() else {
            preconditionFailure("lofi_entries_new returned null; out of memory?")
        }
        self.handle = p
    }

    deinit {
        // Mirrors the Rust contract: `free(null)` is a no-op, but we
        // know the handle is non-null because `init` asserted it.
        lofi_entries_free(handle)
    }

    /// Push an application onto the list. Returns `false` on null
    /// args (impossible from Swift `String`) or invalid-UTF-8 — neither
    /// is reachable in practice, but the return value matches the C
    /// signature for future-proofing.
    @discardableResult
    func pushApplication(name: String, bundleId: String, icon: String?) -> Bool {
        return name.withCString { namePtr in
            bundleId.withCString { bundlePtr in
                let withIcon: (UnsafePointer<CChar>?) -> Bool = { iconPtr in
                    lofi_entries_push_application(
                        self.handle,
                        namePtr,
                        bundlePtr,
                        iconPtr
                    )
                }
                if let icon = icon {
                    return icon.withCString { iconPtr in withIcon(iconPtr) }
                } else {
                    return withIcon(nil)
                }
            }
        }
    }

    var count: Int {
        Int(lofi_entries_len(handle))
    }

    /// Read the display name at `idx`. Returns `nil` if the index is
    /// out of bounds (matches the C contract). The borrowed `*const
    /// c_char` from Rust is copied into a Swift `String` immediately so
    /// callers never see a pointer that could be invalidated by a
    /// later `pushApplication`.
    func name(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_name(handle, UInt(idx)) else {
            return nil
        }
        return String(cString: cstr)
    }

    /// Set the active fuzzy-search query. An empty string clears the
    /// filter (`count` goes back to the unfiltered total). Matches the
    /// Rust contract: whitespace-tokenized, case-insensitive,
    /// intersection semantics; same predicate the GNOME side uses.
    ///
    /// Discardable — the only failure mode the Rust side reports is
    /// "list pointer was null" (impossible here, we own it) or "invalid
    /// UTF-8" (impossible from a Swift `String`).
    @discardableResult
    func setQuery(_ query: String) -> Bool {
        return query.withCString { qPtr in
            lofi_entries_set_query(self.handle, qPtr)
        }
    }

    /// Read the bundle id at `idx`. Returns `nil` when the index is out
    /// of bounds or the entry has no bundle id (only Application entries
    /// carry one today). The borrowed pointer is copied into a Swift
    /// `String` immediately so the same invalidation rules as
    /// `name(at:)` apply.
    func bundleId(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_bundle_id(handle, UInt(idx)) else {
            return nil
        }
        return String(cString: cstr)
    }

    /// Read the stable English category label at `idx` (one of
    /// `"Application"`, `"Window"`, `"Workspace"`, `"Command"`,
    /// `"PowerCommand"`). Returns `nil` when the index is out of bounds.
    /// Same copy-into-`String` borrow rules as `name(at:)`.
    func category(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_category(handle, UInt(idx)) else {
            return nil
        }
        return String(cString: cstr)
    }

    /// Read the icon identifier at `idx` — for Application entries this
    /// is whatever was passed as `icon` on push (macOS pushes the `.app`
    /// bundle path). Returns `nil` when the index is out of bounds or
    /// the entry was pushed without an icon. Same copy-into-`String`
    /// borrow rules as `name(at:)`.
    func icon(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_icon(handle, UInt(idx)) else {
            return nil
        }
        return String(cString: cstr)
    }
}
