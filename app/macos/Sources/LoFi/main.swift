// Process entry point.
//
// Standard NSApplication boot: instantiate the shared application, attach
// the LoFi delegate, then run the main event loop. The delegate is the
// piece that does anything interesting — see `AppDelegate.swift`.

import AppKit

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
