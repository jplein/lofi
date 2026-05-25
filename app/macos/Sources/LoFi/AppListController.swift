// `NSTableView` data source + delegate backed by a Rust-owned
// `EntryList`. Owns the panel's search field — a borderless `NSTextField`
// inside `SearchHeaderView` (this controller is the field's delegate;
// every keystroke flows through to `entries.setQuery(...)` and then
// triggers a `tableView.reloadData()`).
//
// Interaction model (Spotlight-style):
//   - The search field stays first responder the whole time. Typing
//     filters the list; arrow keys move the table's selection without
//     ever taking focus away from the field.
//   - Row 0 is auto-selected after every reload so the user can hit
//     Enter immediately on the most-likely match. A rounded pill
//     highlight makes the selected row visible (drawn by
//     `RoundedSelectionRowView`; see its note for why we draw it
//     ourselves rather than rely on the table style).
//   - Enter or a single click launches the highlighted app via
//     `NSWorkspace.open(_:)` and quits LoFi.
//   - Esc quits LoFi without launching anything.
//
// The view hierarchy is built programmatically rather than via a NIB so
// the project has no .xib artifact to keep in sync.
//
// Each row renders `[icon] name … [category]`:
//   - `NSImageView` (24×24), icon resolved via
//     `NSWorkspace.shared.icon(forFile: bundlePath)` where `bundlePath`
//     comes from `entries.icon(at:)`. macOS pushes the `.app` bundle
//     path into the Rust core's icon field; `NSWorkspace` then handles
//     the actual icon read.
//   - Name `NSTextField`, label color, system font.
//   - Flexible spacer (provided by the stack view's distribution +
//     hugging-priority math).
//   - Category `NSTextField`, `.secondaryLabelColor`, smaller font,
//     trailing-aligned.
//
// Lifetime note: `NSTableView.dataSource` and `.delegate` are weak
// references (and the search field's `NSTextField.delegate` is too).
// Whoever creates an
// `AppListController` must keep a strong reference for the table's
// lifetime, or the table silently stops asking for cell views and rows
// render blank. See `AppDelegate`.

import AppKit

private let kRowHeight: CGFloat = 36
private let kIconSize: CGFloat = 24
private let kCategoryFontSize: CGFloat = 11
// Leading/trailing inset for row content AND the search header, so the
// magnifier/icons and the text share one column. Sized so content clears
// the panel's rounded corners and sits inside the rounded selection pill
// (see `RoundedSelectionRowView`).
private let kCellHorizontalPadding: CGFloat = 16
private let kCellSpacing: CGFloat = 8
// SF Symbol point size for the search magnifier. Centered in the same
// `kIconSize`-wide column the list icons use (see `SearchHeaderView`) so
// the glyph lines up with the app icons below.
private let kSearchGlyphSize: CGFloat = 18
// SF Symbol point size for command rows (whose icon is an SF Symbol, not
// a bundle icon). Sized to read like the app icons in the same 24×24 box.
private let kCommandGlyphSize: CGFloat = 16
// Search input font size. Larger than the list rows' default body text
// for a Spotlight-like prompt.
private let kSearchFontSize: CGFloat = 22
// Top/bottom inset for the search header row.
private let kSearchRowVerticalInset: CGFloat = 6
// Rounded selection pill geometry. We draw selection ourselves (see
// `RoundedSelectionRowView`) because the `.inset` table style that draws
// it for free also adds a hidden horizontal content inset, which pushed
// the rows out of line with the search header. `.plain` + custom drawing
// keeps the pill look without that inset.
private let kSelectionHorizontalInset: CGFloat = 8
private let kSelectionVerticalInset: CGFloat = 2
private let kSelectionCornerRadius: CGFloat = 8

