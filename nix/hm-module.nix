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
#   - Niri requires nothing installed: the compositor's own IPC socket is the
#     whole interface. It may want *configuring*, though — the launcher
#     presents itself as a layer-shell surface, which gets no decorations from
#     either side, so `appearance` below is how it learns what your border,
#     corner radius, and shadow look like.
#
# On a Niri-only machine set `enableShellExtension = false` to skip installing
# the GNOME extension and writing the dconf key.
#
# ## The configuration file
#
# `lockCommand` and everything under `appearance` are settings LoFi reads from
# `~/.config/lofi/config.toml`, and this module renders that whole file. It is
# rendered **only if at least one of them is set**; with none set, no file is
# generated and `~/.config/lofi/config.toml` stays yours to hand-write.
#
# That all-or-nothing rule is forced, not a preference. `xdg.configFile` makes
# the file a read-only symlink into the Nix store, so a generated file and a
# hand-written one cannot share the path — the module has to own all of it or
# none of it. See `app/linux/src/config.rs` for what the file holds and why
# LoFi needs one at all.
#
# `lockCommand` used to be exported as `$LOFI_LOCK_COMMAND` into both the shell
# and systemd user environments. It now goes into the file instead, which is
# what removes this option's long-standing wart: the variable only reached LoFi
# through the compositor's own environment, so a change took effect at the next
# *login* rather than the next rebuild. LoFi reads the file itself, at
# activation, so a change now lands on the next `lofi` invocation.
#
# One caveat on that switch. `$LOFI_LOCK_COMMAND` still outranks the file (so a
# one-off `LOFI_LOCK_COMMAND=… lofi` keeps working), which means a variable
# already exported into the *running* session by a previous generation shadows
# the new file value until the next login. After that it is gone for good.

