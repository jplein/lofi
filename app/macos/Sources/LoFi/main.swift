// Process entry point.
//
// Standard NSApplication boot: instantiate the shared application, attach
// the LoFi delegate, then run the main event loop. The delegate is the
// piece that does anything interesting — see `AppDelegate.swift`.

import AppKit

// Build stamp — no-op marker whose only purpose is to force a fresh binary
// (and thus a new code-signing cdhash) on each build. We are tracking down an
// occasional "LoFi was prevented from modifying apps on your Mac" (App
// Management / kTCCServiceSystemPolicyAppBundles) notification that appears to
// fire the first time the app is launched after a rebuild — i.e. when the
// cdhash changes and macOS re-evaluates the TCC grant. Bump this UUID to
// guarantee a different binary hash when re-testing the trigger. The log line
// makes the running build identifiable in `log stream`/Console.
let buildStamp = "99FEE4FD-650C-47C1-BD32-9D42A126FA11"
NSLog("LoFi: build stamp \(buildStamp)")

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
