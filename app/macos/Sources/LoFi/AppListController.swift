// `NSTableView` data source + delegate backed by a Rust-owned
// `EntryList`. Owns the panel's `NSSearchField` (this controller is the
// field's delegate; every keystroke flows through to
// `entries.setQuery(...)` and then triggers a `tableView.reloadData()`).
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
        // Setting dataSource normally triggers a reload, but the table
        // isn't in a window yet at this point, so deferred reload
        // behavior varies. An explicit call here is cheap and removes
        // a class of "blank table" bugs.
        table.reloadData()
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
