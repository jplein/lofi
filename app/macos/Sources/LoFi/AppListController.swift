// `NSTableView` data source + delegate backed by a Rust-owned
// `EntryList`. Single column, headers hidden — this is a launcher list,
// not a spreadsheet.
//
// The view hierarchy is built programmatically rather than via a NIB so
// the project has no .xib artifact to keep in sync.
//
// Lifetime note: `NSTableView.dataSource` and `.delegate` are weak
// references. Whoever creates an `AppListController` must keep a strong
// reference for the table's lifetime, or the table silently stops
// asking for cell views and rows render blank. See `AppDelegate`.

import AppKit

final class AppListController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let entries: EntryList
    private let tableView: NSTableView
    private let scrollView: NSScrollView

    /// The view to embed in the panel's `contentView`. A scroll view
    /// wrapping the table so long lists overflow cleanly.
    var view: NSView { scrollView }

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
        table.rowHeight = 28
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
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

        self.tableView = table
        self.scrollView = scroll

        super.init()

        table.dataSource = self
        table.delegate = self
        // Setting dataSource normally triggers a reload, but the table
        // isn't in a window yet at this point, so deferred reload
        // behavior varies. An explicit call here is cheap and removes
        // a class of "blank table" bugs.
        table.reloadData()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        entries.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let text = NSTextField(labelWithString: entries.name(at: row) ?? "")
        text.isBezeled = false
        text.drawsBackground = false
        text.isEditable = false
        text.isSelectable = false
        return text
    }
}
