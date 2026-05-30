// `AppDelegate` is the top-level coordinator for the long-running LoFi
// daemon. It registers an Alt+Space global hotkey at launch, stays
// resident in the background until the hotkey fires (or `:activate`
// sends a reopen event), then summons the panel — gathering the
// fresh frontmost-non-LoFi window for command targeting on every
// summon. Activating an entry or pressing Esc hides the panel and
// returns LoFi to the background without terminating. Cmd-Q in the
// hidden menu is the explicit quit path.
//
// Daemon lifecycle
// ----------------
// `applicationDidFinishLaunching` does the *expensive but
// summon-stable* setup once: enumerate `.app` bundles, open the
// MRU SQLite store, build the panel + list controller. It does
// **not** show the panel — the user has to summon. The cheap
// per-summon work (gather command target, push entries, apply MRU,
// reset UI state) happens in `summonPanel` so the command target
// always reflects the foreground window at summon time, not at
// process-start time.
//
// `applicationShouldHandleReopen` is the Launch Services hook
// `:activate` (and a Dock click on a regular-app LoFi build) uses to
// summon. Returning `false` tells AppKit not to open its default
// window (LSUIElement apps don't have one anyway, but the contract
// is clearer if we say so).
//
// The hotkey handler routes through `toggleOrSummon` so pressing
// Alt+Space while the panel is already up (and LoFi is active)
// dismisses — Spotlight behavior. The visibility check uses
// `NSApp.isActive` rather than `panel.isVisible` because
// `hidesOnDeactivate = true` couples those two states tightly and
// `isActive` is the more direct signal.
//
// Permissions
// -----------
// Both Screen Recording and Accessibility are checked at process
// start (TCC freezes the answer there — gotcha 10). The first summon
// that finds either missing fires the system prompts and returns
// WITHOUT showing the panel: our `.floating`-level panel composites
// on top of the centered TCC dialogs (window level beats activation
// for z-order, so `ignoringOtherApps` can't push it behind), so we
// let the dialogs own the screen instead. The prompt fires exactly
// once (`promptedForPermission`); later summons in the same process
// show the panel in degraded apps-only mode. After the user grants
// and relaunches, the new process sees the permissions and runs the
// full path from its first summon.

