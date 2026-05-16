# app

The LoFi launcher application, written in Rust.

Code in this directory (outside of `gnome/` and the future `macos/`) is shared between platforms: the core data model, fuzzy matching, configuration loading, and anything else that doesn't depend on a specific window system or desktop environment.

## Layout

- `core/` — platform-agnostic shared crate (`lofi-core`). Holds the cross-platform data model (`Application`, `Entry`, `EntryKind`, `EntryRef`), the `resolve` helper that pairs persisted references back to live entries, and `matcher::search` (Skim-style fuzzy ranking over `&[Entry]`). See `core/README.md` for the split between runtime and persistence types. No GTK/gio or other platform dependencies.
- `gnome/` — Linux/GNOME-specific code: the GTK4 + libadwaita launcher window (`ui`), `.desktop` enumeration (`apps`), activation via `gio_unix::DesktopAppInfo` (`launch`), and D-Bus clients for `org.gnome.Shell.Introspect` and the LoFi GNOME extension.
- `macos/` — macOS-specific code (planned, not yet present). The macOS UI will be Swift on top of a Rust core exposed via a C ABI.

## Shared concerns

The shared layer defines the uniform item type that the platform layers populate and the UI renders:

- Applications (launchable desktop entries / `.app` bundles)
- Open windows
- Workspaces
- Commands (power management, lock screen, arbitrary user-defined commands)

Each platform implementation gathers these into the shared type so the presentation and matching logic stays platform-agnostic.
