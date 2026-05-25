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

/// Swift wrapper around the `lofi-core` `MruStore` opaque handle.
///
/// The store is the persistent activation history backing MRU ordering:
/// `EntryList.applyMru(store:)` reorders the in-memory entries by recency,
/// and `EntryList.bumpMru(store:, at:)` records that the user activated a
/// specific row. The SQLite file is opened lazily at `init?(path:)` and
/// stays open for the lifetime of the wrapper; `deinit` closes it.
///
/// `init?(path:)` returns nil when `lofi_mru_open` returns null — that
/// covers invalid paths, permission errors, and SQLite failures. The
/// launcher is expected to proceed without MRU ordering on nil, rather
/// than refuse to launch.
final class MruStore {
    /// Underlying C handle. `fileprivate` so `EntryList`'s methods in
    /// this same file can hand the raw pointer through to the C
    /// functions without exposing the pointer to the rest of the app.
    fileprivate let handle: OpaquePointer

    /// Open (or create) the MRU store at `path`. Returns nil on failure
    /// (invalid path, permission denied, SQLite error) — the C side
    /// already logs the underlying cause via `eprintln!`.
    ///
    /// Reads slightly oddly: `withCString` returns whatever its closure
    /// returns, so the outer `guard let` validates the C-side return
    /// pointer (not the C string itself, which is always non-null when
    /// `path` is a Swift `String`). `lofi_mru_open` is imported as
    /// `OpaquePointer?` because cbindgen emits the return as `MruStore *`;
    /// the unwrap converts that to a non-optional handle stored on self.
    init?(path: String) {
        guard let p = path.withCString({ lofi_mru_open($0) }) else {
            return nil
        }
        self.handle = p
    }

    deinit {
        // Mirrors the Rust contract: `free(null)` is a no-op, but we
        // know the handle is non-null because `init?` returned non-nil.
        lofi_mru_free(handle)
    }

