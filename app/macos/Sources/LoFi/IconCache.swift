// Process-lifetime, in-memory cache of resolved application icons.
//
// Why this exists: every list row resolves its icon through
// `NSWorkspace.shared.icon(forFile:)`, which does synchronous filesystem +
// IconServices work. A fresh `EntryRowView` is built for every visible row on
// every summon (there is no `NSTableView` view reuse — see
// `AppListController`), so a machine under heavy load stalls on those per-row
// lookups and paints blank icons for a few seconds until they resolve.
// Holding the resolved `NSImage` keyed by the app means the first summon pays
// the cost once and every later summon is a dictionary hit.
//
// Key = bundle POSIX path + icon/widget style + mtime + ctime:
//   - path        — which app's icon.
//   - style       — the system "Icon & widget style" (Default / Dark / Clear /
//                   Tinted, plus tint color). macOS renders bundle icons in
//                   that style, so folding it into the key means a live style
//                   change re-resolves instead of serving a stale-styled
//                   bitmap. Read from the `AppleIconAppearanceTheme` /
//                   `AppleIconAppearanceTintColor` NSGlobalDomain keys.
//   - mtime+ctime — the bundle directory's modification and inode-change
//                   times. An in-place app update (reinstall, App Store /
//                   Homebrew upgrade) replaces the `.app` and bumps these, so
//                   the key changes and the fresh icon is picked up. A single
//                   `stat(2)` on the bundle reads both — it does NOT walk the
//                   bundle contents.
//
// No eviction, no TTL, cleared only by process exit (per spec): the working
// set is bounded by the number of installed apps a launcher can list — a few
// hundred small `NSImage`s — so keeping every icon for the daemon's lifetime
// is cheap and eliminates all icon flicker after the first paint of each
// (app, style) pair.
//
// Threading: only touched from the main thread (row construction runs on the
// main run loop), so the backing dictionary needs no lock.

import AppKit

final class IconCache {
    static let shared = IconCache()

    /// Resolved icons keyed by `cacheKey(path:)`. Grows monotonically for the
    /// process lifetime — intentional; see the no-eviction note above.
    private var images: [String: NSImage] = [:]

    private init() {}

    /// Icon for the app at `path`, resolved through
    /// `NSWorkspace.shared.icon(forFile:)` on the first request for a given
    /// (path, style, mtime, ctime) tuple and served from memory thereafter.
    func icon(forFile path: String) -> NSImage {
        let key = Self.cacheKey(path: path)
        if let cached = images[key] {
            return cached
        }
        let image = NSWorkspace.shared.icon(forFile: path)
        images[key] = image
        return image
    }

    /// Build the cache key. Fields are joined by a control character
    /// (`\u{1}`, SOH) that can't occur in a POSIX path or in any of the
    /// style/time components, so distinct field values can never collide into
    /// the same key.
    private static func cacheKey(path: String) -> String {
        let sep = "\u{1}"
        return path + sep + styleTag() + sep + statTag(path: path)
    }

    /// The current system "Icon & widget style," read from the two
    /// NSGlobalDomain keys macOS writes when the user changes it in
    /// System Settings → Appearance. `UserDefaults.standard` reflects live
    /// changes to that domain in a running process (the same mechanism that
    /// lets apps track `AppleInterfaceStyle` for Dark Mode), so re-reading it
    /// per key-build is enough to notice a style switch without restarting
    /// LoFi. Defaults are chosen so a machine that has never set the keys
    /// (Default style, no tint) still produces a stable, non-empty tag.
    private static func styleTag() -> String {
        let defaults = UserDefaults.standard
        let theme = defaults.string(forKey: "AppleIconAppearanceTheme") ?? "Regular"
        let tint = defaults.string(forKey: "AppleIconAppearanceTintColor") ?? "None"
        return theme + ":" + tint
    }

    /// The bundle's modification + inode-change times as a compact string.
    /// One `stat(2)` on the bundle directory reads both — cheap relative to
    /// the icon resolution it guards, and it does NOT walk the bundle. On
    /// `stat` failure (app removed mid-session, unreadable) returns a fixed
    /// marker so the entry still caches per (path, style) rather than
    /// re-resolving on every row; a later successful stat yields a different
    /// tag and supersedes it.
    private static func statTag(path: String) -> String {
        var info = stat()
        guard stat(path, &info) == 0 else { return "nostat" }
        let m = info.st_mtimespec
        let c = info.st_ctimespec
        return "\(m.tv_sec).\(m.tv_nsec)-\(c.tv_sec).\(c.tv_nsec)"
    }
}
