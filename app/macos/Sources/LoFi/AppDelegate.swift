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
    // Persistent per-window pre-maximize frame store for the
    // toggle-maximize command. Held for the process lifetime (like
    // `mruStore`) so the save (on maximize) and the take (on un-maximize)
    // hit the same backing store — though in practice those two presses
    // span two LoFi runs, which is exactly why the store is on-disk
    // (UserDefaults). See `SavedFrameStore`.
    private let savedFrameStore = SavedFrameStore()
    // The window-action command target captured at startup (frontmost
    // non-LoFi window). `nil` when there's no usable target, in which case
    // no command rows are pushed. Threaded into `AppListController` so the
    // command dispatch knows the pid/title/work-area/fallback rect.
    private var commandTarget: WindowCommands.CommandTarget?
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
        // act on windows via AX). Both deny? Skip the whole section
        // for this session. Both are captured at process start by
        // TCC, so freshly-granted permissions only take effect on
        // the next launch.
        let canSeeWindows = Permissions.screenRecording() && Permissions.accessibility()
        if canSeeWindows {
            // `WindowDiscovery.discover` is still called because the
            // window-action commands need it to find their target
            // (the frontmost non-LoFi window on the active display)
            // and `SavedFrameStore.prune` needs the live-id list to
            // garbage-collect dropped frame records. What's
            // **intentionally absent** is the per-window
            // `entries.pushWindow(...)` loop — i.e. the window
            // *switcher* feature.
            //
            // Why the window switcher is disabled on macOS:
            // shipping it required solving two macOS limitations a
            // regular (unprivileged, non-Dock-injected) launcher
            // can't work around:
            //
            //   1. **Cross-Space activation** —
            //      `SLSManagedDisplaySetCurrentSpace` from a regular
            //      process on Tahoe returns success but yanks
            //      windows from the target Space onto the
            //      originating Space (gotcha 13). Scoping the list
            //      to the active Space sidesteps this, but...
            //   2. **Cross-display focus retargeting** — AX writes
            //      that retarget another app's key window across a
            //      display boundary are silently dropped (gotcha
            //      14). And even with the list scoped to the active
            //      display, picking a same-display window can still
            //      fail to focus when the owning app has sibling
            //      windows on other displays: the picked window
            //      raises to the front, but keyboard focus stays on
            //      whichever window the app considered key. The
            //      list contents would also depend on mouse-cursor
            //      position (the "active display" determination),
            //      which is surprising UX in its own right.
            //
            // Net: with the scoping in place, the user can't be
            // sure a window they want is *listed*, OR that
            // activating a listed window will actually get them
            // there. That's worse than not having the feature.
            // Yabai-style Dock-injection scripting additions are
            // the only path that would make this reliable, and
            // they're out of scope for a launcher (SIP-disabled,
            // system-modification install). The investigation lives
            // in the project memory entries
            // `project_sls_cross_space.md` and
            // `project_ax_cross_display_focus.md`; see README
            // gotchas 13-14 for the user-facing rationale.
            //
            // The window-action commands stay because they don't
            // require the user to *pick* a window — they always act
            // on the frontmost non-LoFi window on the active
            // display, which is reliably identifiable from
            // `WindowDiscovery.discover`.
            let discoveredWindows = WindowDiscovery.discover()
            savedFrameStore.prune(
                liveWindowIds: Set(discoveredWindows.map { UInt64($0.id) })
            )
            pushCommands()
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
            windowAux: windowAux,
            commandTarget: commandTarget,
            savedFrameStore: savedFrameStore
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

    /// Window-action command ids in display order. Mirrors
    /// `CommandKind::as_id` (`app/core/src/lib.rs`); the GNOME
    /// platform's `commands.rs::ALL_KINDS` is the same order minus
    /// `next_display`/`previous_display` (not implemented in the
    /// GNOME extension yet — they appear only on macOS).
    private static let commandIdsAlwaysAvailable = [
        "center",
        "center_half",
        "center_two_thirds",
        "left_half",
        "right_half",
        "standard_size",
        "minimize",
        "toggle_maximize",
        "toggle_fullscreen",
    ]

    /// Multi-display command ids appended to `commandIdsAlwaysAvailable`
    /// only when at least 2 displays are attached. Single-display users
    /// never see "Next display" / "Previous display" rows because the
    /// commands would be no-ops there — `WindowControl.moveToDisplay`
    /// would just return `false` for `screens.count < 2`, so showing
    /// the rows would be a dead affordance.
    private static let commandIdsMultiDisplay = [
        "next_display",
        "previous_display",
    ]

    /// Gather the command target and push the command entries. No-op
    /// (and leaves `commandTarget` nil) when there's no usable target,
    /// so the command rows simply don't appear — GNOME parity. The
    /// multi-display ids are appended only when ≥ 2 displays are
    /// attached.
    ///
    /// After the push, fill `target.standardRect` by scanning the pushed
    /// rows for the `standard_size` command and reading its computed
    /// geometry. This single-sources the StandardSize restore rect from
    /// Rust's `compute_geometry` (so the 2/3 math isn't duplicated in
    /// Swift); `toggleMaximize` uses it only as the no-saved-frame
    /// fallback. The scan is BY ID (not a fixed index) and happens before
    /// any filter so it's robust to ordering.
    private func pushCommands() {
        guard var target = WindowCommands.gatherTarget() else { return }

        var ids = Self.commandIdsAlwaysAvailable
        if NSScreen.screens.count >= 2 {
            ids.append(contentsOf: Self.commandIdsMultiDisplay)
        }
        for id in ids {
            _ = entries.pushCommand(
                kindId: id,
                targetWindowId: target.windowId,
                waX: Int32(target.workArea.minX.rounded()),
                waY: Int32(target.workArea.minY.rounded()),
                waW: Int32(target.workArea.width.rounded()),
                waH: Int32(target.workArea.height.rounded()),
                frameX: Int32(target.currentFrame.minX.rounded()),
                frameY: Int32(target.currentFrame.minY.rounded()),
                frameW: Int32(target.currentFrame.width.rounded()),
                frameH: Int32(target.currentFrame.height.rounded())
            )
        }

        // Scan by id (no filter is active yet, so the filtered index space
        // equals the push order) for the standard_size command's computed
        // rect. Used as the toggle-maximize fallback.
        for row in 0..<entries.count where entries.commandId(at: row) == "standard_size" {
            if let geo = entries.commandGeometry(at: row) {
                target.standardRect = CGRect(
                    x: CGFloat(geo.x),
                    y: CGFloat(geo.y),
                    width: CGFloat(geo.w),
                    height: CGFloat(geo.h)
                )
            }
            break
        }

        commandTarget = target
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