{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.lofi;
  system = pkgs.stdenv.hostPlatform.system;
  uuid = "lofi-shell@jplein.dev";

  tomlFormat = pkgs.formats.toml { };

  # Drop every option the user left at its `null` default, so an unset option
  # is absent from the generated file rather than present with a null value —
  # LoFi's own defaults then apply, and they stay LoFi's to change.
  filterNull = lib.filterAttrs (_: v: v != null);

  shadow = cfg.appearance.shadow;

  # `offset` is a nested table, so it is only emitted when one of its two
  # components was actually set. Leaving it out entirely is what gets LoFi's
  # default (Niri's own `x=0 y=5`); emitting a half-filled table would pin the
  # other component to 0.
  offsetSettings = filterNull {
    x = shadow.offsetX;
    y = shadow.offsetY;
  };

  shadowSettings = filterNull {
    softness = shadow.softness;
    spread = shadow.spread;
    color = shadow.color;
  } // lib.optionalAttrs (offsetSettings != { }) { offset = offsetSettings; };

  # The presence of the `[appearance.shadow]` table is what turns the shadow
  # on, so `shadow.enable` is a separate option rather than being inferred from
  # the others: an enabled-but-otherwise-empty table is a meaningful state (it
  # means "Niri's default shadow"), and Nix has no way to tell an unset
  # submodule from an empty one.
  appearanceSettings = filterNull {
    corner-radius = cfg.appearance.cornerRadius;
    border-width = cfg.appearance.borderWidth;
    border-color = cfg.appearance.borderColor;
  } // lib.optionalAttrs shadow.enable { shadow = shadowSettings; };

  settings = filterNull {
    lock-command = cfg.lockCommand;
  } // lib.optionalAttrs (appearanceSettings != { }) {
    appearance = appearanceSettings;
  };
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
        Shell command the "Lock" entry runs under Niri, written as
        `lock-command` into `~/.config/lofi/config.toml`.

        Niri-only, and only needed if you want a specific locker or specific
        arguments. Left unset, LoFi runs the first of `swaylock`, `hyprlock`,
        `waylock`, `gtklock` it finds on `$PATH`, and falls back to logind's
        `Session.Lock` if none is installed.

        That fallback is worth understanding before relying on it: logind's
        `Lock` only emits a signal, so it locks nothing unless an idle daemon
        (swayidle, hypridle, xss-lock) is running to act on it. A session with
        no locker *and* no idle daemon will appear to lock and won't. Setting
        this option — or installing a locker — is the fix.

        `$LOFI_LOCK_COMMAND` still overrides this for a single invocation. If
        you previously set this option, note that the variable it used to
        export stays in your running session until the next login, and
        shadows this value until then.

        Has no effect under GNOME, where Lock goes through
        `org.gnome.ScreenSaver`.
      '';
    };

    appearance = {
      cornerRadius = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.unsigned;
        default = null;
        example = 12;
        description = ''
          Corner radius of the launcher window, in logical pixels. `0` gives
          square corners.

          Niri-only. On an ordinary toplevel (GNOME) the corners come from
          GTK's client-side decorations and this is ignored; a layer-shell
          surface has no decorations, so LoFi has to round its own.

          Left unset, LoFi uses 12 — libadwaita's own window radius.
        '';
      };

      borderWidth = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.unsigned;
        default = null;
        example = 3;
        description = ''
          Border width of the launcher window, in logical pixels. `0` — the
          default — draws no border.

          Niri-only, and the one decoration Niri itself cannot supply for a
          layer surface: `border` is a window-rule property and layer rules
          reject it. Match it to your `layout.border.width` by hand.
        '';
      };

      borderColor = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "#e0e0e0";
        description = ''
          Border colour. Ignored unless `borderWidth` is non-zero.

          Accepts a hex colour (`#0007`, `#e0e0e0`, `#00000080`), a GTK named
          colour (`@borders`, `@accent_color`), or a CSS keyword (`black`).
          Anything else is rejected by LoFi with a message on stderr — the
          value ends up in a stylesheet, so it is validated rather than passed
          through.

          Left unset, LoFi uses `@borders`, which follows the light/dark
          scheme.
        '';
      };

      shadow = {
        enable = lib.mkEnableOption ''
          a drop shadow drawn by LoFi itself.

          Leave this off if you would rather Niri drew the shadow, which it can
          do for a layer surface:

          ```kdl
          layer-rule {
              match namespace="^lofi$"
              geometry-corner-radius 12
              shadow { on; }
          }
          ```

          The two are mutually exclusive, and the reason is geometric. To draw
          its own shadow LoFi has to pad its surface with transparent room for
          the blur, and Niri draws a layer surface's shadow around the *whole*
          buffer — it has no `xdg_surface.set_window_geometry` to learn the
          visual bounds from — so with both on you would get Niri's shadow
          floating a blur-width away from the window. With this off, LoFi adds
          no padding and the surface is exactly the visible rectangle, which is
          the shape Niri's shadow wants.

          Either way LoFi still rounds its own corners, so `cornerRadius` and
          the layer rule's `geometry-corner-radius` should agree''
        ;

        softness = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          example = 30;
          description = ''
            Shadow blur radius in logical pixels; `0` gives a hard shadow.
            Same meaning as Niri's `shadow.softness` and CSS's box-shadow blur
            radius, so the number copies straight across from `config.kdl`.

            Left unset, LoFi uses Niri's own default of 30.
          '';
        };

        spread = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          example = 5;
          description = ''
            Distance the shadow is expanded past the window, in logical
            pixels. Same meaning as Niri's `shadow.spread`.

            Left unset, LoFi uses Niri's own default of 5.
          '';
        };

        offsetX = lib.mkOption {
          type = lib.types.nullOr lib.types.int;
          default = null;
          example = 0;
          description = ''
            Horizontal shadow displacement in logical pixels; positive is
            rightwards. Same meaning as the `x` in Niri's `shadow.offset`.

            Left unset — and with `offsetY` unset too — LoFi uses Niri's own
            default of `x=0 y=5`. Setting either one alone pins the other to 0.
          '';
        };

        offsetY = lib.mkOption {
          type = lib.types.nullOr lib.types.int;
          default = null;
          example = 5;
          description = ''
            Vertical shadow displacement in logical pixels; positive is
            downwards. Same meaning as the `y` in Niri's `shadow.offset`.
          '';
        };

        color = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "#00000080";
          description = ''
            Shadow colour and opacity. Takes the same forms as `borderColor`.

            Left unset, LoFi uses Niri's own default of `#0007`.
          '';
        };
      };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];
    }

    # Generated only when the user set at least one of the settings that go
    # into it. With none set there is no file, and the path stays available for
    # a hand-written one — see the header for why it cannot be both.
    (lib.mkIf (settings != { }) {
      xdg.configFile."lofi/config.toml".source =
        tomlFormat.generate "lofi-config.toml" settings;
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
