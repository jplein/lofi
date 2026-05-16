# Home-manager module. Exposed from the flake as
# `homeManagerModules.lofi`. Bring the flake in as an input and import this:
#
#     inputs.lofi.url = "github:jplein/lofi";
#     # ...
#     imports = [ inputs.lofi.homeManagerModules.lofi ];
#     programs.lofi.enable = true;
#
# `enable = true` installs the launcher binary, symlinks the GNOME Shell
# extension into the user's profile, and adds the extension's UUID to
# org.gnome.shell.enabled-extensions via dconf. The extension only loads on
# session start (Wayland constraint), so a log-out / log-in is required the
# first time.

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

        Note: if your config sets `enabled-extensions` elsewhere (e.g. you
        manage other extensions through home-manager), the merge will likely
        conflict. Use `lib.mkForce` on the combined list, or set
        `enableShellExtension = false` here and add the UUID yourself.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];
    }

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
