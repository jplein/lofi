// Enumerates installed `.app` bundles under the two user-visible
// applications roots. Mirrors the GNOME side's "platform discovers,
// Rust holds" data-flow: the Swift layer produces a sorted, deduped
// list and pushes it into the Rust core (see `AppDelegate`).
//
// Out of scope this slice (each is a follow-up):
//   - `/System/Applications` (system-provided apps).
//   - Icon resolution.
//   - Async / progressive enumeration.

import AppKit
import Foundation

/// A `.app` bundle the user can launch. Three fields are enough for
/// this slice — name (display), bundleId (stable identifier), and
/// nothing else. Icons land in a later slice.
struct DiscoveredApp {
    let name: String
    let bundleId: String
}

enum AppDiscovery {
    /// Walk `/Applications` and `~/Applications` for `.app` bundles and
    /// return them deduped by bundle id (first-wins) and sorted by
    /// lowercased name. Synchronous: the panel is shown after this
    /// returns, so the first paint is fully populated.
    static func discover() -> [DiscoveredApp] {
        let fm = FileManager.default

        // Two roots, in the order the user would expect them to win
        // collisions: system-level `/Applications` first, then the
        // per-user `~/Applications`. Matches the GNOME first-dir-wins
        // policy in `app/gnome/src/apps.rs`.
        let roots: [URL] = [
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

                let name = (bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
                    ?? (bundle.object(forInfoDictionaryKey: "CFBundleName") as? String)
                    ?? url.deletingPathExtension().lastPathComponent

                seen.insert(bundleId)
                out.append(DiscoveredApp(name: name, bundleId: bundleId))
            }
        }

        out.sort { $0.name.lowercased() < $1.name.lowercased() }
        return out
    }
}