final class AppListController: NSObject, NSTableViewDataSource, NSTableViewDelegate,
    NSTextFieldDelegate
{
    private let entries: EntryList
    /// Optional persistent activation history. When non-nil, every
    /// `launchRow(_:)` records the activation via `bumpMru` before
    /// opening the bundle so the next launch reorders that entry to
    /// the top. Nil means MRU is disabled for this run (store-open
    /// failed in the delegate); the launcher still works, it just
    /// doesn't remember the activation.
    private let mruStore: MruStore?
    /// Macos-side companion data for Window entries, keyed by the
    /// same `CGWindowID` we pushed into Rust. Used at two points:
    /// `WindowActivation.raise(pid:title:)` reads `pid` + `title`;
    /// `tableView(_:viewFor:row:)` reads `appName` to render the
    /// Window-row label as `"Title — App"`. See `AppDelegate.windowAux`
    /// for why this lives Swift-side rather than crossing the FFI.
    private let windowAux: [UInt64: (pid: pid_t, title: String, appName: String)]
    /// The window-action command target captured at startup (frontmost
    /// non-LoFi window + its work area + current frame). `nil` when there
    /// was no usable target, in which case no command rows were pushed and
    /// the `"Command"` branch in `launchRow` is unreachable — it bails
    /// safely anyway. Carries the StandardSize fallback rect that
    /// `toggleMaximize` uses when no previous frame was saved.
    private let commandTarget: WindowCommands.CommandTarget?
    /// Persistent per-window pre-maximize frame store. Held for the
    /// process lifetime so `toggleMaximize`'s save (on maximize) and
    /// take (on un-maximize, possibly in a later run) hit the same backing
    /// store. See `SavedFrameStore`.
    private let savedFrameStore: SavedFrameStore
    private let tableView: NSTableView
    private let scrollView: NSScrollView

    private let searchHeader: SearchHeaderView

    /// The search header row (magnifier + borderless text field) shown
    /// above the list. Exposed so the panel can stack it.
    var searchView: NSView { searchHeader }

    /// The editable field inside `searchView`. Exposed so the panel can
    /// pin it as the initial first responder, and so the controller reads
    /// the typed query from it.
    var searchInput: NSTextField { searchHeader.textField }

    /// The scrolling list view to embed below the search field. A scroll
    /// view wrapping the table so long lists overflow cleanly.
    var listView: NSView { scrollView }

    init(
        entries: EntryList,
        mruStore: MruStore?,
        windowAux: [UInt64: (pid: pid_t, title: String, appName: String)],
        commandTarget: WindowCommands.CommandTarget?,
        savedFrameStore: SavedFrameStore
    ) {
        self.entries = entries
        self.mruStore = mruStore
        self.windowAux = windowAux
        self.commandTarget = commandTarget
        self.savedFrameStore = savedFrameStore

        // Non-zero initial size. NSScrollView does NOT auto-resize its
        // documentView, so without an explicit frame the table sits at
        // 0×0 inside the scroll view and no cells are ever drawn.
        let initialFrame = NSRect(x: 0, y: 0, width: 640, height: 400)
        let table = NSTableView(frame: initialFrame)
        table.headerView = nil
        table.allowsMultipleSelection = false
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.rowHeight = kRowHeight
        // Transparent table + scroll view so the panel's liquid-glass
        // background shows through the results, not just the search strip.
        table.backgroundColor = .clear
        // `.plain`, not the default `.automatic`/`.inset`, so rows carry no
        // hidden horizontal inset — row content then sits at exactly
        // `kCellHorizontalPadding`, lining up with the search header. The
        // rounded selection the inset style drew for free is reproduced by
        // `RoundedSelectionRowView`.
        table.style = .plain
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("entry"))
        column.resizingMask = .autoresizingMask
        column.width = initialFrame.width
        column.minWidth = 100
        table.addTableColumn(column)

        let scroll = NSScrollView(frame: initialFrame)
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        scroll.drawsBackground = false
        scroll.autoresizingMask = [.width, .height]

        let header = SearchHeaderView()

        self.tableView = table
        self.scrollView = scroll
        self.searchHeader = header

        super.init()

        header.textField.delegate = self
        table.dataSource = self
        table.delegate = self
        // Single-click on a row launches; double-click would do the
        // same so there's no need to wire `doubleAction` separately.
        table.target = self
        table.action = #selector(rowClicked)
        // Setting dataSource normally triggers a reload, but the table
        // isn't in a window yet at this point, so deferred reload
        // behavior varies. An explicit call here is cheap and removes
        // a class of "blank table" bugs.
        table.reloadData()
        selectFirstRowIfAny()
    }

    // MARK: - NSTableViewDataSource

    func numberOfRows(in tableView: NSTableView) -> Int {
        entries.count
    }

    // MARK: - NSTableViewDelegate

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        let bareName = entries.name(at: row) ?? ""
        let category = entries.category(at: row) ?? ""
        let iconPath = entries.icon(at: row)
        // Window rows: stitch the owning app's name into the visible
        // label as `Title — App` so that when the user searches by
        // app name (the matcher *does* match on `Window.app_name`),
        // each matched window is identifiable as belonging to that
        // app. Without this the rows read as bare titles ("Hacker
        // News" with category "Window") and the user can't tell from
        // the label which Chrome / Safari / Finder window they're
        // looking at.
        let name: String = {
            guard category == "Window" else { return bareName }
            let id = entries.windowId(at: row)
            guard let aux = windowAux[id], !aux.appName.isEmpty else {
                return bareName
            }
            return "\(bareName) — \(aux.appName)"
        }()
        // Command rows have no icon path (the Rust core returns null for
        // `get_icon` on Command entries), so we render an SF Symbol chosen
        // per command kind instead. Every other category keeps using the
        // bundle-path icon. See `commandSymbolName(for:)`.
        let symbolName: String? =
            (category == "Command")
            ? commandSymbolName(for: entries.commandId(at: row) ?? "")
            : nil
        return EntryRowView(
            name: name,
            category: category,
            iconPath: iconPath,
            symbolName: symbolName
        )
    }

    /// SF Symbol name for a window-action command row, keyed by the
    /// `CommandKind::as_id` string. Picked to communicate either the
    /// geometry shape (halves / center / inset rectangles) or the action
    /// (minimize / fullscreen). Unknown ids fall back to a generic window
    /// glyph so a future Rust-side kind still renders something rather than
    /// a blank icon column.
    private func commandSymbolName(for id: String) -> String {
        switch id {
        case "center": return "rectangle.center.inset.filled"
        case "center_half": return "rectangle.split.2x1"
        case "center_two_thirds": return "rectangle.split.3x1"
        case "left_half": return "rectangle.lefthalf.filled"
        case "right_half": return "rectangle.righthalf.filled"
        case "standard_size": return "rectangle.inset.filled"
        case "minimize": return "minus.rectangle"
        case "toggle_maximize": return "arrow.up.left.and.arrow.down.right"
        case "toggle_fullscreen": return "arrow.up.left.and.arrow.down.right.rectangle"
        // Move-to-display: directional arrows pointing to a line, the
        // SF Symbol idiom for "send to the next position." Reads
        // alongside the other rectangle-themed glyphs above without
        // being mistaken for half/two-thirds layouts.
        case "next_display": return "arrow.right.to.line"
        case "previous_display": return "arrow.left.to.line"
        default: return "macwindow"
        }
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        // Draw selection as a rounded inset pill ourselves; the `.plain`
        // style (chosen to drop the inset style's content offset) would
        // otherwise give a flat full-width highlight. See
        // `RoundedSelectionRowView`.
        RoundedSelectionRowView()
    }

    // MARK: - NSTextFieldDelegate / NSControlTextEditingDelegate

    func controlTextDidChange(_ notification: Notification) {
        // Every keystroke pushes the new query to Rust, then asks the
        // table to redraw. The Rust side recomputes the filter index in
        // `lofi_entries_set_query`; everything downstream (`count`,
        // `name(at:)`, ...) reads through that filter automatically.
        entries.setQuery(searchInput.stringValue)
        tableView.reloadData()
        // After a filter change the previously selected row is almost
        // never the one the user wants; default to the top match.
        selectFirstRowIfAny()
    }

    /// The search field is always first responder, so the four
    /// navigation/activation keys (↓ ↑ ⏎ ⎋) reach the field editor
    /// first. We intercept them here, dispatch to the table or app,
    /// and return `true` to keep the field editor from doing its own
    /// thing (which for ⎋ would be "revert text", and for ⏎ would be
    /// "commit text" — neither is what we want).
    func control(
        _ control: NSControl,
        textView: NSTextView,
        doCommandBy commandSelector: Selector
    ) -> Bool {
        switch commandSelector {
        case #selector(NSResponder.moveDown(_:)):
            moveSelection(by: +1)
            return true
        case #selector(NSResponder.moveUp(_:)):
            moveSelection(by: -1)
            return true
        case #selector(NSResponder.insertNewline(_:)):
            launchRow(tableView.selectedRow)
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            NSApp.terminate(nil)
            return true
        default:
            return false
        }
    }

    // MARK: - Selection + activation

    /// Click handler wired up in `init` via `table.action`. Single
    /// click on a row launches; clicks on empty space leave
    /// `clickedRow == -1` and fall through harmlessly.
    @objc private func rowClicked() {
        launchRow(tableView.clickedRow)
    }

    private func selectFirstRowIfAny() {
        guard entries.count > 0 else { return }
        tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        tableView.scrollRowToVisible(0)
    }

    private func moveSelection(by delta: Int) {
        let n = entries.count
        guard n > 0 else { return }
        // Treat "no selection" as one-before-the-first so ↓ goes to
        // row 0 and ↑ does nothing.
        let current = tableView.selectedRow
        let base = current < 0 ? -1 : current
        let next = max(0, min(n - 1, base + delta))
        guard next != current else { return }
        tableView.selectRowIndexes(IndexSet(integer: next), byExtendingSelection: false)
        tableView.scrollRowToVisible(next)
    }

    /// Activate the row at `row` and quit LoFi. Out-of-bounds rows
    /// (incl. the `-1` "no clicked row" sentinel) are silently ignored
    /// — the user's mouse landed somewhere that didn't resolve to an
    /// entry; bailing without any visible response is the right thing.
    ///
    /// Bumps the MRU store *before* dispatching: the bump is a
    /// microsecond local SQLite write; the activation paths
    /// (`NSWorkspace.open`, `WindowActivation.raise`) involve IPC plus
    /// our immediate `NSApp.terminate`. If we lose the race we prefer a
    /// double-bump (correctly attributed to "the user tried to activate
    /// this") over a miss-bump.
    ///
    /// Branches on the stable English category label from the FFI:
    ///   - `"Window"` routes through AX (raise the specific window by its
    ///     `CGWindowID`, with pid + title from `windowAux` for the AX lookup
    ///     and its title fallback).
    ///   - `"Command"` runs a window-action command against the captured
    ///     `commandTarget`: geometry kinds (a non-nil
    ///     `entries.commandGeometry(at:)`) call `WindowControl.move`;
    ///     state-toggle kinds dispatch by command id.
    ///   - every other category (currently only `"Application"`) opens the
    ///     `.app` bundle via LaunchServices.
    private func launchRow(_ row: Int) {
        guard row >= 0, row < entries.count else { return }
        if let store = mruStore {
            entries.bumpMru(store: store, at: row)
        }
        let category = entries.category(at: row)
        if category == "Window" {
            let id = entries.windowId(at: row)
            if let aux = windowAux[id] {
                _ = WindowActivation.raise(
                    pid: aux.pid,
                    windowId: CGWindowID(id),
                    title: aux.title
                )
            }
        } else if category == "Command" {
            runCommand(row)
        } else {
            // `entries.icon(at:)` returns the bundle path on macOS —
            // see the `DiscoveredApp.bundlePath` comment in
            // `AppDiscovery.swift` for why the icon field carries the
            // path identifier.
            guard let path = entries.icon(at: row) else { return }
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
        NSApp.terminate(nil)
    }

    /// Run the window-action command at `row` against `commandTarget`.
    /// Bails (no-op) when the command id can't be read or there is no
    /// target — both impossible in practice once a command row exists, but
    /// the launcher quits regardless via `launchRow`.
    ///
    /// Geometry kinds (`entries.commandGeometry(at:)` returns a rect) call
    /// `WindowControl.move` with the precomputed top-left-global rect from
    /// Rust's `compute_geometry`. State-toggle kinds (nil geometry)
    /// dispatch by id: minimize / toggle fullscreen / toggle maximize /
    /// next display / previous display.
    private func runCommand(_ row: Int) {
        guard let commandId = entries.commandId(at: row),
              let target = commandTarget
        else {
            return
        }
        if let geo = entries.commandGeometry(at: row) {
            _ = WindowControl.move(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId,
                x: geo.x,
                y: geo.y,
                width: geo.w,
                height: geo.h
            )
            return
        }
        switch commandId {
        case "minimize":
            _ = WindowControl.minimize(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId
            )
        case "toggle_fullscreen":
            _ = WindowControl.toggleFullscreen(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId
            )
        case "toggle_maximize":
            _ = WindowControl.toggleMaximize(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId,
                workArea: target.workArea,
                fallbackRect: target.standardRect,
                store: savedFrameStore
            )
        case "next_display":
            _ = WindowControl.moveToDisplay(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId,
                direction: +1
            )
        case "previous_display":
            _ = WindowControl.moveToDisplay(
                pid: target.pid,
                title: target.title,
                windowId: target.windowId,
                direction: -1
            )
        default:
            // An unrecognized state-toggle id (a Rust-side kind we don't
            // know how to dispatch yet). Do nothing rather than guess.
            break
        }
    }
}

