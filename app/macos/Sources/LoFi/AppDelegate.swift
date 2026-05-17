// `AppDelegate` is the top-level coordinator for the LoFi process.
//
// On launch it:
//   1. Discovers installed `.app` bundles via `AppDiscovery`.
//   2. Pushes each into a Rust-owned `EntryList` over the C ABI.
//   3. Hands that list to the `PanelController`'s list controller.
//   4. Foregrounds the process (`LSUIElement=YES` apps start in the
//      background and would never become key without this step) and
//      shows the panel.
//
// All four steps are synchronous on launch in this first slice. Async
// discovery and incremental updates are out of scope here.

import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let entries = EntryList()
    private var panelController: PanelController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Gather + push happens before the panel is constructed so the
        // table view sees a fully-populated list at first paint.
        for app in AppDiscovery.discover() {
            // Icon is intentionally `nil` this slice — `.app` icons are
            // a future story (Application Services / NSWorkspace icon
            // resolution). The Rust side accepts a `nil` icon as `None`.
            _ = entries.pushApplication(name: app.name, bundleId: app.bundleId, icon: nil)
        }

        let listController = AppListController(entries: entries)
        let controller = PanelController(content: listController.view)
        panelController = controller

        // `LSUIElement=YES` keeps LoFi out of the Dock and forces a
        // background-only launch. `activate(ignoringOtherApps:)` brings
        // the process to the foreground; without it the panel renders
        // but never becomes key, so keyboard events go to whatever app
        // was previously focused.
        NSApp.activate(ignoringOtherApps: true)
        controller.show()
    }
}
