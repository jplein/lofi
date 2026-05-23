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
    // Held for the process lifetime so the underlying SQLite connection
    // stays open between the initial `applyMru` and any subsequent
    // `bumpMru` on activation. `nil` when `MruStore.init?` failed —
    // the launcher proceeds without MRU ordering in that case.
    private var mruStore: MruStore?
    // Macos-side companion data for Window entries, keyed by the same
    // `CGWindowID` we hand through to Rust. Two distinct uses:
    //
    //   - `pid` + `title` feed `WindowActivation.raise(pid:title:)` at
    //     activation time. The Rust `Window` shape is intentionally
    //     window-id-and-title only (cross-platform constraint); pid is
    //     macOS-only state.
    //   - `appName` is the row's "owning app" label. The matcher
    //     already includes `Window.app_name` in its haystack so typing
    //     an app name matches that app's windows — but the FFI
    //     `lofi_entries_get_name` returns only the bare title. Without
    //     this map the launcher row reads as `"Hacker News [Window]"`
    //     with no indication that it belongs to Chrome. We render the
    //     visible name as `"Hacker News — Google Chrome"` by stitching
    //     `title` and `appName` at draw time.
    private var windowAux: [UInt64: (pid: pid_t, title: String, appName: String)] = [:]
    // Set true when we triggered a TCC prompt on this launch. The
    // prompt is a system-owned window; if we then call
    // `NSApp.activate(ignoringOtherApps: true)` our borderless panel
    // covers it and the user can't grant the permission they were just
    // asked for.
    private var promptedForPermission = false

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

        // Window enumeration is gated on TWO permissions: Screen
        // Recording (to read `kCGWindowName`) and Accessibility (to
        // raise specific windows via AX). Both deny? Skip windows
        // entirely for this session — listing entries we can't activate
        // is a worse experience than silently omitting them. Both are
        // captured at process start by TCC, so freshly-granted
        // permissions only take effect on the next launch.
        let canSeeWindows = Permissions.screenRecording() && Permissions.accessibility()
        if canSeeWindows {
            for w in WindowDiscovery.discover() {
                // `icon` is the icon-resolution input the Swift UI hands
                // to `NSWorkspace.shared.icon(forFile:)` at draw time —
                // it must be a *path*, not a bundle identifier.
                // `appDesktopId` is the stable identifier and stays the
                // bundle id. See `DiscoveredWindow` for the field split.
                _ = entries.pushWindow(
                    id: UInt64(w.id),
                    title: w.title,
                    appName: w.ownerName,
                    icon: w.ownerBundlePath,
                    workspace: w.workspace,
                    appDesktopId: w.ownerBundleId
                )
                windowAux[UInt64(w.id)] = (w.ownerPid, w.title, w.ownerName)
            }
        } else {
            // Trigger the system dialogs once. The state captured by
            // `CGPreflightScreenCaptureAccess` is set at process start,
            // so the user has to relaunch to pick up a freshly-granted
            // permission.
            if !Permissions.screenRecording() { Permissions.requestScreenRecording() }
            if !Permissions.accessibility() { Permissions.requestAccessibility() }
            // The TCC prompt is system-owned. Without this flag clear we'd
            // call `NSApp.activate(ignoringOtherApps: true)` below and
            // shove our borderless panel in front of the prompt — the
            // user then can't see (or click) the grant button. Skip the
            // aggressive activation on this launch; the panel will still
            // render, and the user can relaunch once they've granted.
            promptedForPermission = true
        }

        // After every entry is pushed, apply the persistent MRU order so
        // the most-recently-launched app shows up at the top. A failed
        // open (permission denied, disk full, ...) leaves `mruStore` nil;
        // the launcher falls back to the alphabetical order produced by
        // `AppDiscovery.discover()`.
        if let store = MruStore(path: MruStore.defaultPath()) {
            self.mruStore = store
            entries.applyMru(store: store)
        }

        let listController = AppListController(
            entries: entries,
            mruStore: mruStore,
            windowAux: windowAux
        )
        self.listController = listController
        let controller = PanelController(
            searchView: listController.searchView,
            searchResponder: listController.searchInput,
            listView: listController.listView
        )
        panelController = controller

        // `LSUIElement=YES` keeps LoFi out of the Dock and forces a
        // background-only launch. `activate(ignoringOtherApps:)` brings
        // the process to the foreground; without it the panel renders
        // but never becomes key, so keyboard events go to whatever app
        // was previously focused.
        //
        // Suppress the `ignoringOtherApps` flag when we just fired a
        // TCC prompt — the prompt is a system-owned window and our
        // borderless panel would otherwise cover it. The panel still
        // shows (so the user can use the app list); they just need to
        // click out to the prompt to grant the permission.
        NSApp.activate(ignoringOtherApps: !promptedForPermission)
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
