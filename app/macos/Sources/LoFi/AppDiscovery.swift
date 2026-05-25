// Enumerates installed `.app` bundles under the three applications
// roots a macOS user expects to find launchable apps in. Mirrors the
// GNOME side's "platform discovers, Rust holds" data-flow: the Swift
// layer produces a sorted, deduped list and pushes it into the Rust
// core (see `AppDelegate`).
//
// Out of scope this slice (each is a follow-up):
//   - Async / progressive enumeration.

import AppKit
import Foundation

/// A `.app` bundle the user can launch. Four fields:
///   - `name`     — user-visible display name
///   - `bundleId` — stable identifier (the `CFBundleIdentifier`)
///   - `bundlePath` — the absolute filesystem path to the `.app` bundle.
///     Passed through to the Rust side as the `icon` argument so the Swift
///     UI layer can later resolve it via `NSWorkspace.shared.icon(forFile:)`.
///     The Rust core treats this string as an opaque "icon identifier"; only
///     the Swift UI is allowed to interpret it as a path.
struct DiscoveredApp {
    let name: String
    let bundleId: String
    let bundlePath: String
}

enum AppDiscovery {
    /// Walk `/System/Applications`, `/Applications`, and `~/Applications`
    /// for `.app` bundles and return them deduped by bundle id
    /// (first-wins) and sorted by lowercased name. Synchronous: the
    /// panel is shown after this returns, so the first paint is fully
    /// populated.
    static func discover() -> [DiscoveredApp] {
        let fm = FileManager.default

        // Four roots, in the order the user would expect them to win
        // collisions:
        //
        //   - `/System/Applications` — Apple's stock apps that live on
        //     the system volume (Calculator, Mail, Music, …).
        //   - `/Applications` — third-party installs. Also nominally
        //     contains a `Safari.app` symlink into the Cryptex below,
        //     but bundled (TCC-filtered) apps don't see that symlink
        //     in the directory listing — see the next root.
        //   - `/System/Cryptexes/App/System/Applications` — where
        //     modern macOS actually keeps Safari, so Apple can ship
        //     Rapid Security Response updates to Safari/WebKit
        //     independently of the OS. Reading it directly bypasses
        //     the TCC filter that hides the `/Applications/Safari.app`
        //     symlink from non-privileged apps.
        //   - `~/Applications` — per-user installs.
        //
        // Mirrors the GNOME first-dir-wins policy in
        // `app/gnome/src/apps.rs`.
        let roots: [URL] = [
            URL(fileURLWithPath: "/System/Applications"),
            URL(fileURLWithPath: "/Applications"),
            URL(fileURLWithPath: "/System/Cryptexes/App/System/Applications"),
            fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications"),
        ]

        var seen: Set<String> = []
        var out: [DiscoveredApp] = []

        for root in roots {
            for url in collectAppBundles(under: root) {
                guard let bundle = Bundle(url: url) else { continue }
                guard let bundleId = bundle.bundleIdentifier else { continue }
                if seen.contains(bundleId) { continue }

                // `??` only catches nil, not empty strings. Some apps set
                // `CFBundleDisplayName` to "" rather than omitting the key,
                // which would otherwise leak an empty row to the UI.
                func nonEmpty(_ s: String?) -> String? {
                    guard let s = s, !s.isEmpty else { return nil }
                    return s
                }
                let name =
                    nonEmpty(bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
                    ?? nonEmpty(bundle.object(forInfoDictionaryKey: "CFBundleName") as? String)
                    ?? url.deletingPathExtension().lastPathComponent

                seen.insert(bundleId)
                out.append(
                    DiscoveredApp(
                        name: name,
                        bundleId: bundleId,
                        bundlePath: url.path
                    ))
            }
        }

        out.sort { $0.name.lowercased() < $1.name.lowercased() }
        return out
    }

    /// Manual recursive walk that returns every `.app` bundle URL under
    /// `root`. We *cannot* use `FileManager.enumerator(at:options:)`
    /// here: it silently skips symbolic links to directories, which on
    /// modern macOS means `/Applications/Safari.app` (a symlink into
    /// `/System/Cryptexes/...`) never surfaces. `contentsOfDirectory`
    /// returns symlinks correctly, so we use it level by level.
    ///
    /// A `.app` extension stops the recursion — bundle internals are
    /// never of interest. Non-`.app` subdirectories (e.g.
    /// `Utilities/`) get walked one level deeper. The depth cap is a
    /// belt-and-suspenders guard against any future-or-foreign
    /// directory layout that might recurse pathologically; in practice
    /// Applications trees are 1–2 levels deep.
    private static func collectAppBundles(under root: URL, depth: Int = 0) -> [URL] {
        let maxDepth = 5
        if depth > maxDepth { return [] }
        let children =
            (try? FileManager.default.contentsOfDirectory(
                at: root,
                includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]
            )) ?? []
        var out: [URL] = []
        for child in children {
            if child.pathExtension == "app" {
                out.append(child)
            } else {
                // Best-effort recurse. `contentsOfDirectory` returns
                // an error for non-directories; we treat that as "no
                // children" and move on.
                out.append(contentsOf: collectAppBundles(under: child, depth: depth + 1))
            }
        }
        return out
    }
}