import AppKit
import Carbon.HIToolbox

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
    // stays open between summons. `nil` when `MruStore.init?` failed —
    // LoFi proceeds without MRU ordering in that case.
    private var mruStore: MruStore?
    // Persistent per-window pre-maximize frame store for the
    // toggle-maximize command. The save (on maximize) and the take
    // (on un-maximize) typically span multiple summons of the same
    // long-running process; UserDefaults is the on-disk backing
    // store so it also survives a quit/relaunch.
    private let savedFrameStore = SavedFrameStore()
    // Cached at launch and re-pushed on each summon. App enumeration
    // (file-system walk + bundle reads) costs ~50ms on a typical Mac
    // — fine for a one-time launch cost, too slow to redo on every
    // Alt+Space press. Apps don't change frequently within a session;
    // a user installing a new app mid-session will need to quit and
    // relaunch LoFi to see it. Acceptable trade for snappy summons.
    private var cachedApps: [DiscoveredApp] = []
    // Globally-frontmost non-LoFi window, captured fresh by every
    // `summonPanel`. `nil` between summons / when there's no target.
    // Pushed into the list controller before the panel is shown.
    private var commandTarget: WindowCommands.CommandTarget?
    // macOS-side companion data for Window entries (the window
    // switcher feature is disabled — see README gotchas 13-14 — so
    // this stays empty; the field exists because `AppListController`
    // expects it). NOTE: unlike `commandTarget`, this is handed to the
    // controller once at construction and never refreshed per-summon
    // (there is no `setWindowAux`). Harmless while empty; if Window rows
    // are ever re-enabled, refresh it each summon like `commandTarget`.
    private var windowAux: [UInt64: (pid: pid_t, title: String, appName: String)] = [:]
    // Set once, on the first summon that finds a permission missing,
    // right before we fire the TCC prompts and bail without showing
    // the panel (so the panel doesn't bury the system dialogs). Gates
    // that prompt-and-bail path to exactly once per process — later
    // missing-permission summons skip it and show the panel in
    // degraded apps-only mode instead.
    private var promptedForPermission = false
    // Live for the process lifetime; deinit unregisters via Carbon.
    private var hotkey: GlobalHotkey?

    func applicationDidFinishLaunching(_ notification: Notification) {
        installHiddenMenu()

        // Cache app discovery once. See `cachedApps` comment for the
        // refresh tradeoff.
        cachedApps = AppDiscovery.discover()

        // Open the persistent MRU store. Re-applied on every summon
        // (`applyMru` is cheap — just sorts the in-memory entries
        // vec by the SQLite-backed rank).
        mruStore = MruStore(path: MruStore.defaultPath())

        // Build the panel + list controller once, with empty entries.
        // The first summon will populate the list before showing.
        let listController = AppListController(
            entries: entries,
            mruStore: mruStore,
            windowAux: windowAux,
            commandTarget: nil,
            savedFrameStore: savedFrameStore,
            onDismiss: { [weak self] in self?.dismissPanel() }
        )
        self.listController = listController
        let controller = PanelController(
            searchView: listController.searchView,
            searchResponder: listController.searchInput,
            listView: listController.listView
        )
        panelController = controller

        // Register Option+Space as the system-wide summon hotkey.
        // Carbon constants: `kVK_Space` from `HIToolbox/Events.h`,
        // `optionKey = 1 << 11` from `HIToolbox/Events.h`. The press
        // handler runs on the main thread (Carbon event dispatch is
        // main-threaded), so no extra hop is needed before touching
        // the panel.
        hotkey = GlobalHotkey(
            keyCode: UInt32(kVK_Space),
            modifiers: UInt32(optionKey)
        ) { [weak self] in
            self?.toggleOrSummon()
        }
    }

    /// Launch Services delivers a reopen event when an already-running
    /// LSUIElement app is `open`-ed again (the `:activate` target uses
    /// `open -b dev.jplein.lofi` for exactly this). Treat it as an
    /// explicit summon request.
    ///
    /// Returning `false` keeps AppKit from trying to open its default
    /// window (we don't have one — the panel is owned by the
    /// `PanelController`).
    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        summonPanel()
        return false
    }

    /// Hotkey entry point. Spotlight-style toggle: if LoFi is the
    /// foreground app, the user wants the panel to go away;
    /// otherwise summon it. `NSApp.isActive` is the right
    /// discriminator (rather than `panel.isVisible`) because
    /// `hidesOnDeactivate = true` couples key status and visibility,
    /// and isActive is the cleaner signal for "the user is
    /// currently interacting with LoFi."
    private func toggleOrSummon() {
        if NSApp.isActive {
            dismissPanel()
        } else {
            summonPanel()
        }
    }

    /// Rebuild the entry list from the cached app set + freshly
    /// gathered commands, apply MRU, hand the new state to the list
    /// controller, then activate + show.
    ///
    /// The `entries.clear()` at the top is critical: without it,
    /// every summon would *append* a fresh copy of apps + commands
    /// to the list, the user would see duplicates, and stale
    /// command targets would linger from earlier summons. The
    /// `lofi_entries_clear` FFI was added specifically for this
    /// call.
    ///
    /// Command-target gathering happens *before* `NSApp.activate`
    /// so `WindowDiscovery.discover` reads CGWindowList while LoFi
    /// is still background — meaning the frontmost-non-LoFi
    /// window is the one the user was just using, not LoFi
    /// itself. (`WindowDiscovery` filters by pid anyway, but the
    /// z-order it gets is more useful when LoFi isn't at the top
    /// of it.)
    private func summonPanel() {
        // Window enumeration is gated on TWO permissions: Screen
        // Recording (for `kCGWindowName`) and Accessibility (for AX
        // raise/move). Both are captured at process start by TCC, so
        // freshly-granted permissions only take effect on the next
        // launch (relaunch via `:close` + `:launch`, or Cmd-Q +
        // `:launch`).
        let canSeeWindows = Permissions.screenRecording() && Permissions.accessibility()

        // First summon that finds a permission missing: fire the TCC
        // prompts and bail BEFORE showing the panel. Our `.floating`
        // panel composites on top of the centered system dialogs
        // (window level beats activation for z-order, so
        // `ignoringOtherApps` can't push it behind), so showing it
        // would bury them — instead we let the dialogs own the screen.
        // Runs once per process (`promptedForPermission`); the user
        // grants, relaunches (gotcha 10), and the next process takes
        // the full path. Later missing-permission summons fall through
        // and show the panel in degraded apps-only mode.
        if !canSeeWindows && !promptedForPermission {
            promptedForPermission = true
            if !Permissions.screenRecording() { Permissions.requestScreenRecording() }
            if !Permissions.accessibility() { Permissions.requestAccessibility() }
            return
        }

        // Reset state from the previous summon.
        entries.clear()
        commandTarget = nil

        // Window discovery is still needed by the command-target push
        // and the saved-frame store's prune step. It's NOT the source
        // of the running-app set anymore (see below).
        let discoveredWindows: [DiscoveredWindow]
        if canSeeWindows {
            discoveredWindows = WindowDiscovery.discover()
            savedFrameStore.prune(
                liveWindowIds: Set(discoveredWindows.map { UInt64($0.id) })
            )
        } else {
            discoveredWindows = []
        }

        // Bundle ids of currently-running processes. `runningApplications`
        // returns every process regardless of which macOS Space its
        // windows live on, which is what fixes the cross-Space miss:
        // deriving the set from `WindowDiscovery.discover()` (which
        // uses `CGWindowListCopyWindowInfo(.optionOnScreenOnly, ...)`)
        // only saw windows on the *active* Space, so e.g. Firefox
        // Developer Edition on Space 2 was reported as not running
        // while the user was on Space 1.
        //
        // Trade: this marks menu-bar agents (Karabiner, Rectangle, …) as
        // running whenever their process is alive, even without a window.
        // That's a small semantic shift from "has a window" to "process
        // is up" — acceptable because the dot reads as "this app is
        // alive" to users, and the alternative (a second CGWindowList
        // pass without `.optionOnScreenOnly`) is more code for parity
        // with a model the user doesn't actually distinguish from "is
        // running."
        //
        // No TCC grant required, so the indicator works in degraded
        // (apps-only) mode too.
        var runningBundleIds: Set<String> = []
        for running in NSWorkspace.shared.runningApplications {
            if let bid = running.bundleIdentifier {
                runningBundleIds.insert(bid)
            }
        }

        // Re-push the cached app set. Apps don't change between
        // summons in the same session, but the Rust list does (we
        // just cleared it), so we push from the cache. `isRunning`
        // is recomputed every summon from the freshly-gathered
        // window list above.
        for app in cachedApps {
            _ = entries.pushApplication(
                name: app.name,
                bundleId: app.bundleId,
                icon: app.bundlePath,
                isRunning: runningBundleIds.contains(app.bundleId)
            )
        }

        if canSeeWindows {
            pushCommands()
        }

        // Power commands (Lock / Log Out / Sleep / Restart / Shutdown)
        // are unconditional — they don't depend on the focused window,
        // permissions, or any runtime state, so the rows always appear.
        // Matches GNOME's `power::gather_power_commands` shape.
        pushPowerCommands()

        // Apply MRU after every push so the user's recent picks
        // bubble to the top. Cheap (in-memory sort against the
        // SQLite-backed rank).
        if let store = mruStore {
            entries.applyMru(store: store)
        }

        // Hand the fresh command target to the list controller and
        // reset its UI state (search input, table selection) before
        // the panel becomes visible. Order matters here: if we
        // showed the panel first the user might briefly see the
        // *previous* summon's state.
        listController?.setCommandTarget(commandTarget)
        listController?.reset()

        // `LSUIElement=YES` keeps LoFi out of the Dock; `activate`
        // is what brings it forward each summon. We always ignore
        // other apps here — the one case that must not cover the
        // system permission dialog (the first missing-permission
        // summon) returns above before reaching this point.
        NSApp.activate(ignoringOtherApps: true)
        panelController?.show()
    }

    /// Hide the panel and step LoFi back to the background without
    /// terminating. Two paths reach this: explicit dismiss (Esc, or
    /// the `onDismiss` callback after an entry activation) and
    /// implicit dismiss via `hidesOnDeactivate = true` (the panel
    /// vanishes when LoFi loses key — the dismissPanel call here is
    /// then mostly redundant, but `NSApp.hide` cleans up the
    /// "inactive but technically frontmost" state cmd-tab would
    /// otherwise show).
    private func dismissPanel() {
        panelController?.hide()
        NSApp.hide(nil)
    }

    /// Window-action command ids in display order. Mirrors
    /// `CommandKind::as_id` (`app/core/src/lib.rs`); the GNOME
    /// platform's `commands.rs::ALL_KINDS` is the same order minus
    /// `next_display`/`previous_display` (not implemented in the
    /// GNOME extension yet — they appear only on macOS).
    private static let commandIdsAlwaysAvailable = [
        "center",
        "center_third",
        "center_half",
        "center_two_thirds",
        "left_third",
        "left_half",
        "left_two_thirds",
        "right_third",
        "right_half",
        "right_two_thirds",
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

    /// Power-command ids in display order. Mirrors `PowerCommandKind::as_id`
    /// (`app/core/src/lib.rs`) and matches GNOME's `power::ALL_KINDS`. These
    /// are always available regardless of permissions — the dispatch path
    /// (`PowerCommands.activate`) doesn't depend on the focused window or
    /// any TCC grant. Lock/Sleep shell out to `CGSession` / `pmset`;
    /// Log Out / Restart / Shutdown go through `loginwindow` via raw Apple
    /// events.
    private static let powerCommandIds = [
        "lock_session",
        "logout",
        "suspend",
        "restart",
        "shutdown",
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

    /// Push one row per `PowerCommandKind`. Unlike the window-action
    /// commands these don't depend on a target window or permissions,
    /// so they appear unconditionally — the user always has Lock / Log
    /// Out / Sleep / Restart / Shutdown available from the launcher.
    private func pushPowerCommands() {
        for id in Self.powerCommandIds {
            _ = entries.pushPowerCommand(kindId: id)
        }
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
