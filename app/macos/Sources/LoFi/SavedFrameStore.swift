// Persistent per-window "frame before maximize" store for the
// toggle-maximize command.
//
// WHY persistence is needed
// -------------------------
// `WindowControl.toggleMaximize` is a TRUE toggle: maximizing fills the
// work area, and un-maximizing restores the window's *exact previous
// frame*. GNOME gets that previous frame from Mutter (which remembers it
// across the maximize). On macOS there is no such app-independent saved
// frame, and — critically — LoFi is a short-lived process: it quits
// immediately after each activation. So the maximize press (run 1) and
// the un-maximize press (run 2) happen in two different process
// lifetimes; in-memory state can't bridge them. We persist the frame to
// disk so run 2 can read what run 1 saved.
//
// WHY UserDefaults
// ----------------
// UserDefaults is the idiomatic small-key-value store for a bundled macOS
// app — no file I/O, serialization, or path management to get wrong, and
// it works fine for an `LSUIElement` background app. The whole store is a
// single dictionary under one key.
//
// WHY keyed by CGWindowID
// -----------------------
// A CGWindowID is stable for a window's lifetime, so it's the natural key
// to associate a saved frame with the window it belongs to. The
// staleness caveat: macOS *reuses* CGWindowIDs after a window closes, so
// in principle a saved frame could outlive its window and later restore a
// *different* window (that happened to get the same id) to the wrong
// size. Three things keep that window of risk tiny:
//   1. save->consume lifecycle — `take` removes the entry on un-maximize,
//      so a saved frame normally exists only between one maximize and the
//      next un-maximize.
//   2. startup `prune(liveWindowIds:)` — drops entries for ids not
//      currently on screen, bounding accumulation from windows closed
//      while still maximized.
//   3. the worst case is a single benign wrong-size restore — no crash,
//      no data loss. Acceptable.
//
// macOS-only by design
// --------------------
// This store encodes a macOS-specific concept (CGWindowID + the divergent
// toggle policy). It deliberately stays out of `lofi-core` (the
// platform-clean shared crate) and out of the SQLite MRU store, per
// `app/core/README.md`'s "keep the core platform-clean" rule.

import Foundation

final class SavedFrameStore {
    /// UserDefaults key under which the whole `[String: [Double]]`
    /// dictionary lives. One key, one read/write per operation.
    private static let defaultsKey = "savedFrames"
    /// A saved frame is stored as `[x, y, w, h]` — exactly four doubles.
    /// Entries with a different length are treated as malformed and
    /// ignored (same bad-row tolerance as the MRU store).
    private static let frameComponentCount = 4

    private let defaults: UserDefaults

    /// Injectable for testing; production uses `UserDefaults.standard`.
    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Save (overwriting) the pre-maximize frame for `windowId`. The frame
    /// is top-left global. Overwriting is intentional: we always track the
    /// size the window had right before the *most recent* maximize.
    func save(windowId: UInt64, frame: CGRect) {
        var dict = rawDict()
        dict[String(windowId)] = [
            Double(frame.origin.x),
            Double(frame.origin.y),
            Double(frame.size.width),
            Double(frame.size.height),
        ]
        defaults.set(dict, forKey: Self.defaultsKey)
    }

    /// Read-and-remove the saved frame for `windowId`. Returns the rect if
    /// a well-formed entry exists (and removes it), or nil if absent or
    /// malformed. The read-and-remove keeps the lifecycle tight:
    /// save-on-maximize -> consume-on-restore.
    func take(windowId: UInt64) -> CGRect? {
        var dict = rawDict()
        guard let components = dict[String(windowId)],
              components.count == Self.frameComponentCount
        else {
            // Absent or malformed: drop any malformed entry so it doesn't
            // linger, then report "no saved frame".
            if dict[String(windowId)] != nil {
                dict[String(windowId)] = nil
                defaults.set(dict, forKey: Self.defaultsKey)
            }
            return nil
        }
        dict[String(windowId)] = nil
        defaults.set(dict, forKey: Self.defaultsKey)
        return CGRect(
            x: components[0],
            y: components[1],
            width: components[2],
            height: components[3]
        )
    }

    /// Drop saved entries whose window id is not in `liveWindowIds`. Called
    /// once at startup with the current on-screen id set so frames for
    /// windows closed (while still maximized) don't accumulate forever and
    /// the CGWindowID-reuse risk window stays small.
    func prune(liveWindowIds: Set<UInt64>) {
        let live = Set(liveWindowIds.map(String.init))
        var dict = rawDict()
        let before = dict.count
        dict = dict.filter { live.contains($0.key) }
        guard dict.count != before else { return }
        defaults.set(dict, forKey: Self.defaultsKey)
    }

    /// Read the backing dictionary, coercing a missing / wrong-typed value
    /// to an empty dict. Returning `[String: [Double]]` here means callers
    /// only have to guard the per-entry array length.
    private func rawDict() -> [String: [Double]] {
        defaults.dictionary(forKey: Self.defaultsKey) as? [String: [Double]] ?? [:]
    }
}
