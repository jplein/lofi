//! Power commands under Niri.
//!
//! Niri is a compositor, not a desktop environment: there is no session
//! manager and no screensaver service, so none of the GNOME routing in
//! `backend::gnome::power` applies. What is left is systemd-logind (present in
//! any systemd session, and already the GNOME backend's Suspend path) plus
//! Niri's own IPC.
//!
//! - **Lock** — spawns a locker. See [`lock`] for the chain and why this one
//!   command can't be a D-Bus call.
//! - **Log Out** — Niri's `Quit` action, with `skip_confirmation: false` so
//!   Niri raises its own "press Enter to confirm" prompt. Same reasoning as
//!   GNOME's `Logout(mode=0)`: a power command is far heavier than the average
//!   launcher row and deserves a confirmation step.
//! - **Suspend / Restart / Shutdown** — logind directly. Unlike GNOME there is
//!   no session manager to interpose a confirmation dialog, so these fire
//!   immediately.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use lofi_core::{PowerCommand, PowerCommandKind};

use super::ipc;
use crate::backend::logind;

/// Full set of power-command kinds. Niri can perform all five, so this
/// mirrors the GNOME backend's list exactly.
const ALL_KINDS: &[PowerCommandKind] = &[
    PowerCommandKind::LockSession,
    PowerCommandKind::Logout,
    PowerCommandKind::Suspend,
    PowerCommandKind::Restart,
    PowerCommandKind::Shutdown,
];

/// Environment variable that overrides the lock command. Read as a shell
/// command line rather than an argv so a user can write
/// `LOFI_LOCK_COMMAND='swaylock -f -c 000000'` without LoFi having to invent a
/// quoting convention. The config file's `lock-command` is the same string in
/// a more durable place; see [`lock`] for why the variable still outranks it.
pub const LOCK_COMMAND_ENV: &str = "LOFI_LOCK_COMMAND";

/// Lockers tried, in order, when neither `$LOFI_LOCK_COMMAND` nor the config
/// file names one.
///
/// Order is by how commonly they appear alongside Niri rather than by any
/// judgement about the programs themselves. All are `ext-session-lock-v1`
/// clients, which is the protocol Niri implements.
const LOCKER_CANDIDATES: &[&str] = &["swaylock", "hyprlock", "waylock", "gtklock"];

/// Static set of power commands. Always returned in full — like GNOME's, they
/// don't depend on the focused window or any runtime state.
pub fn gather_power_commands() -> Vec<PowerCommand> {
    ALL_KINDS
        .iter()
        .map(|&kind| PowerCommand { kind })
        .collect()
}

/// Dispatch a power command. Logs and returns on failure; never panics —
/// same `eprintln!`-and-degrade policy as everywhere else, and for the same
/// reason: the launcher window has already closed, so there is no UI surface
/// to report through.
///
/// `configured` is the config file's `lock-command`, threaded down from
/// `NiriBackend`. It is only consulted by [`lock`]; every other kind ignores
/// it.
pub fn activate(kind: PowerCommandKind, configured: Option<&str>) {
    let result = match kind {
        PowerCommandKind::LockSession => lock(configured),
        PowerCommandKind::Logout => logout(),
        PowerCommandKind::Suspend => logind::suspend().map_err(|e| e.to_string()),
        PowerCommandKind::Restart => logind::reboot().map_err(|e| e.to_string()),
        PowerCommandKind::Shutdown => logind::power_off().map_err(|e| e.to_string()),
    };
    if let Err(e) = result {
        eprintln!("power: {kind:?} failed: {e}");
    }
}

