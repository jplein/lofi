// `NSTableView` data source + delegate backed by a Rust-owned
// `EntryList`. Single column, headers hidden — this is a launcher list,
// not a spreadsheet.
//
// The view hierarchy is built programmatically rather than via a NIB so
// the project has no .xib artifact to keep in sync.

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

        let table = NSTableView()
        table.headerView = nil
        table.allowsMultipleSelection = false
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.rowHeight = 28
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        // The panel resizes its content view to fill, so the scroll
        // view follows.
        scroll.autoresizingMask = [.width, .height]

        self.tableView = table
        self.scrollView = scroll

        super.init()

        table.dataSource = self
        table.delegate = self
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        entries.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cellId = NSUserInterfaceItemIdentifier("name-cell")
        let cell: NSTableCellView
        if let recycled = tableView.makeView(withIdentifier: cellId, owner: self) as? NSTableCellView {
            cell = recycled
        } else {
            cell = NSTableCellView()
            cell.identifier = cellId
            let text = NSTextField(labelWithString: "")
            text.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(text)
            cell.textField = text
            NSLayoutConstraint.activate([
                text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
                text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -12),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        cell.textField?.stringValue = entries.name(at: row) ?? ""
        return cell
    }
}
