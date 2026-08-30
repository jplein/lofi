# Home-manager module. Exposed from the flake as
# `homeManagerModules.lofi`. Bring the flake in as an input and import this:
#
#     inputs.lofi.url = "github:jplein/lofi";
#     # ...
#     imports = [ inputs.lofi.homeManagerModules.lofi ];
#     programs.lofi.enable = true;
#
# `enable = true` installs the launcher binary. The launcher itself picks its
# desktop backend at run time (GNOME vs Niri — see `app/linux/src/backend`),
# so there is nothing to select here; what differs is the per-desktop
# *installation* each backend needs, and only GNOME needs any:
#
#   - GNOME requires the LoFi Shell extension, since Wayland clients can't
#     enumerate other apps' windows and Mutter exposes no adequate substitute.
#     `enableShellExtension` (default true) symlinks it into the user's profile
#     and adds its UUID to org.gnome.shell.enabled-extensions via dconf. The
#     extension only loads on session start (a Wayland constraint), so a
#     log-out / log-in is required the first time.
#   - Niri requires nothing: the compositor's own IPC socket is the whole
#     interface, and the launcher presents itself as a layer-shell surface, so
#     there is no window rule to write either.
#
# On a Niri-only machine set `enableShellExtension = false` to skip installing
# the GNOME extension and writing the dconf key.

{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.lofi;
  system = pkgs.stdenv.hostPlatform.system;
  uuid = "lofi-shell@jplein.dev";
in
{
  options.programs.lofi = {
    enable = lib.mkEnableOption
      "LoFi launcher (GTK4 launcher binary + GNOME Shell extension)";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.lofi;
      defaultText = lib.literalExpression
        "lofi.packages.\${pkgs.system}.lofi";
      description = "The LoFi launcher binary package.";
    };

    extensionPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.extension;
      defaultText = lib.literalExpression
        "lofi.packages.\${pkgs.system}.extension";
      description = "The LoFi GNOME Shell extension package.";
    };

    enableShellExtension = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Install the GNOME Shell extension's files and add its UUID to
        `org/gnome/shell/enabled-extensions` via dconf.

        GNOME-only: the Niri backend talks to the compositor directly and
        needs nothing installed, so set this to `false` on a machine that
        only runs Niri.

        Note: if your config sets `enabled-extensions` elsewhere (e.g. you
        manage other extensions through home-manager), the merge will likely
        conflict. Use `lib.mkForce` on the combined list, or set
        `enableShellExtension = false` here and add the UUID yourself.
      '';
    };

    lockCommand = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "swaylock -f -c 000000";
      description = ''
        Shell command the "Lock" entry runs under Niri, exported as
        `LOFI_LOCK_COMMAND`.

        Niri-only, and only needed if you want a specific locker or specific
        arguments. Left unset, LoFi runs the first of `swaylock`, `hyprlock`,
        `waylock`, `gtklock` it finds on `$PATH`, and falls back to logind's
        `Session.Lock` if none is installed.

        That fallback is worth understanding before relying on it: logind's
        `Lock` only emits a signal, so it locks nothing unless an idle daemon
        (swayidle, hypridle, xss-lock) is running to act on it. A session with
        no locker *and* no idle daemon will appear to lock and won't. Setting
        this option — or installing a locker — is the fix.

        Has no effect under GNOME, where Lock goes through
        `org.gnome.ScreenSaver`.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];
    }

    # Exported into the session environment rather than baked into a wrapper
    # so the launcher stays a plain binary: LoFi reads the variable at
    # activation time, and a user can override it for one invocation.
    (lib.mkIf (cfg.lockCommand != null) {
      home.sessionVariables.LOFI_LOCK_COMMAND = cfg.lockCommand;
    })

    (lib.mkIf cfg.enableShellExtension {
      # Symlink the extracted extension into the user's gnome-shell
      # extensions dir. gnome-shell follows symlinks; no copy needed.
      xdg.dataFile."gnome-shell/extensions/${uuid}".source =
        "${cfg.extensionPackage}/share/gnome-shell/extensions/${uuid}";

      # Add the UUID to dconf so gnome-shell knows to load it. Without
      # this the files exist but the extension stays inactive.
      dconf.settings."org/gnome/shell".enabled-extensions = [ uuid ];
    })
  ]);
}