/// Lock the screen.
///
/// This is the one power command that can't be a service call. Under GNOME,
/// `org.gnome.ScreenSaver.Lock` works because GNOME Shell *is* the lock
/// screen. Niri implements `ext-session-lock-v1` but ships no locker of its
/// own, so locking means running whichever one the user installed.
///
/// logind's `Session.Lock` is the closest thing to a standard, but it only
/// emits a signal — it locks nothing unless a daemon (`swayidle`, `hypridle`,
/// `xss-lock`) is listening. On a Niri session with no such daemon it would
/// succeed and do nothing, which is the worst possible outcome for a Lock
/// command: the user walks away believing the screen is locked. So it is the
/// last resort, tried only after no locker binary was found.
///
/// The chain, in order:
///
/// 1. `$LOFI_LOCK_COMMAND`, run through `sh -c` so it can carry arguments.
/// 2. `lock-command` from the config file, run the same way.
/// 3. The first of [`LOCKER_CANDIDATES`] found on `$PATH`.
/// 4. logind's `Session.Lock` signal.
///
/// The environment variable outranks the file deliberately, so a one-off
/// `LOFI_LOCK_COMMAND=… lofi` still overrides a configured locker for that
/// invocation. It is worth knowing which way that cuts on a first switch from
/// the variable to the file: a `LOFI_LOCK_COMMAND` still exported into the
/// running session shadows the new file value until the next login, because
/// LoFi is spawned by the compositor and inherits its environment. The file
/// does not have that problem — LoFi reads it itself, at activation, so every
/// later change lands on the next invocation.
///
/// The spawned locker is deliberately not waited on. LoFi exits moments after
/// this returns and the locker is reparented to init, which is what we want —
/// blocking on it would keep a launcher process alive for the whole locked
/// period.
fn lock(configured: Option<&str>) -> Result<(), String> {
    let from_env = env::var(LOCK_COMMAND_ENV).ok();
    if let Some(command) = resolve_lock_command(from_env.as_deref(), configured) {
        return Command::new("sh")
            .arg("-c")
            .arg(command)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("lock command {command:?} failed to start: {e}"));
    }

    if let Some(locker) = find_locker_on_path(LOCKER_CANDIDATES) {
        return Command::new(&locker)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("{} failed to start: {e}", locker.display()));
    }

    eprintln!(
        "power: no locker found on PATH (tried {}); falling back to logind Session.Lock, \
         which only locks if an idle daemon is listening for it. \
         Set lock-command in ~/.config/lofi/config.toml (or {LOCK_COMMAND_ENV}) \
         to the locker you want.",
        LOCKER_CANDIDATES.join(", ")
    );
    logind::lock_session().map_err(|e| e.to_string())
}

/// Pick the lock command from the environment and the config file, or `None`
/// when neither names one.
///
/// An all-whitespace value counts as unset on both sides: for the variable
/// that is the shell's way of saying "not set", and for the file it is the
/// least surprising reading of `lock-command = ""` — otherwise LoFi would run
/// `sh -c ""`, which exits 0 and locks nothing, hiding the `$PATH` search that
/// would have worked.
///
/// Split out of [`lock`] so the precedence itself is unit-testable without
/// spawning a process.
fn resolve_lock_command<'a>(
    from_env: Option<&'a str>,
    configured: Option<&'a str>,
) -> Option<&'a str> {
    [from_env, configured]
        .into_iter()
        .flatten()
        .find(|command| !command.trim().is_empty())
}

/// Ask Niri to exit. `skip_confirmation: false` leaves Niri's own confirmation
/// prompt in place.
fn logout() -> Result<(), String> {
    ipc::action(ipc::Action::Quit {
        skip_confirmation: false,
    })
    .map_err(|e| e.to_string())
}

/// First candidate that exists and is executable on `$PATH`.
///
/// Hand-rolled instead of pulling in a `which` crate: this is one `$PATH`
/// split and a metadata check, and the dependency would earn its keep only if
/// we needed the rest of `which`'s behaviour (PATHEXT, cwd rules) that no
/// Linux-only binary does.
fn find_locker_on_path(candidates: &[&str]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = env::split_paths(&path).collect();
    find_locker(&dirs, candidates)
}

