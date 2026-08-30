# LoFi

LoFi is a small launcher for GNOME, Niri, and macOS.

<img src="screenshot.png" width="530" alt="LoFi launcher showing a search field over a list of applications, workspaces, and commands">

<img src="screenshot-2.png" width="530" alt="LoFi launcher showing filtering">

## Goals

- Fast: LoFi should launch and display its results instantly
- Predictable: Typing the same input should find the same target, each time

## Feature set

LoFi is limited in what it can do. It can't search for or within files, it can't connect to web applications: these operations can take a long time, so it doesn't try to do them.

What it can do:

- Launch applications
- Window management and navigation:
    - Switch focus to an open window (Linux only)
    - Switch to another workspace (Linux only)
    - Move a window to another workspace (Linux only)
    - Operations on the most recently focused window:
        - Resize / retile
        - Toggle maximize
        - Toggle full-screen
- Power management
- Logout
- Locking the screen

The window-action rows differ by desktop, because a window action means
different things on a floating desktop and on a scrolling tiler. GNOME and
macOS get position-and-size commands (`Left half`, `Center third`, `Minimize`,
…); Niri gets column-width presets and layout toggles (`Width half`,
`Maximize column`, `Expand column`, `Toggle floating`, `Close window`, …).
`Toggle maximize` and `Toggle fullscreen` exist everywhere. See
[app/linux/README.md](app/linux/README.md#window-commands-under-niri).

## System requirements: Linux

- NixOS
- GNOME or [Niri](https://github.com/YaLTeR/niri)

One `lofi` binary serves both. It picks its backend at run time from
`$NIRI_SOCKET` / `$XDG_CURRENT_DESKTOP`, so there is nothing to configure and
nothing to rebuild when you switch sessions.

The two differ in what they need *installed*, and only GNOME needs anything:

- **GNOME** requires the LoFi Shell extension. Wayland clients can't enumerate
  or manipulate other apps' windows, and Mutter exposes no adequate substitute,
  so the extension is how the launcher sees windows and workspaces at all.
- **Niri** requires nothing. The compositor's own IPC socket is the whole
  interface, and the launcher presents itself as a layer-shell overlay, so
  there isn't even a window rule to write.

## Install: Linux

LoFi ships a Nix flake with a home-manager module that installs the launcher
binary and — on GNOME — symlinks the Shell extension into your profile and
enables it via dconf.

1. **Add the LoFi input to your flake** (`flake.nix`):

   ```nix
   inputs = {
     # ...
     lofi = {
       url = "github:jplein/lofi";
       inputs.nixpkgs.follows = "nixpkgs";
     };
   };
   ```

   Pass your flake `inputs` through to home-manager so the module can reach
   the `lofi` input:

   ```nix
   home-manager.extraSpecialArgs = { inherit inputs; };
   ```

2. **Add the home-manager module** to your home-manager config (e.g.
   `home.nix`) and enable it:

   ```nix
   { inputs, ... }:
   {
     imports = [ inputs.lofi.homeManagerModules.lofi ];

     programs.lofi.enable = true;
   }
   ```

   Then rebuild, e.g. `sudo nixos-rebuild switch --flake .#<host>`.

   If you already manage `org/gnome/shell` `enabled-extensions` elsewhere in
   your config, the two lists will conflict on the same dconf key — wrap the
   combined list in `lib.mkForce` and include `"lofi-shell@jplein.dev"`:

   ```nix
   dconf.settings."org/gnome/shell" = {
     enabled-extensions = lib.mkForce [
       # ...your other extensions...
       "lofi-shell@jplein.dev"
     ];
   };
   ```

   Alternatively, set `programs.lofi.enableShellExtension = false` and add the
   UUID to your own `enabled-extensions` list.

   **On Niri**, set `programs.lofi.enableShellExtension = false` — the GNOME
   extension does nothing there, and skipping it avoids writing the
   `org/gnome/shell` dconf key on a machine with no GNOME.

3. **On GNOME, log out and log back in.** The Shell extension only loads on
   session start (a Wayland constraint), so it stays inactive until you start a
   fresh session. Niri needs no restart.

### Binding a key

Bind a shortcut to the `lofi` command to summon the launcher — there is no
default. Running `lofi` a second time while it is open closes it, so a single
binding toggles it.

On **Niri**, add a bind to your `config.kdl`:

```kdl
binds {
    Mod+Space { spawn "lofi"; }
}
```

On **GNOME**, home-manager can add a custom keybinding via dconf — for example
mapping `<Alt>space` to `lofi`:

```nix
dconf.settings = {
  # Register the custom keybinding slot...
  "org/gnome/settings-daemon/plugins/media-keys" = {
    custom-keybindings = [
      "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/"
    ];
  };

  # ...then define it.
  "org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0" = {
    binding = "<Alt>space";
    command = "lofi";
    name = "LoFi";
  };
};
```

### Locking the screen on Niri

Niri implements the session-lock protocol but ships no locker, so the "Lock"
entry runs one. LoFi uses `$LOFI_LOCK_COMMAND` if set, otherwise the first of
`swaylock`, `hyprlock`, `waylock`, `gtklock` it finds on `$PATH`.

If you have none of those installed, LoFi falls back to asking logind to lock
the session — which only emits a signal, and so locks **nothing** unless you
also run an idle daemon (`swayidle`, `hypridle`) listening for it. If you want a
specific locker or specific arguments, set it explicitly:

```nix
programs.lofi.lockCommand = "swaylock -f -c 000000";
```

This exports `LOFI_LOCK_COMMAND` twice — into `hm-session-vars.sh` for a Niri
started from a TTY, and into `~/.config/environment.d/` for one started by a
display manager, where the session is `user@.service` -> `niri.service` and no
login shell runs. Either way the setting takes effect at your next login, not
on rebuild: LoFi is spawned by Niri and inherits the environment Niri started
with. To confirm it landed, check the compositor's own environment:

```sh
tr '\0' '\n' < /proc/$(pgrep -x niri)/environ | grep LOFI
```

## System requirements: macOS

- macOS Tahoe (15+)
- Xcode 26 (for the Swift toolchain)
- Nix + direnv (provides Bazel and the Rust toolchain via the flake)

## Install: macOS

```sh
bazelisk run //app/macos:install
open ~/Applications/LoFi.app
```
