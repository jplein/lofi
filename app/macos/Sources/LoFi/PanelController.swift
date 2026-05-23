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
// - Clear window background + `hasShadow` + a rounded
//   `NSGlassEffectView` (Tahoe Liquid Glass; `NSVisualEffectView`
//   fallback below macOS 26 — see `makeBackground`) give the panel
//   Spotlight's look: rounded corners, a drop shadow, and a translucent
//   glass surface. The window itself MUST be non-opaque with a clear
//   background, or its square corners show through behind the rounded
//   content and the shadow traces the square, not the rounded shape.
// - Fixed 640×400; centered after sizing.
//
// The glass background hosts an NSStackView holding the search field on
// top and the scrolling list below, with padding below the field
// (`setCustomSpacing`) and a small top inset so the field clears the
// rounded corners. The search field is wired up as the initial first
// responder so typing starts immediately when the panel shows —
// `panel.initialFirstResponder` must be set *before*
// `makeKeyAndOrderFront`, hence the order in `show()`.

import AppKit

private let kPanelWidth: CGFloat = 640
private let kPanelHeight: CGFloat = 400
// Rounded-corner radius for the panel. Matches the Tahoe Spotlight
// window so LoFi reads as a system launcher rather than a plain box.
private let kCornerRadius: CGFloat = 20
// Breathing room above the search field so its text and magnifier clear
// the rounded top corners.
private let kContentTopInset: CGFloat = 6
// Padding between the input field and the results list.
private let kInputBottomPadding: CGFloat = 8

final class PanelController {
    let panel: NSPanel

    init(searchView: NSView, searchResponder: NSView, listView: NSView) {
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
        // Clear background + a real window shadow so the rounded glass
        // content view defines the visible shape and casts a Spotlight-
        // style drop shadow. An opaque background would let the square
        // window corners show through behind the rounded content.
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true

        // Stack the search header on top of the list view. `.fill`
        // distribution gives every child its intrinsic size first; the
        // list view's `setContentHuggingPriority(.defaultLow)` (the
        // default for NSScrollView) lets it expand to take whatever's
        // left after the search header claims its intrinsic height. We
        // pin the search header's vertical content-hugging to `.required`
        // so a window resize never grows it past its one-row height.
        searchView.translatesAutoresizingMaskIntoConstraints = false
        searchView.setContentHuggingPriority(.required, for: .vertical)
        listView.translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView(views: [searchView, listView])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.distribution = .fill
        stack.spacing = 0
        // Padding below the input field only — the list still butts
        // against the bottom edge. `setCustomSpacing` inserts it between
        // the two arranged views without touching either outer edge.
        stack.setCustomSpacing(kInputBottomPadding, after: searchView)
        stack.translatesAutoresizingMaskIntoConstraints = false

        // The content sits on a rounded liquid-glass background. `host`
        // is a plain view that fills the glass and carries the stack's
        // constraints; `background` becomes the panel's contentView.
        let (background, host) = Self.makeBackground()
        panel.contentView = background
        host.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            // Small top inset so the field clears the rounded corner; the
            // list fills the rest down to the bottom (clipped to the
            // glass corner radius).
            stack.topAnchor.constraint(equalTo: host.topAnchor, constant: kContentTopInset),
            stack.bottomAnchor.constraint(equalTo: host.bottomAnchor),
            // NSStackView's vertical `.fill` distribution honors each
            // child's vertical content-hugging; the search header hugs at
            // `.required`, so the list view fills the remaining vertical
            // space without extra constraints. We still pin the search
            // header's width so it spans the panel — the stack's `.leading`
            // alignment alone would leave it at its intrinsic narrow width.
            searchView.leadingAnchor.constraint(equalTo: stack.leadingAnchor),
            searchView.trailingAnchor.constraint(equalTo: stack.trailingAnchor),
            listView.leadingAnchor.constraint(equalTo: stack.leadingAnchor),
            listView.trailingAnchor.constraint(equalTo: stack.trailingAnchor),
        ])

        // `initialFirstResponder` must be set BEFORE the panel becomes
        // key. Setting it after `makeKeyAndOrderFront` is a silent no-op:
        // the panel has already picked a default responder by then and
        // typing goes nowhere. The responder is the inner text field, not
        // the header container, so the field editor activates on show.
        panel.initialFirstResponder = searchResponder

        self.panel = panel
    }

    func show() {
        panel.center()
        panel.makeKeyAndOrderFront(nil)
    }

    /// Builds the rounded, translucent background the search field and
    /// list sit on. On macOS 26 (Tahoe) this is a true Liquid Glass
    /// surface via `NSGlassEffectView`; the deployment target is 15.0, so
    /// older systems fall back to a vibrant `NSVisualEffectView` with a
    /// layer-clipped corner radius. Returns the `container` to install as
    /// the panel's `contentView` and the inner `host` to add content to
    /// (for glass that's its `contentView`; for vibrancy a pinned
    /// subview). `host` is pinned to `container`'s edges so it fills the
    /// rounded area regardless of which backend produced it.
    private static func makeBackground() -> (container: NSView, host: NSView) {
        let host = NSView()

        let container: NSView
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = kCornerRadius
            glass.contentView = host
            container = glass
        } else {
            let effect = NSVisualEffectView()
            effect.material = .hudWindow
            effect.blendingMode = .behindWindow
            effect.state = .active
            effect.wantsLayer = true
            effect.layer?.cornerRadius = kCornerRadius
            effect.layer?.masksToBounds = true
            effect.addSubview(host)
            container = effect
        }

        host.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            host.topAnchor.constraint(equalTo: container.topAnchor),
            host.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        return (container, host)
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