/// First candidate that exists and is executable in `dirs`.
///
/// Candidate order dominates directory order: we want the user's preferred
/// locker, not whichever one happens to sit in the earliest `$PATH` entry.
///
/// Takes its search directories as a parameter — like
/// `apps::application_directories` / `apps::gather_applications` — so tests
/// exercise this function directly instead of mutating the process
/// environment, which is global and would make them order-dependent under a
/// threaded test runner.
fn find_locker(dirs: &[PathBuf], candidates: &[&str]) -> Option<PathBuf> {
    for candidate in candidates {
        for dir in dirs {
            let full = dir.join(candidate);
            if is_executable_file(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// True when `path` is a regular file with at least one execute bit set.
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Create `dir/name` with mode `mode`.
    fn write_binary(dir: &std::path::Path, name: &str, mode: u32) {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").expect("fixture should be writable");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("fixture permissions should be settable");
    }

    #[test]
    fn the_environment_outranks_the_config_file() {
        assert_eq!(
            resolve_lock_command(Some("swaylock -f"), Some("hyprlock")),
            Some("swaylock -f"),
            "a one-off LOFI_LOCK_COMMAND must override a configured locker"
        );
        assert_eq!(
            resolve_lock_command(None, Some("hyprlock")),
            Some("hyprlock"),
            "the config file is consulted when the variable is unset"
        );
        assert_eq!(
            resolve_lock_command(Some("swaylock"), None),
            Some("swaylock"),
            "the variable alone still works, as it did before the config file"
        );
        assert_eq!(
            resolve_lock_command(None, None),
            None,
            "with neither set, the caller falls through to the PATH search"
        );
    }

    #[test]
    fn a_blank_lock_command_counts_as_unset() {
        // `sh -c ""` exits 0 and locks nothing, which would silently defeat
        // the PATH search that was about to work.
        assert_eq!(
            resolve_lock_command(Some(""), Some("hyprlock")),
            Some("hyprlock"),
            "an exported-but-empty variable is the shell's way of saying unset"
        );
        assert_eq!(
            resolve_lock_command(Some("   "), None),
            None,
            "an all-whitespace variable must not be spawned"
        );
        assert_eq!(
            resolve_lock_command(None, Some("  ")),
            None,
            "an all-whitespace config value must not be spawned either"
        );
    }

    #[test]
    fn find_locker_picks_the_first_candidate_that_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dirs = vec![dir.path().to_path_buf()];

        // Only the second candidate exists, so it wins despite being listed
        // after one that doesn't.
        write_binary(dir.path(), "hyprlock", 0o755);
        assert_eq!(
            find_locker(&dirs, &["swaylock", "hyprlock"]),
            Some(dir.path().join("hyprlock")),
            "the first candidate that actually exists should win"
        );

        // Install the higher-priority candidate; it must take over.
        write_binary(dir.path(), "swaylock", 0o755);
        assert_eq!(
            find_locker(&dirs, &["swaylock", "hyprlock"]),
            Some(dir.path().join("swaylock")),
            "candidate order decides the winner once both exist"
        );
    }

    #[test]
    fn find_locker_prefers_candidate_order_over_directory_order() {
        // `swaylock` sits in the *later* directory and `hyprlock` in the
        // earlier one; the candidate list still decides.
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        write_binary(first.path(), "hyprlock", 0o755);
        write_binary(second.path(), "swaylock", 0o755);

        let dirs = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        assert_eq!(
            find_locker(&dirs, &["swaylock", "hyprlock"]),
            Some(second.path().join("swaylock")),
            "candidate order must dominate directory order — we want the user's \
             preferred locker, not whichever sits earliest on PATH"
        );
    }

    #[test]
    fn find_locker_ignores_non_executable_and_missing_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dirs = vec![dir.path().to_path_buf()];

        write_binary(dir.path(), "swaylock", 0o644);
        assert_eq!(
            find_locker(&dirs, &["swaylock"]),
            None,
            "a non-executable file must not be treated as a locker"
        );
        assert_eq!(
            find_locker(&dirs, &["nothing-here"]),
            None,
            "a missing candidate must not be reported"
        );
        assert_eq!(
            find_locker(&[], &["swaylock"]),
            None,
            "an empty search path yields no locker rather than panicking"
        );
    }

    #[test]
    fn find_locker_skips_directories_that_do_not_exist() {
        // A stale entry on $PATH is normal and must not shadow a real hit.
        let dir = tempfile::tempdir().expect("tempdir");
        write_binary(dir.path(), "swaylock", 0o755);
        let dirs = vec![
            PathBuf::from("/definitely/not/a/real/directory"),
            dir.path().to_path_buf(),
        ];
        assert_eq!(
            find_locker(&dirs, &["swaylock"]),
            Some(dir.path().join("swaylock")),
            "a non-existent PATH entry should be skipped, not fatal"
        );
    }
}
