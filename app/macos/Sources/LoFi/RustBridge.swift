// Swift wrapper around the `lofi-core` C ABI.
//
// `EntryList` here mirrors the opaque handle the Rust side hands out
// via `lofi_entries_new`. The wrapper owns the handle through its
// lifetime: `deinit` frees it, and the borrow-on-read contract from the
// C API is honored by copying every name into a Swift `String` before
// returning.
//
// The C functions are pulled in via the bridging header
// (`LoFi-Bridging-Header.h`); cbindgen emits the prototypes from
// `app/core/src/ffi/entries.rs`. The opaque `EntryList` struct lands as
// `OpaquePointer` in Swift.

import Foundation

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
        lofi_entries_free(UnsafeMutablePointer(handle))
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
                        UnsafeMutablePointer(self.handle),
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
        Int(lofi_entries_len(UnsafePointer(handle)))
    }

    /// Read the display name at `idx`. Returns `nil` if the index is
    /// out of bounds (matches the C contract). The borrowed `*const
    /// c_char` from Rust is copied into a Swift `String` immediately so
    /// callers never see a pointer that could be invalidated by a
    /// later `pushApplication`.
    func name(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_name(UnsafePointer(handle), idx) else {
            return nil
        }
        return String(cString: cstr)
    }
}