    /// Canonical macOS storage path for the MRU SQLite file:
    /// `~/Library/Application Support/dev.jplein.lofi/mru.sqlite`. The
    /// containing directory is created on demand so a fresh install
    /// can open the store without first running an installer step.
    /// The Rust side also creates missing parents, but doing it here
    /// keeps the WAL files (`-shm`, `-wal`) in a predictable place
    /// alongside the main DB.
    static func defaultPath() -> String {
        let fm = FileManager.default
        let appSupport = fm
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first
            ?? URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support")
        let dir = appSupport.appendingPathComponent("dev.jplein.lofi")
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("mru.sqlite").path
    }
}

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

    /// Reset the list to empty (no entries, no query, no filter, no
    /// MRU state). The daemon calls this on every global-hotkey
    /// summon before re-pushing apps + commands, so the command
    /// target reflects the frontmost-non-LoFi window *now*, not at
    /// process-start time.
    func clear() {
        lofi_entries_clear(handle)
    }

    /// Push an application onto the list. `isRunning` is the boolean
    /// projection of `Application::is_running` on the Rust side — pass
    /// `true` when the app has at least one open window at gather time.
    /// `AppDelegate.summonPanel` derives it from a one-pass scan of the
    /// window list and passes it through here; the matching read on
    /// `isRunning(at:)` drives the running-indicator dot in the row UI.
    /// Returns `false` on null args (impossible from Swift `String`) or
    /// invalid-UTF-8 — neither is reachable in practice, but the return
    /// value matches the C signature for future-proofing.
    @discardableResult
    func pushApplication(
        name: String,
        bundleId: String,
        icon: String?,
        isRunning: Bool
    ) -> Bool {
        return name.withCString { namePtr in
            bundleId.withCString { bundlePtr in
                let withIcon: (UnsafePointer<CChar>?) -> Bool = { iconPtr in
                    lofi_entries_push_application(
                        self.handle,
                        namePtr,
                        bundlePtr,
                        iconPtr,
                        isRunning
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

    /// Push a window onto the list. Returns `false` on null `title` /
    /// invalid-UTF-8 (neither reachable from a Swift `String` in
    /// practice). The three optional args (`appName`, `icon`,
    /// `appDesktopId`) collapse to `nil` C pointers when nil on the
    /// Swift side; the Rust core represents them as `Option<String>`.
    ///
    /// Same nested-`withCString` shape as `pushApplication`, just deeper
    /// because three of the args are optional. Each layer of nesting
    /// keeps the previous pointer alive across the next closure call so
    /// every pointer handed to `lofi_entries_push_window` is valid for
    /// the full duration of that single call.
    @discardableResult
    func pushWindow(
        id: UInt64,
        title: String,
        appName: String?,
        icon: String?,
        workspace: Int32,
        appDesktopId: String?
    ) -> Bool {
        return title.withCString { titlePtr in
            let withApp: (UnsafePointer<CChar>?) -> Bool = { appPtr in
                let withIcon: (UnsafePointer<CChar>?) -> Bool = { iconPtr in
                    let withBundle: (UnsafePointer<CChar>?) -> Bool = { bundlePtr in
                        lofi_entries_push_window(
                            self.handle,
                            id,
                            titlePtr,
                            appPtr,
                            iconPtr,
                            workspace,
                            bundlePtr
                        )
                    }
                    if let appDesktopId = appDesktopId {
                        return appDesktopId.withCString { withBundle($0) }
                    }
                    return withBundle(nil)
                }
                if let icon = icon {
                    return icon.withCString { withIcon($0) }
                }
                return withIcon(nil)
            }
            if let appName = appName {
                return appName.withCString { withApp($0) }
            }
            return withApp(nil)
        }
    }

    /// Push a window-action command onto the list. `kindId` is a
    /// `CommandKind::as_id` snake_case string (e.g. `"center_half"`);
    /// `targetWindowId` is the CGWindowID the command will act on. The
    /// work area (`waX/waY/waW/waH`) and current frame (`frameX/frameY/
    /// frameW/frameH`) are plain `Int32` in the caller's coordinate space
    /// (top-left global on macOS) — taken as scalars rather than CGRects
    /// to keep this bridge CoreGraphics-free; the caller rounds CGFloat
    /// components with `.rounded()` before converting.
    ///
    /// Returns `false` for a null list (impossible here, we own it), a
    /// null/invalid-UTF-8 kind id (impossible from a Swift `String`), or
    /// an UNKNOWN kind id (a real failure mode — Rust rejects ids that
    /// aren't a `CommandKind`, pushing nothing).
    @discardableResult
    func pushCommand(
        kindId: String,
        targetWindowId: UInt64,
        waX: Int32,
        waY: Int32,
        waW: Int32,
        waH: Int32,
        frameX: Int32,
        frameY: Int32,
        frameW: Int32,
        frameH: Int32
    ) -> Bool {
        return kindId.withCString { kindPtr in
            lofi_entries_push_command(
                self.handle,
                kindPtr,
                targetWindowId,
                waX,
                waY,
                waW,
                waH,
                frameX,
                frameY,
                frameW,
                frameH
            )
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

    /// Read the `CGWindowID` for the `Entry::Window` at the filtered
    /// `idx`. Returns `0` for non-Window entries, out-of-bounds indices,
    /// or any other error — the Rust side uses the `0` sentinel because
    /// real `CGWindowID`s on macOS are always strictly greater than 0
    /// for regular application windows. Callers should gate on
    /// `category(at:) == "Window"` before reading; this is a robustness
    /// fallback rather than the primary signal.
    func windowId(at idx: Int) -> UInt64 {
        lofi_entries_get_window_id(handle, UInt(idx))
    }

    /// `true` when the entry at the filtered `idx` is an Application
    /// whose `is_running` flag was set at push time — i.e. the app had
    /// at least one open window when `AppDelegate.summonPanel` ran.
    /// `false` for every other case: non-Application entries,
    /// not-running apps, out-of-bounds indices, or a null list (a
    /// degenerate case here because we own the handle). Drives the
    /// running-indicator dot in `EntryRowView`.
    func isRunning(at idx: Int) -> Bool {
        lofi_entries_get_is_running(handle, UInt(idx))
    }

    /// Read the command id (`CommandKind::as_id`, e.g. `"center_half"`)
    /// for the `Entry::Command` at the filtered `idx`. Returns `nil` for
    /// non-Command entries, out-of-bounds indices, or any other case
    /// (Rust returns null). Callers should gate on
    /// `category(at:) == "Command"` before reading.
    ///
    /// Unlike the other string accessors the Rust pointer is a
    /// process-lifetime `&'static CStr` and is never invalidated by a
    /// later mutation, but we copy it into a Swift `String` immediately
    /// anyway for uniformity with `name(at:)`.
    func commandId(at idx: Int) -> String? {
        guard let cstr = lofi_entries_get_command_id(handle, UInt(idx)) else {
            return nil
        }
        return String(cString: cstr)
    }

    /// Read the computed geometry for the command at the filtered `idx`,
    /// as `(x, y, w, h)` in the coordinate space the command was pushed in
    /// (top-left global on macOS). Returns the tuple only for *geometry*
    /// command kinds; returns `nil` for state-toggle kinds (minimize /
    /// toggle_maximize / toggle_fullscreen), non-Command entries,
    /// out-of-bounds indices, or a null list. A `nil` here means "dispatch
    /// by `commandId(at:)` instead" — the state-toggle commands have no
    /// rectangle.
    func commandGeometry(at idx: Int) -> (x: Int32, y: Int32, w: Int32, h: Int32)? {
        var x: Int32 = 0
        var y: Int32 = 0
        var w: Int32 = 0
        var h: Int32 = 0
        let ok = lofi_entries_get_command_geometry(
            handle,
            UInt(idx),
            &x,
            &y,
            &w,
            &h
        )
        guard ok else { return nil }
        return (x, y, w, h)
    }

    /// Reorder the underlying entries by recency (most-recently-used
    /// first) using the persistent state from `store`. Stable: entries
    /// with no MRU row keep their relative order at the bottom of the
    /// list. Like `setQuery(_:)`, this is a mutating call: every
    /// pointer previously returned through `name(at:)` / `bundleId(at:)`
    /// / `category(at:)` / `icon(at:)` is invalidated, and the active
    /// query (if any) is re-evaluated against the new order.
    ///
    /// Returns `false` only on store/read errors; on success returns
    /// `true`. Discardable — the launcher treats a failed apply as
    /// "degrade silently and show the input order".
    @discardableResult
    func applyMru(store: MruStore) -> Bool {
        lofi_entries_apply_mru(handle, store.handle)
    }

    /// Record an activation of the row at filtered index `idx`. The
    /// C side resolves the filtered index to the underlying entry,
    /// pulls its `EntryRef`, and UPSERTs it into the MRU store with
    /// the current timestamp. Subsequent `applyMru(store:)` calls (on
    /// the next launch, typically) will sort that entry to the top.
    ///
    /// Returns `false` when the index is out of bounds against the
    /// current filtered view or when the SQLite write fails;
    /// discardable in the launch path because the worst case is "we
    /// failed to record the activation" — the launch itself still
    /// proceeds.
    @discardableResult
    func bumpMru(store: MruStore, at idx: Int) -> Bool {
        lofi_mru_bump_entry(store.handle, handle, UInt(idx))
    }
}
