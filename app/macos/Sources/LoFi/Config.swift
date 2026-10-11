// The user's configuration file: `~/Library/Preferences/LoFi/config.toml`.
//
// Swift owns the *path* and Rust owns the *parse* — the same split as the
// MRU store (`MruStore.defaultPath()` + `lofi_mru_open`). Swift has no TOML
// parser, and threading a Swift Package dependency through Bazel for a file
// that holds one integer is not worth it when `lofi-core` already has
// `serde`, already reads the launcher's other files, and already has the
// C-ABI plumbing. See `app/core/src/config.rs` for the schema and the
// error policy.
//
// WHY ~/Library/Preferences and not Application Support
// -----------------------------------------------------
// The MRU store lives under `~/Library/Application Support/dev.jplein.lofi`
// because it is *state* LoFi writes for itself. The config file is a
// *preference* the user writes for LoFi, and `~/Library/Preferences` is
// where a Mac user goes looking for one. It is a hand-edited text file, not
// a `.plist`, so it does not go through `UserDefaults`/`cfprefsd` — those
// only manage property lists, and a TOML file beside them is inert to
// them. The directory is spelled `LoFi`, not the bundle id: the user types
// this path, and the product name is what they know.
//
// WHY re-read on every summon
// ---------------------------
// LoFi is a resident daemon, so a one-shot read at launch would mean an
// edit takes effect only after Cmd-Q + relaunch. The Linux launcher is a
// process per invocation and so picks up edits on the next Alt+Space for
// free; reading the file in `summonPanel` gives macOS the same behaviour.
// The file is tiny and the read is one `stat` + one small `read`, well
// under the ~50ms the `.app` enumeration already costs per summon.

import Foundation
import LoFiCore

/// The values from `config.toml`, with the defaults filled in for anything
/// the file does not say (or for no file at all).
struct Config {
    /// Space to leave between a window placed by a window-action command
    /// and the work-area edges, in points. `0` tiles edge to edge.
    ///
    /// Read and carried through to `WindowCommands.CommandTarget`; the
    /// geometry commands do not honour it yet.
    let gaps: Int

    /// What an absent file means. Kept in step with
    /// `lofi_core::config::Config::default()`.
    static let defaults = Config(gaps: 0)

    /// Read the file at `path`, creating it with the commented defaults if
    /// it does not exist yet. Never fails: `lofi_config_load` returns the
    /// defaults for a file it could not create, read, or parse (logging
    /// why to stderr), so there is always a `Config`.
    static func load(path: String = defaultPath()) -> Config {
        let raw = path.withCString { lofi_config_load($0) }
        return Config(gaps: Int(raw.gaps))
    }

    /// Canonical macOS path for the configuration file:
    /// `~/Library/Preferences/LoFi/config.toml`.
    ///
    /// Unlike `MruStore.defaultPath()` this does not create the directory
    /// itself: the Rust side creates both the directory and the file on the
    /// first load (`config::load_or_create`), so there is never an empty
    /// `LoFi` folder without a file in it. `NSHomeDirectory()`
    /// rather than the `.libraryDirectory` search path on purpose — the
    /// documented location is the literal `~/Library/Preferences`, and the
    /// search-path API can answer differently for a sandboxed container
    /// (LoFi is not sandboxed today, but the path should not silently move
    /// if that changes).
    static func defaultPath() -> String {
        URL(fileURLWithPath: NSHomeDirectory())
            .appendingPathComponent("Library/Preferences/LoFi/config.toml")
            .path
    }
}
