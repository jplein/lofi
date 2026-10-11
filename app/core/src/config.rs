//! The macOS launcher's configuration file:
//! `~/Library/Preferences/LoFi/config.toml`.
//!
//! ## Why this lives in `core` and not in `app/macos`
//!
//! The file is read by the Swift frontend's process, but parsed here, because
//! Swift has no TOML parser of its own and pulling one in would mean a Swift
//! Package dependency threaded through Bazel for a file that holds one
//! integer. The Rust core already has `serde`, already does the launcher's
//! other file I/O (`mru`), and already has the C-ABI plumbing to hand values
//! back to Swift — so the parse is a dozen lines here and one FFI call on the
//! other side. The *path* is still chosen by Swift (`Config.defaultPath()` in
//! `app/macos/Sources/LoFi/Config.swift`), the same split as the MRU store:
//! the platform layer knows where its files live, the core knows what is in
//! them.
//!
//! ## Why it is a separate schema from the Linux file
//!
//! The Linux launcher has its own configuration file (`app/linux/src/config.rs`,
//! `~/.config/lofi/config.toml`) and the two deliberately do **not** share a
//! struct. Their keys are disjoint: the Linux file carries a lock command and
//! the border/corner/shadow the launcher has to draw for itself on a
//! `wlr-layer-shell` surface — values typed as GTK colours and consumed by a
//! GTK stylesheet — none of which means anything on macOS, where the `NSPanel`
//! gets its chrome from AppKit. A union schema would have every key documented
//! as "ignored on the other platform", and `deny_unknown_fields` would stop
//! catching a Linux key pasted into the macOS file. If a key ever becomes
//! genuinely shared, the shared part moves here and both files embed it.
//!
//! ## Why a missing file is written, not just tolerated
//!
//! The Linux launcher treats an absent file as the silent common case and
//! never creates one. The macOS launcher instead writes [`DEFAULT_FILE`] the
//! first time it looks and finds nothing there. The difference is
//! discoverability: on Linux the file is documented next to the home-manager
//! module that usually generates it, and its keys are things a Niri user
//! already has numbers for. On macOS nothing generates the file and nothing
//! else teaches the schema, so a user who opens `~/Library/Preferences/LoFi/`
//! should find a file with every key, its default, and a comment saying what
//! it does — the file is its own documentation. The template therefore
//! **must** parse to `Config::default()`; a test pins that.
//!
//! The write is `create_new`, never a truncating `create`: the decision to
//! write was made from a `NotFound` a moment earlier, and a file that has
//! appeared in between (another summon, the user's editor) is the user's and
//! must not be clobbered. On `AlreadyExists` the loader simply reads it.
//!
//! ## Error policy
//!
//! Same as the Linux file and the rest of the crate: an unreadable or
//! malformed file logs to stderr and yields the defaults, and so does a
//! missing file that could not be created (read-only home, missing
//! permissions). A launcher that refused to come up over a typo in a layout
//! preference — or over not being able to write a file it does not strictly
//! need — would be a much worse outcome than one that comes up without the
//! preference.

use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;

/// The whole macOS configuration file. Every key is optional; the `Default`
/// impl is what an absent file means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Space to leave between a window placed by a window-action command and
    /// the edges of the work area, and between windows placed side by side,
    /// in points. `0` — the default — tiles windows edge to edge, which is what
    /// the commands have always done. The placement rules live on
    /// `crate::compute_geometry_with_gaps`.
    ///
    /// Unsigned so that a negative value is a parse error rather than a window
    /// pushed off its screen.
    #[serde(default)]
    pub gaps: u32,
}

/// What [`load_or_create`] writes when there is no file. Every key appears
/// with its default and a comment, so the file teaches its own schema; see
/// the module docs. Kept in step with `Config::default()` by
/// `tests::the_default_file_parses_to_the_defaults`.
pub const DEFAULT_FILE: &str = "\
# LoFi configuration. Every key is optional; a missing key is its default.
# Edits take effect on the next summon, no relaunch needed.

# Space to leave between a window placed by a window-action command and the
# edges of the work area, in points. 0 tiles windows edge to edge.
gaps = 0
";

/// Parse a configuration file's text.
///
/// Pure and separate from [`load_from`] so the interesting half is
/// unit-testable without touching the filesystem — the same split as
/// `lofi_linux::config::parse`.
pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

/// Read and parse the configuration file at `path`, writing [`DEFAULT_FILE`]
/// there first if nothing exists yet.
///
/// This is what the launcher calls. The result is always a `Config`: a file
/// that could not be created logs and yields the defaults, exactly as a file
/// that could not be read does — the launcher does not need the file to run,
/// it only needs it to exist for the user's sake.
pub fn load_or_create(path: &Path) -> Config {
    if !path.exists() {
        match create_default(path) {
            Ok(()) => return Config::default(),
            // Lost a race with another writer: whatever is there now is the
            // user's; fall through and read it.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                eprintln!("config: {} could not be created: {e}", path.display());
                return Config::default();
            }
        }
    }
    load_from(path)
}