/// Single row view: `[icon] name <flexible spacer> [category]`. A plain
/// `NSStackView` carries the layout; the spacer comes from the name
/// field's low horizontal content-hugging priority. The category field
/// hugs at high priority so it never gets stretched.
private final class EntryRowView: NSView {
    /// `iconPath` (the app/window bundle path) takes precedence: if set,
    /// the icon is read via `NSWorkspace.shared.icon(forFile:)`.
    /// `symbolName` is the SF Symbol fallback for command rows, which have
    /// no bundle icon — rendered as a template glyph tinted like the dimmed
    /// category labels. Both nil ⇒ an empty (but still 24×24) icon box so
    /// the name column stays aligned with the other rows.
    init(name: String, category: String, iconPath: String?, symbolName: String?) {
        super.init(frame: .zero)

        let imageView = NSImageView()
        imageView.imageScaling = .scaleProportionallyDown
        if let path = iconPath {
            imageView.image = NSWorkspace.shared.icon(forFile: path)
        } else if let symbolName = symbolName {
            imageView.image = NSImage(
                systemSymbolName: symbolName,
                accessibilityDescription: nil
            )
            imageView.symbolConfiguration = NSImage.SymbolConfiguration(
                pointSize: kCommandGlyphSize,
                weight: .regular
            )
            // Template SF Symbol; tint it like the dimmed category labels
            // and the search magnifier so command rows read as actions
            // rather than launchable apps.
            imageView.contentTintColor = .secondaryLabelColor
        }
        imageView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            imageView.widthAnchor.constraint(equalToConstant: kIconSize),
            imageView.heightAnchor.constraint(equalToConstant: kIconSize),
        ])

        let nameField = NSTextField(labelWithString: name)
        nameField.isBezeled = false
        nameField.drawsBackground = false
        nameField.isEditable = false
        nameField.isSelectable = false
        nameField.textColor = .labelColor
        nameField.lineBreakMode = .byTruncatingTail
        nameField.translatesAutoresizingMaskIntoConstraints = false
        // Let the name field absorb any extra horizontal space so the
        // category sits flush against the trailing edge.
        nameField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        nameField.setContentCompressionResistancePriority(
            .defaultLow,
            for: .horizontal
        )

        let categoryField = NSTextField(labelWithString: category)
        categoryField.isBezeled = false
        categoryField.drawsBackground = false
        categoryField.isEditable = false
        categoryField.isSelectable = false
        categoryField.textColor = .secondaryLabelColor
        categoryField.font = NSFont.systemFont(ofSize: kCategoryFontSize)
        categoryField.alignment = .right
        categoryField.translatesAutoresizingMaskIntoConstraints = false
        categoryField.setContentHuggingPriority(.required, for: .horizontal)
        categoryField.setContentCompressionResistancePriority(
            .required,
            for: .horizontal
        )

        let stack = NSStackView(views: [imageView, nameField, categoryField])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.distribution = .fill
        stack.spacing = kCellSpacing
        stack.edgeInsets = NSEdgeInsets(
            top: 0,
            left: kCellHorizontalPadding,
            bottom: 0,
            right: kCellHorizontalPadding
        )
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("EntryRowView is not loadable from a NIB / coder")
    }
}

