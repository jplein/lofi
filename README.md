# LoFi

LoFi is a small launcher for GNOME and macOS (planned).

## Goals

- Fast: LoFi should launch and display its results instantly
- Predictable: Typing the same input should find the same target, each time

## Feature set

LoFi is limited in what it can do. It can't search for or within files, it can't connect to web applications: these operations can take a long time, so it doesn't try to do them.

What it can do:

- Launch applications
- Window management and navigation:
    - Switch focus to an open window
    - Switch to another workspace
    - Operations on the active window:
        - Resize
        - Move to another workspace
        - Close
- Anything that can be defined as a command:
    - Power management
    - Locking the screen

## System requirements: Linux

- NixOS
- GNOME

## System requirements: macOS

(Experimental)

- macOS Tahoe (15+)

The macOS frontend at `app/macos/` is implemented but unverified end-to-end: the build pipeline (cargo + xcodegen + xcodebuild) and the Swift sources are in place, the Rust FFI integration tests pass, but the Xcode build itself has not yet been run on a Mac. When it runs, the `.app` floats an `NSPanel` listing every `.app` bundle under `/Applications` and `~/Applications`. It does not yet support search, MRU, launching, icons, or a global hotkey — see `app/macos/README.md` for the slice-by-slice rollout plan.
