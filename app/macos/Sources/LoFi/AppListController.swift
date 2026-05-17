// `NSTableView` data source + delegate backed by a Rust-owned
// `EntryList`. Owns the panel's `NSSearchField` (this controller is the
// field's delegate; every keystroke flows through to
// `entries.setQuery(...)` and then triggers a `tableView.reloadData()`).
//
// Interaction model (Spotlight-style):
//   - The search field stays first responder the whole time. Typing
//     filters the list; arrow keys move the table's selection without
//     ever taking focus away from the field.
//   - Row 0 is auto-selected after every reload so the user can hit
//     Enter immediately on the most-likely match. The system blue
//     highlight makes the selected row visible (NSTableRowView's
//     default behavior; we don't draw it ourselves).
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
// references (and `NSSearchField.delegate` is too). Whoever creates an
// `AppListController` must keep a strong reference for the table's
// lifetime, or the table silently stops asking for cell views and rows
// render blank. See `AppDelegate`.

import AppKit

private let kRowHeight: CGFloat = 36
private let kIconSize: CGFloat = 24
private let kCategoryFontSize: CGFloat = 11
private let kCellHorizontalPadding: CGFloat = 8
private let kCellSpacing: CGFloat = 8

final class AppListController: NSObject, NSTableViewDataSource, NSTableViewDelegate,
    NSSearchFieldDelegate
{
    private let entries: EntryList
    private let tableView: NSTableView
    private let scrollView: NSScrollView

    /// The search field shown above the list. Owned here so the
    /// controller can keep itself as the delegate; exposed to the panel
    /// so it can pin it as the initial first responder.
    let searchField: NSSearchField

    /// The scrolling list view to embed below the search field. A scroll
    /// view wrapping the table so long lists overflow cleanly.
    var listView: NSView { scrollView }

    init(entries: EntryList) {
        self.entries = entries

        // Non-zero initial size. NSScrollView does NOT auto-resize its
        // documentView, so without an explicit frame the table sits at
        // 0×0 inside the scroll view and no cells are ever drawn.
        let initialFrame = NSRect(x: 0, y: 0, width: 640, height: 400)
        let table = NSTableView(frame: initialFrame)
        table.headerView = nil
        table.allowsMultipleSelection = false
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.rowHeight = kRowHeight
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
        scroll.autoresizingMask = [.width, .height]

        let field = NSSearchField()
        field.placeholderString = "Search"

        self.tableView = table
        self.scrollView = scroll
        self.searchField = field

        super.init()

        field.delegate = self
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
        let name = entries.name(at: row) ?? ""
        let category = entries.category(at: row) ?? ""
        let iconPath = entries.icon(at: row)
        return EntryRowView(name: name, category: category, iconPath: iconPath)
    }

    // MARK: - NSSearchFieldDelegate / NSControlTextDelegate

    func controlTextDidChange(_ notification: Notification) {
        // Every keystroke pushes the new query to Rust, then asks the
        // table to redraw. The Rust side recomputes the filter index in
        // `lofi_entries_set_query`; everything downstream (`count`,
        // `name(at:)`, ...) reads through that filter automatically.
        entries.setQuery(searchField.stringValue)
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

    /// Open the `.app` at the given filtered row and quit LoFi. Out-of-
    /// bounds rows (incl. the `-1` "no clicked row" sentinel) are
    /// silently ignored — the user's mouse landed somewhere that didn't
    /// resolve to an entry; bailing without any visible response is
    /// the right thing.
    private func launchRow(_ row: Int) {
        guard row >= 0, row < entries.count else { return }
        // `entries.icon(at:)` returns the bundle path on macOS — see
        // the `DiscoveredApp.bundlePath` comment in `AppDiscovery.swift`
        // for why the icon field carries the path identifier.
        guard let path = entries.icon(at: row) else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
        NSApp.terminate(nil)
    }
}

/// Single row view: `[icon] name <flexible spacer> [category]`. A plain
/// `NSStackView` carries the layout; the spacer comes from the name
/// field's low horizontal content-hugging priority. The category field
/// hugs at high priority so it never gets stretched.
private final class EntryRowView: NSView {
    init(name: String, category: String, iconPath: String?) {
        super.init(frame: .zero)

        let imageView = NSImageView()
        imageView.imageScaling = .scaleProportionallyDown
        if let path = iconPath {
            imageView.image = NSWorkspace.shared.icon(forFile: path)
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
