// `AppDelegate` is the top-level coordinator for the LoFi process.
//
// On launch it:
//   1. Installs a hidden main menu so Cmd-Q has a handler (the
//      LSUIElement=YES suppresses the system-provided Application
//      menu).
//   2. Discovers installed `.app` bundles via `AppDiscovery`.
//   3. Pushes each into a Rust-owned `EntryList` over the C ABI.
//   4. Hands that list to the `PanelController`'s list controller.
//   5. Foregrounds the process (`LSUIElement=YES` apps start in the
//      background and would never become key without this step) and
//      shows the panel.
//
// All steps are synchronous on launch in this first slice. Async
// discovery and incremental updates are out of scope here.

import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let entries = EntryList()
    private var panelController: PanelController?
    // NSTableView holds dataSource and delegate as weak references; if
    // the only strong reference to the list controller is a local var
    // in `applicationDidFinishLaunching`, it deallocates as soon as
    // that method returns and the table silently stops asking for cell
    // views. Pinning it on the delegate keeps it alive for the process
    // lifetime.
    private var listController: AppListController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        installHiddenMenu()

        // Gather + push happens before the panel is constructed so the
        // table view sees a fully-populated list at first paint.
        for app in AppDiscovery.discover() {
            // Per the cross-platform contract (mirroring GNOME's "icon
            // identifier, not bytes"), the `icon` argument is whatever
            // identifier the platform layer needs to resolve a real icon
            // later. On macOS that's the `.app` bundle path; the UI calls
            // `NSWorkspace.shared.icon(forFile:)` at draw time.
            _ = entries.pushApplication(
                name: app.name,
                bundleId: app.bundleId,
                icon: app.bundlePath
            )
        }

        let listController = AppListController(entries: entries)
        self.listController = listController
        let controller = PanelController(
            searchField: listController.searchField,
            listView: listController.listView
        )
        panelController = controller

        // `LSUIElement=YES` keeps LoFi out of the Dock and forces a
        // background-only launch. `activate(ignoringOtherApps:)` brings
        // the process to the foreground; without it the panel renders
        // but never becomes key, so keyboard events go to whatever app
        // was previously focused.
        NSApp.activate(ignoringOtherApps: true)
        controller.show()
    }

    private func installHiddenMenu() {
        let mainMenu = NSMenu()
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        let quit = NSMenuItem(
            title: "Quit LoFi",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        quit.keyEquivalentModifierMask = [.command]
        appMenu.addItem(quit)
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)
        NSApp.mainMenu = mainMenu
    }
}
