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

- macOS Tahoe (15+)
- Xcode 26 (for the Swift toolchain)
- Nix + direnv (provides Bazel and the Rust toolchain via the flake)

## Install: macOS

```sh
bazelisk run //app/macos:install
open ~/Applications/LoFi.app
```