/// Write [`DEFAULT_FILE`] to `path`, creating missing parent directories.
///
/// Refuses (`AlreadyExists`) rather than truncates if a file is already there:
/// the caller decided to write from a `NotFound` a moment ago, and anything
/// that has appeared since is the user's.
pub fn create_default(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    io::Write::write_all(&mut file, DEFAULT_FILE.as_bytes())
}

/// Read and parse the configuration file at `path`.
///
/// A missing file yields the defaults **silently** — [`load_or_create`] is
/// the layer that decides whether to do something about that. Anything else
/// (unreadable, malformed) logs to stderr and yields the defaults, so a
/// broken file costs the user their preference rather than their launcher.
pub fn load_from(path: &Path) -> Config {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("config: {} could not be read: {e}", path.display());
            return Config::default();
        }
    };

    match parse(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("config: {} is not valid: {e}", path.display());
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        // The common case is an absent file, but an empty or comment-only one
        // must land in the same place rather than erroring.
        let config = parse("# nothing here\n").expect("an empty file should parse");
        assert_eq!(config, Config::default());
        assert_eq!(
            config.gaps, 0,
            "the default gap is zero: edge-to-edge tiling"
        );
    }

    #[test]
    fn gaps_is_read() {
        let config = parse("gaps = 8\n").expect("the documented example should parse");
        assert_eq!(config.gaps, 8);
    }

    #[test]
    fn a_negative_gap_is_an_error_not_a_wrap() {
        // `u32` is the whole guard: serde reports the sign, nothing wraps.
        let err = parse("gaps = -4\n").expect_err("a negative gap should be reported");
        assert!(
            err.to_string().contains("gaps"),
            "the error should name the offending key, got: {err}"
        );
    }

    #[test]
    fn a_fractional_gap_is_an_error() {
        // Points are integral in every API the value will reach (AX frames are
        // `CGFloat` but the geometry math is integer `compute_geometry`), so a
        // float is a typo to report rather than a value to truncate silently.
        assert!(parse("gaps = 2.5\n").is_err(), "a float is not a gap");
    }

    #[test]
    fn an_unknown_key_is_an_error_not_a_shrug() {
        // `deny_unknown_fields` is deliberate, and it is also what keeps a
        // Linux key pasted into this file from being silently ignored.
        let err = parse("gap = 8\n").expect_err("a misspelled key should be reported");
        assert!(
            err.to_string().contains("gap"),
            "the error should name the offending key, got: {err}"
        );
        assert!(
            parse("lock-command = \"swaylock\"\n").is_err(),
            "a Linux-only key does not belong in the macOS file"
        );
    }

    #[test]
    fn the_default_file_parses_to_the_defaults() {
        // The whole point of writing a template is that it documents the
        // schema; a template that parsed to anything but the defaults would
        // change the launcher's behaviour just by being looked at.
        let config = parse(DEFAULT_FILE).expect("the template must be valid TOML");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_missing_file_is_created_with_the_template_and_parents() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Two levels deep: `~/Library/Preferences/LoFi/` does not exist on a
        // fresh machine either, so the parent must be created too.
        let path = dir.path().join("LoFi").join("config.toml");

        assert_eq!(load_or_create(&path), Config::default());
        assert_eq!(
            fs::read_to_string(&path).expect("the file should now exist"),
            DEFAULT_FILE,
            "the first load writes the template verbatim"
        );
    }

    #[test]
    fn an_existing_file_is_read_not_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "gaps = 12\n").expect("fixture should be writable");

        assert_eq!(load_or_create(&path).gaps, 12);
        assert_eq!(
            fs::read_to_string(&path).expect("the file should still exist"),
            "gaps = 12\n",
            "the user's file must come back byte for byte"
        );
    }

    #[test]
    fn create_default_refuses_to_clobber() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "gaps = 3\n").expect("fixture should be writable");

        let err = create_default(&path).expect_err("an existing file must not be truncated");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(&path).expect("the file should still exist"),
            "gaps = 3\n"
        );
    }

    #[test]
    fn an_uncreatable_file_degrades_to_the_defaults() {
        // `/dev/null` is a character device, so nothing can be created
        // beneath it. The loader must log and carry on, not panic.
        assert_eq!(
            load_or_create(Path::new("/dev/null/LoFi/config.toml")),
            Config::default()
        );
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            load_from(&dir.path().join("does-not-exist.toml")),
            Config::default(),
            "an absent config file is the common case, not a failure"
        );
    }

    #[test]
    fn a_malformed_file_degrades_to_the_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "this is not [ toml").expect("fixture should be writable");
        assert_eq!(
            load_from(&path),
            Config::default(),
            "a broken file should cost the user their preference, not their launcher"
        );
    }

    #[test]
    fn a_file_on_disk_is_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "gaps = 12\n").expect("fixture should be writable");
        assert_eq!(load_from(&path).gaps, 12);
    }
}