/// The search input, rendered as a list-style header row: a magnifier in
/// the same icon column the list rows use, then a borderless text field
/// where the row name labels begin.
///
/// Why not `NSSearchField`: its cell positions the magnifier and text via
/// `searchButtonRect`/`searchTextRect`, but stripping the bezel (which we
/// need for the borderless look) makes the cell fall back to plain
/// text-field drawing and ignore those rects — the glyph and text then
/// collide at the left edge. Composing the row from the same
/// `[icon] text` stack as `EntryRowView` (identical insets and spacing)
/// is what guarantees the magnifier and typed text line up with the app
/// icons and names below.
private final class SearchHeaderView: NSView {
    let textField: NSTextField

    init() {
        let magnifier = NSImageView()
        magnifier.image = NSImage(
            systemSymbolName: "magnifyingglass",
            accessibilityDescription: "Search"
        )
        magnifier.symbolConfiguration = NSImage.SymbolConfiguration(
            pointSize: kSearchGlyphSize,
            weight: .regular
        )
        // Template SF Symbol; tint it like the dimmed category labels.
        magnifier.contentTintColor = .secondaryLabelColor
        magnifier.imageScaling = .scaleProportionallyDown
        magnifier.translatesAutoresizingMaskIntoConstraints = false
        // Same 24×24 box as the list icons so the glyph shares their column.
        NSLayoutConstraint.activate([
            magnifier.widthAnchor.constraint(equalToConstant: kIconSize),
            magnifier.heightAnchor.constraint(equalToConstant: kIconSize),
        ])

        let field = NSTextField()
        field.placeholderString = "Search"
        // Borderless: no bezel, border, background, or focus ring so the
        // field reads as plain text on the panel's glass.
        field.isBezeled = false
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = NSFont.systemFont(ofSize: kSearchFontSize)
        field.usesSingleLineMode = true
        field.lineBreakMode = .byTruncatingTail
        field.cell?.isScrollable = true
        field.translatesAutoresizingMaskIntoConstraints = false
        // Let the field absorb the horizontal slack like the row name field.
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        self.textField = field

        super.init(frame: .zero)

        let stack = NSStackView(views: [magnifier, field])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.distribution = .fill
        stack.spacing = kCellSpacing
        // Identical to EntryRowView's insets so the columns line up.
        stack.edgeInsets = NSEdgeInsets(
            top: kSearchRowVerticalInset,
            left: kCellHorizontalPadding,
            bottom: kSearchRowVerticalInset,
            right: kCellHorizontalPadding
        )
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("SearchHeaderView is not loadable from a NIB / coder")
    }
}

/// Draws the row's selection as a rounded, inset pill — the look the
/// `.inset` table style gives for free, reimplemented so the table can
/// run in `.plain` style instead. `.plain` is what removes that style's
/// hidden horizontal content inset, which had pushed the row icons/text
/// to the right of the search header and broke their alignment.
private final class RoundedSelectionRowView: NSTableRowView {
    override func drawSelection(in dirtyRect: NSRect) {
        guard isSelected else { return }
        let rect = bounds.insetBy(
            dx: kSelectionHorizontalInset,
            dy: kSelectionVerticalInset
        )
        let path = NSBezierPath(
            roundedRect: rect,
            xRadius: kSelectionCornerRadius,
            yRadius: kSelectionCornerRadius
        )
        // The table is never first responder (the search field keeps
        // focus), so selection is always the dimmed/unemphasized variant.
        NSColor.unemphasizedSelectedContentBackgroundColor.setFill()
        path.fill()
    }
}
