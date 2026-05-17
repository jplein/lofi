// Owns the launcher's `NSPanel` and the small set of properties that
// make it behave as a floating launcher overlay (not a normal window).
//
// Design notes (each one has a story behind it; see `app/macos/README.md`
// for the deeper rationale):
//
// - `.borderless | .nonactivatingPanel` — no titlebar/chrome, no
//   activation of LoFi as the front-most app on click. Important:
//   borderless panels return `canBecomeKey = false` by default, which
//   silently breaks keyboard events. The `LoFiPanel` subclass below
//   overrides that.
// - `.floating` level — sits above ordinary application windows.
// - `[.canJoinAllSpaces, .fullScreenAuxiliary]` — the panel follows the
//   user across spaces and overlays full-screen apps the same way the
//   system Spotlight does.
// - `hidesOnDeactivate = false` *for this slice only*. Spotlight-style
//   "dismiss on focus loss" is the eventual UX, but with no global
//   hotkey yet to bring the panel back, a hide-on-deactivate panel
//   would vanish the moment `open LoFi.app` returns control to the
//   launching terminal. Keeping it visible lets the static-list demo
//   actually be seen; flip back to `true` once the hotkey slice lands.
// - Fixed 640×400; centered after sizing.
//
// The panel's contentView is an NSStackView holding the search field on
// top and the scrolling list below. The search field is wired up as the
// initial first responder so typing starts immediately when the panel
// shows — `panel.initialFirstResponder` must be set *before*
// `makeKeyAndOrderFront`, hence the order in `show()`.

import AppKit

private let kPanelWidth: CGFloat = 640
private let kPanelHeight: CGFloat = 400

final class PanelController {
    let panel: NSPanel

    init(searchField: NSSearchField, listView: NSView) {
        let frame = NSRect(x: 0, y: 0, width: kPanelWidth, height: kPanelHeight)
        let panel = LoFiPanel(
            contentRect: frame,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isMovableByWindowBackground = false
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .windowBackgroundColor

        // Stack the search field on top of the list view. `.fill`
        // distribution gives every child its intrinsic size first; the
        // list view's `setContentHuggingPriority(.defaultLow)` (the
        // default for NSScrollView) lets it expand to take whatever's
        // left after the search field claims its intrinsic height. We
        // pin the search field's vertical content-hugging to `.required`
        // so a window resize never grows it past one line.
        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.setContentHuggingPriority(.required, for: .vertical)
        listView.translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView(views: [searchField, listView])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.distribution = .fill
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false

        if let contentView = panel.contentView {
            contentView.addSubview(stack)
            NSLayoutConstraint.activate([
                stack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
                stack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
                stack.topAnchor.constraint(equalTo: contentView.topAnchor),
                stack.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
                // NSStackView's vertical `.fill` distribution honors each
                // child's vertical content-hugging; the search field hugs
                // at `.required`, so the list view fills the remaining
                // vertical space without extra constraints. We still pin
                // the search field's width so it spans the panel — the
                // stack's `.leading` alignment alone would leave it at
                // its intrinsic narrow width.
                searchField.leadingAnchor.constraint(equalTo: stack.leadingAnchor),
                searchField.trailingAnchor.constraint(equalTo: stack.trailingAnchor),
                listView.leadingAnchor.constraint(equalTo: stack.leadingAnchor),
                listView.trailingAnchor.constraint(equalTo: stack.trailingAnchor),
            ])
        }

        // `initialFirstResponder` must be set BEFORE the panel becomes
        // key. Setting it after `makeKeyAndOrderFront` is a silent no-op:
        // the panel has already picked a default responder by then and
        // typing goes nowhere.
        panel.initialFirstResponder = searchField

        self.panel = panel
    }

    func show() {
        panel.center()
        panel.makeKeyAndOrderFront(nil)
    }
}

/// `NSPanel` returns `canBecomeKey = false` for borderless panels by
/// default. That silently breaks keyboard event delivery — typing into
/// what looks like a focused panel just sends events to whatever app
/// was previously frontmost. Overriding to `true` is mandatory for any
/// borderless launcher UI.
final class LoFiPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}
