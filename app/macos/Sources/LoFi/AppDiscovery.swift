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

        // Three roots, in the order the user would expect them to win
        // collisions: Apple's stock apps first (`/System/Applications`,
        // where modern macOS puts Calculator, Safari, etc.), then the
        // third-party install root (`/Applications`), then per-user
        // (`~/Applications`). Matches the GNOME first-dir-wins policy
        // in `app/gnome/src/apps.rs`.
        let roots: [URL] = [
            URL(fileURLWithPath: "/System/Applications"),
            URL(fileURLWithPath: "/Applications"),
            fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications"),
        ]

        var seen: Set<String> = []
        var out: [DiscoveredApp] = []

        for root in roots {
            guard let enumerator = fm.enumerator(
                at: root,
                includingPropertiesForKeys: nil,
                options: [.skipsPackageDescendants, .skipsHiddenFiles]
            ) else { continue }

            for case let url as URL in enumerator where url.pathExtension == "app" {
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
                let name = nonEmpty(bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
                    ?? nonEmpty(bundle.object(forInfoDictionaryKey: "CFBundleName") as? String)
                    ?? url.deletingPathExtension().lastPathComponent

                seen.insert(bundleId)
                out.append(DiscoveredApp(
                    name: name,
                    bundleId: bundleId,
                    bundlePath: url.path
                ))
            }
        }

        out.sort { $0.name.lowercased() < $1.name.lowercased() }
        return out
    }
}
